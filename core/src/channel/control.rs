use biquad::Q_BUTTERWORTH_F32;

use crate::{
    helpers::{db_to_amp, FREQS},
    voice::VoiceControlData,
};

use super::{ChannelAudioEvent, ChannelEvent, ControlEvent, VoiceChannel};

pub(crate) struct ValueLerp {
    lerp_length: f32,
    step: f32,
    current: f32,
    end: f32,
}

impl ValueLerp {
    pub fn new(current: f32, sample_rate: u32) -> Self {
        Self {
            lerp_length: sample_rate as f32 * 0.01,
            step: 0.0,
            current,
            end: current,
        }
    }

    pub fn set_end(&mut self, end: f32) {
        self.step = (end - self.current) / self.lerp_length;
        self.end = end;
    }

    pub fn get_next(&mut self) -> f32 {
        if self.end > self.current {
            self.current = (self.current + self.step).min(self.end);
        } else if self.end < self.current {
            self.current = (self.current + self.step).max(self.end);
        }
        self.current
    }
}

pub(super) struct ControlEventData {
    selected_lsb: i8,
    selected_msb: i8,
    pitch_bend_sensitivity_lsb: u8,
    pitch_bend_sensitivity_msb: u8,
    pitch_bend_sensitivity: f32,
    pitch_bend_value: f32,
    fine_tune_lsb: u8,
    fine_tune_msb: u8,
    fine_tune_value: f32,
    coarse_tune_value: f32,
    pub volume: ValueLerp, // 0.0 = silent, 1.0 = max volume
    pub pan: ValueLerp,    // 0.0 = left, 0.5 = center, 1.0 = right
    pub expression: ValueLerp,

    // Low-pass filter
    pub cutoff: f32,
    pub cutoff_active: bool,
    pub resonance: f32,
    pub resonance_active: bool,

    // High-pass filter
    pub highpass_cutoff: f32,
    pub highpass_active: bool,
    pub highpass_resonance: f32,
    pub highpass_resonance_active: bool,

    sample_rate: f32,
}

impl ControlEventData {
    pub fn new_defaults(sample_rate: u32) -> Self {
        let sr = sample_rate as f32;
        ControlEventData {
            selected_lsb: -1,
            selected_msb: -1,
            pitch_bend_sensitivity_lsb: 0,
            pitch_bend_sensitivity_msb: 2,
            pitch_bend_sensitivity: 2.0,
            pitch_bend_value: 0.0,
            fine_tune_lsb: 0,
            fine_tune_msb: 0,
            fine_tune_value: 0.0,
            coarse_tune_value: 0.0,
            volume: ValueLerp::new(1.0, sample_rate),
            pan: ValueLerp::new(0.5, sample_rate),
            expression: ValueLerp::new(1.0, sample_rate),
            cutoff: sr / 2.0 - 100.0,
            cutoff_active: false,
            resonance: Q_BUTTERWORTH_F32,
            resonance_active: false,
            highpass_cutoff: 0.0,
            highpass_active: false,
            highpass_resonance: Q_BUTTERWORTH_F32,
            highpass_resonance_active: false,
            sample_rate: sr,
        }
    }

    pub fn is_lowpass_active(&self) -> bool {
        self.cutoff_active && self.cutoff < self.sample_rate / 2.0 - 100.0
    }

    pub fn is_highpass_active(&self) -> bool {
        self.highpass_active && self.highpass_cutoff > 1.0
    }
}

