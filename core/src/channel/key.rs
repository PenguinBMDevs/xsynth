use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use super::{
    channel_sf::ChannelSoundfont, event::KeyNoteEvent, voice_buffer::VoiceBuffer,
    ChannelInitOptions, VoiceControlData,
};

pub struct KeyData {
    key: u8,
    voices: VoiceBuffer,
    last_voice_count: usize,
    shared_voice_counter: Arc<AtomicU64>,
}

impl KeyData {
    pub fn new(
        key: u8,
        shared_voice_counter: Arc<AtomicU64>,
        options: ChannelInitOptions,
    ) -> KeyData {
        KeyData {
            key,
            voices: VoiceBuffer::new(options),
            last_voice_count: 0,
            shared_voice_counter,
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
    /// Each voice adds directly to the output buffer
    #[inline(always)]
    pub fn render_to(&mut self, out: &mut [f32]) {
        let voice_count = self.voices.voice_count();
        if voice_count == 0 {
            self.update_voice_counter(0);
            return;
        }

        // Direct sequential rendering - most efficient approach
        // Avoid any intermediate allocations
        let voices = self.voices.get_voices_mut();
        for voice in voices.iter_mut() {
            voice.voice.render_to(out);
        }

        self.voices.remove_ended_voices();
        self.update_voice_counter(self.voices.voice_count());
    }

    #[inline(always)]
    fn update_voice_counter(&mut self, new_count: usize) {
        let change = new_count as i64 - self.last_voice_count as i64;
        if change < 0 {
            self.shared_voice_counter
                .fetch_sub((-change) as u64, Ordering::Relaxed);
        } else if change > 0 {
            self.shared_voice_counter
                .fetch_add(change as u64, Ordering::Relaxed);
        }
        self.last_voice_count = new_count;
    }

    #[inline(always)]
    pub fn has_voices(&self) -> bool {
        self.voices.has_voices()
    }

    #[inline(always)]
    pub fn set_damper(&mut self, damper: bool) {
        self.voices.set_damper(damper);
    }
}
