use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicI64, AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Device, PauseStreamError, PlayStreamError, SizedSample, Stream, SupportedStreamConfig,
};
use crossbeam_channel::{bounded, unbounded};

use xsynth_core::{
    buffered_renderer::{BufferedRenderer, BufferedRendererStatsReader},
    channel::{ChannelConfigEvent, VoiceChannel},
    channel_group::{ChannelGroup, ChannelGroupConfig, ParallelismOptions, SynthFormat},
    effects::VolumeLimiter,
    helpers::{fast_zero_fill, sum_simd},
    AudioPipe, AudioStreamParams, FunctionAudioPipe,
};

use crate::{RealtimeEventSender, RealtimeRenderMode, SynthEvent, ThreadCount, XSynthRealtimeConfig};

/// Holds the statistics for an instance of RealtimeSynth.
#[derive(Debug, Clone)]
struct RealtimeSynthStats {
    voice_count: Arc<AtomicU64>,
}

impl RealtimeSynthStats {
    pub fn new() -> RealtimeSynthStats {
        RealtimeSynthStats {
            voice_count: Arc::new(AtomicU64::new(0)),
        }
    }
}

/// Reads the statistics of an instance of RealtimeSynth in a usable way.
pub struct RealtimeSynthStatsReader {
    buffered_stats: BufferedRendererStatsReader,
    stats: RealtimeSynthStats,
}

impl RealtimeSynthStatsReader {
    pub(self) fn new(
        stats: RealtimeSynthStats,
        buffered_stats: BufferedRendererStatsReader,
    ) -> RealtimeSynthStatsReader {
        RealtimeSynthStatsReader {
            stats,
            buffered_stats,
        }
    }

    /// Returns the active voice count of all the MIDI channels.
    pub fn voice_count(&self) -> u64 {
        self.stats.voice_count.load(Ordering::Relaxed)
    }

    /// Returns the statistics of the buffered renderer used.
    ///
    /// See the BufferedRendererStatsReader documentation for more information.
    pub fn buffer(&self) -> &BufferedRendererStatsReader {
        &self.buffered_stats
    }
}

/// Performance statistics for a single render cycle.
#[derive(Debug, Clone, Copy)]
pub struct RenderPerfStats {
    /// Duration of the last render call in microseconds.
    pub last_render_us: i64,
    /// Peak render duration seen so far, in microseconds.
    pub peak_render_us: i64,
    /// Number of MIDI events drained in the last render call.
    pub last_event_count: u64,
}

/// Shared mutable performance counters accessible from the render thread.
struct RenderPerfShared {
    last_render_ns: AtomicI64,
    peak_render_ns: AtomicI64,
    last_event_count: AtomicU64,
}

impl RenderPerfShared {
    fn new() -> Self {
        Self {
            last_render_ns: AtomicI64::new(0),
            peak_render_ns: AtomicI64::new(0),
            last_event_count: AtomicU64::new(0),
        }
    }

    fn snapshot(&self) -> RenderPerfStats {
        let last = self.last_render_ns.load(Ordering::Relaxed);
        let peak = self.peak_render_ns.load(Ordering::Relaxed);
        RenderPerfStats {
            last_render_us: last / 1000,
            peak_render_us: peak / 1000,
            last_event_count: self.last_event_count.load(Ordering::Relaxed),
        }
    }
}

// A helper for making the stream be send/sync, allowing the entire synth to be passed between threads.
// The stream is never actually accessed from multiple threads, it's only stored for ownership and then dropped.
struct SendSyncStream(Stream);
unsafe impl Sync for SendSyncStream {}
unsafe impl Send for SendSyncStream {}

struct RealtimeSynthThreadSharedData {
    buffered_renderer: Arc<std::sync::Mutex<BufferedRenderer>>,
    /// Pre-cloned stats reader to avoid locking the Mutex in the audio callback path.
    /// All fields behind BufferedRendererStats are Arc<Atomic*> — cloning is cheap.
    buffered_stats: BufferedRendererStatsReader,
    stream: SendSyncStream,
    event_senders: RealtimeEventSender,
}

/// A realtime MIDI synthesizer using an audio device for output.
pub struct RealtimeSynth {
    data: Option<RealtimeSynthThreadSharedData>,
    join_handles: Vec<thread::JoinHandle<()>>,

    stats: RealtimeSynthStats,
    perf: Arc<RenderPerfShared>,

