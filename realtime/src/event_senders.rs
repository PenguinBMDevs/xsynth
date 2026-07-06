use std::{
    collections::VecDeque,
    ops::RangeInclusive,
    sync::atomic::{AtomicU64, Ordering},
    sync::Arc,
    time::{Duration, Instant},
};

use crossbeam_channel::Sender;

use xsynth_core::channel::{
    ChannelAudioEvent, ChannelConfigEvent, ChannelEvent, ControlEvent,
};

use crate::{RealtimeRenderMode, SynthEvent};

static NPS_WINDOW_MILLISECONDS: u64 = 20;

#[derive(Clone)]
struct NpsWindow {
    time: Instant,
    notes: u64,
}

/// A struct for tracking the estimated NPS, as fast as possible with the focus on speed
/// rather than precision. Used for NPS limiting on extremely spammy midis.
///
/// Uses on-demand `Instant::now()` instead of a dedicated thread, eliminating the
/// per-sender OS thread overhead that previously resulted in ~16 idle threads for MIDI.
#[derive(Clone)]
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
    // max == 0 或 u64::MAX 视为“无限制”，全量播放。
    if max == 0 || max == u64::MAX {
        return true;
    }
    // 避免 (vel * max) 溢出：先除法再乘法。
    let threshold = nps.saturating_mul(127) / max.max(1);
    vel as u64 > threshold
}

/// The underlying channel used by an `EventSender`.
pub(crate) enum EventSenderInner {
    /// Legacy threaded renderer: each sender talks to one VoiceChannel thread.
    Threaded(Sender<ChannelEvent>),
    /// ChannelGroup renderer: every sender forwards to the same render-thread
    /// channel that feeds the synchronous `ChannelGroup`.
    ChannelGroup(Sender<SynthEvent>),
}

pub(crate) struct EventSender {
    inner: EventSenderInner,
    nps: RoughNpsTracker,
    max_nps: Arc<AtomicU64>,
    skipped_notes: [u64; 128],
    ignore_range: RangeInclusive<u8>,
}

impl EventSender {
    pub fn new(
        max_nps: Arc<AtomicU64>,
        inner: EventSenderInner,
        ignore_range: RangeInclusive<u8>,
    ) -> Self {
        EventSender {
            inner,
            nps: RoughNpsTracker::new(),
            max_nps,
            skipped_notes: [0; 128],
            ignore_range,
        }
    }

    fn send_note_on(&mut self, channel: u32, key: u8, vel: u8) {
        let nps = self.nps.calculate_nps();
        if should_send_for_vel_and_nps(vel, nps, self.max_nps.load(Ordering::Relaxed))
            && !self.ignore_range.contains(&vel)
        {
            match &self.inner {
                EventSenderInner::Threaded(sender) => {
                    let _ = sender.send(ChannelEvent::Audio(ChannelAudioEvent::NoteOn {
                        key,
                        vel,
                    }));
                }
                EventSenderInner::ChannelGroup(sender) => {
                    let _ = sender.send(SynthEvent::Channel(
                        channel,
                        ChannelEvent::Audio(ChannelAudioEvent::NoteOn { key, vel }),
                    ));
                }
            }
            self.nps.add_note();
        } else {
            self.skipped_notes[key as usize] += 1;
        }
    }

    fn send_note_off(&mut self, channel: u32, key: u8) {
        if self.skipped_notes[key as usize] > 0 {
            self.skipped_notes[key as usize] -= 1;
        } else {
            match &self.inner {
                EventSenderInner::Threaded(sender) => {
                    let _ = sender.send(ChannelEvent::Audio(ChannelAudioEvent::NoteOff {
                        key,
                    }));
                }
                EventSenderInner::ChannelGroup(sender) => {
                    let _ = sender.send(SynthEvent::Channel(
                        channel,
                        ChannelEvent::Audio(ChannelAudioEvent::NoteOff { key }),
                    ));
                }
            }
        }
    }

