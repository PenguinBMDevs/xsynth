use crate::LoopMode;
use soundfont::{raw::GeneratorType, Zone};
use std::ops::RangeInclusive;

use super::{default_note_modulators, Sf2NoteModulator};

#[derive(Default, Clone, Debug)]
pub struct Sf2Zone {
    pub index: Option<u16>,
    pub offset: Option<i16>,
    pub end_offset: Option<i16>,
    pub offset_coarse: Option<i16>,
    pub end_offset_coarse: Option<i16>,
    pub loop_start_offset: Option<i16>,
    pub loop_start_offset_coarse: Option<i16>,
    pub loop_end_offset: Option<i16>,
    pub loop_end_offset_coarse: Option<i16>,
    pub loop_mode: Option<LoopMode>,
    pub cutoff: Option<i16>,
    pub resonance: Option<i16>,
    pub pan: Option<i16>,
    pub env_delay: Option<i16>,
    pub env_attack: Option<i16>,
    pub env_hold: Option<i16>,
    pub env_decay: Option<i16>,
    pub env_sustain: Option<i16>,
    pub env_release: Option<i16>,
    pub keynum_to_vol_env_hold: Option<i16>,
    pub keynum_to_vol_env_decay: Option<i16>,
    pub velrange: Option<RangeInclusive<u8>>,
    pub keyrange: Option<RangeInclusive<u8>>,
    pub attenuation: Option<i16>,
    pub fine_tune: Option<i16>,
    pub coarse_tune: Option<i16>,
    pub root_override: Option<i16>,
    pub fixed_key: Option<u8>,
    pub fixed_velocity: Option<u8>,
    pub scale_tuning: Option<i16>,
    pub exclusive_class: Option<u8>,
    pub note_modulators: Vec<Sf2NoteModulator>,
}

impl Sf2Zone {
    pub fn parse(zones: Vec<Zone>, instrument_level: bool) -> Vec<Self> {
        let mut regions: Vec<Sf2Zone> = Vec::new();
        let mut global_region = Sf2Zone::default();
        if instrument_level {
            global_region.note_modulators = default_note_modulators().to_vec();
        }

        for (i, zone) in zones.iter().enumerate() {
            let mut region = global_region.clone();

            for gen in &zone.gen_list {
                let Ok(gen_ty) = gen.ty.into_result() else {
                    // Some synths use non-spec generators let's just ignore them.
                    continue;
                };

                match gen_ty {
                    GeneratorType::StartAddrsOffset => region.offset = gen.amount.as_i16().copied(),
                    GeneratorType::EndAddrsOffset => {
                        region.end_offset = gen.amount.as_i16().copied()
                    }
                    GeneratorType::StartAddrsCoarseOffset => {
                        region.offset_coarse = gen.amount.as_i16().copied()
                    }
                    GeneratorType::EndAddrsCoarseOffset => {
                        region.end_offset_coarse = gen.amount.as_i16().copied()
                    }
                    GeneratorType::StartloopAddrsOffset => {
                        region.loop_start_offset = gen.amount.as_i16().copied()
                    }
                    GeneratorType::StartloopAddrsCoarseOffset => {
                        region.loop_start_offset_coarse = gen.amount.as_i16().copied()
                    }
                    GeneratorType::EndloopAddrsOffset => {
                        region.loop_end_offset = gen.amount.as_i16().copied()
                    }
                    GeneratorType::EndloopAddrsCoarseOffset => {
                        region.loop_end_offset_coarse = gen.amount.as_i16().copied()
                    }
                    GeneratorType::InitialFilterFc => region.cutoff = gen.amount.as_i16().copied(),
                    GeneratorType::InitialFilterQ => {
                        region.resonance = gen.amount.as_i16().copied()
                    }
                    GeneratorType::Pan => region.pan = gen.amount.as_i16().copied(),
                    GeneratorType::DelayVolEnv => region.env_delay = gen.amount.as_i16().copied(),
                    GeneratorType::AttackVolEnv => region.env_attack = gen.amount.as_i16().copied(),
                    GeneratorType::HoldVolEnv => region.env_hold = gen.amount.as_i16().copied(),
                    GeneratorType::DecayVolEnv => region.env_decay = gen.amount.as_i16().copied(),
                    GeneratorType::SustainVolEnv => {
                        region.env_sustain = gen.amount.as_i16().copied()
                    }
                    GeneratorType::ReleaseVolEnv => {
                        region.env_release = gen.amount.as_i16().copied()
                    }
                    GeneratorType::KeynumToVolEnvHold => {
                        region.keynum_to_vol_env_hold = gen.amount.as_i16().copied()
                    }
                    GeneratorType::KeynumToVolEnvDecay => {
                        region.keynum_to_vol_env_decay = gen.amount.as_i16().copied()
                    }
                    GeneratorType::KeyRange => {
                        let range = gen.amount.as_range().copied();
                        region.keyrange = range.map(|v| v.low..=v.high)
                    }
                    GeneratorType::VelRange => {
                        let range = gen.amount.as_range().copied();
                        region.velrange = range.map(|v| v.low..=v.high)
                    }
                    GeneratorType::InitialAttenuation => {
                        region.attenuation = gen.amount.as_i16().copied()
                    }
                    GeneratorType::CoarseTune => region.coarse_tune = gen.amount.as_i16().copied(),
                    GeneratorType::FineTune => region.fine_tune = gen.amount.as_i16().copied(),
                    GeneratorType::SampleID => region.index = gen.amount.as_u16().copied(),
                    GeneratorType::Instrument => region.index = gen.amount.as_u16().copied(),
                    GeneratorType::SampleModes => {
                        region.loop_mode = gen.amount.as_i16().map(|v| match v {
                            1 => LoopMode::LoopContinuous,
                            3 => LoopMode::LoopSustain,
                            _ => LoopMode::NoLoop,
                        })
                    }
                    GeneratorType::Keynum => {
                        region.fixed_key = gen.amount.as_i16().map(|v| (*v).clamp(0, 127) as u8)
                    }
                    GeneratorType::Velocity => {
                        region.fixed_velocity =
                            gen.amount.as_i16().map(|v| (*v).clamp(0, 127) as u8)
                    }
                    GeneratorType::ScaleTuning => {
                        region.scale_tuning = gen.amount.as_i16().copied()
                    }
                    GeneratorType::ExclusiveClass => {
                        region.exclusive_class = gen
                            .amount
                            .as_i16()
                            .map(|v| (*v).clamp(0, u8::MAX as i16) as u8)
                    }
                    GeneratorType::OverridingRootKey => {
                        region.root_override = gen.amount.as_i16().copied()
                    }
                    _ => {}
                }
            }

            for modulator in zone
                .mod_list
                .iter()
                .filter_map(Sf2NoteModulator::parse_zone)
            {
                region
                    .note_modulators
                    .retain(|existing| !modulator.suppresses_default(existing));
                region.note_modulators.push(modulator);
            }

            if i == 0 && region.index.is_none() {
                global_region = region;
            } else {
                regions.push(region);
            }
        }

        regions
    }
}
