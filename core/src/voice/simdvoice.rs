use std::marker::PhantomData;

use simdeez::prelude::*;

use crate::voice::{ReleaseType, VoiceControlData};

use super::{
    SIMDSample, SIMDSampleMono, SIMDSampleStereo, SIMDVoiceGenerator, VoiceGeneratorBase,
    VoiceSampleGenerator,
};

pub struct SIMDStereoVoice<S: Simd, T: SIMDVoiceGenerator<S, SIMDSampleStereo<S>>> {
    generator: T,
    remainder: SIMDSampleStereo<S>,
    remainder_pos: usize,
    _s: PhantomData<S>,
}

impl<S: Simd, T: SIMDVoiceGenerator<S, SIMDSampleStereo<S>>> SIMDStereoVoice<S, T> {
    pub fn new(generator: T) -> SIMDStereoVoice<S, T> {
        SIMDStereoVoice {
            generator,
            remainder: SIMDSampleStereo::<S>::zero(),
            remainder_pos: S::Vf32::WIDTH,
            _s: PhantomData,
        }
    }
}

impl<S, T> VoiceGeneratorBase for SIMDStereoVoice<S, T>
where
    S: Simd,
    T: SIMDVoiceGenerator<S, SIMDSampleStereo<S>>,
{
    #[inline]
    fn ended(&self) -> bool {
        self.generator.ended()
    }

    #[inline]
    fn signal_release(&mut self, rel_type: ReleaseType) {
        self.generator.signal_release(rel_type)
    }

    #[inline]
    fn process_controls(&mut self, control: &VoiceControlData) {
        self.generator.process_controls(control)
    }
}

impl<S, T> VoiceSampleGenerator for SIMDStereoVoice<S, T>
where
    S: Simd,
    T: SIMDVoiceGenerator<S, SIMDSampleStereo<S>>,
{
    #[inline]
    fn render_to(&mut self, buffer: &mut [f32]) {
        simd_invoke!(S, {
            let width = S::Vf32::WIDTH;
            let mut buf_idx = 0;
            let buf_len = buffer.len();

            // First, consume any remainder from previous call
            while buf_idx < buf_len && self.remainder_pos < width {
                unsafe {
                    *buffer.get_unchecked_mut(buf_idx) +=
                        self.remainder.0.get_unchecked(self.remainder_pos);
                    *buffer.get_unchecked_mut(buf_idx + 1) +=
                        self.remainder.1.get_unchecked(self.remainder_pos);
                }
                buf_idx += 2;
                self.remainder_pos += 1;
            }

            // Stereo has interleaved L/R, so we need to process samples individually
            // But we can still benefit from batching generator calls
            let samples_per_batch = width * 2;
            while buf_idx + samples_per_batch <= buf_len {
                let sample = self.generator.next_sample();

                #[cfg(target_arch = "x86_64")]
                {
                    if width == 8 {
                        use std::arch::x86_64::*;
                        unsafe {
                            let l_m256 = std::ptr::read(&sample.0 as *const _ as *const __m256);
                            let r_m256 = std::ptr::read(&sample.1 as *const _ as *const __m256);

                            let lo = _mm256_unpacklo_ps(l_m256, r_m256);
                            let hi = _mm256_unpackhi_ps(l_m256, r_m256);

                            let first_lo128 = _mm256_castps256_ps128(lo);
                            let first_hi128 = _mm256_castps256_ps128(hi);
                            let mut first = _mm256_castps128_ps256(first_lo128);
                            first = _mm256_insertf128_ps(first, first_hi128, 1);

                            let second_lo128 = _mm256_extractf128_ps(lo, 1);
                            let second_hi128 = _mm256_extractf128_ps(hi, 1);
                            let mut second = _mm256_castps128_ps256(second_lo128);
                            second = _mm256_insertf128_ps(second, second_hi128, 1);

                            let buf_ptr = buffer.as_mut_ptr().add(buf_idx);

                            let dst1 = _mm256_loadu_ps(buf_ptr);
                            let dst2 = _mm256_loadu_ps(buf_ptr.add(8));

                            let res1 = _mm256_add_ps(dst1, first);
                            let res2 = _mm256_add_ps(dst2, second);

                            _mm256_storeu_ps(buf_ptr, res1);
                            _mm256_storeu_ps(buf_ptr.add(8), res2);
                        }
                        buf_idx += samples_per_batch;
                        continue;
                    }
                }

                #[cfg(target_arch = "aarch64")]
                {
                    if width == 4 {
                        use std::arch::aarch64::*;
                        unsafe {
                            let l_m128 =
                                std::ptr::read(&sample.0 as *const _ as *const float32x4_t);
                            let r_m128 =
                                std::ptr::read(&sample.1 as *const _ as *const float32x4_t);

                            let zip1 = vzip1q_f32(l_m128, r_m128);
                            let zip2 = vzip2q_f32(l_m128, r_m128);

                            let buf_ptr = buffer.as_mut_ptr().add(buf_idx);

                            let dst1 = vld1q_f32(buf_ptr);
                            let dst2 = vld1q_f32(buf_ptr.add(4));

                            let res1 = vaddq_f32(dst1, zip1);
                            let res2 = vaddq_f32(dst2, zip2);

                            vst1q_f32(buf_ptr, res1);
                            vst1q_f32(buf_ptr.add(4), res2);
                        }
                        buf_idx += samples_per_batch;
                        continue;
                    }
                }

                unsafe {
                    let buf_ptr = buffer.as_mut_ptr().add(buf_idx);
                    for i in 0..width {
                        *buf_ptr.add(i * 2) += sample.0.get_unchecked(i);
                        *buf_ptr.add(i * 2 + 1) += sample.1.get_unchecked(i);
                    }
                }
                buf_idx += samples_per_batch;
            }

            // Handle remaining samples
            if buf_idx < buf_len {
                self.remainder = self.generator.next_sample();
                self.remainder_pos = 0;
                while buf_idx < buf_len {
                    unsafe {
                        *buffer.get_unchecked_mut(buf_idx) +=
                            self.remainder.0.get_unchecked(self.remainder_pos);
                        *buffer.get_unchecked_mut(buf_idx + 1) +=
                            self.remainder.1.get_unchecked(self.remainder_pos);
                    }
                    buf_idx += 2;
                    self.remainder_pos += 1;
                }
            }
        })
    }
}

