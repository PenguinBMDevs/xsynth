use std::{marker::PhantomData, sync::Arc};

use simdeez::prelude::*;

use crate::soundfont::LoopParams;
use crate::voice::{ReleaseType, VoiceControlData};

use super::{SIMDSampleMono, SIMDSampleStereo, SIMDVoiceGenerator, VoiceGeneratorBase};

mod linear;
pub use linear::*;

mod nearest;
pub use nearest::*;

// I believe some terminology reference is relevant for this one.
//
// BufferSampler: Something that grabs a sample based on an index
//
// SampleReader: Something that grabs the sample value at an arbitrary index,
// and implements sample start/end/looping
//
// SIMDSampleGrabber: Something that takes a SIMD array of float64 locations and
// returns a SIMD array of f32 interpolated sample values

// Base traits

pub trait BufferSampler: Send + Sync {
    fn get(&self, pos: usize) -> f32;
    fn length(&self) -> usize;
    fn ptr(&self) -> *const f32;
}

pub trait SIMDSampleGrabber<S: Simd>: Send + Sync {
    /// Indexes: the rounded index of the sample
    ///
    /// Fractional: The fractional part of the index, i.e. the 0-1 range decimal
    fn get(&mut self, indexes: S::Vi32, fractional: S::Vf32) -> S::Vf32;

    fn is_past_end(&self, pos: f64) -> bool;

    fn signal_release(&mut self);
}

// F32 sampler

pub struct F32BufferSampler(Arc<[f32]>);

impl BufferSampler for F32BufferSampler {
    #[inline]
    fn get(&self, pos: usize) -> f32 {
        // SAFETY: Callers ensure pos is within bounds via is_past_end checks
        // Use unchecked access for maximum performance in the hot path
        unsafe { *self.0.get_unchecked(pos) }
    }

    fn length(&self) -> usize {
        self.0.len()
    }

    #[inline]
    fn ptr(&self) -> *const f32 {
        self.0.as_ptr()
    }
}

// Generalized enum sampler

pub enum BufferSamplers {
    F32(F32BufferSampler),
}

impl BufferSamplers {
    #[inline]
    pub fn new_f32(sample: Arc<[f32]>) -> BufferSamplers {
        BufferSamplers::F32(F32BufferSampler(sample))
    }
}

impl BufferSampler for BufferSamplers {
    #[inline]
    fn get(&self, pos: usize) -> f32 {
        match self {
            BufferSamplers::F32(sampler) => sampler.get(pos),
        }
    }

    fn length(&self) -> usize {
        match self {
            BufferSamplers::F32(sampler) => sampler.length(),
        }
    }

    #[inline]
    fn ptr(&self) -> *const f32 {
        match self {
            BufferSamplers::F32(sampler) => sampler.ptr(),
        }
    }
}

// Enum sampler reader

pub trait SampleReader: Send + Sync {
    fn get(&mut self, pos: usize) -> f32;
    fn is_past_end(&self, pos: usize) -> bool;
    fn signal_release(&mut self);

    #[cfg(target_arch = "x86_64")]
    unsafe fn get_simd_avx2(
        &mut self,
        indexes: std::arch::x86_64::__m256i,
    ) -> std::arch::x86_64::__m256;

    #[cfg(target_arch = "aarch64")]
    unsafe fn get_simd_neon(
        &mut self,
        indexes: std::arch::aarch64::int32x4_t,
    ) -> std::arch::aarch64::float32x4_t;
}

pub struct SampleReaderNoLoop<Sampler: BufferSampler> {
    buffer: Sampler,
    length: Option<usize>,
    offset: usize,
}

impl<Sampler: BufferSampler> SampleReaderNoLoop<Sampler> {
    pub fn new(buffer: Sampler, loop_params: LoopParams) -> Self {
        let length = Some(buffer.length());
        Self {
            buffer,
            length,
            offset: loop_params.offset as usize,
        }
    }
}

impl<Sampler: BufferSampler> SampleReader for SampleReaderNoLoop<Sampler> {
    fn get(&mut self, pos: usize) -> f32 {
        self.buffer.get(pos + self.offset)
    }

    fn is_past_end(&self, pos: usize) -> bool {
        if let Some(len) = self.length {
            pos - self.offset.min(pos) >= len
        } else {
            false
        }
    }

    fn signal_release(&mut self) {}