    stream_params: AudioStreamParams,
}

impl RealtimeSynth {
    /// Initializes a new realtime synthesizer using the default config and
    /// the default audio output.
    pub fn open_with_all_defaults() -> Self {
        let host = cpal::default_host();

        let device = host
            .default_output_device()
            .expect("failed to find output device");
        println!("Output device: {}", device.name().unwrap());

        let stream_config = device.default_output_config().unwrap();

        RealtimeSynth::open(Default::default(), &device, stream_config)
    }

    /// Initializes as new realtime synthesizer using a given config and
    /// the default audio output.
    ///
    /// See the `XSynthRealtimeConfig` documentation for the available options.
    pub fn open_with_default_output(config: XSynthRealtimeConfig) -> Self {
        let host = cpal::default_host();

        let device = host
            .default_output_device()
            .expect("failed to find output device");
        println!("Output device: {}", device.name().unwrap());

        let stream_config = device.default_output_config().unwrap();

        RealtimeSynth::open(config, &device, stream_config)
    }

    /// Initializes a new realtime synthesizer using a given config and a
    /// specified audio output device.
    ///
    /// See the `XSynthRealtimeConfig` documentation for more information.
    /// See the `cpal` crate documentation for the `device` and `stream_config` parameters.
    pub fn open(
        config: XSynthRealtimeConfig,
        device: &Device,
        stream_config: SupportedStreamConfig,
    ) -> Self {
        let sample_rate = stream_config.sample_rate().0;
        let stream_params = AudioStreamParams::new(sample_rate, stream_config.channels().into());

        let stats = RealtimeSynthStats::new();
        let total_voice_count = stats.voice_count.clone();

        let perf = Arc::new(RenderPerfShared::new());

        let channel_count = match config.format {
            SynthFormat::Midi => 16,
            SynthFormat::Custom { channels } => channels,
        };

        let max_nps = Arc::new(AtomicU64::new(config.max_nps));

        let (render, event_senders, thread_handles) = match config.render_mode {
            RealtimeRenderMode::ChannelGroup => Self::build_channel_group_render(
                &config,
                stream_params,
                channel_count,
                total_voice_count.clone(),
                max_nps.clone(),
                perf.clone(),
            ),
            RealtimeRenderMode::Threaded => Self::build_threaded_render(
                &config,
                stream_params,
                channel_count,
                total_voice_count.clone(),
                max_nps.clone(),
            ),
        };

        let buffered_renderer = BufferedRenderer::new(
            render,
            stream_params,
            calculate_render_size(sample_rate, config.render_window_ms),
        );
        // Pre-clone stats reader before wrapping in Mutex.
        // This allows get_stats() to read stats without locking, avoiding
        // priority inversion in the audio callback path.
        let buffered_stats = buffered_renderer.get_buffer_stats();
        let buffered = Arc::new(std::sync::Mutex::new(buffered_renderer));

        let stream = match stream_config.sample_format() {
            cpal::SampleFormat::F32 => build_stream::<f32>(device, stream_config, buffered.clone()),
            cpal::SampleFormat::I16 => build_stream::<i16>(device, stream_config, buffered.clone()),
            cpal::SampleFormat::U16 => build_stream::<u16>(device, stream_config, buffered.clone()),
            _ => panic!("unsupported sample format"),
        };

        stream.play().unwrap();

        Self {
            data: Some(RealtimeSynthThreadSharedData {
                buffered_renderer: buffered,
                buffered_stats,
                event_senders,
                stream: SendSyncStream(stream),
            }),
            join_handles: thread_handles,
            stats,
            perf,
            stream_params,
        }
    }

