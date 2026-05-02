#![allow(dead_code)]
#![allow(non_camel_case_types)] // For the SIMD library

mod envelopes;
pub(crate) use envelopes::*;

mod simd;
pub(crate) use simd::*;

mod simdvoice;
pub(crate) use simdvoice::*;

mod base;
pub(crate) use base::*;

mod squarewave;
#[allow(unused_imports)]
pub(crate) use squarewave::*;

mod channels;
#[allow(unused_imports)]
pub(crate) use channels::*;

mod constant;
pub(crate) use constant::*;

mod sampler;
pub(crate) use sampler::*;

mod control;
pub(crate) use control::*;

mod cutoff;
pub(crate) use cutoff::*;

/// Options to modify the envelope of a voice using high-precision values.
#[derive(Copy, Clone)]
pub struct EnvelopeControlData {
    /// Controls the delay time in seconds.
    pub delay: Option<f32>,

    /// Controls the attack time in seconds.
    pub attack: Option<f32>,

    /// Controls the hold time in seconds.
    pub hold: Option<f32>,

    /// Controls the decay time in seconds.
    pub decay: Option<f32>,

    /// Controls the sustain level (0.0 = silent, 1.0 = max).
    pub sustain_percent: Option<f32>,

    /// Controls the release time in seconds.
    pub release: Option<f32>,
}

/// Options to modify the envelope of a voice using MIDI CC values (0-127).
/// These are relative to the original envelope duration.
#[derive(Copy, Clone)]
pub struct EnvelopeCCControlData {
    pub delay: Option<u8>,
    pub attack: Option<u8>,
    pub hold: Option<u8>,
    pub decay: Option<u8>,
    pub sustain_percent: Option<u8>,
    pub release: Option<u8>,
}

/// How a voice should be released.
#[derive(Copy, Clone, PartialEq)]
pub enum ReleaseType {
    /// Standard release. Uses the voice's envelope.
    Standard,

    /// Kills the voice with a fadeout of 1ms.
    Kill,
}

/// Options to control the parameters of a voice.
#[derive(Copy, Clone)]
pub struct VoiceControlData {
    /// Pitch multiplier
    pub voice_pitch_multiplier: f32,

    /// Envelope control (high-precision, seconds)
    pub envelope: EnvelopeControlData,

    /// Envelope control via MIDI CC (0-127, relative to original duration)
    pub cc_envelope: EnvelopeCCControlData,
}

impl VoiceControlData {
    pub fn new_defaults() -> Self {
        VoiceControlData {
            voice_pitch_multiplier: 1.0,
            envelope: EnvelopeControlData {
                delay: None,
                attack: None,
                hold: None,
                decay: None,
                sustain_percent: None,
                release: None,
            },
            cc_envelope: EnvelopeCCControlData {
                delay: None,
                attack: None,
                hold: None,
                decay: None,
                sustain_percent: None,
                release: None,
            },
        }
    }
}

pub trait VoiceGeneratorBase: Sync + Send {
    fn ended(&self) -> bool;
    fn signal_release(&mut self, rel_type: ReleaseType);
    fn process_controls(&mut self, control: &VoiceControlData);
}

pub trait VoiceSampleGenerator: VoiceGeneratorBase {
    fn render_to(&mut self, buffer: &mut [f32]);
}

pub trait Voice: VoiceSampleGenerator + Send + Sync {
    fn is_releasing(&self) -> bool;
    fn is_killed(&self) -> bool;

    fn velocity(&self) -> u8;
    fn exclusive_class(&self) -> Option<u8>;
}
