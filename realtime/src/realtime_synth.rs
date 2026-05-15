use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    }
};

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Device, PauseStreamError, PlayStreamError, SizedSample, Stream, SupportedStreamConfig,
};
use crossbeam_channel::unbounded;

#[cfg(feature = "pipewire")]
fn select_host() -> cpal::Host {
    match cpal::host_from_id(cpal::HostId::Jack) {
        Ok(host) => {
            println!("Audio host: PipeWire (via JACK API)");
            host
        }
        Err(_) => {
            let host = cpal::default_host();
            println!("Audio host: ALSA (via PipeWire ALSA compatibility)");
            host
        }
    }
}

#[cfg(not(feature = "pipewire"))]
fn select_host() -> cpal::Host {
    cpal::default_host()
}

use xsynth_core::{
    buffered_renderer::{BufferedRenderer, BufferedRendererStatsReader},
    channel::{ChannelConfigEvent, ChannelEvent, VoiceChannel},
    channel_group::SynthFormat,
    effects::VolumeLimiter,
    helpers::{fast_zero_fill, sum_simd},
    AudioPipe, AudioStreamParams, FunctionAudioPipe,
};

use crate::{
    util::ReadWriteAtomicU64, RealtimeEventSender, SynthEvent, ThreadCount, XSynthRealtimeConfig,
};

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
    stream: SendSyncStream,
    event_senders: RealtimeEventSender,
}

/// A realtime MIDI synthesizer using an audio device for output.
pub struct RealtimeSynth {
    data: Option<RealtimeSynthThreadSharedData>,

    stats: RealtimeSynthStats,

    stream_params: AudioStreamParams,
}

impl RealtimeSynth {
    /// Initializes a new realtime synthesizer using the default config and
    /// the default audio output.
    pub fn open_with_all_defaults() -> Self {
        let host = select_host();

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
        let host = select_host();

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

        let mut channels = Vec::new();
        let mut event_receivers = Vec::new();

        for _ in 0u32..channel_count {
            let channel =
                VoiceChannel::new(config.channel_init_options, stream_params, pool.clone());
            let stats = channel.get_channel_stats();
            channel_stats.push(stats);

            let (event_sender, event_receiver) = unbounded();
            senders.push(event_sender);
            
            channels.push(channel);
            event_receivers.push(event_receiver);
        }

        if config.format == SynthFormat::Midi {
            senders[9]
                .send(ChannelEvent::Config(ChannelConfigEvent::SetPercussionMode(
                    true,
                )))
                .unwrap();
        }

        let mut vec_cache: Vec<Vec<f32>> = Vec::new();
        for _ in 0..channel_count {
            vec_cache.push(Vec::new());
        }

        let stats = RealtimeSynthStats::new();

        let total_voice_count = stats.voice_count.clone();

        let render = FunctionAudioPipe::new(stream_params, move |out| {
            if let Some(pool) = &pool {
                use rayon::prelude::*;
                pool.install(|| {
                    channels
                        .par_iter_mut()
                        .zip(event_receivers.par_iter())
                        .zip(vec_cache.par_iter_mut())
                        .for_each(|((channel, event_receiver), buf)| {
                            channel.push_events_iter(event_receiver.try_iter());
                            fast_zero_fill(buf, out.len());
                            channel.read_samples(buf);
                        });
                });
            } else {
                for ((channel, event_receiver), buf) in channels
                    .iter_mut()
                    .zip(event_receivers.iter())
                    .zip(vec_cache.iter_mut())
                {
                    channel.push_events_iter(event_receiver.try_iter());
                    fast_zero_fill(buf, out.len());
                    channel.read_samples(buf);
                }
            }

            for buf in vec_cache.iter() {
                sum_simd(buf, out);
            }

            let total_voices = channel_stats.iter().map(|c| c.voice_count()).sum();
            total_voice_count.store(total_voices, Ordering::Relaxed);
        });

        let buffered = Arc::new(std::sync::Mutex::new(BufferedRenderer::new(
            render,
            stream_params,
            calculate_render_size(sample_rate, config.render_window_ms),
        )));

        fn build_stream<T: SizedSample + ConvertSample>(
            device: &Device,
            stream_config: SupportedStreamConfig,
            buffered: Arc<std::sync::Mutex<BufferedRenderer>>,
        ) -> Stream {
            let err_fn = |err| eprintln!("an error occurred on stream: {err}");
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

        let max_nps = Arc::new(ReadWriteAtomicU64::new(10000));

        Self {
            data: Some(RealtimeSynthThreadSharedData {
                buffered_renderer: buffered,

                event_senders: RealtimeEventSender::new(senders, max_nps, config.ignore_range),
                stream: SendSyncStream(stream),
            }),

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
        let buffered_stats = data.buffered_renderer.lock().unwrap().get_buffer_stats();

        RealtimeSynthStatsReader::new(self.stats.clone(), buffered_stats)
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