    /// Build the synchronous ChannelGroup render path with channel-level parallelism.
    ///
    /// This eliminates the 16 per-channel OS threads and the blocking collect phase
    /// that caused audio dropouts on macOS, while still using rayon for channel-level
    /// parallel rendering inside the single render thread.
    fn build_channel_group_render(
        config: &XSynthRealtimeConfig,
        stream_params: AudioStreamParams,
        channel_count: u32,
        total_voice_count: Arc<AtomicU64>,
        max_nps: Arc<AtomicU64>,
        perf: Arc<RenderPerfShared>,
    ) -> (FunctionAudioPipe<Box<dyn FnMut(&mut [f32]) + Send>>, RealtimeEventSender, Vec<thread::JoinHandle<()>>) {
        // Map the legacy `multithreading` option to ChannelGroup channel-level
        // parallelism. When `None`, use Auto so that channel rendering is still
        // parallelised via rayon without creating per-channel OS threads.
        let channel_thread_count = match config.multithreading {
            ThreadCount::None => ThreadCount::Auto,
            ThreadCount::Auto => ThreadCount::Auto,
            ThreadCount::Manual(threads) => ThreadCount::Manual(threads),
        };

        let parallelism = ParallelismOptions {
            channel: channel_thread_count,
            key: ThreadCount::None,
        };

        let cg_config = ChannelGroupConfig {
            channel_init_options: config.channel_init_options,
            format: config.format,
            audio_params: stream_params,
            parallelism,
        };

        let mut channel_group = ChannelGroup::new(cg_config);

        let warn_threshold_us = (config.render_warn_threshold_ms * 1000.0) as i64;

        // Bounded channel acts as natural backpressure: when the channel is full,
        // the playback thread blocks on send(), automatically matching the event
        // production rate to the synth's processing capacity.
        // 65536 allows ~10-20ms of event burst to be buffered without blocking.
        let (event_sender, event_receiver) = crossbeam_channel::bounded::<SynthEvent>(65536);

        let render_fn: Box<dyn FnMut(&mut [f32]) + Send> = Box::new(move |out| {
            let start = Instant::now();

            // Drain ALL pending events from the bounded channel.
            // Because the channel itself limits total in-flight events to 65536,
            // no single render cycle can be overwhelmed. The bounded capacity
            // provides natural backpressure without needing a software count limit.
            let mut event_count = 0u64;
            for event in event_receiver.try_iter() {
                channel_group.send_event(event);
                event_count += 1;
            }

            channel_group.read_samples_unchecked(out);
            total_voice_count.store(channel_group.voice_count(), Ordering::Relaxed);

            let elapsed = start.elapsed();
            let elapsed_ns = elapsed.as_nanos() as i64;
            let elapsed_us = elapsed.as_micros() as i64;

            // Update shared perf counters
            perf.last_render_ns.store(elapsed_ns, Ordering::Relaxed);
            let prev_peak = perf.peak_render_ns.load(Ordering::Relaxed);
            if elapsed_ns > prev_peak {
                perf.peak_render_ns.store(elapsed_ns, Ordering::Relaxed);
            }
            perf.last_event_count.store(event_count, Ordering::Relaxed);

            // Log warning if render exceeds threshold (disabled by user request)
            // if warn_threshold_us > 0 && elapsed_us > warn_threshold_us {
            //     let delayed = event_receiver.len();
            //     let voice_count = channel_group.voice_count();
            //     if delayed > 0 {
            //         eprintln!(
            //             "[xsynth WARN] render slow: {:.2} ms (events={}, voices={}, delayed={}, peak={:.2} ms)",
            //             elapsed_us as f64 / 1000.0,
            //             event_count,
            //             voice_count,
            //             delayed,
            //             elapsed_ns.max(prev_peak) as f64 / 1_000_000.0,
            //         );
            //     } else {
            //         eprintln!(
            //             "[xsynth WARN] render slow: {:.2} ms (voice={}, peak={:.2} ms)",
            //             elapsed_us as f64 / 1000.0,
            //             voice_count,
            //             elapsed_ns.max(prev_peak) as f64 / 1_000_000.0,
            //         );
            //     }
            // }
        });
        let render = FunctionAudioPipe::new(stream_params, render_fn);

        let senders: Vec<_> = (0..channel_count)
            .map(|_| {
                crate::event_senders::EventSender::new(
                    max_nps.clone(),
                    crate::event_senders::EventSenderInner::ChannelGroup(event_sender.clone()),
                    config.ignore_range.clone(),
                )
            })
            .collect();

        let event_senders = RealtimeEventSender::new(senders, RealtimeRenderMode::ChannelGroup);

        (render, event_senders, Vec::new())
    }

