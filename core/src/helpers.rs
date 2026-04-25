use std::cell::RefCell;
use std::sync::Arc;

mod frequencies;
pub use frequencies::*;

mod simd;
pub use simd::*;

/// Take any f32 vec, set its length and fill it with the default value.
#[inline]
pub fn prepapre_cache_vec<T: Copy>(vec: &mut Vec<T>, len: usize, default: T) {
    if vec.len() < len {
        vec.reserve(len - vec.len());
    }
    unsafe {
        vec.set_len(len);
    }
    vec.fill(default);
}

/// Fast zero-fill for f32 buffers using SIMD-like operations
#[inline]
pub fn fast_zero_fill(vec: &mut Vec<f32>, len: usize) {
    if vec.len() < len {
        vec.reserve(len - vec.len());
    }
    unsafe {
        vec.set_len(len);
        // Use write_bytes for fast zeroing - this is optimized by the compiler
        // to use SIMD instructions when available
        std::ptr::write_bytes(vec.as_mut_ptr(), 0, len);
    }
}

// Thread-local buffer pool for voice rendering to avoid allocations
thread_local! {
    static VOICE_RENDER_BUFFERS: RefCell<Vec<Vec<f32>>> = const { RefCell::new(Vec::new()) };
}

/// Get a buffer from the thread-local pool or create a new one
#[inline]
pub fn get_render_buffer(size: usize) -> Vec<f32> {
    VOICE_RENDER_BUFFERS.with(|pool| {
        let mut pool = pool.borrow_mut();
        if let Some(mut buf) = pool.pop() {
            if buf.capacity() < size {
                buf.reserve(size - buf.capacity());
            }
            unsafe {
                buf.set_len(size);
            }
            buf.fill(0.0);
            buf
        } else {
            vec![0.0; size]
        }
    })
}

/// Return a buffer to the thread-local pool
#[inline]
pub fn return_render_buffer(buf: Vec<f32>) {
    VOICE_RENDER_BUFFERS.with(|pool| {
        let mut pool = pool.borrow_mut();
        // Keep at most 16 buffers in pool to limit memory usage
        if pool.len() < 16 {
            pool.push(buf);
        }
    });
}

/// Ultra-fast SIMD sum of multiple buffers into target
/// Uses SIMD-optimized sum_simd for each buffer
#[inline(always)]
pub fn sum_buffers_to_target(sources: &[Vec<f32>], target: &mut [f32]) {
    for source in sources {
        sum_simd(source, target);
    }
}

/// Converts a dB value to 0-1 amplitude.
pub fn db_to_amp(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Checks if two `Arc<T>` vecs are equal based on `Arc::ptr_eq`.
pub fn are_arc_vecs_equal<T: ?Sized>(old: &[Arc<T>], new: &[Arc<T>]) -> bool {
    // First, check if the lengths are the same
    if old.len() != new.len() {
        return false;
    }

    // Then, check each pair of elements using Arc::ptr_eq
    for (old_item, new_item) in old.iter().zip(new.iter()) {
        if !Arc::ptr_eq(old_item, new_item) {
            return false;
        }
    }

    true
}
