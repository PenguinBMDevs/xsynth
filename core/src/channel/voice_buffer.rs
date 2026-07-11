use super::ChannelInitOptions;
use crate::voice::{ReleaseType, Voice};
use rustc_hash::FxHashSet;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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
    /// Shared counter tracking total voices across all keys in the channel.
    /// Used to enforce the global voice limit.
    global_voice_counter: Arc<AtomicU64>,
    /// Maximum voices allowed globally across all keys in the channel.
    /// None means no limit.
    global_voice_limit: Option<usize>,
}

impl VoiceBuffer {
    pub fn new(options: ChannelInitOptions, global_voice_counter: Arc<AtomicU64>) -> Self {
        let max_voices = options.max_voices_per_key;
        let pre_alloc = max_voices.map_or(16, |m| m.min(64));
        VoiceBuffer {
            options,
            id_counter: 0,
            // Conservative pre-allocation based on max_voices to avoid excessive memory usage
            voices: Vec::with_capacity(pre_alloc),
            damper_held: false,
            held_by_damper: FxHashSet::default(),
            max_voices,
            global_voice_counter,
            global_voice_limit: options.global_voice_limit,
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
        let mut quietest_idx = None;

        for (idx, voice) in self.voices.iter().enumerate() {
            if voice.id == ignored_id || voice.is_killed() {
                continue;
            }
            let vel = voice.velocity();
            if vel < quietest_vel {
                quietest_vel = vel;
                quietest_id = Some(voice.id);
                quietest_idx = Some(idx);
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
                if let Some(idx) = quietest_idx {
                    self.voices.swap_remove(idx);
                    self.global_voice_counter.fetch_sub(1, Ordering::Relaxed);
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
            let count = self.voices.len();
            self.voices.clear();
            if count > 0 {
                self.global_voice_counter
                    .fetch_sub(count as u64, Ordering::Relaxed);
            }
        }
        self.held_by_damper.clear();
    }

    #[inline(always)]
    pub fn push_voices(&mut self, voices: impl Iterator<Item = Box<dyn Voice>>) {
        let id = self.get_id();
        let mut pushed = 0usize;

        for voice in voices {
            // Check global voice limit before pushing each voice
            if let Some(limit) = self.global_voice_limit {
                if self.global_voice_counter.load(Ordering::Relaxed) as usize >= limit {
                    break;
                }
            }
            self.voices.push(GroupVoice { id, voice });
            pushed += 1;
        }

        // Update global counter with newly pushed voices
        if pushed > 0 {
            self.global_voice_counter
                .fetch_add(pushed as u64, Ordering::Relaxed);
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

    /// Batch remove ended voices using swap_remove for efficiency.
    /// Also properly cleans up held_by_damper entries to avoid
    /// voice management corruption (was: clearing all entries blindly).
    #[inline(always)]
    pub fn remove_ended_voices(&mut self) {
        // Use retain to preserve voice order and cache locality.
        // swap_remove was O(1) per-element but scrambled voice order,
        // causing cache misses in the subsequent render_to loop.
        let mut removed = 0usize;
        let mut to_remove_ids: Vec<usize> = Vec::new();

        self.voices.retain(|voice| {
            if voice.ended() {
                to_remove_ids.push(voice.id);
                removed += 1;
                false
            } else {
                true
            }
        });

        for id in to_remove_ids {
            self.held_by_damper.remove(&id);
        }

        if removed > 0 {
            self.global_voice_counter
                .fetch_sub(removed as u64, Ordering::Relaxed);
        }
    }

    #[inline(always)]
    pub fn iter_voices_mut(&mut self) -> impl Iterator<Item = &mut Box<dyn Voice>> {
        self.voices.iter_mut().map(|group| &mut group.voice)
    }

    /// Get mutable access to voices for parallel processing
    #[allow(dead_code)]
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

    /// Returns the current global voice count for diagnostics.
    #[allow(dead_code)]
    pub fn global_voice_count(&self) -> u64 {
        self.global_voice_counter.load(Ordering::Relaxed)
    }

    /// Set the maximum number of voices per key. None means no limit.
    pub fn set_max_voices(&mut self, max: Option<usize>) {
        self.max_voices = max;
    }
}
