use std::sync::{
    atomic::AtomicU64,
    Arc,
};

use super::{
    channel_sf::ChannelSoundfont, event::KeyNoteEvent, voice_buffer::VoiceBuffer,
    ChannelInitOptions, VoiceControlData,
};

pub struct KeyData {
    key: u8,
    voices: VoiceBuffer,
    /// Shared counter used to pass to VoiceBuffer for global voice tracking.
    /// Only used at construction time; VoiceBuffer manages the counter internally.
    _shared_voice_counter: Arc<AtomicU64>,
}

impl KeyData {
    pub fn new(
        key: u8,
        shared_voice_counter: Arc<AtomicU64>,
        options: ChannelInitOptions,
    ) -> KeyData {
        KeyData {
            key,
            voices: VoiceBuffer::new(options, shared_voice_counter.clone()),
            _shared_voice_counter: shared_voice_counter,
        }
    }

    #[inline(always)]
    pub fn send_event(
        &mut self,
        event: KeyNoteEvent,
        control: &VoiceControlData,
        channel_sf: &ChannelSoundfont,
    ) {
        match event {
            KeyNoteEvent::On(vel) => {
                let voices = channel_sf.spawn_voices_attack(control, self.key, vel);
                self.voices.push_voices(voices);
            }
            KeyNoteEvent::Off => {
                let vel = self.voices.release_next_voice();
                if let Some(vel) = vel {
                    let voices = channel_sf.spawn_voices_release(control, self.key, vel);
                    self.voices.push_voices(voices);
                }
            }
            KeyNoteEvent::AllOff => {
                while let Some(vel) = self.voices.release_next_voice() {
                    let voices = channel_sf.spawn_voices_release(control, self.key, vel);
                    self.voices.push_voices(voices);
                }
            }
            KeyNoteEvent::AllKilled => {
                self.voices.kill_all_voices();
            }
        }
    }

    #[inline(always)]
    pub fn process_controls(&mut self, control: &VoiceControlData) {
        for voice in &mut self.voices.iter_voices_mut() {
            voice.process_controls(control);
        }
    }

    /// Ultra-optimized sequential rendering
    /// Each voice adds directly to the output buffer.
    /// Uses 2-pass approach: pre-remove ended → render → post-remove ended.
    /// Global voice counter is managed by VoiceBuffer operations internally.
    #[inline(always)]
    pub fn render_to(&mut self, out: &mut [f32]) {
        // Pre-remove voices that already ended on the previous frame.
        // This reduces iteration overhead in the main render loop.
        self.voices.remove_ended_voices();

        let voice_count = self.voices.voice_count();
        if voice_count == 0 {
            return;
        }

        // Direct sequential rendering — all remaining voices are alive
        let voices = self.voices.get_voices_mut();
        for voice in voices.iter_mut() {
            voice.voice.render_to(out);
        }

        // Remove any voices that finished during this render call
        self.voices.remove_ended_voices();
    }

    #[inline(always)]
    pub fn has_voices(&self) -> bool {
        self.voices.has_voices()
    }

    #[inline(always)]
    pub fn set_damper(&mut self, damper: bool) {
        self.voices.set_damper(damper);
    }

    /// Set the maximum number of voices per key. None means no limit.
    #[inline(always)]
    pub fn set_max_voices(&mut self, max: Option<usize>) {
        self.voices.set_max_voices(max);
    }
}