pub struct SIMDMonoVoice<S: Simd, T: SIMDVoiceGenerator<S, SIMDSampleMono<S>>> {
    generator: T,
    remainder: SIMDSampleMono<S>,
    remainder_pos: usize,
    _s: PhantomData<S>,
}

impl<S: Simd, T: SIMDVoiceGenerator<S, SIMDSampleMono<S>>> SIMDMonoVoice<S, T> {
    pub fn new(generator: T) -> SIMDMonoVoice<S, T> {
        SIMDMonoVoice {
            generator,
            remainder: SIMDSampleMono::<S>::zero(),
            remainder_pos: S::Vf32::WIDTH,
            _s: PhantomData,
        }
    }
}

impl<S, T> VoiceGeneratorBase for SIMDMonoVoice<S, T>
where
    S: Simd,
    T: SIMDVoiceGenerator<S, SIMDSampleMono<S>>,
{
    #[inline]
    fn ended(&self) -> bool {
        self.generator.ended()
    }

    #[inline]
    fn signal_release(&mut self, rel_type: ReleaseType) {
        self.generator.signal_release(rel_type)
    }

    #[inline]
    fn process_controls(&mut self, control: &VoiceControlData) {
        self.generator.process_controls(control)
    }
}

impl<S, T> VoiceSampleGenerator for SIMDMonoVoice<S, T>
where
    S: Simd,
    T: SIMDVoiceGenerator<S, SIMDSampleMono<S>>,
{
    #[inline]
    fn render_to(&mut self, buffer: &mut [f32]) {
        simd_invoke!(S, {
            let width = S::Vf32::WIDTH;
            let mut buf_idx = 0;
            let buf_len = buffer.len();

            // First, consume any remainder from previous call
            while buf_idx < buf_len && self.remainder_pos < width {
                unsafe {
                    *buffer.get_unchecked_mut(buf_idx) +=
                        self.remainder.0.get_unchecked(self.remainder_pos);
                }
                buf_idx += 1;
                self.remainder_pos += 1;
            }

            // Process SIMD batches using SIMD load/add/store
            while buf_idx + width <= buf_len {
                let sample = self.generator.next_sample();
                unsafe {
                    let buf_ptr = buffer.as_mut_ptr().add(buf_idx);
                    let dst = S::Vf32::load_from_ptr_unaligned(buf_ptr);
                    (dst + sample.0).copy_to_ptr_unaligned(buf_ptr);
                }
                buf_idx += width;
            }

            // Handle remaining samples
            if buf_idx < buf_len {
                self.remainder = self.generator.next_sample();
                self.remainder_pos = 0;
                while buf_idx < buf_len {
                    unsafe {
                        *buffer.get_unchecked_mut(buf_idx) +=
                            self.remainder.0.get_unchecked(self.remainder_pos);
                    }
                    buf_idx += 1;
                    self.remainder_pos += 1;
                }
            }
        })
    }
}
