use std::sync::Arc;
use std::cell::RefCell;

mod frequencies;
pub use frequencies::*;

mod simd;
pub use simd::*;

/// Take any f32 vec, set its length and fill it with the default value.
#[inline(always)]
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
#[inline(always)]
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
#[inline(always)]
pub fn get_render_buffer(size: usize) -> Vec<f32> {
    VOICE_RENDER_BUFFERS.with(|pool| {
        let mut pool = pool.borrow_mut();
        if let Some(mut buf) = pool.pop() {
            if buf.capacity() < size {
                buf.reserve(size - buf.capacity());
            }
            unsafe { buf.set_len(size); }
            buf.fill(0.0);
            buf
        } else {
            vec![0.0; size]
        }
    })
}

/// Return a buffer to the thread-local pool
#[inline(always)]
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
/// Uses unsafe code to eliminate bounds checking
#[inline(always)]
pub fn sum_buffers_to_target(sources: &[Vec<f32>], target: &mut [f32]) {
    if sources.is_empty() {
        return;
    }
    
    let len = target.len();
    
    // Process 8 elements at a time for better cache utilization
    let chunks = len / 8;
    let remainder = len % 8;
    
    for source in sources {
        debug_assert!(source.len() >= len);
        
        unsafe {
            let src_ptr = source.as_ptr();
            let dst_ptr = target.as_mut_ptr();
            
            // Unrolled loop for 8 elements at a time
            for i in 0..chunks {
                let base = i * 8;
                *dst_ptr.add(base) += *src_ptr.add(base);
                *dst_ptr.add(base + 1) += *src_ptr.add(base + 1);
                *dst_ptr.add(base + 2) += *src_ptr.add(base + 2);
                *dst_ptr.add(base + 3) += *src_ptr.add(base + 3);
                *dst_ptr.add(base + 4) += *src_ptr.add(base + 4);
                *dst_ptr.add(base + 5) += *src_ptr.add(base + 5);
                *dst_ptr.add(base + 6) += *src_ptr.add(base + 6);
                *dst_ptr.add(base + 7) += *src_ptr.add(base + 7);
            }
            
            // Handle remainder
            let base = chunks * 8;
            for i in 0..remainder {
                *dst_ptr.add(base + i) += *src_ptr.add(base + i);
            }
        }
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
