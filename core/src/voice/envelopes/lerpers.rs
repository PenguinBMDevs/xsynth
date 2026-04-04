use simdeez::prelude::*;

// The lerp equation is `start + (end - start) * factor`
// We store: start, length (= end - start)
pub(super) struct SIMDLerper<T: Simd> {
    start_simd: T::Vf32,
    length_simd: T::Vf32,
    start: f32,
    length: f32,
}

impl<T: Simd> SIMDLerper<T> {
    pub fn new(start: f32, end: f32) -> Self {
        simd_invoke!(T, {
            SIMDLerper {
                start_simd: T::Vf32::set1(start),
                length_simd: T::Vf32::set1(end - start),
                start,
                length: end - start,
            }
        })
    }

    pub fn lerp(&self, factor: f32) -> f32 {
        self.start + self.length * factor
    }

    pub fn lerp_simd(&self, factor: T::Vf32) -> T::Vf32 {
        simd_invoke!(T, self.start_simd + self.length_simd * factor)
    }
}

pub(super) struct SIMDLerperConcave<T: Simd> {
    length_simd: T::Vf32,
    end_simd: T::Vf32,
    length: f32,
    end: f32,
}

impl<T: Simd> SIMDLerperConcave<T> {
    pub fn new(start: f32, end: f32) -> Self {
        simd_invoke!(T, {
            SIMDLerperConcave {
                length_simd: T::Vf32::set1(start - end),
                end_simd: T::Vf32::set1(end),
                length: start - end,
                end,
            }
        })
    }

    pub fn lerp(&self, factor: f32) -> f32 {
        let mult = (1.0 - factor).powi(8);
        self.length * mult + self.end
    }

    pub fn lerp_simd(&self, factor: T::Vf32) -> T::Vf32 {
        simd_invoke!(T, {
            let one = T::Vf32::set1(1.0);
            let r1 = one - factor;
            let r2 = r1 * r1;
            let r3 = r2 * r2;
            let mult = r3 * r3;
            self.length_simd * mult + self.end_simd
        })
    }
}

pub(super) struct SIMDLerperConvex<T: Simd> {
    start_simd: T::Vf32,
    length_simd: T::Vf32,
    start: f32,
    length: f32,
}

impl<T: Simd> SIMDLerperConvex<T> {
    pub fn new(start: f32, end: f32) -> Self {
        simd_invoke!(T, {
            SIMDLerperConvex {
                start_simd: T::Vf32::set1(start),
                length_simd: T::Vf32::set1(end - start),
                start,
                length: end - start,
            }
        })
    }

    pub fn lerp(&self, factor: f32) -> f32 {
        let mult = factor.powi(8);
        self.length * mult + self.start
    }

    pub fn lerp_simd(&self, factor: T::Vf32) -> T::Vf32 {
        simd_invoke!(T, {
            let r1 = factor * factor;
            let r2 = r1 * r1;
            let mult = r2 * r2;
            self.length_simd * mult + self.start_simd
        })
    }
}

pub(super) struct StageTime<T: Simd> {
    stage_time_simd: T::Vf32,
    stage_end_time_f32: f32,
    increment_simd: T::Vf32,      // The SIMD width as a SIMD float
    stage_end_time_simd: T::Vf32, // The stage end time as a SIMD float
}

impl<T: Simd> StageTime<T> {
    pub fn new(start_offset: u32, stage_end_time: u32) -> Self {
        simd_invoke!(T, {
            let mut stage_time_simd = T::Vf32::set1(start_offset as f32);
            for i in 0..T::Vf32::WIDTH {
                stage_time_simd[i] += i as f32;
            }

            StageTime {
                stage_time_simd,
                stage_end_time_f32: stage_end_time as f32,
                increment_simd: T::Vf32::set1(T::Vf32::WIDTH as f32),
                stage_end_time_simd: T::Vf32::set1(stage_end_time as f32),
            }
        })
    }

    #[inline(always)]
    pub fn increment(&mut self) {
        simd_invoke!(T, self.stage_time_simd += self.increment_simd);
    }

    #[inline(always)]
    pub fn increment_by(&mut self, by: u32) {
        simd_invoke!(T, self.stage_time_simd += T::Vf32::set1(by as f32));
    }

    #[inline(always)]
    /// Is the upper most value in the SIMD array past the end?
    pub fn is_ending(&self) -> bool {
        self.simd_array_end_f32() >= self.stage_end_time_f32
    }

    #[inline(always)]
    /// Is the SIMD array intersecting the end? Or has it completely passed the end
    pub fn is_intersecting_end(&self) -> bool {
        self.is_ending() && self.simd_array_start_f32() < self.stage_end_time_f32
    }

    #[inline(always)]
    pub fn raw_simd_array(&self) -> &T::Vf32 {
        &self.stage_time_simd
    }

    #[inline(always)]
    pub fn progress_simd_array(&self) -> T::Vf32 {
        simd_invoke!(T, *self.raw_simd_array() / self.stage_end_time_simd)
    }

    #[inline(always)]
    pub fn simd_array_start_f32(&self) -> f32 {
        self.stage_time_simd[0]
    }

    #[inline(always)]
    pub fn stage_end_time_f32(&self) -> f32 {
        self.stage_end_time_f32
    }

    #[inline(always)]
    pub fn simd_array_end_f32(&self) -> f32 {
        self.stage_time_simd[T::Vf32::WIDTH - 1]
    }

    #[allow(unused)]
    #[inline(always)]
    pub fn simd_array_start(&self) -> u32 {
        self.simd_array_start_f32() as u32
    }

    #[allow(unused)]
    #[inline(always)]
    pub fn simd_array_end(&self) -> u32 {
        self.simd_array_end_f32() as u32
    }
}