    /// Build the legacy threaded render path with one OS thread per channel.
    fn build_threaded_render(
        config: &XSynthRealtimeConfig,
        stream_params: AudioStreamParams,
        channel_count: u32,
        total_voice_count: Arc<AtomicU64>,
        max_nps: Arc<AtomicU64>,
    ) -> (FunctionAudioPipe<Box<dyn FnMut(&mut [f32]) + Send>>, RealtimeEventSender, Vec<thread::JoinHandle<()>>) {
        let pool = match config.multithreading {
            ThreadCount::None => None,
            ThreadCount::Auto => Some(Arc::new(rayon::ThreadPoolBuilder::new().build().unwrap())),
            ThreadCount::Manual(threads) => Some(Arc::new(
                rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .build()
                    .unwrap(),
            )),
        };

        let (output_sender, output_receiver) = bounded::<Vec<f32>>(channel_count as usize);

        let mut channel_stats = Vec::new();
        let mut senders = Vec::new();
        let mut command_senders = Vec::new();
        let mut thread_handles = vec![];

        for _ in 0u32..channel_count {
            let mut channel =
                VoiceChannel::new(config.channel_init_options, stream_params, pool.clone());
            let stats = channel.get_channel_stats();
            channel_stats.push(stats);

            let (event_sender, event_receiver) = unbounded();
            let (command_sender, command_receiver) = bounded::<Vec<f32>>(1);
            command_senders.push(command_sender);

            let output_sender = output_sender.clone();
            let join_handle = thread::Builder::new()
                .name("xsynth_channel_handler".to_string())
                .spawn(move || loop {
                    channel.push_events_iter(event_receiver.try_iter());
                    let mut vec = match command_receiver.recv() {
                        Ok(vec) => vec,
                        Err(_) => break,
                    };
                    channel.push_events_iter(event_receiver.try_iter());
                    channel.read_samples(&mut vec);
                    output_sender.send(vec).unwrap();
                })
                .unwrap();

            senders.push(crate::event_senders::EventSender::new(
                max_nps.clone(),
                crate::event_senders::EventSenderInner::Threaded(event_sender),
                config.ignore_range.clone(),
            ));
            thread_handles.push(join_handle);
        }

        if config.format == SynthFormat::Midi {
            senders[9]
                .send_config(9, ChannelConfigEvent::SetPercussionMode(true));
        }

        let mut vec_cache: VecDeque<Vec<f32>> = VecDeque::new();
        for _ in 0..channel_count {
            vec_cache.push_front(Vec::new());
        }

        let render_fn: Box<dyn FnMut(&mut [f32]) + Send> = Box::new(move |out| {
            // Dispatch phase: send pre-zeroed buffers to each channel thread
            for sender in command_senders.iter() {
                let mut buf = vec_cache.pop_front().unwrap();
                fast_zero_fill(&mut buf, out.len());
                sender.send(buf).unwrap();
            }

            // Collect phase: receive rendered buffers and sum into output.
            // Use try_recv with a short busy-wait bound so that a single slow
            // channel thread cannot stall the entire render pipeline indefinitely.
            let mut received = 0usize;
            let mut spins_without_progress = 0usize;
            while received < channel_count as usize {
                match output_receiver.try_recv() {
                    Ok(buf) => {
                        sum_simd(&buf, out);
                        vec_cache.push_front(buf);
                        received += 1;
                        spins_without_progress = 0;
                    }
                    Err(_) => {
                        spins_without_progress += 1;
                        if spins_without_progress > 1024 {
                            // Back off to avoid burning the CPU if a channel is
                            // temporarily delayed. A short sleep is still far less
                            // damaging than blocking the audio callback directly.
                            thread::sleep(Duration::from_micros(10));
                            spins_without_progress = 0;
                        } else {
                            thread::yield_now();
                        }
                    }
                }
            }

            let total_voices = channel_stats.iter().map(|c| c.voice_count()).sum();
            total_voice_count.store(total_voices, Ordering::Relaxed);
        });
        let render = FunctionAudioPipe::new(stream_params, render_fn);

        let event_senders = RealtimeEventSender::new(senders, RealtimeRenderMode::Threaded);

        (render, event_senders, thread_handles)
    }

    /// Sends a SynthEvent to the realtime synthesizer.
    ///
    /// See the `SynthEvent` documentation for more information.
    pub fn send_event(&mut self, event: SynthEvent) {
        let data = self.data.as_mut().unwrap();
        data.event_senders.send_event(event);
    }

    /// Sends a u32 event to the realtime synthesizer.
    pub fn send_event_u32(&mut self, event: u32) {
        let data = self.data.as_mut().unwrap();
        data.event_senders.send_event_u32(event);
    }