impl VoiceChannel {
    /// Sends a ControlEvent to the channel.
    /// See the `ControlEvent` documentation for more information.
    pub fn process_control_event(&mut self, event: ControlEvent) {
        match event {
            ControlEvent::Raw(controller, value) => match controller {
                0x00 => {
                    // Bank select
                    self.params.set_bank(value);
                }
                0x64 => {
                    self.control_event_data.selected_lsb = value as i8;
                }
                0x65 => {
                    self.control_event_data.selected_msb = value as i8;
                }
                0x06 | 0x26 => {
                    let (lsb, msb) = {
                        let data = &self.control_event_data;
                        (data.selected_lsb, data.selected_msb)
                    };
                    if msb == 0 {
                        match lsb {
                            0 => {
                                // Pitch
                                match controller {
                                    0x06 => {
                                        self.control_event_data.pitch_bend_sensitivity_msb = value
                                    }
                                    0x26 => {
                                        self.control_event_data.pitch_bend_sensitivity_lsb = value
                                    }
                                    _ => (),
                                }

                                let sensitivity = {
                                    let data = &self.control_event_data;
                                    (data.pitch_bend_sensitivity_msb as f32)
                                        + (data.pitch_bend_sensitivity_lsb as f32) / 100.0
                                };

                                self.process_control_event(ControlEvent::PitchBendSensitivity(
                                    sensitivity,
                                ))
                            }
                            1 => {
                                // Fine tune
                                match controller {
                                    0x06 => self.control_event_data.fine_tune_msb = value,
                                    0x26 => self.control_event_data.fine_tune_lsb = value,
                                    _ => (),
                                }
                                let val: u16 = ((self.control_event_data.fine_tune_msb as u16)
                                    << 6)
                                    + self.control_event_data.fine_tune_lsb as u16;
                                let val = (val as f32 - 4096.0) / 4096.0 * 100.0;
                                self.process_control_event(ControlEvent::FineTune(val));
                            }
                            2 if controller == 0x06 => {
                                // Coarse tune
                                self.process_control_event(ControlEvent::CoarseTune(
                                    value as f32 - 64.0,
                                ))
                            }
                            2 => {}
                            _ => {}
                        }
                    }
                }
                0x07 => {
                    // Volume
                    let vol: f32 = value as f32 / 128.0;
                    self.control_event_data.volume.set_end(vol);
                }
                0x0A | 0x08 => {
                    // Pan
                    let pan: f32 = value as f32 / 128.0;
                    self.control_event_data.pan.set_end(pan);
                }
                0x0B => {
                    // Expression
                    let expr = value as f32 / 128.0;
                    self.control_event_data.expression.set_end(expr);
                }
                0x40 => {
                    // Damper / Sustain
                    let damper = match value {
                        0..=63 => false,
                        64..=127 => true,
                        _ => false,
                    };

                    for key in self.key_voices.iter_mut() {
                        key.data.set_damper(damper);
                    }
                }
                0x47 => {
                    // Resonance (CC71)
                    let db = (value as f32 - 64.0) / 2.4;
                    self.control_event_data.resonance = db_to_amp(db) * Q_BUTTERWORTH_F32;
                    self.control_event_data.resonance_active = true;
                }
                0x48 => {
                    // Release (CC72)
                    self.voice_control_data.cc_envelope.release = Some(value);
                    self.propagate_voice_controls();
                }
                0x49 => {
                    // Attack (CC73)
                    self.voice_control_data.cc_envelope.attack = Some(value);
                    self.propagate_voice_controls();
                }
                0x4A => {
                    // Cutoff (CC74) — low-pass only, 0..63 active, 64..127 off
                    if value < 64 {
                        let idx = value as usize + 64;
                        let mut freq = FREQS[idx];
                        if freq > 7000.0 {
                            let mult = freq / 7000.0 - 1.0;
                            let mult = mult * 2.36 + 1.0;
                            freq = mult * 7000.0;
                        }
                        self.control_event_data.cutoff = freq;
                        self.control_event_data.cutoff_active = true;
                    } else {
                        self.control_event_data.cutoff_active = false;
                    }
                }
                0x46 => {
                    // Decay (CC75)
                    self.voice_control_data.cc_envelope.decay = Some(value);
                    self.propagate_voice_controls();
                }
                0x4F => {
                    // Sustain (CC79, custom mapping)
                    self.voice_control_data.cc_envelope.sustain_percent = Some(value);
                    self.propagate_voice_controls();
                }
                0x5E => {
                    // Delay (CC94, custom mapping)
                    self.voice_control_data.cc_envelope.delay = Some(value);
                    self.propagate_voice_controls();
                }
                0x78 if value == 0 => {
                    // All Sounds Off
                    self.process_event(ChannelEvent::Audio(ChannelAudioEvent::AllNotesKilled));
                }
                0x79 if value == 0 => {
                    // Reset All Controllers
                    self.reset_control();
                }
                0x7B if value == 0 => {
                    // All Notes Off
                    self.process_event(ChannelEvent::Audio(ChannelAudioEvent::AllNotesOff));
                }
                _ => {}
            },
            ControlEvent::PitchBendSensitivity(sensitivity) => {
                let pitch_bend = {
                    let data = &mut self.control_event_data;
                    data.pitch_bend_sensitivity = sensitivity;
                    data.pitch_bend_sensitivity * data.pitch_bend_value
                };
                self.process_control_event(ControlEvent::PitchBend(pitch_bend));
            }
            ControlEvent::PitchBendValue(value) => {
                let pitch_bend = {
                    let data = &mut self.control_event_data;
                    data.pitch_bend_value = value;
                    data.pitch_bend_sensitivity * data.pitch_bend_value
                };
                self.process_control_event(ControlEvent::PitchBend(pitch_bend));
            }
            ControlEvent::PitchBend(value) => {
                self.control_event_data.pitch_bend_value = value;
                self.process_pitch();
            }
            ControlEvent::FineTune(value) => {
                self.control_event_data.fine_tune_value = value;
                self.process_pitch();
            }
            ControlEvent::CoarseTune(value) => {
                self.control_event_data.coarse_tune_value = value;
                self.process_pitch();
            }
            ControlEvent::Volume(value) => {
                self.control_event_data.volume.set_end(value.clamp(0.0, 1.0));
            }
            ControlEvent::Pan(value) => {
                self.control_event_data.pan.set_end(value.clamp(0.0, 1.0));
            }
            ControlEvent::Expression(value) => {
                self.control_event_data
                    .expression
                    .set_end(value.clamp(0.0, 1.0));
            }
            ControlEvent::Cutoff(value) => {
                self.control_event_data.cutoff = value;
                self.control_event_data.cutoff_active = true;
            }
            ControlEvent::Resonance(value) => {
                self.control_event_data.resonance = value.max(0.01);
                self.control_event_data.resonance_active = true;
            }
            ControlEvent::HighPassCutoff(value) => {
                self.control_event_data.highpass_cutoff = value.max(0.0);
                self.control_event_data.highpass_active = true;
            }
            ControlEvent::HighPassResonance(value) => {
                self.control_event_data.highpass_resonance = value.max(0.01);
                self.control_event_data.highpass_resonance_active = true;
            }
            ControlEvent::DelayTime(value) => {
                self.voice_control_data.envelope.delay = Some(value.max(0.0));
                self.voice_control_data.cc_envelope.delay = None;
                self.propagate_voice_controls();
            }
            ControlEvent::AttackTime(value) => {
                self.voice_control_data.envelope.attack = Some(value.max(0.0));
                self.voice_control_data.cc_envelope.attack = None;
                self.propagate_voice_controls();
            }
            ControlEvent::HoldTime(value) => {
                self.voice_control_data.envelope.hold = Some(value.max(0.0));
                self.voice_control_data.cc_envelope.hold = None;
                self.propagate_voice_controls();
            }
            ControlEvent::DecayTime(value) => {
                self.voice_control_data.envelope.decay = Some(value.max(0.0));
                self.voice_control_data.cc_envelope.decay = None;
                self.propagate_voice_controls();
            }
            ControlEvent::SustainLevel(value) => {
                self.voice_control_data.envelope.sustain_percent =
                    Some(value.clamp(0.0, 1.0));
                self.voice_control_data.cc_envelope.sustain_percent = None;
                self.propagate_voice_controls();
            }
            ControlEvent::ReleaseTime(value) => {
                self.voice_control_data.envelope.release = Some(value.max(0.0));
                self.voice_control_data.cc_envelope.release = None;
                self.propagate_voice_controls();
            }
            ControlEvent::Damper(value) => {
                for key in self.key_voices.iter_mut() {
                    key.data.set_damper(value);
                }
            }
        }
    }

