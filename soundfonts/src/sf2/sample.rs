use super::Sf2ParseError;
use crate::resample::resample_i16;
use rayon::prelude::*;
use soundfont::raw::{SampleChunk, SampleData, SampleHeader, SampleLink};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    sync::Arc,
};

#[derive(Clone, Debug)]
pub struct Sf2Sample {
    pub data: Arc<[f32]>,
    pub link_type: i8,
    pub loop_start: u32,
    pub loop_end: u32,
    pub sample_rate: u32,
    pub origpitch: u8,
    pub pitchadj: i8,
}

impl Sf2Sample {
    fn read_chunk(file: &mut File, chunk: SampleChunk) -> io::Result<Vec<u8>> {
        let mut buff = vec![0u8; chunk.len as usize];
        file.seek(SeekFrom::Start(chunk.offset))?;
        file.read_exact(&mut buff)?;
        Ok(buff)
    }

    pub fn parse_sf2_samples(
        file: &mut File,
        headers: Vec<SampleHeader>,
        data: SampleData,
        sample_rate: u32,
    ) -> Result<Vec<Self>, Sf2ParseError> {
        let smpl = if let Some(chunk) = data.smpl {
            Self::read_chunk(file, chunk).map_err(|_| {
                Sf2ParseError::FailedToParseFile("Error reading sample contents".to_string())
            })?
        } else {
            return Err(Sf2ParseError::FailedToParseFile(
                "Soundfont does not contain samples".to_string(),
            ));
        };

        if let Some(sm24) = data.sm24 {
            return Self::parse_sf2_samples_f32(&smpl, Some(sm24), file, headers, sample_rate);
        }

        // 16-bit path: reinterpret bytes as i16 slice
        let all_i16: &[i16] = unsafe {
            let ptr = smpl.as_ptr() as *const i16;
            let len = smpl.len() / 2;
            std::slice::from_raw_parts(ptr, len)
        };

        let needs_resample = headers.iter().any(|h| h.sample_rate != sample_rate);

        if !needs_resample {
            // Fast path: no resampling needed
            let all_f32: Vec<f32> = all_i16
                .par_iter()
                .map(|&s| s as f32 / i16::MAX as f32)
                .collect();

            let out: Vec<Sf2Sample> = headers
                .into_iter()
                .map(|h| {
                    let start = h.start as usize;
                    let end = h.end as usize;
                    Sf2Sample {
                        data: all_f32[start..end].into(),
                        link_type: match h.sample_type {
                            SampleLink::LeftSample => -1,
                            SampleLink::RightSample => 1,
                            _ => 0,
                        },
                        loop_start: h.loop_start - h.start,
                        loop_end: h.loop_end - h.start,
                        sample_rate: h.sample_rate,
                        origpitch: h.origpitch,
                        pitchadj: h.pitchadj,
                    }
                })
                .collect();
            return Ok(out);
        }

        // Per-sample parallel resampling (best balance of parallelism vs overhead)
        let results: Vec<Sf2Sample> = headers
            .into_par_iter()
            .map(|h| {
                let start = h.start as usize;
                let end = h.end as usize;
                let i16_slice = &all_i16[start..end];

                let data = if h.sample_rate != sample_rate && !i16_slice.is_empty() {
                    resample_i16(i16_slice, h.sample_rate as f32, sample_rate as f32)
                } else {
                    let f: Vec<f32> = i16_slice
                        .iter()
                        .map(|&s| s as f32 / i16::MAX as f32)
                        .collect();
                    f.into()
                };

                Sf2Sample {
                    data,
                    link_type: match h.sample_type {
                        SampleLink::LeftSample => -1,
                        SampleLink::RightSample => 1,
                        _ => 0,
                    },
                    loop_start: h.loop_start - h.start,
                    loop_end: h.loop_end - h.start,
                    sample_rate: h.sample_rate,
                    origpitch: h.origpitch,
                    pitchadj: h.pitchadj,
                }
            })
            .collect();

        Ok(results)
    }

    /// Original f32-based path for 24-bit samples
    fn parse_sf2_samples_f32(
        smpl: &[u8],
        sm24: Option<SampleChunk>,
        file: &mut File,
        headers: Vec<SampleHeader>,
        sample_rate: u32,
    ) -> Result<Vec<Self>, Sf2ParseError> {
        let all_samples: Vec<f32> = if let Some(chunk) = sm24 {
            let extra = Self::read_chunk(file, chunk).map_err(|_| {
                Sf2ParseError::FailedToParseFile("Error reading extra sample contents".to_string())
            })?;

            let smpllen = smpl.len() / 2;
            let extralen = extra.len() - (smpllen % 2);
            if smpllen != extralen {
                return Err(Sf2ParseError::FailedToParseFile(
                    "Invalid sample length".to_string(),
                ));
            }

            let mut vec = Vec::with_capacity(extralen);
            for i in 0..extralen {
                let sample = i32::from_le_bytes([0, extra[i], smpl[i * 2], smpl[i * 2 + 1]]);
                vec.push(sample as f32 / i32::MAX as f32);
            }
            vec
        } else {
            let mut vec = Vec::with_capacity(smpl.len() / 2);
            for i in smpl.chunks(2) {
                let sample = i16::from_le_bytes([i[0], i[1]]);
                vec.push(sample as f32 / i16::MAX as f32);
            }
            vec
        };

        let needs_resample = headers.iter().any(|h| h.sample_rate != sample_rate);

        if !needs_resample {
            let out: Vec<Sf2Sample> = headers
                .into_iter()
                .map(|h| {
                    let start = h.start as usize;
                    let end = h.end as usize;
                    Sf2Sample {
                        data: all_samples[start..end].into(),
                        link_type: match h.sample_type {
                            SampleLink::LeftSample => -1,
                            SampleLink::RightSample => 1,
                            _ => 0,
                        },
                        loop_start: h.loop_start - h.start,
                        loop_end: h.loop_end - h.start,
                        sample_rate: h.sample_rate,
                        origpitch: h.origpitch,
                        pitchadj: h.pitchadj,
                    }
                })
                .collect();
            return Ok(out);
        }

        let sample_slices: Vec<(SampleHeader, Vec<f32>)> = headers
            .iter()
            .map(|h| {
                let start = h.start as usize;
                let end = h.end as usize;
                (h.clone(), all_samples[start..end].to_vec())
            })
            .collect();

        use crate::resample::resample_vec;
        let results: Vec<Sf2Sample> = sample_slices
            .into_par_iter()
            .map(|(h, sample)| {
                let data = if h.sample_rate != sample_rate && !sample.is_empty() {
                    resample_vec(sample, h.sample_rate as f32, sample_rate as f32)
                } else {
                    sample.into()
                };

                Sf2Sample {
                    data,
                    link_type: match h.sample_type {
                        SampleLink::LeftSample => -1,
                        SampleLink::RightSample => 1,
                        _ => 0,
                    },
                    loop_start: h.loop_start - h.start,
                    loop_end: h.loop_end - h.start,
                    sample_rate: h.sample_rate,
                    origpitch: h.origpitch,
                    pitchadj: h.pitchadj,
                }
            })
            .collect();

        Ok(results)
    }
}
