use std::marker::PhantomData;

use simdeez::prelude::*;

use super::{SIMDSampleGrabber, SampleReader};

pub struct SIMDLinearSampleGrabber<S: Simd, Reader: SampleReader> {
    sampler_reader: Reader,
    _s: PhantomData<S>,
}

impl<S: Simd, Reader: SampleReader> SIMDLinearSampleGrabber<S, Reader> {
    pub fn new(sampler_reader: Reader) -> Self {
        SIMDLinearSampleGrabber {
            sampler_reader,
            _s: PhantomData,
        }
    }
}

impl<S: Simd, Reader: SampleReader> SIMDSampleGrabber<S> for SIMDLinearSampleGrabber<S, Reader> {
    fn get(&mut self, indexes: S::Vi32, fractional: S::Vf32) -> S::Vf32 {
        simd_invoke!(S, {
            let ones = S::Vf32::set1(1.0f32);
            let blend = fractional;
            let mut values_first = S::Vf32::zeroes();
            let mut values_second = S::Vf32::zeroes();

            #[cfg(target_arch = "x86_64")]
            {
                if S::Vf32::WIDTH == 8 {
                    use std::arch::x86_64::*;
                    let idx = unsafe { std::ptr::read(&indexes as *const _ as *const __m256i) };
                    let next_idx = unsafe { _mm256_add_epi32(idx, _mm256_set1_epi32(1)) };
                    let values_first_m256 = unsafe { self.sampler_reader.get_simd_avx2(idx) };
                    let values_second_m256 = unsafe { self.sampler_reader.get_simd_avx2(next_idx) };

                    let blend_m256 = unsafe { std::ptr::read(&blend as *const _ as *const __m256) };
                    let ones_m256 = unsafe { _mm256_set1_ps(1.0) };
                    let inv_blend = unsafe { _mm256_sub_ps(ones_m256, blend_m256) };
                    let term2 = unsafe { _mm256_mul_ps(values_second_m256, blend_m256) };
                    let blended = unsafe { _mm256_fmadd_ps(values_first_m256, inv_blend, term2) };

                    return unsafe { std::ptr::read(&blended as *const _ as *const S::Vf32) };
                }
            }

            #[cfg(target_arch = "aarch64")]
            {
                if S::Vf32::WIDTH == 4 {
                    use std::arch::aarch64::*;
                    let idx = unsafe { std::ptr::read(&indexes as *const _ as *const int32x4_t) };
                    let next_idx = unsafe { vaddq_s32(idx, vdupq_n_s32(1)) };
                    let values_first_m128 = unsafe { self.sampler_reader.get_simd_neon(idx) };
                    let values_second_m128 = unsafe { self.sampler_reader.get_simd_neon(next_idx) };

                    let blend_m128 =
                        unsafe { std::ptr::read(&blend as *const _ as *const float32x4_t) };
                    let ones_m128 = unsafe { vdupq_n_f32(1.0) };
                    let inv_blend = unsafe { vsubq_f32(ones_m128, blend_m128) };
                    let term1 = unsafe { vmulq_f32(values_first_m128, inv_blend) };
                    let term2 = unsafe { vmulq_f32(values_second_m128, blend_m128) };
                    let blended = unsafe { vaddq_f32(term1, term2) };

                    return unsafe { std::ptr::read(&blended as *const _ as *const S::Vf32) };
                }
            }

            unsafe {
                for i in 0..S::Vf32::WIDTH {
                    let index = indexes.get_unchecked(i) as usize;
                    *values_first.get_unchecked_mut(i) = self.sampler_reader.get(index);
                    *values_second.get_unchecked_mut(i) = self.sampler_reader.get(index + 1);
                }
            }

            let blended = values_first * (ones - blend) + values_second * blend;

            blended
        },)
    }

    fn is_past_end(&self, pos: f64) -> bool {
        let pos = pos as usize;
        self.sampler_reader.is_past_end(pos)
    }

    fn signal_release(&mut self) {
        self.sampler_reader.signal_release();
    }
}
