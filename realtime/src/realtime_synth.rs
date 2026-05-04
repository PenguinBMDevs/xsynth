use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    thread::{self},
};

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Device, Host, PauseStreamError, PlayStreamError, SizedSample, Stream, SupportedStreamConfig,
};
use crossbeam_channel::{bounded, unbounded};

use xsynth_core::{
    buffered_renderer::{BufferedRenderer, BufferedRendererStatsReader},
    channel::{ChannelConfigEvent, ChannelEvent, VoiceChannel},
    channel_group::SynthFormat,
    effects::VolumeLimiter,
    helpers::{fast_zero_fill, sum_simd},
    AudioPipe, AudioStreamParams, FunctionAudioPipe,
};

use crate::{RealtimeEventSender, SynthEvent, ThreadCount, XSynthRealtimeConfig};

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

    stream_params: AudioStreamParams,
}

impl RealtimeSynth {
    /// Selects the audio host, preferring JACK on Linux when available.
    ///
    /// On Linux, if cpal was compiled with the `jack` feature and a JACK daemon
    /// is running, this returns the JACK host. Otherwise falls back to the
    /// platform default host (ALSA on Linux).
    ///
    /// We verify the host by attempting to get a default output device:
    /// `host_from_id()` may succeed even when jackd is not running, because
    /// the JACK library can be loaded without an active server connection.
    /// Only a subsequent `default_output_device()` call reveals the runtime failure.
    ///
    /// Override via `XSYNTH_AUDIO_BACKEND=alsa` environment variable to force ALSA
    /// when JACK causes issues (e.g. port connection failures).
    fn choose_host() -> Host {
        #[cfg(target_os = "linux")]
        {
            // Allow env override to skip JACK detection entirely
            if std::env::var("XSYNTH_AUDIO_BACKEND")
                .map(|v| v.eq_ignore_ascii_case("alsa"))
                .unwrap_or(false)
            {
                println!("XSYNTH_AUDIO_BACKEND=alsa, skipping JACK detection");
                return cpal::default_host();
            }

            let available = cpal::available_hosts();
            if available.contains(&cpal::HostId::Jack) {
                match cpal::host_from_id(cpal::HostId::Jack) {
                    Ok(host) => {
                        // Verify JACK is actually usable — host_from_id can succeed
                        // even when jackd is not running (library loaded, no server).
                        if let Some(device) = host.default_output_device() {
                            if let Ok(name) = device.name() {
                                println!(
                                    "Using JACK audio backend (device: {})",
                                    name
                                );
                            } else {
                                println!("Using JACK audio backend");
                            }
                            return host;
                        }
                        eprintln!(
                            "WARNING: JACK host found but no output device (jackd not running?), \
                             falling back to ALSA"
                        );
                    }
                    Err(e) => {
                        eprintln!(
                            "WARNING: JACK host unavailable ({}), falling back to ALSA",
                            e
                        );
                    }
                }
            } else {
                println!("JACK not available, using default audio backend (ALSA)");
            }
        }
        cpal::default_host()
    }

    /// Initializes a new realtime synthesizer using the default config and
    /// the default audio output.
    pub fn open_with_all_defaults() -> Self {
        let host = Self::choose_host();

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
        let host = Self::choose_host();

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
    /// See the `XSynthRealtimeConfig` documentation for the available options.
    /// See the `cpal` crate documentation for the `device` and `stream_config` parameters.
    pub fn open(
        config: XSynthRealtimeConfig,
        device: &Device,
        stream_config: SupportedStreamConfig,
    ) -> Self {
        let mut channel_stats = Vec::new();
        let mut senders = Vec::new();
        let mut command_senders = Vec::new();

        let sample_rate = stream_config.sample_rate().0;
        let stream_params = AudioStreamParams::new(sample_rate, stream_config.channels().into());

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

        let channel_count = match config.format {
            SynthFormat::Midi => 16,
            SynthFormat::Custom { channels } => channels,
        };

        let (output_sender, output_receiver) = bounded::<Vec<f32>>(channel_count as usize);

        let mut thread_handles = vec![];

        for _ in 0u32..channel_count {
            let mut channel =
                VoiceChannel::new(config.channel_init_options, stream_params, pool.clone());
            let stats = channel.get_channel_stats();
            channel_stats.push(stats);

            let (event_sender, event_receiver) = unbounded();
            senders.push(event_sender);

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

            thread_handles.push(join_handle);
        }

        if config.format == SynthFormat::Midi {
            senders[9]
                .send(ChannelEvent::Config(ChannelConfigEvent::SetPercussionMode(
                    true,
                )))
                .unwrap();
        }

        let mut vec_cache: VecDeque<Vec<f32>> = VecDeque::new();
        for _ in 0..channel_count {
            vec_cache.push_front(Vec::new());
        }

        let stats = RealtimeSynthStats::new();

        let total_voice_count = stats.voice_count.clone();

        let render = FunctionAudioPipe::new(stream_params, move |out| {
            // Dispatch phase: send pre-zeroed buffers to each channel thread
            for sender in command_senders.iter() {
                let mut buf = vec_cache.pop_front().unwrap();
                fast_zero_fill(&mut buf, out.len());
                sender.send(buf).unwrap();
            }

            // Collect phase: receive rendered buffers and sum into output
            for _ in 0..channel_count {
                let buf = output_receiver.recv().unwrap();
                sum_simd(&buf, out);
                vec_cache.push_front(buf);
            }

            let total_voices = channel_stats.iter().map(|c| c.voice_count()).sum();
            total_voice_count.store(total_voices, Ordering::Relaxed);
        });

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

        fn build_stream<T: SizedSample + ConvertSample>(
            device: &Device,
            stream_config: SupportedStreamConfig,
            buffered: Arc<std::sync::Mutex<BufferedRenderer>>,
        ) -> Stream {
            let err_fn = |err: cpal::StreamError| {
                match &err {
                    // BackendSpecificError from JACK buffer_size changes are benign.
                    // JACK may change buffer size at runtime (e.g. 1024 frames),
                    // and the cpal JACK backend reallocates temp buffers correctly.
                    cpal::StreamError::BackendSpecific { .. } => {
                        // Decode the description to filter buffer-size notifications
                        let desc = format!("{err}");
                        if desc.contains("buffer size changed") {
                            // Benign JACK notification — don't alarm the user
                            eprintln!("[xsynth] audio buffer size changed: {}", desc);
                        } else {
                            eprintln!("[xsynth] audio backend error: {err}");
                        }
                    }
                    _ => {
                        eprintln!("[xsynth] audio stream error: {err}");
                    }
                }
            };
            let mut output_vec = Vec::new();

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

        let stream = match stream_config.sample_format() {
            cpal::SampleFormat::F32 => build_stream::<f32>(device, stream_config, buffered.clone()),
            cpal::SampleFormat::I16 => build_stream::<i16>(device, stream_config, buffered.clone()),
            cpal::SampleFormat::U16 => build_stream::<u16>(device, stream_config, buffered.clone()),
            _ => panic!("unsupported sample format"),
        };

        stream.play().unwrap();

        let max_nps = Arc::new(AtomicU64::new(10000));

        Self {
            data: Some(RealtimeSynthThreadSharedData {
                buffered_renderer: buffered,
                buffered_stats,
                event_senders: RealtimeEventSender::new(senders, max_nps, config.ignore_range),
                stream: SendSyncStream(stream),
            }),
            join_handles: thread_handles,
            stats,
            stream_params,
        }
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
