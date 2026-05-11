use super::ChannelInitOptions;
use crate::voice::{ReleaseType, Voice};
use rustc_hash::FxHashSet;
use std::ops::{Deref, DerefMut};

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

pub struct VoiceBuffer {
    options: ChannelInitOptions,
    id_counter: usize,
    voices: Vec<GroupVoice>,
    damper_held: bool,
    held_by_damper: FxHashSet<usize>,
}

impl VoiceBuffer {
    pub fn new(options: ChannelInitOptions) -> Self {
        VoiceBuffer {
            options,
            id_counter: 0,
            voices: Vec::with_capacity(256),
            damper_held: false,
            held_by_damper: FxHashSet::default(),
        }
    }

    #[inline(always)]
    fn get_id(&mut self) -> usize {
        self.id_counter += 1;
        self.id_counter
    }

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

    pub fn kill_by_exclusive_class(&mut self, class: u8) {
        for voice in &mut self.voices {
            if voice.exclusive_class() == Some(class) {
                voice.signal_release(ReleaseType::Kill);
            }
        }
    }

    fn get_active_count(&self) -> usize {
        self.voices.iter().filter(|v| !v.is_killed()).count()
    }

    #[inline(always)]
    pub fn push_voices(
        &mut self,
        voices: impl Iterator<Item = Box<dyn Voice>>,
        max_voices: Option<usize>,
    ) {
        let id = self.get_id();

        for voice in voices {
            self.voices.push(GroupVoice { id, voice });
        }

        if let Some(max_voices) = max_voices {
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

    #[inline(always)]
    pub fn remove_ended_voices(&mut self) {
        let mut i = 0;
        while i < self.voices.len() {
            if self.voices[i].ended() {
                self.voices.swap_remove(i);
            } else {
                i += 1;
            }
        }

        let voice_ids: FxHashSet<usize> = self.voices.iter().map(|v| v.id).collect();
        self.held_by_damper.retain(|id| voice_ids.contains(id));
    }

    #[inline(always)]
    pub fn iter_voices_mut(&mut self) -> impl Iterator<Item = &mut Box<dyn Voice>> {
        self.voices.iter_mut().map(|group| &mut group.voice)
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
}