    #[cfg(target_arch = "x86_64")]
    #[inline]
    unsafe fn get_simd_avx2(
        &mut self,
        indexes: std::arch::x86_64::__m256i,
    ) -> std::arch::x86_64::__m256 {
        use std::arch::x86_64::*;
        let offset = _mm256_set1_epi32(self.offset as i32);
        let pos = _mm256_add_epi32(indexes, offset);
        _mm256_i32gather_ps(self.buffer.ptr(), pos, 4)
    }

    #[cfg(target_arch = "aarch64")]
    #[inline]
    unsafe fn get_simd_neon(
        &mut self,
        indexes: std::arch::aarch64::int32x4_t,
    ) -> std::arch::aarch64::float32x4_t {
        use std::arch::aarch64::*;
        let mut idx = [0i32; 4];
        let offset = vdupq_n_s32(self.offset as i32);
        vst1q_s32(idx.as_mut_ptr(), vaddq_s32(indexes, offset));

        let mut res = [0.0f32; 4];
        res[0] = self.buffer.get(idx[0] as usize);
        res[1] = self.buffer.get(idx[1] as usize);
        res[2] = self.buffer.get(idx[2] as usize);
        res[3] = self.buffer.get(idx[3] as usize);
        vld1q_f32(res.as_ptr())
    }
}

pub struct SampleReaderLoop<Sampler: BufferSampler> {
    buffer: Sampler,
    offset: usize,
    loop_start: usize,
    loop_end: usize,
}

impl<Sampler: BufferSampler> SampleReaderLoop<Sampler> {
    pub fn new(buffer: Sampler, loop_params: LoopParams) -> Self {
        Self {
            buffer,
            offset: loop_params.offset as usize,
            loop_start: loop_params.start as usize,
            loop_end: loop_params.end as usize,
        }
    }
}

impl<Sampler: BufferSampler> SampleReader for SampleReaderLoop<Sampler> {
    fn get(&mut self, pos: usize) -> f32 {
        let mut pos = pos + self.offset;
        let end = self.loop_end;
        let start = self.loop_start;

        if pos > end {
            pos = (pos - end - 1) % (end - start) + start;
        }

        self.buffer.get(pos)
    }

    fn is_past_end(&self, _pos: usize) -> bool {
        false
    }

    fn signal_release(&mut self) {}

    #[cfg(target_arch = "x86_64")]
    #[inline]
    unsafe fn get_simd_avx2(
        &mut self,
        indexes: std::arch::x86_64::__m256i,
    ) -> std::arch::x86_64::__m256 {
        use std::arch::x86_64::*;
        let offset = _mm256_set1_epi32(self.offset as i32);
        let pos = _mm256_add_epi32(indexes, offset);

        let end = _mm256_set1_epi32(self.loop_end as i32);
        let start = _mm256_set1_epi32(self.loop_start as i32);
        let loop_len = _mm256_set1_ps((self.loop_end - self.loop_start) as f32);

        let cmp = _mm256_cmpgt_epi32(pos, end);

        let mask_cmp = _mm256_movemask_epi8(cmp);
        if mask_cmp == 0 {
            _mm256_i32gather_ps(self.buffer.ptr(), pos, 4)
        } else {
            let a = _mm256_sub_epi32(_mm256_sub_epi32(pos, end), _mm256_set1_epi32(1));
            let a_f32 = _mm256_cvtepi32_ps(a);
            let div = _mm256_div_ps(a_f32, loop_len);
            let div_i32 = _mm256_cvttps_epi32(div);

            let mul = _mm256_mullo_epi32(
                div_i32,
                _mm256_set1_epi32((self.loop_end - self.loop_start) as i32),
            );
            let rem = _mm256_sub_epi32(a, mul);
            let wrapped_pos = _mm256_add_epi32(rem, start);

            let final_pos = _mm256_blendv_epi8(pos, wrapped_pos, cmp);
            _mm256_i32gather_ps(self.buffer.ptr(), final_pos, 4)
        }
    }

    #[cfg(target_arch = "aarch64")]
    #[inline]
    unsafe fn get_simd_neon(
        &mut self,
        indexes: std::arch::aarch64::int32x4_t,
    ) -> std::arch::aarch64::float32x4_t {
        use std::arch::aarch64::*;
        let mut idx = [0i32; 4];
        let offset = vdupq_n_s32(self.offset as i32);
        vst1q_s32(idx.as_mut_ptr(), vaddq_s32(indexes, offset));

        let mut res = [0.0f32; 4];
        let end = self.loop_end as i32;
        let start = self.loop_start as i32;
        let loop_len = (end - start).max(1); // prevent div by zero

        for i in 0..4 {
            let mut pos = idx[i];
            if pos > end {
                pos = (pos - end - 1) % loop_len + start;
            }
            res[i] = self.buffer.get(pos as usize);
        }

        vld1q_f32(res.as_ptr())
    }
}

