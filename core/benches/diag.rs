use std::sync::Arc;
use std::time::Instant;
use xsynth_core::channel::{ChannelAudioEvent, ChannelConfigEvent, ChannelEvent, VoiceChannel};
use xsynth_core::soundfont::{SampleSoundfont, SoundfontBase};
use xsynth_core::{AudioPipe, AudioStreamParams, ChannelCount};

fn main() {
    let sfz = std::env::var("XSYNTH_EXAMPLE_SFZ").expect("XSYNTH_EXAMPLE_SFZ not set");
    let stream_params = AudioStreamParams::new(48000, ChannelCount::Stereo);

    println!("Loading soundfont...");
    let t0 = Instant::now();
    let soundfonts: Vec<Arc<dyn SoundfontBase>> = vec![Arc::new(
        SampleSoundfont::new(&sfz, stream_params, Default::default()).unwrap(),
    )];
    println!("  SF2 loaded in {:?}", t0.elapsed());

    // Measure channel creation + SetSoundfonts (rebuild_matrix)
    let t1 = Instant::now();
    for _ in 0..100 {
        let mut ch = VoiceChannel::new(Default::default(), stream_params, None);
        ch.process_event(ChannelEvent::Config(ChannelConfigEvent::SetSoundfonts(
            soundfonts.clone(),
        )));
    }
    let matrix100 = t1.elapsed();
    println!(
        "  rebuild_matrix x100 in {:?} (avg {:?})",
        matrix100,
        matrix100 / 100
    );

    // Full benchmark: fresh channel every iteration (current behavior)
    let mut buffer = vec![0.0; 4800];
    let t3 = Instant::now();
    for iter in 0..100 {
        let mut ch = VoiceChannel::new(Default::default(), stream_params, None);
        ch.process_event(ChannelEvent::Config(ChannelConfigEvent::SetSoundfonts(
            soundfonts.clone(),
        )));
        ch.process_event(ChannelEvent::Config(ChannelConfigEvent::SetLayerCount(
            None,
        )));
        for _ in 0..30 {
            for i in 0..127u8 {
                ch.process_event(ChannelEvent::Audio(ChannelAudioEvent::NoteOn {
                    key: i,
                    vel: 127,
                }));
            }
            ch.read_samples(&mut buffer);
            for i in 0..127u8 {
                ch.process_event(ChannelEvent::Audio(ChannelAudioEvent::NoteOff { key: i }));
            }
        }
        if iter == 0 {
            println!("  First iteration done");
        }
    }
    let full_bench = t3.elapsed();
    println!(
        "  100 iterations (fresh channel) in {:?} (avg {:?})",
        full_bench,
        full_bench / 100
    );

    // Same benchmark but with channel reuse (no SetSoundfonts per iteration)
    let t4 = Instant::now();
    for iter in 0..100 {
        let mut ch = VoiceChannel::new(Default::default(), stream_params, None);
        ch.process_event(ChannelEvent::Config(ChannelConfigEvent::SetSoundfonts(
            soundfonts.clone(),
        )));
        ch.process_event(ChannelEvent::Config(ChannelConfigEvent::SetLayerCount(
            None,
        )));
        for _ in 0..30 {
            for i in 0..127u8 {
                ch.process_event(ChannelEvent::Audio(ChannelAudioEvent::NoteOn {
                    key: i,
                    vel: 127,
                }));
            }
            ch.read_samples(&mut buffer);
            for i in 0..127u8 {
                ch.process_event(ChannelEvent::Audio(ChannelAudioEvent::NoteOff { key: i }));
            }
        }
        if iter == 0 {
            println!("  First fresh iteration done");
        }
    }
    let fresh_bench = t4.elapsed();
    println!(
        "  100 iterations (fresh channel each) in {:?} (avg {:?})",
        fresh_bench,
        fresh_bench / 100
    );

    // Now try with reuse (no recreation)
    let mut ch = VoiceChannel::new(Default::default(), stream_params, None);
    ch.process_event(ChannelEvent::Config(ChannelConfigEvent::SetSoundfonts(
        soundfonts.clone(),
    )));
    ch.process_event(ChannelEvent::Config(ChannelConfigEvent::SetLayerCount(
        None,
    )));
    let t5 = Instant::now();
    for iter in 0..100 {
        // Kill all by sending SystemReset
        ch.process_event(ChannelEvent::Audio(ChannelAudioEvent::SystemReset));
        ch.read_samples(&mut buffer); // drain events

        for _ in 0..30 {
            for i in 0..127u8 {
                ch.process_event(ChannelEvent::Audio(ChannelAudioEvent::NoteOn {
                    key: i,
                    vel: 127,
                }));
            }
            ch.read_samples(&mut buffer);
            for i in 0..127u8 {
                ch.process_event(ChannelEvent::Audio(ChannelAudioEvent::NoteOff { key: i }));
            }
        }
        if iter == 0 {
            println!("  First reuse iteration done");
        }
    }
    let reuse_bench = t5.elapsed();
    println!(
        "  100 iterations (reuse + SystemReset) in {:?} (avg {:?})",
        reuse_bench,
        reuse_bench / 100
    );

    // ==== DETAILED PER-FRAME PROFILING ====
    println!("\n--- Per-frame timing & voice count profile (single iteration) ---");
    let mut ch = VoiceChannel::new(Default::default(), stream_params, None);
    ch.process_event(ChannelEvent::Config(ChannelConfigEvent::SetSoundfonts(
        soundfonts.clone(),
    )));
    ch.process_event(ChannelEvent::Config(ChannelConfigEvent::SetLayerCount(
        None,
    )));

    let mut frame_times = Vec::with_capacity(30);
    for frame in 0..30 {
        for i in 0..127u8 {
            ch.process_event(ChannelEvent::Audio(ChannelAudioEvent::NoteOn {
                key: i,
                vel: 127,
            }));
        }
        let t = Instant::now();
        ch.read_samples(&mut buffer);
        let elapsed = t.elapsed();
        frame_times.push(elapsed);
        for i in 0..127u8 {
            ch.process_event(ChannelEvent::Audio(ChannelAudioEvent::NoteOff { key: i }));
        }

        if !(5..25).contains(&frame) || frame % 5 == 0 {
            let stats = ch.get_channel_stats();
            let total_voices = stats.voice_count();
            println!(
                "  Frame {:2}: {:8.1?} | total_voices={}",
                frame, elapsed, total_voices
            );
        }
    }

    let total: std::time::Duration = frame_times.iter().sum();
    println!("  Total frame time: {:?} (avg {:?})", total, total / 30);
    println!(
        "  Fastest frame: {:?}  Slowest frame: {:?}",
        frame_times.iter().min().unwrap(),
        frame_times.iter().max().unwrap()
    );

    // ==== VOICE COUNT vs PERFORMANCE ====
    // Test how layer count (max voices per key) affects frame time
    println!("\n--- Voice count vs frame time (steady-state) ---");
    for &layers in &[None, Some(1), Some(2), Some(4), Some(8), Some(16)] {
        let mut ch = VoiceChannel::new(Default::default(), stream_params, None);
        ch.process_event(ChannelEvent::Config(ChannelConfigEvent::SetSoundfonts(
            soundfonts.clone(),
        )));
        ch.process_event(ChannelEvent::Config(ChannelConfigEvent::SetLayerCount(
            layers,
        )));

        // Warm up: build steady-state voice count
        for _ in 0..20 {
            for i in 0..127u8 {
                ch.process_event(ChannelEvent::Audio(ChannelAudioEvent::NoteOn {
                    key: i,
                    vel: 127,
                }));
            }
            ch.read_samples(&mut buffer);
            for i in 0..127u8 {
                ch.process_event(ChannelEvent::Audio(ChannelAudioEvent::NoteOff { key: i }));
            }
        }

        // Measure 50 steady-state frames
        let t = Instant::now();
        for _ in 0..50 {
            for i in 0..127u8 {
                ch.process_event(ChannelEvent::Audio(ChannelAudioEvent::NoteOn {
                    key: i,
                    vel: 127,
                }));
            }
            ch.read_samples(&mut buffer);
            for i in 0..127u8 {
                ch.process_event(ChannelEvent::Audio(ChannelAudioEvent::NoteOff { key: i }));
            }
        }
        let elapsed = t.elapsed() / 50;
        let stats = ch.get_channel_stats();
        println!(
            "  layers={:4?} voices={:4} frame_time={:8.1?}",
            layers,
            stats.voice_count(),
            elapsed
        );
    }

    // ==== BREAKDOWN: rendering vs sum vs effects ====
    println!("\n--- Breakdown: render vs sum vs effects ---");

    // Test 1: normal benchmark frame (steady-state, frame 15 in reuse)
    let mut ch1 = VoiceChannel::new(Default::default(), stream_params, None);
    ch1.process_event(ChannelEvent::Config(ChannelConfigEvent::SetSoundfonts(
        soundfonts.clone(),
    )));
    ch1.process_event(ChannelEvent::Config(ChannelConfigEvent::SetLayerCount(
        None,
    )));

    // Warm up: build up to 15 frames of voice state
    for _ in 0..15 {
        for i in 0..127u8 {
            ch1.process_event(ChannelEvent::Audio(ChannelAudioEvent::NoteOn {
                key: i,
                vel: 127,
            }));
        }
        ch1.read_samples(&mut buffer);
        for i in 0..127u8 {
            ch1.process_event(ChannelEvent::Audio(ChannelAudioEvent::NoteOff { key: i }));
        }
    }

    // Now measure one frame's read_samples time
    for i in 0..127u8 {
        ch1.process_event(ChannelEvent::Audio(ChannelAudioEvent::NoteOn {
            key: i,
            vel: 127,
        }));
    }
    let t_effects = Instant::now();
    ch1.read_samples(&mut buffer);
    let full_frame_time = t_effects.elapsed();
    for i in 0..127u8 {
        ch1.process_event(ChannelEvent::Audio(ChannelAudioEvent::NoteOff { key: i }));
    }

    // Test 2: measure event-injection + rendering WITHOUT effects
    // We do this by zeroing out volume before read_samples (so effects are near-zero cost)
    let mut ch2 = VoiceChannel::new(Default::default(), stream_params, None);
    ch2.process_event(ChannelEvent::Config(ChannelConfigEvent::SetSoundfonts(
        soundfonts.clone(),
    )));
    ch2.process_event(ChannelEvent::Config(ChannelConfigEvent::SetLayerCount(
        None,
    )));

    for _ in 0..15 {
        for i in 0..127u8 {
            ch2.process_event(ChannelEvent::Audio(ChannelAudioEvent::NoteOn {
                key: i,
                vel: 127,
            }));
        }
        ch2.read_samples(&mut buffer);
        for i in 0..127u8 {
            ch2.process_event(ChannelEvent::Audio(ChannelAudioEvent::NoteOff { key: i }));
        }
    }

    // Set volume to 0 so effects are minimal
    ch2.process_event(ChannelEvent::Audio(ChannelAudioEvent::Control(
        xsynth_core::channel::ControlEvent::Raw(0x07, 0),
    )));
    ch2.read_samples(&mut buffer); // let it take effect

    for i in 0..127u8 {
        ch2.process_event(ChannelEvent::Audio(ChannelAudioEvent::NoteOn {
            key: i,
            vel: 127,
        }));
    }
    let t_no_effects = Instant::now();
    ch2.read_samples(&mut buffer);
    let _no_effects_frame_time = t_no_effects.elapsed();
    for i in 0..127u8 {
        ch2.process_event(ChannelEvent::Audio(ChannelAudioEvent::NoteOff { key: i }));
    }

    // But voices still render! Volume=0 just means effects multiply by 0.
    // Better approach: measure event-processing cost separately from rendering.

    // Test 3: Just process events without rendering (empty output)
    let mut ch3 = VoiceChannel::new(Default::default(), stream_params, None);
    ch3.process_event(ChannelEvent::Config(ChannelConfigEvent::SetSoundfonts(
        soundfonts.clone(),
    )));
    ch3.process_event(ChannelEvent::Config(ChannelConfigEvent::SetLayerCount(
        None,
    )));

    for _ in 0..15 {
        for i in 0..127u8 {
            ch3.process_event(ChannelEvent::Audio(ChannelAudioEvent::NoteOn {
                key: i,
                vel: 127,
            }));
        }
        ch3.read_samples(&mut buffer);
        for i in 0..127u8 {
            ch3.process_event(ChannelEvent::Audio(ChannelAudioEvent::NoteOff { key: i }));
        }
    }

    // Measure just the event processing (NoteOn + NoteOff), no read_samples
    let t_events = Instant::now();
    for _ in 0..100 {
        for i in 0..127u8 {
            ch3.process_event(ChannelEvent::Audio(ChannelAudioEvent::NoteOn {
                key: i,
                vel: 127,
            }));
        }
        for i in 0..127u8 {
            ch3.process_event(ChannelEvent::Audio(ChannelAudioEvent::NoteOff { key: i }));
        }
    }
    let event_time = t_events.elapsed() / 100;

    // Test 4: measure rendering cost without voice spawning
    // Create a channel with NO voices and measure read_samples time
    let mut ch4 = VoiceChannel::new(Default::default(), stream_params, None);
    ch4.process_event(ChannelEvent::Config(ChannelConfigEvent::SetSoundfonts(
        soundfonts.clone(),
    )));

    let t_empty = Instant::now();
    for _ in 0..1000 {
        ch4.read_samples(&mut buffer);
    }
    let empty_render_time = t_empty.elapsed() / 1000;

    println!("  Events only (127 ON+OFF)  : {:8.1?}", event_time);
    println!("  Empty render (no voices)   : {:8.1?}", empty_render_time);
    println!("  Full frame (steady state)  : {:8.1?}", full_frame_time);
    // Estimated breakdown:
    let events_cost = event_time;
    let setup_cost = empty_render_time; // memset + for_each overhead
    let render_cost = full_frame_time
        .checked_sub(events_cost)
        .and_then(|t| t.checked_sub(setup_cost))
        .unwrap_or(full_frame_time);
    println!("\n  Estimated breakdown:");
    println!("    Voice spawning + event handling: {:8.1?}", events_cost);
    println!("    Zero-fill + pipeline overhead   : {:8.1?}", setup_cost);
    println!("    Voice rendering + sum + effects : {:8.1?}", render_cost);
}
