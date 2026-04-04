use std::{
    process,
    sync::atomic::{AtomicBool, Ordering},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use midi_toolkit::{
    events::Event,
    io::MIDIFile,
    pipe,
    sequence::{
        event::{cancel_tempo_events, scale_event_time},
        unwrap_items, TimeCaster,
    },
};
use xsynth_core::{
    channel::{
        ChannelAudioEvent, ChannelConfigEvent, ChannelEvent, ChannelInitOptions, ControlEvent,
    },
    soundfont::{SampleSoundfont, SoundfontBase},
};
use xsynth_realtime::{RealtimeSynth, SynthEvent};

/// Maximum allowed render time in seconds before forcing exit
const MAX_RENDER_TIME: f64 = 1.0;
/// Maximum consecutive high render time readings before exit
const MAX_CONSECUTIVE_HIGH: u32 = 3;
/// Maximum consecutive negative buffer readings before exit
const MAX_CONSECUTIVE_NEGATIVE_BUFFER: u32 = 3;

fn main() {
    let args = std::env::args().collect::<Vec<String>>();
    let (Some(midi), Some(sfz)) = (
        args.get(1)
            .cloned()
            .or_else(|| std::env::var("XSYNTH_EXAMPLE_MIDI").ok())
            .or_else(|| {
                Some(
                    r"D:\2026寒假-炭黑烤馒头系列\necrofantasiaτ6283185307notes62831tracks(1)\necrofantasia τ 6283185307 notes 62831 tracks.mid".to_string(),
                )
            }),
        args.get(2)
            .cloned()
            .or_else(|| std::env::var("XSYNTH_EXAMPLE_SF").ok())
            .or_else(|| Some(r"D:\BM-DATA\SF2\Nexus Trap Piano V2（黑乐谱推荐）.sf2".to_string())),
    ) else {
        println!(
            "Usage: {} [midi] [sfz/sf2]",
            std::env::current_exe()
                .unwrap_or("example".into())
                .display()
        );
        return;
    };

    // Use multithreading for best performance with high voice counts
    let synth = RealtimeSynth::open_with_default_output(xsynth_realtime::XSynthRealtimeConfig {
        channel_init_options: ChannelInitOptions {
            max_voices_per_key: None,
            ..Default::default()
        },
        render_window_ms: 10.0,
        multithreading: xsynth_realtime::ThreadCount::Auto,
        ..Default::default()
    });
    let mut sender = synth.get_sender_ref().clone();

    let params = synth.stream_params();

    println!("Loading Soundfont");
    let soundfonts: Vec<Arc<dyn SoundfontBase>> = vec![Arc::new(
        SampleSoundfont::new(sfz, params, Default::default()).unwrap(),
    )];
    println!("Loaded");

    sender.send_event(SynthEvent::AllChannels(ChannelEvent::Config(
        ChannelConfigEvent::SetSoundfonts(soundfonts),
    )));

    let stats = synth.get_stats();

    // Flag to signal when render time is too high
    let should_exit = Arc::new(AtomicBool::new(false));
    let should_exit_clone = should_exit.clone();

    thread::spawn(move || {
        let mut consecutive_high = 0u32;
        let mut consecutive_negative_buffer = 0u32;
        loop {
            let render_time = stats.buffer().average_renderer_load();
            let voice_count = stats.voice_count();
            let buffer = stats.buffer().last_samples_after_read();

            println!(
                "Voice Count: {}\tBuffer: {}\tRender time: {}",
                voice_count, buffer, render_time
            );

            // Check if buffer is negative (underrun)
            if buffer < 0 {
                consecutive_negative_buffer += 1;
                eprintln!(
                    "WARNING: Buffer underrun! Buffer: {} (consecutive: {})",
                    buffer, consecutive_negative_buffer
                );

                if consecutive_negative_buffer >= MAX_CONSECUTIVE_NEGATIVE_BUFFER {
                    eprintln!(
                        "CRITICAL: Buffer underrun for {} consecutive readings. Forcing exit!",
                        MAX_CONSECUTIVE_NEGATIVE_BUFFER
                    );
                    should_exit_clone.store(true, Ordering::Relaxed);
                    thread::sleep(Duration::from_millis(100));
                    process::exit(1);
                }
            } else {
                consecutive_negative_buffer = 0;
            }

            // Check if render time exceeds threshold
            if render_time > MAX_RENDER_TIME {
                consecutive_high += 1;
                eprintln!(
                    "WARNING: Render time {}s exceeds threshold {}s (consecutive: {})",
                    render_time, MAX_RENDER_TIME, consecutive_high
                );

                if consecutive_high >= MAX_CONSECUTIVE_HIGH {
                    eprintln!(
                        "CRITICAL: Render time exceeded {}s for {} consecutive readings. Forcing exit!",
                        MAX_RENDER_TIME, MAX_CONSECUTIVE_HIGH
                    );
                    should_exit_clone.store(true, Ordering::Relaxed);
                    thread::sleep(Duration::from_millis(100));
                    process::exit(1);
                }
            } else {
                consecutive_high = 0;
            }

            thread::sleep(Duration::from_millis(10));
        }
    });

    let midi = MIDIFile::open(&midi, None).unwrap();

    let ppq = midi.ppq();
    let merged = pipe!(
        midi.iter_all_events_merged_batches()
        |>TimeCaster::<f64>::cast_event_delta()
        |>cancel_tempo_events(250000)
        |>scale_event_time(1.0 / ppq as f64)
        |>unwrap_items()
    );

    let (snd, rcv) = crossbeam_channel::bounded(100);

    thread::spawn(move || {
        for batch in merged {
            snd.send(batch).unwrap();
        }
    });

    let now = Instant::now();
    let mut time = 0.0;
    for batch in rcv {
        // Check if we should exit due to high render time
        if should_exit.load(Ordering::Relaxed) {
            eprintln!("Playback aborted due to excessive render time");
            process::exit(1);
        }

        if batch.delta != 0.0 {
            time += batch.delta;
            let diff = time - now.elapsed().as_secs_f64();
            if diff > 0.0 {
                spin_sleep::sleep(Duration::from_secs_f64(diff));
            }
        }

        for e in batch.iter_inner() {
            match e {
                Event::NoteOn(e) => {
                    sender.send_event(SynthEvent::Channel(
                        e.channel as u32,
                        ChannelEvent::Audio(ChannelAudioEvent::NoteOn {
                            key: e.key,
                            vel: e.velocity,
                        }),
                    ));
                }
                Event::NoteOff(e) => {
                    sender.send_event(SynthEvent::Channel(
                        e.channel as u32,
                        ChannelEvent::Audio(ChannelAudioEvent::NoteOff { key: e.key }),
                    ));
                }
                Event::ControlChange(e) => {
                    sender.send_event(SynthEvent::Channel(
                        e.channel as u32,
                        ChannelEvent::Audio(ChannelAudioEvent::Control(ControlEvent::Raw(
                            e.controller,
                            e.value,
                        ))),
                    ));
                }
                Event::PitchWheelChange(e) => {
                    sender.send_event(SynthEvent::Channel(
                        e.channel as u32,
                        ChannelEvent::Audio(ChannelAudioEvent::Control(
                            ControlEvent::PitchBendValue(e.pitch as f32 / 8192.0),
                        )),
                    ));
                }
                Event::ProgramChange(e) => {
                    sender.send_event(SynthEvent::Channel(
                        e.channel as u32,
                        ChannelEvent::Audio(ChannelAudioEvent::ProgramChange(e.program)),
                    ));
                }
                _ => {}
            }
        }
    }

    std::thread::sleep(Duration::from_secs(10000));
}