pub struct SampleReaderLoopSustain<Sampler: BufferSampler> {
    buffer: Sampler,
    length: Option<usize>,
    offset: usize,
    loop_start: usize,
    loop_end: usize,
    last: usize,
    is_released: bool,
}

impl<Sampler: BufferSampler> SampleReaderLoopSustain<Sampler> {
    pub fn new(buffer: Sampler, loop_params: LoopParams) -> Self {
        let length = Some(buffer.length());
        Self {
            buffer,
            length,
            offset: loop_params.offset as usize,
            loop_start: loop_params.start as usize,
            loop_end: loop_params.end as usize,
            last: 0,
            is_released: false,
        }
    }
}

impl<Sampler: BufferSampler> SampleReader for SampleReaderLoopSustain<Sampler> {
    fn get(&mut self, pos: usize) -> f32 {
        let pos = pos + self.offset;
        let end = self.loop_end;
        let start = self.loop_start;

        let final_pos = if !self.is_released {
            if pos > end && end > start {
                let loop_len = end - start;
                let wrapped = start + (pos - end - 1) % loop_len;
                self.last = pos - self.offset;
                wrapped
            } else {
                pos
            }
        } else {
            let release_pos = pos - self.last;
            release_pos
        };

        self.buffer.get(final_pos)
    }

    fn is_past_end(&self, pos: usize) -> bool {
        if !self.is_released {
            return false;
        }

        if let Some(len) = self.length {
            let effective_pos = if self.last > self.offset {
                pos + self.last - self.offset
            } else {
                pos
            };
            effective_pos >= len
        } else {
            false
        }
    }

