use std::{
    io::BufWriter,
    path::PathBuf,
    sync::Arc,
};

use hound::{SampleFormat, WavSpec, WavWriter};
use thiserror::Error;
use xsynth_core::{
    AudioPipe, AudioStreamParams,
    channel::{ChannelConfigEvent, ChannelEvent},
    channel_group::{ChannelGroup, ChannelGroupConfig, SynthEvent},
    effects::VolumeLimiter,
    soundfont::SoundfontBase,
};

#[derive(Clone, Debug, PartialEq)]
pub struct OfflineRenderConfig {
    pub group_options: ChannelGroupConfig,
    pub use_limiter: bool,
}

#[derive(Debug, Error)]
pub enum OfflineRenderError {
    #[error("wav error: {0}")]
    Wav(#[from] hound::Error),
}

pub struct OfflineWavRenderer {
    config: OfflineRenderConfig,
    channel_group: ChannelGroup,
    writer: WavWriter<BufWriter<std::fs::File>>,
    limiter: Option<VolumeLimiter>,
    scratch: Vec<f32>,
    missed_samples: f64,
    frames_written: u64,
}

impl OfflineWavRenderer {
    pub fn new(
        config: OfflineRenderConfig,
        out_path: PathBuf,
        soundfonts: Vec<Arc<dyn SoundfontBase>>,
        layers: Option<usize>,
    ) -> Result<Self, OfflineRenderError> {
        let mut channel_group = ChannelGroup::new(config.group_options.clone());
        channel_group.send_event(SynthEvent::AllChannels(ChannelEvent::Config(
            ChannelConfigEvent::SetSoundfonts(soundfonts),
        )));
        channel_group.send_event(SynthEvent::AllChannels(ChannelEvent::Config(
            ChannelConfigEvent::SetLayerCount(layers),
        )));

        let spec = WavSpec {
            channels: config.group_options.audio_params.channels.count(),
            sample_rate: config.group_options.audio_params.sample_rate,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };
        let writer = WavWriter::create(out_path, spec)?;

        let limiter = if config.use_limiter {
            Some(VolumeLimiter::new(
                config.group_options.audio_params.channels.count(),
            ))
        } else {
            None
        };

        Ok(Self {
            config,
            channel_group,
            writer,
            limiter,
            scratch: vec![0.0],
            missed_samples: 0.0,
            frames_written: 0,
        })
    }

    pub fn get_params(&self) -> AudioStreamParams {
        self.config.group_options.audio_params
    }

    pub fn send_event(&mut self, event: SynthEvent) {
        self.channel_group.send_event(event);
    }

    pub fn render_batch(&mut self, event_time: f64) -> Result<(), OfflineRenderError> {
        if event_time > 10.0 {
            let mut remaining_time = event_time;
            while remaining_time > 10.0 {
                self.render_batch(10.0)?;
                remaining_time -= 10.0;
            }
            if remaining_time > 0.0 {
                self.render_batch(remaining_time)?;
            }
            return Ok(());
        }

        let samples =
            self.config.group_options.audio_params.sample_rate as f64 * event_time + self.missed_samples;
        self.missed_samples = samples % 1.0;
        let sample_count =
            samples as usize * self.config.group_options.audio_params.channels.count() as usize;

        self.scratch.resize(sample_count, 0.0);
        self.channel_group.read_samples(&mut self.scratch);
        if let Some(limiter) = &mut self.limiter {
            limiter.limit(&mut self.scratch);
        }
        self.write_scratch()?;
        Ok(())
    }

    pub fn finalize(mut self) -> Result<u64, OfflineRenderError> {
        loop {
            self.scratch.resize(
                self.config.group_options.audio_params.sample_rate as usize
                    * self.config.group_options.audio_params.channels.count() as usize,
                0.0,
            );
            self.channel_group.read_samples(&mut self.scratch);
            if let Some(limiter) = &mut self.limiter {
                limiter.limit(&mut self.scratch);
            }
            if self.scratch.iter().all(|sample| sample.abs() <= 0.0001) {
                break;
            }
            self.write_scratch()?;
        }
        self.writer.finalize()?;
        Ok(self.frames_written)
    }

    pub fn voice_count(&self) -> u64 {
        self.channel_group.voice_count()
    }

    pub fn frames_written(&self) -> u64 {
        self.frames_written
    }

    fn write_scratch(&mut self) -> Result<(), OfflineRenderError> {
        let channels = self.config.group_options.audio_params.channels.count() as usize;
        self.frames_written += (self.scratch.len() / channels) as u64;
        for sample in self.scratch.drain(..) {
            self.writer.write_sample(sample)?;
        }
        Ok(())
    }
}