    pub fn send_audio(&mut self, channel: u32, event: ChannelAudioEvent) {
        match event {
            ChannelAudioEvent::NoteOn { key, vel } => {
                if key > 127 {
                    return;
                }
                self.send_note_on(channel, key, vel);
            }
            ChannelAudioEvent::NoteOff { key } => {
                if key > 127 {
                    return;
                }
                self.send_note_off(channel, key);
            }
            _ => match &self.inner {
                EventSenderInner::Threaded(sender) => {
                    let _ = sender.send(ChannelEvent::Audio(event));
                }
                EventSenderInner::ChannelGroup(sender) => {
                    let _ = sender.send(SynthEvent::Channel(
                        channel,
                        ChannelEvent::Audio(event),
                    ));
                }
            },
        }
    }

    /// Used for `SynthEvent::AllChannels` in ChannelGroup mode: a single event is
    /// expanded to all channels by the renderer, so we only need to send once.
    pub fn send_audio_all_channels(&mut self, event: ChannelAudioEvent) {
        match &self.inner {
            EventSenderInner::Threaded(_) => {
                // Threaded mode dispatches per-channel via each EventSender.
                self.send_audio(0, event);
            }
            EventSenderInner::ChannelGroup(sender) => {
                let _ = sender.send(SynthEvent::AllChannels(ChannelEvent::Audio(event)));
            }
        }
    }

    pub fn send_config(&mut self, channel: u32, event: ChannelConfigEvent) {
        match &self.inner {
            EventSenderInner::Threaded(sender) => {
                let _ = sender.send(ChannelEvent::Config(event));
            }
            EventSenderInner::ChannelGroup(sender) => {
                let _ = sender.send(SynthEvent::Channel(channel, ChannelEvent::Config(event)));
            }
        }
    }

    pub fn send_config_all_channels(&mut self, event: ChannelConfigEvent) {
        match &self.inner {
            EventSenderInner::Threaded(sender) => {
                let _ = sender.send(ChannelEvent::Config(event));
            }
            EventSenderInner::ChannelGroup(sender) => {
                let _ = sender.send(SynthEvent::AllChannels(ChannelEvent::Config(event)));
            }
        }
    }

    pub fn set_ignore_range(&mut self, ignore_range: RangeInclusive<u8>) {
        self.ignore_range = ignore_range;
    }
}

impl Clone for EventSender {
    fn clone(&self) -> Self {
        EventSender {
            inner: match &self.inner {
                EventSenderInner::Threaded(sender) => {
                    EventSenderInner::Threaded(sender.clone())
                }
                EventSenderInner::ChannelGroup(sender) => {
                    EventSenderInner::ChannelGroup(sender.clone())
                }
            },
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
    mode: RealtimeRenderMode,
}

impl RealtimeEventSender {
    pub(super) fn new(
        senders: Vec<EventSender>,
        mode: RealtimeRenderMode,
    ) -> RealtimeEventSender {
        RealtimeEventSender { senders, mode }
    }

    /// Sends a SynthEvent to the realtime synthesizer.
    ///
    /// See the `SynthEvent` documentation for more information.
    pub fn send_event(&mut self, event: SynthEvent) {
        match event {
            SynthEvent::Channel(channel, event) => match event {
                ChannelEvent::Audio(e) => {
                    if let Some(sender) = self.senders.get_mut(channel as usize) {
                        sender.send_audio(channel, e);
                    }
                }
                ChannelEvent::Config(e) => {
                    if let Some(sender) = self.senders.get_mut(channel as usize) {
                        sender.send_config(channel, e);
                    }
                }
            },
            SynthEvent::AllChannels(event) => {
                // In ChannelGroup mode, AllChannels events are sent once and the
                // renderer expands them. In Threaded mode, each channel sender
                // forwards the event to its own VoiceChannel thread.
                let first = match self.mode {
                    RealtimeRenderMode::ChannelGroup => self.senders.first_mut(),
                    RealtimeRenderMode::Threaded => None,
                };

                if let Some(sender) = first {
                    match event {
                        ChannelEvent::Audio(e) => sender.send_audio_all_channels(e),
                        ChannelEvent::Config(e) => sender.send_config_all_channels(e),
                    }
                } else {
                    for (channel, sender) in self.senders.iter_mut().enumerate() {
                        let channel = channel as u32;
                        match event.clone() {
                            ChannelEvent::Audio(e) => sender.send_audio(channel, e),
                            ChannelEvent::Config(e) => sender.send_config(channel, e),
                        }
                    }
                }
            }
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
