use simdeez::prelude::*;

use crate::soundfont::{EnvelopeCurveType, EnvelopeOptions};
use crate::voice::{EnvelopeCCControlData, EnvelopeControlData, ReleaseType, VoiceControlData};

use self::lerpers::{SIMDLerper, SIMDLerperConcave, SIMDLerperConvex, StageTime};

use super::{SIMDSampleMono, SIMDVoiceGenerator, VoiceGeneratorBase};

mod lerpers;

/// The stages in envelopes as a numbered enum
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum EnvelopeStage {
    Delay = 0,
    Attack = 1,
    Hold = 2,
    Decay = 3,
    Sustain = 4,
    Release = 5, // Goes to this stage as soon as the voice is released
    Finished = 6,
}

impl EnvelopeStage {
    pub fn as_usize(&self) -> usize {
        *self as usize
    }

    pub fn next_stage(&self) -> EnvelopeStage {
        match self {
            EnvelopeStage::Delay => EnvelopeStage::Attack,
            EnvelopeStage::Attack => EnvelopeStage::Hold,
            EnvelopeStage::Hold => EnvelopeStage::Decay,
            EnvelopeStage::Decay => EnvelopeStage::Sustain,
            EnvelopeStage::Sustain => EnvelopeStage::Release,
            EnvelopeStage::Release => EnvelopeStage::Finished,
            EnvelopeStage::Finished => EnvelopeStage::Finished,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum EnvelopePart {
    Lerp {
        target: f32,   // Target value by the end of the envelope part
        duration: u32, // Duration in samples
    },
    LerpConcave {
        target: f32,
        duration: u32,
    },
    LerpConvex {
        target: f32,
        duration: u32,
    },
    Hold(f32),
}

impl EnvelopePart {
    pub fn lerp(target: f32, duration: u32) -> EnvelopePart {
        EnvelopePart::Lerp { target, duration }
    }

    pub fn lerp_concave(target: f32, duration: u32) -> EnvelopePart {
        EnvelopePart::LerpConcave { target, duration }
    }

    pub fn lerp_convex(target: f32, duration: u32) -> EnvelopePart {
        EnvelopePart::LerpConvex { target, duration }
    }

    pub fn hold(value: f32) -> EnvelopePart {
        EnvelopePart::Hold(value)
    }
}

/// The original envelope descriptor
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct EnvelopeDescriptor {
    pub start_percent: f32,   // % (0-1)
    pub delay: f32,           // Seconds
    pub attack: f32,          // Seconds
    pub hold: f32,            // Seconds
    pub decay: f32,           // Seconds
    pub sustain_percent: f32, // % (0-1)
    pub release: f32,         // Seconds
}

impl EnvelopeDescriptor {
    #[allow(clippy::wrong_self_convention)]
    pub fn to_envelope_params(
        &self,
        samplerate: u32,
        options: EnvelopeOptions,
    ) -> EnvelopeParameters {
        let samplerate = samplerate as f32;

        // The following are in dB scale (or cents for modulation) so:
        // Linear dB -> Concave or Convex in amp
        // Concave or Convex in dB -> Linear in amp

        let attack = match options.attack_curve {
            EnvelopeCurveType::Linear => {
                EnvelopePart::lerp_convex(1.0, (self.attack * samplerate) as u32)
            }
            EnvelopeCurveType::Exponential => {
                EnvelopePart::lerp(1.0, (self.attack * samplerate) as u32)
            }
        };

        let decay = match options.decay_curve {
            EnvelopeCurveType::Exponential => {
                EnvelopePart::lerp(self.sustain_percent, (self.decay * samplerate) as u32)
            }
            EnvelopeCurveType::Linear => {
                EnvelopePart::lerp_concave(self.sustain_percent, (self.decay * samplerate) as u32)
            }
        };

        let release = match options.release_curve {
            EnvelopeCurveType::Exponential => {
                EnvelopePart::lerp(0.0, (self.release * samplerate) as u32)
            }
            EnvelopeCurveType::Linear => {
                EnvelopePart::lerp_concave(0.0, (self.release * samplerate) as u32)
            }
        };

        EnvelopeParameters {
            start: self.start_percent,
            parts: [
                // Delay
                EnvelopePart::lerp(self.start_percent, (self.delay * samplerate) as u32),
                // Attack
                attack,
                // Hold
                EnvelopePart::lerp(1.0, (self.hold * samplerate) as u32),
                // Decay
                decay,
                // Sustain
                EnvelopePart::hold(self.sustain_percent),
                // Release
                release,
                // Finished
                EnvelopePart::hold(0.0),
            ],
        }
    }
}

/// The raw envelope parameters used to generate the envelope.
/// Is a separate struct to EnvelopeDescriptor for performance reasons.
/// Use EnvelopeDescriptor to generate the EnvelopeParameters struct.
#[derive(Debug, Clone, Copy)]
pub struct EnvelopeParameters {
    start: f32,
    pub parts: [EnvelopePart; 7],
}

impl EnvelopeParameters {
    fn get_stage_data<T: Simd>(
        &self,
        stage: EnvelopeStage,
        start_amp: f32,
    ) -> VoiceEnvelopeState<T> {
        simd_invoke!(T, {
            let stage_info = &self.parts[stage.as_usize()];
            match stage_info {
                EnvelopePart::Lerp { target, duration } => {
                    let duration = *duration;
                    let target = *target;
                    if duration == 0 {
                        self.get_stage_data(stage.next_stage(), target)
                    } else {
                        let data = StageData::Lerp(
                            SIMDLerper::new(start_amp, target),
                            StageTime::new(0, duration),
                        );
                        VoiceEnvelopeState {
                            current_stage: stage,
                            stage_data: data,
                        }
                    }
                }
                EnvelopePart::LerpConcave { target, duration } => {
                    let duration = *duration;
                    let target = *target;
                    if duration == 0 {
                        self.get_stage_data(stage.next_stage(), target)
                    } else {
                        let data = StageData::LerpConcave(
                            SIMDLerperConcave::new(start_amp, target),
                            StageTime::new(0, duration),
                        );
                        VoiceEnvelopeState {
                            current_stage: stage,
                            stage_data: data,
                        }
                    }
                }
                EnvelopePart::LerpConvex { target, duration } => {
                    let duration = *duration;
                    let target = *target;
                    if duration == 0 {
                        self.get_stage_data(stage.next_stage(), target)
                    } else {
                        let data = StageData::LerpConvex(
                            SIMDLerperConvex::new(start_amp, target),
                            StageTime::new(0, duration),
                        );
                        VoiceEnvelopeState {
                            current_stage: stage,
                            stage_data: data,
                        }
                    }
                }
                EnvelopePart::Hold(value) => {
                    let data = StageData::Constant(T::Vf32::set1(*value));
                    VoiceEnvelopeState {
                        current_stage: stage,
                        stage_data: data,
                    }
                }
            }
        })
    }

    pub fn get_stage_duration(&self, stage: EnvelopeStage) -> u32 {
        let stage_info = &self.parts[stage.as_usize()];
        match stage_info {
            EnvelopePart::Lerp {
                target: _,
                duration,
            } => *duration,
            EnvelopePart::LerpConcave {
                target: _,
                duration,
            } => *duration,
            EnvelopePart::LerpConvex {
                target: _,
                duration,
            } => *duration,
            EnvelopePart::Hold(_) => 0,
        }
    }

    pub fn modify_stage_data(&mut self, part: usize, data: EnvelopePart) {
        self.parts[part] = data;
    }
}

enum StageData<T: Simd> {
    Lerp(SIMDLerper<T>, StageTime<T>),
    LerpConcave(SIMDLerperConcave<T>, StageTime<T>),
    LerpConvex(SIMDLerperConvex<T>, StageTime<T>),
    Constant(T::Vf32),
}

struct VoiceEnvelopeState<T: Simd> {
    current_stage: EnvelopeStage,
    stage_data: StageData<T>,
}

/// Threshold below which a voice in release stage is considered silent
/// and can be safely terminated. Set to ~-90dB (below 16-bit LSB),
/// which is inaudible in any reasonable listening scenario.
const FINISH_THRESHOLD: f32 = 1.0 / 32768.0;

pub struct SIMDVoiceEnvelope<T: Simd> {
    original_params: EnvelopeParameters,
    params: EnvelopeParameters,
    allow_release: bool,
    state: VoiceEnvelopeState<T>,
    sample_rate: f32,
    killed: bool,
}

impl<T: Simd> SIMDVoiceEnvelope<T> {
    pub fn new(
        original_params: EnvelopeParameters,
        params: EnvelopeParameters,
        allow_release: bool,
        sample_rate: f32,
    ) -> Self {
        let state = params.get_stage_data(EnvelopeStage::Delay, params.start);

        SIMDVoiceEnvelope {
            original_params,
            params,
            allow_release,
            state,
            sample_rate,
            killed: false,
        }
    }

    pub fn get_value_at_current_time(&self) -> f32 {
        match &self.state.stage_data {
            StageData::Lerp(lerper, stage_time) => {
                lerper.lerp(stage_time.simd_array_start_f32() / stage_time.stage_end_time_f32())
            }
            StageData::LerpConcave(lerper, stage_time) => {
                lerper.lerp(stage_time.simd_array_start_f32() / stage_time.stage_end_time_f32())
            }
            StageData::LerpConvex(lerper, stage_time) => {
                lerper.lerp(stage_time.simd_array_start_f32() / stage_time.stage_end_time_f32())
            }
            StageData::Constant(constant) => constant[0],
        }
    }

    pub fn current_stage(&self) -> &EnvelopeStage {
        &self.state.current_stage
    }

    fn switch_to_next_stage(&mut self) {
        let amp = self.get_value_at_current_time();
        self.state = self
            .params
            .get_stage_data(self.current_stage().next_stage(), amp);
    }

    fn update_stage(&mut self) {
        let amp = self.get_value_at_current_time();
        self.state = self.params.get_stage_data(*self.current_stage(), amp);
    }

    fn increment_time_by(&mut self, increment: u32) {
        match &mut self.state.stage_data {
            StageData::Lerp(_, stage_time) => {
                stage_time.increment_by(increment);
            }
            StageData::LerpConcave(_, stage_time) => {
                stage_time.increment_by(increment);
            }
            StageData::LerpConvex(_, stage_time) => {
                stage_time.increment_by(increment);
            }
            StageData::Constant(_) => {}
        }
    }

    fn manually_build_simd_sample(&mut self) -> SIMDSampleMono<T> {
        simd_invoke!(T, {
            let mut values = T::Vf32::set1(0.0);
            for i in 0..T::Vf32::WIDTH {
                let sample = self.get_value_at_current_time();
                values[i] = sample;
                self.increment_time_by(1);
                let should_progress = match &mut self.state.stage_data {
                    StageData::Lerp(_, stage_time)
                    | StageData::LerpConcave(_, stage_time)
                    | StageData::LerpConvex(_, stage_time) => {
                        stage_time.is_ending() && !stage_time.is_intersecting_end()
                    }
                    StageData::Constant(_) => false,
                };
                if should_progress {
                    self.switch_to_next_stage();
                }
            }
            SIMDSampleMono(values)
        })
    }

    pub fn get_modified_envelope(
        mut params: EnvelopeParameters,
        envelope: EnvelopeControlData,
        cc_envelope: EnvelopeCCControlData,
        sample_rate: f32,
    ) -> EnvelopeParameters {
        fn calculate_curve(value: u8, duration: f32) -> f32 {
            match value {
                0..=64 => (value as f32 / 64.0).powi(5) * duration,
                65..=128 => duration + ((value as f32 - 64.0) / 64.0).powi(3) * 15.0,
                _ => duration,
            }
        }

        let apply_duration = |params: &mut EnvelopeParameters,
                              part: EnvelopeStage,
                              duration_secs: f32| {
            let duration = (duration_secs.max(0.0) * sample_rate) as u32;
            let idx = part.as_usize();
            match params.parts[idx] {
                EnvelopePart::Lerp {
                    target,
                    duration: _,
                } => params.modify_stage_data(idx, EnvelopePart::lerp(target, duration)),
                EnvelopePart::LerpConcave {
                    target,
                    duration: _,
                } => params.modify_stage_data(idx, EnvelopePart::lerp_concave(target, duration)),
                EnvelopePart::LerpConvex {
                    target,
                    duration: _,
                } => params.modify_stage_data(idx, EnvelopePart::lerp_convex(target, duration)),
                _ => {}
            }
        };

        let apply_cc_duration = |params: &mut EnvelopeParameters,
                                 part: EnvelopeStage,
                                 cc_value: u8,
                                 min_secs: f32| {
            let old_duration = params.get_stage_duration(part) as f32 / sample_rate;
            let duration_secs = calculate_curve(cc_value, old_duration).max(min_secs);
            apply_duration(params, part, duration_secs);
        };

        // Delay: high-precision seconds takes priority, else fall back to CC
        if let Some(delay_secs) = envelope.delay {
            apply_duration(&mut params, EnvelopeStage::Delay, delay_secs);
        } else if let Some(cc_delay) = cc_envelope.delay {
            apply_cc_duration(&mut params, EnvelopeStage::Delay, cc_delay, 0.0);
        }

        // Attack: high-precision seconds takes priority, else fall back to CC
        if let Some(attack_secs) = envelope.attack {
            apply_duration(&mut params, EnvelopeStage::Attack, attack_secs);
        } else if let Some(cc_attack) = cc_envelope.attack {
            apply_cc_duration(&mut params, EnvelopeStage::Attack, cc_attack, 0.0);
        }

        // Hold: high-precision seconds takes priority, else fall back to CC
        if let Some(hold_secs) = envelope.hold {
            apply_duration(&mut params, EnvelopeStage::Hold, hold_secs);
        } else if let Some(cc_hold) = cc_envelope.hold {
            apply_cc_duration(&mut params, EnvelopeStage::Hold, cc_hold, 0.0);
        }

        // Decay: high-precision seconds takes priority, else fall back to CC
        if let Some(decay_secs) = envelope.decay {
            apply_duration(&mut params, EnvelopeStage::Decay, decay_secs);
        } else if let Some(cc_decay) = cc_envelope.decay {
            apply_cc_duration(&mut params, EnvelopeStage::Decay, cc_decay, 0.0);
        }

        // Sustain: high-precision level takes priority, else fall back to CC
        if let Some(sustain) = envelope.sustain_percent {
            let idx = EnvelopeStage::Sustain.as_usize();
            params.modify_stage_data(idx, EnvelopePart::hold(sustain.clamp(0.0, 1.0)));
        } else if let Some(cc_sustain) = cc_envelope.sustain_percent {
            let idx = EnvelopeStage::Sustain.as_usize();
            let level = (cc_sustain as f32 / 127.0).clamp(0.0, 1.0);
            params.modify_stage_data(idx, EnvelopePart::hold(level));
        }

        // Release: high-precision seconds takes priority, else fall back to CC
        if let Some(release_secs) = envelope.release {
            apply_duration(&mut params, EnvelopeStage::Release, release_secs);
        } else if let Some(cc_release) = cc_envelope.release {
            apply_cc_duration(&mut params, EnvelopeStage::Release, cc_release, 0.02);
        }

        params
    }

    /// Check if all SIMD lanes of the computed envelope values are below
    /// the silence threshold during the release stage. If so, force the
    /// voice to Finished immediately — it's producing inaudible output
    /// (below ~-90dB) and continuing would waste CPU cycles.
    #[inline(always)]
    fn try_finish_silent(&mut self, values: &T::Vf32) {
        if self.state.current_stage != EnvelopeStage::Release {
            return;
        }
        for i in 0..T::Vf32::WIDTH {
            if values[i].abs() >= FINISH_THRESHOLD {
                return;
            }
        }
        self.state = self.params.get_stage_data(EnvelopeStage::Finished, 0.0);
    }

    pub fn modify_envelope(&mut self, envelope: EnvelopeControlData, cc_envelope: EnvelopeCCControlData) {
        if !self.killed {
            self.params =
                Self::get_modified_envelope(self.original_params, envelope, cc_envelope, self.sample_rate);
            self.update_stage();
        }
    }
}

impl<T: Simd> VoiceGeneratorBase for SIMDVoiceEnvelope<T> {
    #[inline(always)]
    fn ended(&self) -> bool {
        self.state.current_stage == EnvelopeStage::Finished
    }

    #[inline(always)]
    fn signal_release(&mut self, rel_type: ReleaseType) {
        if rel_type == ReleaseType::Kill {
            self.params.modify_stage_data(
                5,
                EnvelopePart::lerp(0.0, (0.001 * self.sample_rate) as u32),
            );
            self.update_stage();
            self.killed = true;
        }
        if self.allow_release || self.killed {
            let amp = self.get_value_at_current_time();
            self.state = self.params.get_stage_data(EnvelopeStage::Release, amp);
        }
    }

    #[inline(always)]
    fn process_controls(&mut self, control: &VoiceControlData) {
        self.modify_envelope(control.envelope, control.cc_envelope);
    }
}

impl<T: Simd> SIMDVoiceGenerator<T, SIMDSampleMono<T>> for SIMDVoiceEnvelope<T> {
    #[inline(always)]
    fn next_sample(&mut self) -> SIMDSampleMono<T> {
        simd_invoke!(T, {
            // Use loop instead of recursion to avoid function call overhead
            loop {
                match &mut self.state.stage_data {
                    StageData::Lerp(lerper, stage_time) => {
                        if stage_time.is_ending() {
                            if stage_time.is_intersecting_end() {
                                return self.manually_build_simd_sample();
                            } else {
                                self.switch_to_next_stage();
                                continue;
                            }
                        } else {
                            let values = lerper.lerp_simd(stage_time.progress_simd_array());
                            stage_time.increment();
                            self.try_finish_silent(&values);
                            return SIMDSampleMono(values);
                        }
                    }
                    StageData::LerpConcave(lerper, stage_time) => {
                        if stage_time.is_ending() {
                            if stage_time.is_intersecting_end() {
                                return self.manually_build_simd_sample();
                            } else {
                                self.switch_to_next_stage();
                                continue;
                            }
                        } else {
                            let values = lerper.lerp_simd(stage_time.progress_simd_array());
                            stage_time.increment();
                            self.try_finish_silent(&values);
                            return SIMDSampleMono(values);
                        }
                    }
                    StageData::LerpConvex(lerper, stage_time) => {
                        if stage_time.is_ending() {
                            if stage_time.is_intersecting_end() {
                                return self.manually_build_simd_sample();
                            } else {
                                self.switch_to_next_stage();
                                continue;
                            }
                        } else {
                            let values = lerper.lerp_simd(stage_time.progress_simd_array());
                            stage_time.increment();
                            self.try_finish_silent(&values);
                            return SIMDSampleMono(values);
                        }
                    }
                    StageData::Constant(constant) => return SIMDSampleMono(*constant),
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use simdeez::simd_runtime_generate;
    use to_vec::ToVec;

    use super::*;

    fn assert_vf32_equal<S: Simd>(a: S::Vf32, b: S::Vf32) {
        for i in 0..S::Vf32::WIDTH {
            assert_eq!(a[i], b[i]);
        }
    }

    fn assert_vf32_close<S: Simd>(a: S::Vf32, b: S::Vf32, epsilon: f32) {
        for i in 0..S::Vf32::WIDTH {
            assert!(
                (a[i] - b[i]).abs() < epsilon,
                "a[{}] = {}, b[{}] = {}, diff = {}",
                i, a[i], i, b[i], (a[i] - b[i]).abs()
            );
        }
    }

    fn simd_from_vec<S: Simd>(vec: Vec<f32>) -> S::Vf32 {
        let mut initial = S::Vf32::set1(0.0);
        let mut iter = vec.into_iter();
        for i in 0..S::Vf32::WIDTH {
            initial[i] = iter.next().unwrap();
        }
        initial
    }

    #[test]
    fn test_simd_lerp() {
        simd_runtime_generate!(
            fn run() {
                let lerper = SIMDLerper::<S>::new(0.0, 1.0);
                assert_eq!(lerper.lerp(0.0), 0.0);
                assert_eq!(lerper.lerp(0.5), 0.5);
                assert_eq!(lerper.lerp(1.0), 1.0);
                assert_vf32_equal::<S>(lerper.lerp_simd(S::Vf32::set1(0.0)), S::Vf32::set1(0.0));
                assert_vf32_equal::<S>(lerper.lerp_simd(S::Vf32::set1(0.5)), S::Vf32::set1(0.5));
                assert_vf32_equal::<S>(lerper.lerp_simd(S::Vf32::set1(1.0)), S::Vf32::set1(1.0));
            }
        );

        run();
    }

    #[test]
    fn test_stage_time() {
        fn simd_from_range<S: Simd>(range: std::ops::Range<usize>) -> S::Vf32 {
            simd_from_vec::<S>(range.map(|v| v as f32).to_vec())
        }

        simd_runtime_generate!(
            fn run() {
                let mut time = StageTime::<S>::new(5, 20);
                let mut time2 = StageTime::<S>::new(4, 20);
                assert_eq!(time.simd_array_start(), 5);
                assert!(!time.is_ending());

                let inv_end = S::Vf32::set1(1.0 / 20.0);

                assert_vf32_equal::<S>(
                    *time.raw_simd_array(),
                    simd_from_range::<S>(5..(5 + S::Vf32::WIDTH)),
                );
                // progress_simd_array uses multiply-by-reciprocal (not division),
                // so results may differ at ULP level from a/b
                assert_vf32_close::<S>(
                    time.progress_simd_array(),
                    simd_from_range::<S>(5..(5 + S::Vf32::WIDTH)) * inv_end,
                    1e-6,
                );

                let mut i = 5;
                while time.simd_array_start() + S::Vf32::WIDTH as u32 <= 20 {
                    assert_vf32_equal::<S>(
                        *time.raw_simd_array(),
                        simd_from_range::<S>(i..(i + S::Vf32::WIDTH)),
                    );
                    assert_vf32_close::<S>(
                        time.progress_simd_array(),
                        simd_from_range::<S>(i..(i + S::Vf32::WIDTH)) * inv_end,
                        1e-6,
                    );
                    assert_eq!(time.simd_array_start(), i as u32);
                    assert!(!time.is_ending());

                    assert!(!time.is_intersecting_end());

                    time.increment();
                    time2.increment();
                    i += S::Vf32::WIDTH;
                }
                assert_eq!(time.simd_array_start(), i as u32);
                assert!(time.is_ending());
                assert!(time.is_intersecting_end());

                assert!(!time2.is_ending());
                time2.increment();
                assert!(time2.is_ending());
                assert!(!time2.is_intersecting_end());
            }
        );

        run();
    }

    #[test]
    fn test_envelope() {
        #![allow(clippy::same_item_push)]

        fn push_simd_to_vec<S: Simd>(vec: &mut Vec<f32>, simd: S::Vf32) {
            for i in 0..S::Vf32::WIDTH {
                vec.push(simd[i]);
            }
        }

        fn lerp(from: f32, to: f32, fac: f32) -> f32 {
            from + (to - from) * fac
        }

        fn lerp_to_zero_curve(from: f32, fac: f32) -> f32 {
            let mult = (1. - fac).powi(8);
            mult * from
        }

        fn lerp_concave(from: f32, to: f32, fac: f32) -> f32 {
            let mult = (1. - fac).powi(8);
            (from - to) * mult + to
        }

        simd_runtime_generate!(
            fn run() {
                let mut vec = Vec::new();

                let descriptor = EnvelopeDescriptor {
                    start_percent: 0.5,
                    delay: 0.0,
                    attack: 15.0,
                    hold: 0.0,
                    decay: 17.0,
                    sustain_percent: 0.4,
                    release: 16.0,
                };
                let params = descriptor.to_envelope_params(1, Default::default());

                assert!(matches!(
                    params.parts[EnvelopeStage::Decay.as_usize()],
                    EnvelopePart::LerpConcave {
                        target,
                        duration
                    } if target == 0.4 && duration == 17
                ));

                let mut env = SIMDVoiceEnvelope::<S>::new(params, params, true, 1.0);

                let mut i = 0;
                while i < 48 {
                    push_simd_to_vec::<S>(&mut vec, env.next_sample().0);
                    i += S::Vf32::WIDTH;
                }
                env.signal_release(ReleaseType::Standard);
                assert_eq!(env.current_stage(), &EnvelopeStage::Release);
                while i < 48 + 32 {
                    push_simd_to_vec::<S>(&mut vec, env.next_sample().0);
                    i += S::Vf32::WIDTH;
                }

                let mut expected_vec = Vec::new();

                for i in 0..15 {
                    expected_vec.push(lerp(0.5, 1.0, i as f32 / 15.0));
                }
                for i in 0..17 {
                    expected_vec.push(lerp_concave(1.0, 0.4, i as f32 / 17.0));
                }
                for _ in 0..16 {
                    expected_vec.push(0.4);
                }
                for i in 0..16 {
                    expected_vec.push(lerp_to_zero_curve(0.4, i as f32 / 16.0));
                }
                for _ in 0..16 {
                    expected_vec.push(0.0);
                }

                for v in vec.iter_mut().chain(expected_vec.iter_mut()) {
                    // Rounding as cached values are sometimes off by tiny fractions
                    *v = (*v * 10000.0).round() / 10000.0;
                }

                assert_eq!(vec, expected_vec);
            }
        );

        run();
    }
}
