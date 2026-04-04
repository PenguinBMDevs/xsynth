#![allow(clippy::uninit_vec, clippy::excessive_precision, clippy::needless_range_loop)]
use std::sync::Arc;

/// Fast linear interpolation resampler.
#[inline(never)]
pub fn resample_fast(input: &[f32], from_rate: f32, to_rate: f32) -> Arc<[f32]> {
    if from_rate == to_rate || input.is_empty() {
        return input.to_vec().into();
    }

    let ratio = (from_rate / to_rate) as f64;
    let output_len = ((input.len() - 1) as f64 / ratio) as usize + 1;
    #[allow(clippy::uninit_vec)] let mut output: Vec<f32> = Vec::with_capacity(output_len);
    unsafe { output.set_len(output_len); }
    let input_len = input.len();
    let mut src_pos = 0.0f64;

    for i in 0..output_len {
        let src_idx = src_pos as usize;
        let frac = (src_pos - src_idx as f64) as f32;
        output[i] = unsafe {
            let si = src_idx.min(input_len - 1);
            let s0 = *input.get_unchecked(si);
            let s1 = if si + 1 < input_len { *input.get_unchecked(si + 1) } else { s0 };
            s0 + (s1 - s0) * frac
        };
        src_pos += ratio;
    }

    output.into()
}

/// Resample directly from i16 data with fused i16->f32 + linear interpolation.
/// Optimized for speed: uses unchecked access, pre-computed scale, accumulated src_pos.
#[inline(never)]
pub fn resample_i16(input: &[i16], from_rate: f32, to_rate: f32) -> Arc<[f32]> {
    if from_rate == to_rate || input.is_empty() {
        let output: Vec<f32> = input.iter().map(|&s| s as f32 * 3.051850947599719e-5).collect();
        return output.into();
    }

    let ratio = (from_rate / to_rate) as f64;
    let output_len = ((input.len() - 1) as f64 / ratio) as usize + 1;
    #[allow(clippy::uninit_vec)] let mut output: Vec<f32> = Vec::with_capacity(output_len);
    unsafe { output.set_len(output_len); }
    let input_len = input.len();
    let mut src_pos = 0.0f64;

    for i in 0..output_len {
        let src_idx = src_pos as usize;
        let frac = (src_pos - src_idx as f64) as f32;
        unsafe {
            let si = src_idx.min(input_len - 1);
            let s0 = *input.get_unchecked(si) as f32 * 3.051850947599719e-5;
            let s1 = if si + 1 < input_len {
                *input.get_unchecked(si + 1) as f32 * 3.051850947599719e-5
            } else {
                s0
            };
            *output.get_unchecked_mut(i) = s0 + (s1 - s0) * frac;
        }
        src_pos += ratio;
    }

    output.into()
}

/// Resample multiple audio sample vectors.
pub fn resample_vecs(
    vecs: Vec<Vec<f32>>,
    sample_rate: f32,
    new_sample_rate: f32,
) -> Arc<[Arc<[f32]>]> {
    vecs.into_iter()
        .map(|samples| resample_fast(&samples, sample_rate, new_sample_rate))
        .collect()
}

/// Resample a single audio sample vector.
pub fn resample_vec(vec: Vec<f32>, sample_rate: f32, new_sample_rate: f32) -> Arc<[f32]> {
    resample_fast(&vec, sample_rate, new_sample_rate)
}