    /// Returns a reference to the event sender of the realtime synthesizer.
    /// This can be used to clone the sender so it can be passed in threads.
    ///
    /// See the `RealtimeEventSender` documentation for more information
    /// on how to use.
    pub fn get_sender_ref(&self) -> &RealtimeEventSender {
        let data = self.data.as_ref().unwrap();
        &data.event_senders
    }

    /// Returns a mutable reference the event sender of the realtime synthesizer.
    /// This can be used to modify its parameters (eg. ignore range).
    /// Please note that each clone will store its own distinct parameters.
    ///
    /// See the `RealtimeEventSender` documentation for more information
    /// on how to use.
    pub fn get_sender_mut(&mut self) -> &mut RealtimeEventSender {
        let data = self.data.as_mut().unwrap();
        &mut data.event_senders
    }

    /// Returns the statistics reader of the realtime synthesizer.
    ///
    /// See the `RealtimeSynthStatsReader` documentation for more information
    /// on how to use.
    pub fn get_stats(&self) -> RealtimeSynthStatsReader {
        let data = self.data.as_ref().unwrap();
        // Uses pre-cloned stats reader — no Mutex lock needed.
        // This avoids priority inversion where the audio callback (which holds
        // the BufferedRenderer lock via read()) would be blocked.
        RealtimeSynthStatsReader::new(self.stats.clone(), data.buffered_stats.clone())
    }

    /// Returns the performance statistics for the last render cycle.
    ///
    /// `RenderPerfStats` includes render timing and event counts.
    /// Useful for diagnosing audio dropouts caused by overloaded rendering.
    pub fn perf_stats(&self) -> RenderPerfStats {
        self.perf.snapshot()
    }

    /// Returns the stream parameters of the audio output device.
    pub fn stream_params(&self) -> AudioStreamParams {
        self.stream_params
    }

    /// Pauses the playback of the audio output device.
    pub fn pause(&mut self) -> Result<(), PauseStreamError> {
        let data = self.data.as_mut().unwrap();
        data.stream.0.pause()
    }

    /// Resumes the playback of the audio output device.
    pub fn resume(&mut self) -> Result<(), PlayStreamError> {
        let data = self.data.as_mut().unwrap();
        data.stream.0.play()
    }

    /// Changes the length of the buffer reader.
    pub fn set_buffer(&self, render_window_ms: f64) {
        let data = self.data.as_ref().unwrap();
        let sample_rate = self.stream_params.sample_rate;
        let size = calculate_render_size(sample_rate, render_window_ms);
        data.buffered_renderer.lock().unwrap().set_render_size(size);
    }
}

impl Drop for RealtimeSynth {
    fn drop(&mut self) {
        let data = self.data.take().unwrap();
        drop(data);
        for handle in self.join_handles.drain(..) {
            handle.join().unwrap();
        }
    }
}

fn build_stream<T: SizedSample + ConvertSample>(
    device: &Device,
    stream_config: SupportedStreamConfig,
    buffered: Arc<std::sync::Mutex<BufferedRenderer>>,
) -> Stream {
    let err_fn = |err| eprintln!("an error occurred on stream: {err}");
    // Pre-allocate the scratch vector to the expected callback size to avoid
    // repeated resize/allocation inside the audio callback hot path.
    let mut output_vec = Vec::with_capacity(
        stream_config.sample_rate().0 as usize * stream_config.channels() as usize / 100,
    );

    let mut limiter = VolumeLimiter::new(stream_config.channels());

    device
        .build_output_stream(
            &stream_config.into(),
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                output_vec.resize(data.len(), 0.0);
                buffered.lock().unwrap().read(&mut output_vec);
                for (i, s) in limiter.limit_iter(output_vec.drain(..)).enumerate() {
                    data[i] = ConvertSample::from_f32(s);
                }
            },
            err_fn,
            None,
        )
        .unwrap()
}

trait ConvertSample: SizedSample {
    fn from_f32(s: f32) -> Self;
}

impl ConvertSample for f32 {
    fn from_f32(s: f32) -> Self {
        s
    }
}

impl ConvertSample for i16 {
    fn from_f32(s: f32) -> Self {
        (s * i16::MAX as f32) as i16
    }
}

impl ConvertSample for u16 {
    fn from_f32(s: f32) -> Self {
        ((s * u16::MAX as f32) as i32 + i16::MIN as i32) as u16
    }
}

fn calculate_render_size(sample_rate: u32, buffer_ms: f64) -> usize {
    (sample_rate as f64 * buffer_ms / 1000.0) as usize
}
