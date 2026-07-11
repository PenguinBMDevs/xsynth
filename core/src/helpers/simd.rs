use std::sync::OnceLock;

#[cfg(target_arch = "aarch64")]
use core::arch::aarch64::*;
#[cfg(target_arch = "wasm32")]
use core::arch::wasm32::*;
#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::*;

/// Sum the values of `source` to the values of `target`, writing to `target`.
///
/// Uses runtime selected SIMD operations with aggressive optimization.
/// The selected SIMD backend is cached in a `OnceLock` to avoid re-running
/// feature detection on every call.
/// Panics if source and target have different lengths.
type SumFn = unsafe fn(&[f32], &mut [f32]);

static SUM_FN: OnceLock<SumFn> = OnceLock::new();

#[allow(unreachable_code)]
#[inline]
pub fn sum_simd(source: &[f32], target: &mut [f32]) {
    let len = source.len().min(target.len());
    if len == 0 {
        return;
    }

    debug_assert_eq!(
        source.len(),
        target.len(),
        "sum_simd: source length ({}) != target length ({})",
        source.len(),
        target.len()
    );

    let f = SUM_FN.get_or_init(|| {
        #[cfg(target_arch = "x86_64")]
        {
            if std::arch::is_x86_feature_detected!("avx2") {
                return sum_simd_avx2 as SumFn;
            }
            if std::arch::is_x86_feature_detected!("sse2") {
                return sum_simd_sse2 as SumFn;
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            if std::arch::is_aarch64_feature_detected!("neon") {
                return sum_simd_neon as SumFn;
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            return sum_simd_wasm as SumFn;
        }
        sum_simd_fallback as SumFn
    });
    unsafe { f(&source[..len], &mut target[..len]) }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
#[inline]
unsafe fn sum_simd_avx2(source: &[f32], target: &mut [f32]) {
    let mut i = 0;
    let len = source.len();
    while i + 32 <= len {
        let s0 = _mm256_loadu_ps(source.as_ptr().add(i));
        let s1 = _mm256_loadu_ps(source.as_ptr().add(i + 8));
        let s2 = _mm256_loadu_ps(source.as_ptr().add(i + 16));
        let s3 = _mm256_loadu_ps(source.as_ptr().add(i + 24));

        let t0 = _mm256_loadu_ps(target.as_ptr().add(i));
        let t1 = _mm256_loadu_ps(target.as_ptr().add(i + 8));
        let t2 = _mm256_loadu_ps(target.as_ptr().add(i + 16));
        let t3 = _mm256_loadu_ps(target.as_ptr().add(i + 24));

        _mm256_storeu_ps(target.as_mut_ptr().add(i), _mm256_add_ps(s0, t0));
        _mm256_storeu_ps(target.as_mut_ptr().add(i + 8), _mm256_add_ps(s1, t1));
        _mm256_storeu_ps(target.as_mut_ptr().add(i + 16), _mm256_add_ps(s2, t2));
        _mm256_storeu_ps(target.as_mut_ptr().add(i + 24), _mm256_add_ps(s3, t3));

        i += 32;
    }
    while i + 8 <= len {
        let s0 = _mm256_loadu_ps(source.as_ptr().add(i));
        let t0 = _mm256_loadu_ps(target.as_ptr().add(i));
        _mm256_storeu_ps(target.as_mut_ptr().add(i), _mm256_add_ps(s0, t0));
        i += 8;
    }
    while i < len {
        *target.get_unchecked_mut(i) += *source.get_unchecked(i);
        i += 1;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
#[inline]
unsafe fn sum_simd_sse2(source: &[f32], target: &mut [f32]) {
    let mut i = 0;
    let len = source.len();
    while i + 16 <= len {
        let s0 = _mm_loadu_ps(source.as_ptr().add(i));
        let s1 = _mm_loadu_ps(source.as_ptr().add(i + 4));
        let s2 = _mm_loadu_ps(source.as_ptr().add(i + 8));
        let s3 = _mm_loadu_ps(source.as_ptr().add(i + 12));

        let t0 = _mm_loadu_ps(target.as_ptr().add(i));
        let t1 = _mm_loadu_ps(target.as_ptr().add(i + 4));
        let t2 = _mm_loadu_ps(target.as_ptr().add(i + 8));
        let t3 = _mm_loadu_ps(target.as_ptr().add(i + 12));

        _mm_storeu_ps(target.as_mut_ptr().add(i), _mm_add_ps(s0, t0));
        _mm_storeu_ps(target.as_mut_ptr().add(i + 4), _mm_add_ps(s1, t1));
        _mm_storeu_ps(target.as_mut_ptr().add(i + 8), _mm_add_ps(s2, t2));
        _mm_storeu_ps(target.as_mut_ptr().add(i + 12), _mm_add_ps(s3, t3));

        i += 16;
    }
    while i + 4 <= len {
        let s0 = _mm_loadu_ps(source.as_ptr().add(i));
        let t0 = _mm_loadu_ps(target.as_ptr().add(i));
        _mm_storeu_ps(target.as_mut_ptr().add(i), _mm_add_ps(s0, t0));
        i += 4;
    }
    while i < len {
        *target.get_unchecked_mut(i) += *source.get_unchecked(i);
        i += 1;
    }
}

#[inline]
fn sum_simd_fallback(source: &[f32], target: &mut [f32]) {
    let len = source.len();
    let mut i = 0;
    while i + 4 <= len {
        unsafe {
            *target.get_unchecked_mut(i) += *source.get_unchecked(i);
            *target.get_unchecked_mut(i + 1) += *source.get_unchecked(i + 1);
            *target.get_unchecked_mut(i + 2) += *source.get_unchecked(i + 2);
            *target.get_unchecked_mut(i + 3) += *source.get_unchecked(i + 3);
        }
        i += 4;
    }
    while i < len {
        unsafe {
            *target.get_unchecked_mut(i) += *source.get_unchecked(i);
        }
        i += 1;
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
#[inline]
unsafe fn sum_simd_neon(source: &[f32], target: &mut [f32]) {
    let mut i = 0;
    let len = source.len();
    while i + 16 <= len {
        let s0 = vld1q_f32(source.as_ptr().add(i));
        let s1 = vld1q_f32(source.as_ptr().add(i + 4));
        let s2 = vld1q_f32(source.as_ptr().add(i + 8));
        let s3 = vld1q_f32(source.as_ptr().add(i + 12));

        let t0 = vld1q_f32(target.as_ptr().add(i));
        let t1 = vld1q_f32(target.as_ptr().add(i + 4));
        let t2 = vld1q_f32(target.as_ptr().add(i + 8));
        let t3 = vld1q_f32(target.as_ptr().add(i + 12));

        vst1q_f32(target.as_mut_ptr().add(i), vaddq_f32(s0, t0));
        vst1q_f32(target.as_mut_ptr().add(i + 4), vaddq_f32(s1, t1));
        vst1q_f32(target.as_mut_ptr().add(i + 8), vaddq_f32(s2, t2));
        vst1q_f32(target.as_mut_ptr().add(i + 12), vaddq_f32(s3, t3));

        i += 16;
    }
    while i + 4 <= len {
        let s0 = vld1q_f32(source.as_ptr().add(i));
        let t0 = vld1q_f32(target.as_ptr().add(i));
        vst1q_f32(target.as_mut_ptr().add(i), vaddq_f32(s0, t0));
        i += 4;
    }
    while i < len {
        *target.get_unchecked_mut(i) += *source.get_unchecked(i);
        i += 1;
    }
}

#[cfg(target_arch = "wasm32")]
#[target_feature(enable = "simd128")]
#[inline]
unsafe fn sum_simd_wasm(source: &[f32], target: &mut [f32]) {
    let mut i = 0;
    let len = source.len();
    while i + 16 <= len {
        let s0 = v128_load(source.as_ptr().add(i) as *const v128);
        let s1 = v128_load(source.as_ptr().add(i + 4) as *const v128);
        let s2 = v128_load(source.as_ptr().add(i + 8) as *const v128);
        let s3 = v128_load(source.as_ptr().add(i + 12) as *const v128);

        let t0 = v128_load(target.as_ptr().add(i) as *const v128);
        let t1 = v128_load(target.as_ptr().add(i + 4) as *const v128);
        let t2 = v128_load(target.as_ptr().add(i + 8) as *const v128);
        let t3 = v128_load(target.as_ptr().add(i + 12) as *const v128);

        v128_store(target.as_mut_ptr().add(i) as *mut v128, f32x4_add(s0, t0));
        v128_store(
            target.as_mut_ptr().add(i + 4) as *mut v128,
            f32x4_add(s1, t1),
        );
        v128_store(
            target.as_mut_ptr().add(i + 8) as *mut v128,
            f32x4_add(s2, t2),
        );
        v128_store(
            target.as_mut_ptr().add(i + 12) as *mut v128,
            f32x4_add(s3, t3),
        );

        i += 16;
    }
    while i + 4 <= len {
        let s0 = v128_load(source.as_ptr().add(i) as *const v128);
        let t0 = v128_load(target.as_ptr().add(i) as *const v128);
        v128_store(target.as_mut_ptr().add(i) as *mut v128, f32x4_add(s0, t0));
        i += 4;
    }
    while i < len {
        *target.get_unchecked_mut(i) += *source.get_unchecked(i);
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::sum_simd;

    #[test]
    fn test_simd_add() {
        let src = vec![1.0, 2.0, 3.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        let mut dst = vec![0.0, 1.0, 3.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        sum_simd(&src, &mut dst);
        assert_eq!(dst, vec![1.0, 3.0, 6.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0]);
    }
}
