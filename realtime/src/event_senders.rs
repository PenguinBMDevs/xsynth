use std::{
    collections::VecDeque,
    ops::RangeInclusive,
    sync::atomic::{AtomicU64, Ordering},
    sync::Arc,
    time::{Duration, Instant},
};

use crossbeam_channel::Sender;

use xsynth_core::channel::{ChannelAudioEvent, ChannelConfigEvent, ChannelEvent, ControlEvent};

use crate::SynthEvent;

static NPS_WINDOW_MILLISECONDS: u64 = 20;

struct NpsWindow {
    time: Instant,
    notes: u64,
}

/// A struct for tracking the estimated NPS, as fast as possible with the focus on speed
/// rather than precision. Used for NPS limiting on extremely spammy midis.
///
/// Uses on-demand `Instant::now()` instead of a dedicated thread, eliminating the
/// per-sender OS thread overhead that previously resulted in ~16 idle threads for MIDI.
struct RoughNpsTracker {
    windows: VecDeque<NpsWindow>,
    total_window_sum: u64,
    current_window_sum: u64,
    last_window_time: Instant,
}

impl RoughNpsTracker {
    pub fn new() -> RoughNpsTracker {
        RoughNpsTracker {
            windows: VecDeque::new(),
            total_window_sum: 0,
            current_window_sum: 0,
            last_window_time: Instant::now(),
        }
    }

    pub fn calculate_nps(&mut self) -> u64 {
        self.check_time();

        // Remove windows older than 1 second
        while let Some(window) = self.windows.front() {
            // Safe: last_window_time is always >= window.time (windows are pushed in time order)
            if self.last_window_time >= window.time + Duration::from_millis(1000) {
                self.total_window_sum -= window.notes;
                self.windows.pop_front();
            } else {
                break;
            }
        }

        let short_nps = self.current_window_sum * (1000 / NPS_WINDOW_MILLISECONDS) * 4 / 3;
        let long_nps = self.total_window_sum;

        short_nps.max(long_nps)
    }

    fn check_time(&mut self) {
        let now = Instant::now();
        if now >= self.last_window_time + Duration::from_millis(NPS_WINDOW_MILLISECONDS) {
            self.windows.push_back(NpsWindow {
                time: self.last_window_time,
                notes: self.current_window_sum,
            });
            self.current_window_sum = 0;
            self.last_window_time = now;
        }
    }

    pub fn add_note(&mut self) {
        self.current_window_sum += 1;
        self.total_window_sum += 1;
    }
}

fn should_send_for_vel_and_nps(vel: u8, nps: u64, max: u64) -> bool {
    (vel as u64) * max / 127 > nps
}

struct EventSender {
    sender: Sender<ChannelEvent>,
    nps: RoughNpsTracker,
    max_nps: Arc<AtomicU64>,
    skipped_notes: [u64; 128],
    ignore_range: RangeInclusive<u8>,
}

impl EventSender {
    pub fn new(
        max_nps: Arc<AtomicU64>,
        sender: Sender<ChannelEvent>,
        ignore_range: RangeInclusive<u8>,
    ) -> Self {
        EventSender {
            sender,
            nps: RoughNpsTracker::new(),
            max_nps,
            skipped_notes: [0; 128],
            ignore_range,
        }
    }

    pub fn send_audio(&mut self, event: ChannelAudioEvent) {
        match &event {
            ChannelAudioEvent::NoteOn { vel, key } => {
                if *key > 127 {
                    return;
                }

                let nps = self.nps.calculate_nps();

                if should_send_for_vel_and_nps(*vel, nps, self.max_nps.load(Ordering::Relaxed))
                    && !self.ignore_range.contains(vel)
                {
                    self.sender.send(ChannelEvent::Audio(event)).ok();
                    self.nps.add_note();
                } else {
                    self.skipped_notes[*key as usize] += 1;
                }
            }
            ChannelAudioEvent::NoteOff { key } => {
                if *key > 127 {
                    return;
                }

                if self.skipped_notes[*key as usize] > 0 {
                    self.skipped_notes[*key as usize] -= 1;
                } else {
                    self.sender.send(ChannelEvent::Audio(event)).ok();
                }
            }
            _ => {
                self.sender.send(ChannelEvent::Audio(event)).ok();
            }
        }
    }

    pub fn send_config(&mut self, event: ChannelConfigEvent) {
        self.sender.send(ChannelEvent::Config(event)).ok();
    }