    fn signal_release(&mut self) {
        if !self.is_released {
            self.is_released = true;
            self.last = self.last.max(self.offset);
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[inline]
    unsafe fn get_simd_avx2(
        &mut self,
        indexes: std::arch::x86_64::__m256i,
    ) -> std::arch::x86_64::__m256 {
        use std::arch::x86_64::*;
        let offset = _mm256_set1_epi32(self.offset as i32);
        let pos = _mm256_add_epi32(indexes, offset);

        if !self.is_released {
            let end = _mm256_set1_epi32(self.loop_end as i32);
            let start = _mm256_set1_epi32(self.loop_start as i32);

            let cmp_pos = _mm256_cmpgt_epi32(pos, end);
            let cmp_end = _mm256_cmpgt_epi32(end, start);
            let cmp = _mm256_and_si256(cmp_pos, cmp_end);

            let mask_cmp = _mm256_movemask_epi8(cmp);
            if mask_cmp == 0 {
                _mm256_i32gather_ps(self.buffer.ptr(), pos, 4)
            } else {
                let loop_len_f32 =
                    _mm256_set1_ps((self.loop_end.saturating_sub(self.loop_start)) as f32);
                let loop_len_i32 =
                    _mm256_set1_epi32((self.loop_end.saturating_sub(self.loop_start)) as i32);

                let a = _mm256_sub_epi32(_mm256_sub_epi32(pos, end), _mm256_set1_epi32(1));
                let a_f32 = _mm256_cvtepi32_ps(a);
                let div = _mm256_div_ps(a_f32, loop_len_f32);
                let div_i32 = _mm256_cvttps_epi32(div);

                let mul = _mm256_mullo_epi32(div_i32, loop_len_i32);
                let rem = _mm256_sub_epi32(a, mul);
                let wrapped_pos = _mm256_add_epi32(rem, start);

                let max_idx = _mm256_extract_epi32(pos, 7);
                if max_idx > self.loop_end as i32 && self.loop_end > self.loop_start {
                    self.last = (max_idx - self.offset as i32) as usize;
                }

                let final_pos = _mm256_blendv_epi8(pos, wrapped_pos, cmp);
                _mm256_i32gather_ps(self.buffer.ptr(), final_pos, 4)
            }
        } else {
            let last_vec = _mm256_set1_epi32(self.last as i32);
            let release_pos = _mm256_sub_epi32(pos, last_vec);
            _mm256_i32gather_ps(self.buffer.ptr(), release_pos, 4)
        }
    }

    #[cfg(target_arch = "aarch64")]
    #[inline]
    unsafe fn get_simd_neon(
        &mut self,
        indexes: std::arch::aarch64::int32x4_t,
    ) -> std::arch::aarch64::float32x4_t {
        use std::arch::aarch64::*;
        let mut idx = [0i32; 4];
        let offset = vdupq_n_s32(self.offset as i32);
        vst1q_s32(idx.as_mut_ptr(), vaddq_s32(indexes, offset));

        let mut res = [0.0f32; 4];
        let end = self.loop_end as i32;
        let start = self.loop_start as i32;

        if !self.is_released {
            let loop_len = (end - start).max(1);
            let mut max_idx = 0;

            for i in 0..4 {
                let mut pos = idx[i];
                max_idx = max_idx.max(pos);

                if pos > end && end > start {
                    pos = start + (pos - end - 1) % loop_len;
                }
                res[i] = self.buffer.get(pos as usize);
            }

            if max_idx > end && end > start {
                self.last = (max_idx - self.offset as i32) as usize;
            }
        } else {
            for i in 0..4 {
                res[i] = self.buffer.get((idx[i] - self.last as i32) as usize);
            }
        }

        vld1q_f32(res.as_ptr())
    }
}

// Sample grabbers enum

pub enum SIMDSampleGrabbers<S: Simd, Reader: SampleReader> {
    Nearest(SIMDNearestSampleGrabber<S, Reader>),
    Linear(SIMDLinearSampleGrabber<S, Reader>),
}

impl<S: Simd, Reader: SampleReader> SIMDSampleGrabbers<S, Reader> {
    pub fn nearest(reader: Reader) -> Self {
        SIMDSampleGrabbers::Nearest(SIMDNearestSampleGrabber::new(reader))
    }

    pub fn linear(reader: Reader) -> Self {
        SIMDSampleGrabbers::Linear(SIMDLinearSampleGrabber::new(reader))
    }
}

impl<S: Simd, Reader: SampleReader> SIMDSampleGrabber<S> for SIMDSampleGrabbers<S, Reader> {
    #[inline]
    fn get(&mut self, indexes: S::Vi32, fractional: S::Vf32) -> S::Vf32 {
        match self {
            SIMDSampleGrabbers::Linear(grabber) => grabber.get(indexes, fractional),
            SIMDSampleGrabbers::Nearest(grabber) => grabber.get(indexes, fractional),
        }
    }

    #[inline]
    fn is_past_end(&self, pos: f64) -> bool {
        match self {
            SIMDSampleGrabbers::Linear(grabber) => grabber.is_past_end(pos),
            SIMDSampleGrabbers::Nearest(grabber) => grabber.is_past_end(pos),
        }
    }

    #[inline]
    fn signal_release(&mut self) {
        match self {
            SIMDSampleGrabbers::Linear(grabber) => grabber.signal_release(),
            SIMDSampleGrabbers::Nearest(grabber) => grabber.signal_release(),
        }
    }
}

// Sampler generator

pub struct SIMDMonoVoiceSampler<S, Pitch, Grabber>
where
    S: Simd,
    Pitch: SIMDVoiceGenerator<S, SIMDSampleMono<S>>,
    Grabber: SIMDSampleGrabber<S>,
{
    grabber: Grabber,

    pitch_gen: Pitch,

    time: f64,

    _s: PhantomData<S>,
}

impl<S, Pitch, Grabber> SIMDMonoVoiceSampler<S, Pitch, Grabber>
where
    S: Simd,
    Pitch: SIMDVoiceGenerator<S, SIMDSampleMono<S>>,
    Grabber: SIMDSampleGrabber<S>,
{
    pub fn new(grabber: Grabber, pitch_gen: Pitch) -> Self {
        SIMDMonoVoiceSampler {
            grabber,
            pitch_gen,
            time: 0.0,
            _s: PhantomData,
        }
    }

    fn increment_time(&mut self, by: f64) -> f64 {
        let time = self.time;
        self.time += by;
        time
    }
}

impl<S, Pitch, Grabber> VoiceGeneratorBase for SIMDMonoVoiceSampler<S, Pitch, Grabber>
where
    S: Simd,
    Pitch: SIMDVoiceGenerator<S, SIMDSampleMono<S>>,
    Grabber: SIMDSampleGrabber<S>,
{
    #[inline]
    fn ended(&self) -> bool {
        self.grabber.is_past_end(self.time)
    }

    #[inline]
    fn signal_release(&mut self, rel_type: ReleaseType) {
        self.pitch_gen.signal_release(rel_type);
        self.grabber.signal_release();
    }

    #[inline]
    fn process_controls(&mut self, control: &VoiceControlData) {
        self.pitch_gen.process_controls(control);
    }
}

impl<S, Pitch, Grabber> SIMDVoiceGenerator<S, SIMDSampleMono<S>>
    for SIMDMonoVoiceSampler<S, Pitch, Grabber>
where
    S: Simd,
    Pitch: SIMDVoiceGenerator<S, SIMDSampleMono<S>>,
    Grabber: SIMDSampleGrabber<S>,
{
    #[inline]
    fn next_sample(&mut self) -> SIMDSampleMono<S> {
        simd_invoke!(S, {
            let speed = self.pitch_gen.next_sample().0;
            let mut indexes = S::Vi32::zeroes();
            let mut fractionals = S::Vf32::zeroes();

            #[cfg(target_arch = "x86_64")]
            {
                if S::Vf32::WIDTH == 8 {
                    use std::arch::x86_64::*;
                    unsafe {
                        let speed_m256 = std::ptr::read(&speed as *const _ as *const __m256);

                        let mask = _mm256_castsi256_ps(_mm256_set1_epi32(0x7FFFFFFF));
                        let abs_speed = _mm256_and_ps(speed_m256, mask);
                        let clamped_speed = _mm256_max_ps(abs_speed, _mm256_set1_ps(0.0001));

                        let mut x = clamped_speed;
                        let shifted1 =
                            _mm256_castsi256_ps(_mm256_slli_si256(_mm256_castps_si256(x), 4));
                        x = _mm256_add_ps(x, shifted1);
                        let shifted2 =
                            _mm256_castsi256_ps(_mm256_slli_si256(_mm256_castps_si256(x), 8));
                        x = _mm256_add_ps(x, shifted2);

                        let cross = _mm256_permute2f128_ps(x, x, 0x08);
                        let cross = _mm256_shuffle_ps(cross, cross, 0xFF);
                        x = _mm256_add_ps(x, cross);

                        let indices = _mm256_set_epi32(6, 5, 4, 3, 2, 1, 0, 0);
                        let shifted_x = _mm256_permutevar8x32_ps(x, indices);
                        let exclusive = _mm256_blend_ps(shifted_x, _mm256_setzero_ps(), 1);

                        let t_eval = _mm256_add_ps(_mm256_set1_ps(self.time as f32), exclusive);

                        let idx = _mm256_cvttps_epi32(t_eval);
                        let floor_float = _mm256_cvtepi32_ps(idx);
                        let frac = _mm256_sub_ps(t_eval, floor_float);

                        indexes = std::ptr::read(&idx as *const _ as *const S::Vi32);
                        fractionals = std::ptr::read(&frac as *const _ as *const S::Vf32);

                        let mut x_arr = [0.0; 8];
                        _mm256_storeu_ps(x_arr.as_mut_ptr(), x);
                        self.time += x_arr[7] as f64;
                    }

                    let sample = self.grabber.get(indexes, fractionals);
                    return SIMDSampleMono(sample);
                }
            }

            unsafe {
                for i in 0..S::Vf32::WIDTH {
                    let speed_val = speed.get_unchecked(i);
                    let speed_val = speed_val.abs().max(0.0001);
                    let time = self.time;
                    self.time += speed_val as f64;
                    *indexes.get_unchecked_mut(i) = time as i32;
                    *fractionals.get_unchecked_mut(i) = (time.fract()) as f32;
                }
            }

            let sample = self.grabber.get(indexes, fractionals);

            SIMDSampleMono(sample)
        })
    }
}

pub struct SIMDStereoVoiceSampler<S, Pitch, Grabber>
where
    S: Simd,
    Pitch: SIMDVoiceGenerator<S, SIMDSampleMono<S>>,
    Grabber: SIMDSampleGrabber<S>,
{
    grabber_left: Grabber,
    grabber_right: Grabber,

    pitch_gen: Pitch,

    time: f64,

    _s: PhantomData<S>,
}

impl<S, Pitch, Grabber> SIMDStereoVoiceSampler<S, Pitch, Grabber>
where
    S: Simd,
    Pitch: SIMDVoiceGenerator<S, SIMDSampleMono<S>>,
    Grabber: SIMDSampleGrabber<S>,
{
    pub fn new(grabber_left: Grabber, grabber_right: Grabber, pitch_gen: Pitch) -> Self {
        SIMDStereoVoiceSampler {
            grabber_left,
            grabber_right,
            pitch_gen,
            time: 0.0,
            _s: PhantomData,
        }
    }

    fn increment_time(&mut self, by: f64) -> f64 {
        let time = self.time;
        self.time += by;
        time
    }
}

impl<S, Pitch, Grabber> VoiceGeneratorBase for SIMDStereoVoiceSampler<S, Pitch, Grabber>
where
    S: Simd,
    Pitch: SIMDVoiceGenerator<S, SIMDSampleMono<S>>,
    Grabber: SIMDSampleGrabber<S>,
{
    #[inline]
    fn ended(&self) -> bool {
        self.grabber_left.is_past_end(self.time) || self.grabber_right.is_past_end(self.time)
    }

    #[inline]
    fn signal_release(&mut self, rel_type: ReleaseType) {
        self.pitch_gen.signal_release(rel_type);
        self.grabber_left.signal_release();
        self.grabber_right.signal_release();
    }

    #[inline]
    fn process_controls(&mut self, control: &VoiceControlData) {
        self.pitch_gen.process_controls(control);
    }
}

impl<S, Pitch, Grabber> SIMDVoiceGenerator<S, SIMDSampleStereo<S>>
    for SIMDStereoVoiceSampler<S, Pitch, Grabber>
where
    S: Simd,
    Pitch: SIMDVoiceGenerator<S, SIMDSampleMono<S>>,
    Grabber: SIMDSampleGrabber<S>,
{
    #[inline]
    fn next_sample(&mut self) -> SIMDSampleStereo<S> {
        simd_invoke!(S, {
            let speed = self.pitch_gen.next_sample().0;
            let mut indexes = S::Vi32::zeroes();
            let mut fractionals = S::Vf32::zeroes();

            #[cfg(target_arch = "x86_64")]
            {
                if S::Vf32::WIDTH == 8 {
                    use std::arch::x86_64::*;
                    unsafe {
                        let speed_m256 = std::ptr::read(&speed as *const _ as *const __m256);

                        let mask = _mm256_castsi256_ps(_mm256_set1_epi32(0x7FFFFFFF));
                        let abs_speed = _mm256_and_ps(speed_m256, mask);
                        let clamped_speed = _mm256_max_ps(abs_speed, _mm256_set1_ps(0.0001));

                        let mut x = clamped_speed;
                        let shifted1 =
                            _mm256_castsi256_ps(_mm256_slli_si256(_mm256_castps_si256(x), 4));
                        x = _mm256_add_ps(x, shifted1);
                        let shifted2 =
                            _mm256_castsi256_ps(_mm256_slli_si256(_mm256_castps_si256(x), 8));
                        x = _mm256_add_ps(x, shifted2);

                        let cross = _mm256_permute2f128_ps(x, x, 0x08);
                        let cross = _mm256_shuffle_ps(cross, cross, 0xFF);
                        x = _mm256_add_ps(x, cross);

                        let indices = _mm256_set_epi32(6, 5, 4, 3, 2, 1, 0, 0);
                        let shifted_x = _mm256_permutevar8x32_ps(x, indices);
                        let exclusive = _mm256_blend_ps(shifted_x, _mm256_setzero_ps(), 1);

                        let t_eval = _mm256_add_ps(_mm256_set1_ps(self.time as f32), exclusive);

                        let idx = _mm256_cvttps_epi32(t_eval);
                        let floor_float = _mm256_cvtepi32_ps(idx);
                        let frac = _mm256_sub_ps(t_eval, floor_float);

                        indexes = std::ptr::read(&idx as *const _ as *const S::Vi32);
                        fractionals = std::ptr::read(&frac as *const _ as *const S::Vf32);

                        let mut x_arr = [0.0; 8];
                        _mm256_storeu_ps(x_arr.as_mut_ptr(), x);
                        self.time += x_arr[7] as f64;
                    }

                    let left = self.grabber_left.get(indexes, fractionals);
                    let right = self.grabber_right.get(indexes, fractionals);
                    return SIMDSampleStereo(left, right);
                }
            }

            unsafe {
                for i in 0..S::Vf32::WIDTH {
                    let speed_val = speed.get_unchecked(i);
                    let speed_val = speed_val.abs().max(0.0001);
                    let time = self.time;
                    self.time += speed_val as f64;
                    *indexes.get_unchecked_mut(i) = time as i32;
                    *fractionals.get_unchecked_mut(i) = (time.fract()) as f32;
                }
            }

            let left = self.grabber_left.get(indexes, fractionals);
            let right = self.grabber_right.get(indexes, fractionals);

            SIMDSampleStereo(left, right)
        })
    }
}