    fn process_pitch(&mut self) {
        let data = &mut self.control_event_data;
        let pitch_bend = data.pitch_bend_value;
        let fine_tune = data.fine_tune_value;
        let coarse_tune = data.coarse_tune_value;
        let combined = pitch_bend + coarse_tune + fine_tune / 100.0;

        self.voice_control_data.voice_pitch_multiplier = 2.0f32.powf(combined / 12.0);
        self.propagate_voice_controls();
    }

    pub(super) fn reset_control(&mut self) {
        self.control_event_data = ControlEventData::new_defaults(self.stream_params.sample_rate);
        self.voice_control_data = VoiceControlData::new_defaults();
        self.propagate_voice_controls();

        for key in self.key_voices.iter_mut() {
            key.data.set_damper(false);
        }
    }

    pub(super) fn reset_program(&mut self) {
        self.params.set_bank(0);
        self.params.set_preset(0);
    }
}

#[cfg(test)]
mod tests {
    use crate::{AudioStreamParams, ChannelCount};

    use super::*;

    fn new_channel() -> VoiceChannel {
        VoiceChannel::new(
            Default::default(),
            AudioStreamParams::new(48_000, ChannelCount::Stereo),
            None,
        )
    }

    #[test]
    fn coarse_tune_only_uses_data_entry_msb() {
        let mut channel = new_channel();

        channel.process_control_event(ControlEvent::Raw(0x65, 0));
        channel.process_control_event(ControlEvent::Raw(0x64, 2));
        channel.process_control_event(ControlEvent::Raw(0x26, 100));

        assert_eq!(channel.control_event_data.coarse_tune_value, 0.0);

        channel.process_control_event(ControlEvent::Raw(0x06, 70));

        assert_eq!(channel.control_event_data.coarse_tune_value, 6.0);
    }

    #[test]
    fn reset_all_controllers_requires_zero_value() {
        let mut channel = new_channel();
        channel.process_control_event(ControlEvent::Raw(0x07, 32));
        channel.process_control_event(ControlEvent::Raw(0x79, 1));

        assert_eq!(channel.control_event_data.volume.end, 32.0 / 128.0);

        channel.process_control_event(ControlEvent::Raw(0x79, 0));

        assert_eq!(channel.control_event_data.volume.end, 1.0);
    }
}