    pub fn set_ignore_range(&mut self, ignore_range: RangeInclusive<u8>) {
        self.ignore_range = ignore_range;
    }
}

impl Clone for EventSender {
    fn clone(&self) -> Self {
        EventSender {
            sender: self.sender.clone(),
            max_nps: self.max_nps.clone(),

            // Rough nps tracker is only used for very extreme spam situations,
            // so creating a new one when cloning shouldn't be an issue
            nps: RoughNpsTracker::new(),

            // Skipped notes is related to nps limiter, therefore it's also not cloned
            skipped_notes: [0; 128],

            ignore_range: self.ignore_range.clone(),
        }
    }
}

/// A helper object to send events to the realtime synthesizer.
#[derive(Clone)]
pub struct RealtimeEventSender {
    senders: Vec<EventSender>,
}

impl RealtimeEventSender {
    pub(super) fn new(
        senders: Vec<Sender<ChannelEvent>>,
        max_nps: Arc<AtomicU64>,
        ignore_range: RangeInclusive<u8>,
    ) -> RealtimeEventSender {
        RealtimeEventSender {
            senders: senders
                .into_iter()
                .map(|s| EventSender::new(max_nps.clone(), s, ignore_range.clone()))
                .collect(),
        }
    }

    /// Sends a SynthEvent to the realtime synthesizer.
    ///
    /// See the `SynthEvent` documentation for more information.
    pub fn send_event(&mut self, event: SynthEvent) {
        match event {
            SynthEvent::Channel(channel, event) => match event {
                ChannelEvent::Audio(e) => self.senders[channel as usize].send_audio(e),
                ChannelEvent::Config(e) => self.senders[channel as usize].send_config(e),
            },
            SynthEvent::AllChannels(event) => match event {
                ChannelEvent::Audio(e) => {
                    for sender in self.senders.iter_mut() {
                        sender.send_audio(e);
                    }
                }
                ChannelEvent::Config(e) => {
                    for sender in self.senders.iter_mut() {
                        sender.send_config(e.clone());
                    }
                }
            },
        }
    }

    /// Sends a MIDI event as raw bytes.
    pub fn send_event_u32(&mut self, event: u32) {
        let head = event & 0xFF;
        let channel = head & 0xF;
        let code = head >> 4;

        macro_rules! val1 {
            () => {
                (event >> 8) as u8
            };
        }

        macro_rules! val2 {
            () => {
                (event >> 16) as u8
            };
        }

        match code {
            0x8 => {
                self.send_event(SynthEvent::Channel(
                    channel,
                    ChannelEvent::Audio(ChannelAudioEvent::NoteOff { key: val1!() }),
                ));
            }
            0x9 => {
                self.send_event(SynthEvent::Channel(
                    channel,
                    ChannelEvent::Audio(ChannelAudioEvent::NoteOn {
                        key: val1!(),
                        vel: val2!(),
                    }),
                ));
            }
            0xB => {
                self.send_event(SynthEvent::Channel(
                    channel,
                    ChannelEvent::Audio(ChannelAudioEvent::Control(ControlEvent::Raw(
                        val1!(),
                        val2!(),
                    ))),
                ));
            }
            0xC => {
                self.send_event(SynthEvent::Channel(
                    channel,
                    ChannelEvent::Audio(ChannelAudioEvent::ProgramChange(val1!())),
                ));
            }
            0xE => {
                let value = (((val2!() as i16) << 7) | val1!() as i16) - 8192;
                let value = value as f32 / 8192.0;
                self.send_event(SynthEvent::Channel(
                    channel,
                    ChannelEvent::Audio(ChannelAudioEvent::Control(ControlEvent::PitchBendValue(
                        value,
                    ))),
                ));
            }

            _ => {}
        }
    }

    /// Resets all note and control change data of the realtime synthesizer.
    pub fn reset_synth(&mut self) {
        self.send_event(SynthEvent::AllChannels(ChannelEvent::Audio(
            ChannelAudioEvent::AllNotesKilled,
        )));

        for sender in &mut self.senders {
            for i in 0..128 {
                sender.skipped_notes[i] = 0;
            }
        }

        self.send_event(SynthEvent::AllChannels(ChannelEvent::Audio(
            ChannelAudioEvent::ResetControl,
        )));
    }

    /// Changes the range of velocities that will be ignored for the
    /// specific sender instance.
    pub fn set_ignore_range(&mut self, ignore_range: RangeInclusive<u8>) {
        for sender in self.senders.iter_mut() {
            sender.set_ignore_range(ignore_range.clone());
        }
    }
}
