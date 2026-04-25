use super::ChannelInitOptions;
use crate::voice::{ReleaseType, Voice};
use rustc_hash::FxHashSet;
use std::ops::{Deref, DerefMut};

/// A voice with its group ID for tracking
pub struct GroupVoice {
    pub id: usize,
    pub voice: Box<dyn Voice>,
}

impl GroupVoice {
    #[inline(always)]
    pub fn ended(&self) -> bool {
        self.voice.ended()
    }
}

impl Deref for GroupVoice {
    type Target = Box<dyn Voice>;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &self.voice
    }
}

impl DerefMut for GroupVoice {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Box<dyn Voice> {
        &mut self.voice
    }
}

/// Voice buffer optimized for high voice counts with parallel processing support
pub struct VoiceBuffer {
    options: ChannelInitOptions,
    id_counter: usize,
    // Pre-allocated Vec for better performance with high voice counts
    voices: Vec<GroupVoice>,
    damper_held: bool,
    held_by_damper: FxHashSet<usize>,
    pub max_voices: Option<usize>,
}

impl VoiceBuffer {
    pub fn new(options: ChannelInitOptions) -> Self {
        let max_voices = options.max_voices_per_key;
        VoiceBuffer {
            options,
            id_counter: 0,
            // Pre-allocate for high voice count scenarios
            voices: Vec::with_capacity(256),
            damper_held: false,
            held_by_damper: FxHashSet::default(),
            max_voices,
        }
    }

    #[inline(always)]
    fn get_id(&mut self) -> usize {
        self.id_counter += 1;
        self.id_counter
    }

    /// Fast linear scan to find quietest voice
    fn pop_quietest_voice_group(&mut self, ignored_id: usize) {
        if self.voices.is_empty() {
            return;
        }

        let mut quietest_vel = u8::MAX;
        let mut quietest_id = None;

        for voice in &self.voices {
            if voice.id == ignored_id || voice.is_killed() {
                continue;
            }
            let vel = voice.velocity();
            if vel < quietest_vel {
                quietest_vel = vel;
                quietest_id = Some(voice.id);
                if vel == 0 {
                    break;
                }
            }
        }

        if let Some(id) = quietest_id {
            if self.options.fade_out_killing {
                for voice in &mut self.voices {
                    if voice.id == id {
                        voice.signal_release(ReleaseType::Kill);
                    }
                }
            } else {
                let mut i = 0;
                while i < self.voices.len() {
                    if self.voices[i].id == id {
                        self.voices.swap_remove(i);
                    } else {
                        i += 1;
                    }
                }
            }

            self.held_by_damper.remove(&id);
        }
    }

    pub fn kill_all_voices(&mut self) {
        if self.options.fade_out_killing {
            for voice in &mut self.voices {
                voice.signal_release(ReleaseType::Kill);
            }
        } else {
            self.voices.clear();
        }
        self.held_by_damper.clear();
    }

    #[inline(always)]
    pub fn push_voices(&mut self, voices: impl Iterator<Item = Box<dyn Voice>>) {
        let id = self.get_id();

        for voice in voices {
            self.voices.push(GroupVoice { id, voice });
        }

        if let Some(max_voices) = self.max_voices {
            if self.options.fade_out_killing {
                while self.get_active_count() > max_voices {
                    self.pop_quietest_voice_group(id);
                }
            } else {
                while self.voices.len() > max_voices {
                    self.pop_quietest_voice_group(id);
                }
            }
        }
    }

    fn get_active_count(&self) -> usize {
        self.voices.iter().filter(|v| !v.is_killed()).count()
    }

    pub fn release_next_voice(&mut self) -> Option<u8> {
        if !self.damper_held {
            let mut id: Option<usize> = None;
            let mut vel = None;

            for voice in &mut self.voices {
                if voice.is_releasing() || voice.is_killed() {
                    continue;
                }

                if id.is_none() {
                    id = Some(voice.id);
                    vel = Some(voice.velocity())
                }

                if id != Some(voice.id) {
                    break;
                }

                voice.signal_release(ReleaseType::Standard);
            }

            vel
        } else {
            for voice in &mut self.voices {
                if voice.is_releasing() || voice.is_killed() {
                    continue;
                }

                if self.held_by_damper.contains(&voice.id) {
                    continue;
                }

                self.held_by_damper.insert(voice.id);
                break;
            }

            None
        }
    }

    /// Batch remove ended voices using swap_remove for efficiency
    /// swap_remove is O(1) per removal but changes order
    #[inline(always)]
    pub fn remove_ended_voices(&mut self) {
        // Use swap_remove for O(1) per-element removal
        // This is faster than retain when removing many elements
        let mut i = 0;
        while i < self.voices.len() {
            if self.voices[i].ended() {
                self.voices.swap_remove(i);
                // Don't increment i, check the swapped element
            } else {
                i += 1;
            }
        }

        // Always clear held_by_damper to avoid O(n*m) complexity
        // This is the fastest approach for high voice counts
        if !self.held_by_damper.is_empty() {
            self.held_by_damper.clear();
        }
    }

    #[inline(always)]
    pub fn iter_voices_mut(&mut self) -> impl Iterator<Item = &mut Box<dyn Voice>> {
        self.voices.iter_mut().map(|group| &mut group.voice)
    }

    /// Get mutable access to voices for parallel processing
    #[inline(always)]
    pub fn get_voices_mut(&mut self) -> &mut [GroupVoice] {
        &mut self.voices
    }

    #[inline(always)]
    pub fn has_voices(&self) -> bool {
        !self.voices.is_empty()
    }

    #[inline(always)]
    pub fn voice_count(&self) -> usize {
        self.voices.len()
    }

    pub fn set_damper(&mut self, damper: bool) {
        if self.damper_held && !damper {
            for voice in &mut self.voices {
                if self.held_by_damper.contains(&voice.id) {
                    voice.signal_release(ReleaseType::Standard);
                }
            }
            self.held_by_damper.clear();
        }
        self.damper_held = damper;
    }

    /// Set the maximum number of voices per key. None means no limit.
    #[allow(dead_code)]
    pub fn set_max_voices(&mut self, max: Option<usize>) {
        self.max_voices = max;
    }
}
