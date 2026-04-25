use std::marker::PhantomData;

struct SingleChannelLimiter {
    loudness: f32,
    attack: f32,
    falloff: f32,
    strength: f32,
    min_thresh: f32,
    max_output: f32,
}

impl SingleChannelLimiter {
    fn new() -> SingleChannelLimiter {
        SingleChannelLimiter {
            loudness: 1.0,
            attack: 100.0,
            falloff: 16000.0,
            strength: 1.0,
            min_thresh: 0.1,  // Lower threshold to allow more dynamic range
            max_output: 0.95, // Prevent hard clipping by limiting maximum output
        }
    }

    fn limit(&mut self, val: f32) -> f32 {
        let abs = val.abs();

        // Smooth envelope follower with different attack/release times
        if self.loudness > abs {
            // Release phase: slower decay
            self.loudness = (self.loudness * self.falloff + abs) / (self.falloff + 1.0);
        } else {
            // Attack phase: faster response
            self.loudness = (self.loudness * self.attack + abs) / (self.attack + 1.0);
        }

        // Ensure minimum threshold to prevent division by very small numbers
        let effective_loudness = self.loudness.max(self.min_thresh);

        // Calculate gain reduction: when loudness is high, reduce more
        // The formula now uses a softer knee to prevent hard limiting artifacts
        let gain_reduction = 1.0 / (1.0 + (effective_loudness - 1.0).max(0.0) * self.strength);

        // Apply limiting with soft clipping for values near the threshold
        let limited = val * gain_reduction;

        // Soft clipping to prevent any hard digital clipping
        // Using tanh-like soft clipping for smooth transition
        let soft_clipped = if limited.abs() > self.max_output {
            let sign = limited.signum();
            let excess = limited.abs() - self.max_output;
            // Soft knee: compress excess rather than hard clip
            sign * (self.max_output + excess / (1.0 + excess * 2.0))
        } else {
            limited
        };

        // Final hard limit as safety net
        soft_clipped.clamp(-0.99, 0.99)
    }
}

/// A multi-channel audio limiter.
///
/// Can be useful to prevent clipping on loud audio.
pub struct VolumeLimiter {
    channels: Vec<SingleChannelLimiter>,
    channel_count: usize,
}

pub struct VolumeLimiterIter<'a, 'b, T: 'b + Iterator<Item = f32>> {
    limiter: &'a mut VolumeLimiter,
    samples: T,
    pos: usize,
    _b: PhantomData<&'b T>,
}

impl VolumeLimiter {
    /// Initializes a new audio limiter with a specified audio channel count.
    pub fn new(channel_count: u16) -> VolumeLimiter {
        let mut limiters = Vec::new();
        for _ in 0..channel_count {
            limiters.push(SingleChannelLimiter::new());
        }
        VolumeLimiter {
            channels: limiters,
            channel_count: channel_count as usize,
        }
    }

    /// Applies the limiting algorithm to the given sample buffer to prevent clipping.
    pub fn limit(&mut self, sample: &mut [f32]) {
        for (i, s) in sample.iter_mut().enumerate() {
            *s = self.channels[i % self.channel_count].limit(*s);
        }
    }

    pub fn limit_iter<'a, 'b, T: 'b + Iterator<Item = f32>>(
        &'a mut self,
        samples: T,
    ) -> VolumeLimiterIter<'a, 'b, T> {
        impl<'b, T: 'b + Iterator<Item = f32>> Iterator for VolumeLimiterIter<'_, 'b, T> {
            type Item = f32;

            fn next(&mut self) -> Option<Self::Item> {
                let next = self.samples.next();
                if let Some(next) = next {
                    let val =
                        self.limiter.channels[self.pos % self.limiter.channel_count].limit(next);
                    self.pos += 1;
                    Some(val)
                } else {
                    None
                }
            }
        }

        VolumeLimiterIter::<'a, 'b, T> {
            _b: PhantomData,
            limiter: self,
            samples,
            pos: 0,
        }
    }
}
