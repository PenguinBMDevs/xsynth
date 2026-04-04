use crate::sfz::AmpegEnvelopeParams;
use soundfont::raw::{
    ControllerPalette, GeneralPalette, GeneratorType, Modulator, ModulatorTransform,
    SourceDirection, SourcePolarity, SourceType,
};

use super::Sf2Region;

const DEFAULT_FILTER_CUTOFF_CENTS: f32 = 13_500.0;
const DEFAULT_VOL_ENV_TIMECENTS: f32 = -12_000.0;

#[derive(Clone, Debug, Default)]
pub struct Sf2NoteParams {
    pub volume: f32,
    pub pan: i16,
    pub cutoff: Option<f32>,
    pub resonance: f32,
    pub tune_cents: f32,
    pub ampeg_envelope: AmpegEnvelopeParams,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Sf2RawEnvelope {
    pub delay_tc: i32,
    pub attack_tc: i32,
    pub hold_tc: i32,
    pub decay_tc: i32,
    pub sustain_cb: i32,
    pub release_tc: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Sf2NoteModulation {
    attenuation_cb: f32,
    cutoff_cents: f32,
    resonance_db: f32,
    pan: f32,
    tune_cents: f32,
    delay_tc: f32,
    attack_tc: f32,
    hold_tc: f32,
    decay_tc: f32,
    sustain_cb: f32,
    release_tc: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sf2NoteModDestination {
    InitialAttenuation,
    InitialFilterFc,
    InitialFilterQ,
    Pan,
    DelayVolEnv,
    AttackVolEnv,
    HoldVolEnv,
    DecayVolEnv,
    SustainVolEnv,
    ReleaseVolEnv,
    FineTune,
    CoarseTune,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sf2NoteModulator {
    src: Sf2NoteModSource,
    amount_src: Option<Sf2NoteModSource>,
    dest: Sf2NoteModDestination,
    amount: i16,
    transform: Sf2NoteTransform,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sf2NoteModSource {
    controller: Sf2NoteController,
    direction: Sf2NoteDirection,
    polarity: Sf2NotePolarity,
    curve: Sf2NoteCurve,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sf2NoteController {
    Velocity,
    KeyNumber,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sf2NoteDirection {
    Positive,
    Negative,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sf2NotePolarity {
    Unipolar,
    Bipolar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sf2NoteCurve {
    Linear,
    Switch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sf2NoteTransform {
    Linear,
    Absolute,
}

impl Sf2Region {
    pub fn note_params(&self, key: u8, velocity: u8) -> Sf2NoteParams {
        let modulation = self.note_modulation(key, velocity);
        let cutoff = self
            .cutoff_cents
            .map(|cents| cents as f32 + modulation.cutoff_cents);
        let cutoff = if cutoff.is_some() || modulation.cutoff_cents != 0.0 {
            Some(raw_cutoff_to_hz(
                cutoff.unwrap_or(DEFAULT_FILTER_CUTOFF_CENTS),
            ))
        } else {
            None
        };

        let raw_env = self.raw_envelope;
        let ampeg_envelope = AmpegEnvelopeParams {
            ampeg_start: 0.0,
            ampeg_delay: timecents_to_seconds(raw_env.delay_tc as f32 + modulation.delay_tc),
            ampeg_attack: timecents_to_seconds(raw_env.attack_tc as f32 + modulation.attack_tc),
            ampeg_hold: timecents_to_seconds(raw_env.hold_tc as f32 + modulation.hold_tc),
            ampeg_decay: timecents_to_seconds(raw_env.decay_tc as f32 + modulation.decay_tc),
            ampeg_sustain: sustain_cb_to_percent(raw_env.sustain_cb as f32 + modulation.sustain_cb),
            ampeg_release: timecents_to_seconds(raw_env.release_tc as f32 + modulation.release_tc),
        };

        Sf2NoteParams {
            volume: self.volume * centibels_to_amp(modulation.attenuation_cb),
            pan: ((self.pan as f32 + modulation.pan).round() as i16).clamp(-500, 500),
            cutoff,
            resonance: self.resonance + modulation.resonance_db,
            tune_cents: modulation.tune_cents,
            ampeg_envelope,
        }
    }

    fn note_modulation(&self, key: u8, velocity: u8) -> Sf2NoteModulation {
        let mut out = Sf2NoteModulation::default();

        for modulator in self.note_modulators.iter() {
            let amount = modulator.evaluate(key, velocity);
            if amount == 0.0 {
                continue;
            }

            match modulator.dest {
                Sf2NoteModDestination::InitialAttenuation => out.attenuation_cb += amount,
                Sf2NoteModDestination::InitialFilterFc => out.cutoff_cents += amount,
                Sf2NoteModDestination::InitialFilterQ => out.resonance_db += amount / 10.0,
                Sf2NoteModDestination::Pan => out.pan += amount,
                Sf2NoteModDestination::DelayVolEnv => out.delay_tc += amount,
                Sf2NoteModDestination::AttackVolEnv => out.attack_tc += amount,
                Sf2NoteModDestination::HoldVolEnv => out.hold_tc += amount,
                Sf2NoteModDestination::DecayVolEnv => out.decay_tc += amount,
                Sf2NoteModDestination::SustainVolEnv => out.sustain_cb += amount,
                Sf2NoteModDestination::ReleaseVolEnv => out.release_tc += amount,
                Sf2NoteModDestination::FineTune => out.tune_cents += amount,
                Sf2NoteModDestination::CoarseTune => out.tune_cents += amount * 100.0,
            }
        }

        out
    }
}

impl Sf2NoteModulator {
    pub(crate) fn destination(&self) -> Sf2NoteModDestination {
        self.dest
    }

    pub(crate) fn parse(modulator: &Modulator) -> Option<Self> {
        Some(Self {
            src: Sf2NoteModSource::parse_primary(modulator)?,
            amount_src: Sf2NoteModSource::parse_secondary(modulator)?,
            dest: match modulator.dest {
                GeneratorType::InitialAttenuation => Sf2NoteModDestination::InitialAttenuation,
                GeneratorType::InitialFilterFc => Sf2NoteModDestination::InitialFilterFc,
                GeneratorType::InitialFilterQ => Sf2NoteModDestination::InitialFilterQ,
                GeneratorType::Pan => Sf2NoteModDestination::Pan,
                GeneratorType::DelayVolEnv => Sf2NoteModDestination::DelayVolEnv,
                GeneratorType::AttackVolEnv => Sf2NoteModDestination::AttackVolEnv,
                GeneratorType::HoldVolEnv => Sf2NoteModDestination::HoldVolEnv,
                GeneratorType::DecayVolEnv => Sf2NoteModDestination::DecayVolEnv,
                GeneratorType::SustainVolEnv => Sf2NoteModDestination::SustainVolEnv,
                GeneratorType::ReleaseVolEnv => Sf2NoteModDestination::ReleaseVolEnv,
                GeneratorType::FineTune => Sf2NoteModDestination::FineTune,
                GeneratorType::CoarseTune => Sf2NoteModDestination::CoarseTune,
                _ => return None,
            },
            amount: modulator.amount,
            transform: match modulator.transform {
                ModulatorTransform::Linear => Sf2NoteTransform::Linear,
                ModulatorTransform::Absolute => Sf2NoteTransform::Absolute,
            },
        })
    }

    fn evaluate(&self, key: u8, velocity: u8) -> f32 {
        let mut value = self.amount as f32 * self.src.evaluate(key, velocity);

        if let Some(amount_src) = self.amount_src {
            value *= amount_src.evaluate(key, velocity);
        }

        match self.transform {
            Sf2NoteTransform::Linear => value,
            Sf2NoteTransform::Absolute => value.abs(),
        }
    }
}

impl Sf2NoteModSource {
    fn parse_primary(modulator: &Modulator) -> Option<Self> {
        Self::parse(modulator.src)
    }

    fn parse_secondary(modulator: &Modulator) -> Option<Option<Self>> {
        match modulator.amt_src.controller_palette {
            ControllerPalette::General(GeneralPalette::NoController) => Some(None),
            _ => Self::parse(modulator.amt_src).map(Some),
        }
    }

    fn parse(source: soundfont::raw::ModulatorSource) -> Option<Self> {
        Some(Self {
            controller: match source.controller_palette {
                ControllerPalette::General(GeneralPalette::NoteOnVelocity) => {
                    Sf2NoteController::Velocity
                }
                ControllerPalette::General(GeneralPalette::NoteOnKeyNumber) => {
                    Sf2NoteController::KeyNumber
                }
                _ => return None,
            },
            direction: match source.direction {
                SourceDirection::Positive => Sf2NoteDirection::Positive,
                SourceDirection::Negative => Sf2NoteDirection::Negative,
            },
            polarity: match source.polarity {
                SourcePolarity::Unipolar => Sf2NotePolarity::Unipolar,
                SourcePolarity::Bipolar => Sf2NotePolarity::Bipolar,
            },
            curve: match source.ty {
                SourceType::Linear => Sf2NoteCurve::Linear,
                SourceType::Switch => Sf2NoteCurve::Switch,
                _ => return None,
            },
        })
    }

    fn evaluate(&self, key: u8, velocity: u8) -> f32 {
        let value = match self.controller {
            Sf2NoteController::Velocity => velocity as f32,
            Sf2NoteController::KeyNumber => key as f32,
        };
        let normalized = value / 127.0;

        match (self.curve, self.polarity, self.direction) {
            (Sf2NoteCurve::Linear, Sf2NotePolarity::Unipolar, Sf2NoteDirection::Positive) => {
                normalized
            }
            (Sf2NoteCurve::Linear, Sf2NotePolarity::Unipolar, Sf2NoteDirection::Negative) => {
                1.0 - normalized
            }
            (Sf2NoteCurve::Linear, Sf2NotePolarity::Bipolar, Sf2NoteDirection::Positive) => {
                -1.0 + 2.0 * normalized
            }
            (Sf2NoteCurve::Linear, Sf2NotePolarity::Bipolar, Sf2NoteDirection::Negative) => {
                1.0 - 2.0 * normalized
            }
            (Sf2NoteCurve::Switch, Sf2NotePolarity::Unipolar, Sf2NoteDirection::Positive) => {
                if normalized >= 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
            (Sf2NoteCurve::Switch, Sf2NotePolarity::Unipolar, Sf2NoteDirection::Negative) => {
                if normalized >= 0.5 {
                    0.0
                } else {
                    1.0
                }
            }
            (Sf2NoteCurve::Switch, Sf2NotePolarity::Bipolar, Sf2NoteDirection::Positive) => {
                if normalized >= 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            (Sf2NoteCurve::Switch, Sf2NotePolarity::Bipolar, Sf2NoteDirection::Negative) => {
                if normalized >= 0.5 {
                    -1.0
                } else {
                    1.0
                }
            }
        }
    }
}

fn raw_cutoff_to_hz(cents: f32) -> f32 {
    2f32.powf(cents.clamp(1500.0, 13_500.0) / 1200.0) * 8.176
}

fn timecents_to_seconds(timecents: f32) -> f32 {
    if timecents <= -32_768.0 {
        0.0
    } else {
        2f32.powf(timecents.clamp(-12_000.0, 8_000.0) / 1200.0)
    }
}

fn sustain_cb_to_percent(cb: f32) -> f32 {
    10f32.powf(-cb.max(0.0) / 200.0) * 100.0
}

fn centibels_to_amp(cb: f32) -> f32 {
    10f32.powf(-cb.max(0.0) / 200.0)
}

pub(crate) fn default_raw_envelope() -> Sf2RawEnvelope {
    Sf2RawEnvelope {
        delay_tc: DEFAULT_VOL_ENV_TIMECENTS as i32,
        attack_tc: DEFAULT_VOL_ENV_TIMECENTS as i32,
        hold_tc: DEFAULT_VOL_ENV_TIMECENTS as i32,
        decay_tc: DEFAULT_VOL_ENV_TIMECENTS as i32,
        sustain_cb: 0,
        release_tc: DEFAULT_VOL_ENV_TIMECENTS as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundfont::raw::{
        ControllerPalette, GeneralPalette, GeneratorType, ModulatorSource, SourceDirection,
        SourcePolarity, SourceType,
    };

    fn source(
        palette: ControllerPalette,
        direction: SourceDirection,
        polarity: SourcePolarity,
        ty: SourceType,
    ) -> soundfont::raw::ModulatorSource {
        ModulatorSource {
            index: 0,
            controller_palette: palette,
            direction,
            polarity,
            ty,
        }
    }

    #[test]
    fn parses_velocity_attack_modulator() {
        let modulator = Modulator {
            src: source(
                ControllerPalette::General(GeneralPalette::NoteOnVelocity),
                SourceDirection::Positive,
                SourcePolarity::Unipolar,
                SourceType::Linear,
            ),
            dest: GeneratorType::AttackVolEnv,
            amount: 2400,
            amt_src: source(
                ControllerPalette::General(GeneralPalette::NoController),
                SourceDirection::Positive,
                SourcePolarity::Unipolar,
                SourceType::Linear,
            ),
            transform: ModulatorTransform::Linear,
        };

        let parsed = Sf2NoteModulator::parse(&modulator).unwrap();
        let value = parsed.evaluate(0, 127);
        assert!((value - 2400.0).abs() < 0.001);
    }

    #[test]
    fn parses_switch_secondary_source() {
        let modulator = Modulator {
            src: source(
                ControllerPalette::General(GeneralPalette::NoteOnVelocity),
                SourceDirection::Negative,
                SourcePolarity::Unipolar,
                SourceType::Linear,
            ),
            dest: GeneratorType::InitialFilterFc,
            amount: -2400,
            amt_src: source(
                ControllerPalette::General(GeneralPalette::NoteOnVelocity),
                SourceDirection::Negative,
                SourcePolarity::Unipolar,
                SourceType::Switch,
            ),
            transform: ModulatorTransform::Linear,
        };

        let parsed = Sf2NoteModulator::parse(&modulator).unwrap();
        assert_eq!(parsed.evaluate(60, 100), 0.0);
        assert!(parsed.evaluate(60, 40) < 0.0);
    }
}
