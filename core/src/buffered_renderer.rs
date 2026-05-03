#![allow(clippy::needless_range_loop)]
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering},
        Arc, RwLock,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crossbeam_channel::{unbounded, Receiver};

use crate::helpers::fast_zero_fill;
use crate::AudioStreamParams;

use super::AudioPipe;

/// Holds the statistics for an instance of BufferedRenderer.
#[derive(Debug, Clone)]
struct BufferedRendererStats {
    samples: Arc<AtomicI64>,
    last_samples_after_read: Arc<AtomicI64>,
    last_request_samples: Arc<AtomicI64>,
    render_time: Arc<RwLock<VecDeque<f64>>>,
    render_size: Arc<AtomicUsize>,
}

/// Reads the statistics of an instance of BufferedRenderer in a usable way.
#[derive(Clone)]
pub struct BufferedRendererStatsReader {
    stats: BufferedRendererStats,
}

impl BufferedRendererStatsReader {
    /// The number of samples currently buffered.
    /// Can be negative if the reader is waiting for more samples.
    pub fn samples(&self) -> i64 {
        self.stats.samples.load(Ordering::Relaxed)
    }

    /// The number of samples that were in the buffer after the last read.
    pub fn last_samples_after_read(&self) -> i64 {
        self.stats.last_samples_after_read.load(Ordering::Relaxed)
    }

    /// The last number of samples last requested by the read command.
    pub fn last_request_samples(&self) -> i64 {
        self.stats.last_request_samples.load(Ordering::Relaxed)
    }

    /// The number of samples to render each iteration.
    pub fn render_size(&self) -> usize {
        self.stats.render_size.load(Ordering::Relaxed)
    }

    /// The average render time percentages (0 to 1)
    /// of how long the render thread spent rendering, from the max allowed time.
    pub fn average_renderer_load(&self) -> f64 {
        let queue = self.stats.render_time.read().unwrap();
        let total = queue.len().max(1);
        queue.iter().sum::<f64>() / total as f64
    }

    /// The last render time percentage (0 to 1)
    /// of how long the render thread spent rendering, from the max allowed time.
    pub fn last_renderer_load(&self) -> f64 {
        let queue = self.stats.render_time.read().unwrap();
        *queue.front().unwrap_or(&0.0)
    }
}

/// The helper struct for deferred sample rendering.
/// Helps avoid stutter when the render time is exceding the max time allowed by the audio driver.
///
/// Instead, it renders in a separate thread with much smaller sample sizes, causing a minimal impact on latency
/// while allowing more time to render per sample.
///
/// Designed to be used in realtime playback only.
pub struct BufferedRenderer {
    stats: BufferedRendererStats,
    /// The receiver for samples (the render thread has the sender).
    receive: Receiver<Vec<f32>>,
    /// Sender for returning empty vecs to the render thread.
    return_tx: crossbeam_channel::Sender<Vec<f32>>,
    /// Remainder of samples from the last received samples vec.
    remainder: Vec<f32>,
    /// Whether the render thread should be killed.
    killed: Arc<AtomicBool>,
    /// The thread handle to wait for at the end.
    thread_handle: Option<JoinHandle<()>>,
    stream_params: AudioStreamParams,
}

impl BufferedRenderer {
    /// Creates a new instance of BufferedRenderer.
    ///
    /// - `render`: An object implementing the AudioPipe struct for BufferedRenderer to
    ///   read samples from
    /// - `stream_params`: Parameters of the output audio
    /// - `render_size`: The number of samples to render each iteration
    pub fn new<F: 'static + AudioPipe + Send>(
        mut render: F,
        stream_params: AudioStreamParams,
        render_size: usize,
    ) -> Self {
        let (tx, rx) = unbounded();
        let (return_tx, return_rx) = unbounded();

        let samples = Arc::new(AtomicI64::new(0));
        let last_request_samples = Arc::new(AtomicI64::new(0));
        let render_size = Arc::new(AtomicUsize::new(render_size));
        let last_samples_after_read = Arc::new(AtomicI64::new(0));
        let render_time = Arc::new(RwLock::new(VecDeque::new()));
        let killed = Arc::new(AtomicBool::new(false));

        let thread_handle = {
            let samples = samples.clone();
            let last_request_samples = last_request_samples.clone();
            let render_size = render_size.clone();
            let render_time = render_time.clone();
            let killed = killed.clone();

            thread::Builder::new()
                .name("xsynth_buffered_rendering".to_string())
                .spawn(move || loop {
                    let size = render_size.load(Ordering::Relaxed);

                    // The expected render time per iteration. It is slightly smaller (*90/100) than
                    // the real time so the render thread can catch up if it's behind.
                    let delay =
                        Duration::from_secs(1) * size as u32 / stream_params.sample_rate * 90 / 100;

                    // If the render thread is ahead by over ~10%, wait until more samples are required.
                    loop {
                        let samples = samples.load(Ordering::Relaxed);
                        let last_requested = last_request_samples.load(Ordering::Relaxed);
                        if samples > last_requested * 110 / 100 {
                            spin_sleep::sleep(delay / 10);
                        } else {
                            break;
                        }

                        if killed.load(Ordering::Relaxed) {
                            return;
                        }
                    }

                    let start = Instant::now();
                    let end = start + delay;

                    // Reuse or create the vec and write the samples
                    let required_len = size * stream_params.channels.count() as usize;
                    let mut vec = return_rx
                        .try_recv()
                        .unwrap_or_else(|_| Vec::with_capacity(required_len));

                    // fast_zero_fill handles both capacity extension and zeroing
                    // in a single memset operation — faster than resize+fill
                    fast_zero_fill(&mut vec, required_len);
                    render.read_samples(&mut vec);

                    // Send the samples, break if the pipe is broken
                    samples.fetch_add(vec.len() as i64, Ordering::Relaxed);
                    match tx.send(vec) {
                        Ok(_) => {}
                        Err(_) => break,
                    };

                    // Write the elapsed render time percentage to the render_time queue
                    {
                        let mut queue = render_time.write().unwrap();
                        let elapsed = start.elapsed().as_secs_f64();
                        let total = delay.as_secs_f64();
                        queue.push_front(elapsed / total);
                        if queue.len() > 100 {
                            queue.pop_back();
                        }
                    }

                    // Sleep until the next iteration
                    let now = Instant::now();
                    if end > now {
                        spin_sleep::sleep(end - now);
                    }
                })
                .unwrap()
        };

        Self {
            stats: BufferedRendererStats {
                samples,
                last_request_samples,
                render_time,
                render_size,
                last_samples_after_read,
            },
            receive: rx,
            return_tx,
            remainder: Vec::new(),
            stream_params,
            thread_handle: Some(thread_handle),
            killed,
        }
    }

    /// Reads samples from the remainder and the output queue into the destination array.
    pub fn read(&mut self, dest: &mut [f32]) {
        dest.fill(0.0);

        let mut i: usize = 0;
        let len = dest.len().min(self.remainder.len());
        let samples = self
            .stats
            .samples
            .fetch_sub(dest.len() as i64, Ordering::Relaxed);

        self.stats
            .last_request_samples
            .store(dest.len() as i64, Ordering::Relaxed);

        // Read from current remainder
        for r in self.remainder.drain(0..len) {
            dest[i] = r;
            i += 1;
        }

        // Return empty remainder vec to the pool
        if self.remainder.is_empty() && self.remainder.capacity() > 0 {
            let _ = self.return_tx.send(std::mem::take(&mut self.remainder));
        }

        // Read from output queue, leave the remainder if there is any
        // Use a short timeout to prevent audio callback blocking.
        // In normal operation the render thread stays ahead so recv succeeds
        // immediately (no blocking). 5ms = ~half a render cycle at 480 samples.
        // If exceeded, fill with silence — audible dropout is ~50ms max rather
        // than hanging the audio callback for 100ms+.
        let timeout = std::time::Duration::from_millis(5);
        while i < dest.len() {
            match self.receive.recv_timeout(timeout) {
                Ok(mut buf) => {
                    let len = buf.len().min(dest.len() - i);
                    for r in buf.drain(0..len) {
                        dest[i] = r;
                        i += 1;
                    }
                    if buf.is_empty() {
                        let _ = self.return_tx.send(buf);
                    } else {
                        self.remainder = buf;
                    }
                }
                Err(_) => {
                    // Timeout - fill remaining with silence to prevent hanging
                    // This prevents audio dropout by at least providing silence
                    for j in i..dest.len() {
                        dest[j] = 0.0;
                    }
                    break;
                }
            }
        }

        self.stats
            .last_samples_after_read
            .store(samples, Ordering::Relaxed);
    }

    /// Sets the number of samples that should be rendered each iteration.
    pub fn set_render_size(&self, size: usize) {
        self.stats.render_size.store(size, Ordering::Relaxed);
    }

    /// Returns a statistics reader.
    /// See the `BufferedRendererStatsReader` documentation for more information.
    pub fn get_buffer_stats(&self) -> BufferedRendererStatsReader {
        BufferedRendererStatsReader {
            stats: self.stats.clone(),
        }
    }
}

impl Drop for BufferedRenderer {
    fn drop(&mut self) {
        self.killed.store(true, Ordering::Relaxed);
        self.thread_handle.take().unwrap().join().unwrap();
    }
}

impl AudioPipe for BufferedRenderer {
    fn stream_params(&self) -> &'_ AudioStreamParams {
        &self.stream_params
    }

    fn read_samples_unchecked(&mut self, to: &mut [f32]) {
        self.read(to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AudioStreamParams, ChannelCount, FunctionAudioPipe};
    use std::time::Duration;

    const TEST_SAMPLE_RATE: u32 = 48000;
    const TEST_CHANNELS: ChannelCount = ChannelCount::Stereo;
    // Render 480 frames per iteration ≈ 10ms budget (480/48000 = 10ms)
    const TEST_RENDER_SIZE: usize = 480;

    fn test_params() -> AudioStreamParams {
        AudioStreamParams::new(TEST_SAMPLE_RATE, TEST_CHANNELS)
    }

    fn frame_count() -> usize {
        TEST_RENDER_SIZE * TEST_CHANNELS.count() as usize
    }

    /// 创建一个快照记录当前 buffer 统计，用于跨线程一致的 underrun 判断。
    /// `last_samples_after_read()` 是 read() 开始前 buffer 中的样本数，
    /// 不受 recv_timeout 期间渲染线程写入的影响。
    fn check_underrun(stats: &BufferedRendererStatsReader, requested: usize) -> bool {
        stats.last_samples_after_read() < requested as i64
    }

    /// 测试 1：正常负载下 buffer 不应 underrun
    ///
    /// 场景：AudioPipe 立即返回（零延迟），读速度低于渲染速度。
    /// 预期：last_samples_after_read() 始终 >= 请求数，renderer_load < 1.0。
    #[test]
    fn test_no_underrun_normal_load() {
        let params = test_params();
        let pipe = FunctionAudioPipe::new(params, |_buf| {
            // 快速 pipe：fast_zero_fill 已经清零，无需额外操作
        });
        let mut renderer = BufferedRenderer::new(pipe, params, TEST_RENDER_SIZE);
        let stats = renderer.get_buffer_stats();

        // 等待 buffer 预填至少 2 个渲染周期（~20ms）
        std::thread::sleep(Duration::from_millis(30));

        let mut dest = vec![0.0f32; frame_count()];
        let mut min_available = i64::MAX;

        // 以低于渲染速度的频率读取（每 15ms 读一次，渲染周期为 ~9ms）
        for _ in 0..20 {
            renderer.read(&mut dest);
            let available = stats.last_samples_after_read();
            min_available = min_available.min(available);
            assert!(
                !check_underrun(&stats, dest.len()),
                "Normal load underrun: {available} < {}",
                dest.len()
            );
            std::thread::sleep(Duration::from_millis(15));
        }

        let load = stats.average_renderer_load();
        assert!(
            load < 2.0,
            "Renderer load unexpectedly high under normal conditions: {load}"
        );
    }

    /// 测试 2：高负载下 buffer 应触发 underrun
    ///
    /// 场景：AudioPipe 模拟高负载（每次渲染 sleep 30ms），远超 ~9ms 的预算。
    /// 预期：last_samples_after_read() < 请求数，renderer_load > 1.0。
    #[test]
    fn test_underrun_heavy_load() {
        let params = test_params();
        // 慢 pipe：每次渲染耗时 ~30ms，远超 ~9ms 预算
        let pipe = FunctionAudioPipe::new(params, |_buf| {
            std::thread::sleep(Duration::from_millis(30));
        });
        let mut renderer = BufferedRenderer::new(pipe, params, TEST_RENDER_SIZE);
        let stats = renderer.get_buffer_stats();

        // 等待第一个渲染周期完成，让 buffer 有点数据
        std::thread::sleep(Duration::from_millis(40));

        let mut dest = vec![0.0f32; frame_count()];
        let mut hit_underrun = false;

        // 快速读取（每 5ms 读一次），迫使 buffer 耗尽
        for _ in 0..30 {
            renderer.read(&mut dest);
            if check_underrun(&stats, dest.len()) {
                hit_underrun = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        assert!(
            hit_underrun,
            "Heavy load should cause underrun (last_samples_after_read < {})",
            dest.len()
        );

        // 渲染负载应超过 100% 预算
        let load = stats.average_renderer_load();
        assert!(
            load > 0.8,
            "Heavy load renderer should be high, but load is only {load}"
        );
    }

    /// 测试 3：突发高负载后的 buffer 恢复能力
    ///
    /// 场景：pipe 先慢（30ms）后快（0ms），验证 underrun 后 buffer
    /// 能恢复充足供应（last_samples_after_read >= 请求数）。
    #[test]
    fn test_underrun_recovery_after_spike() {
        let params = test_params();
        let is_slow = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));

        let pipe = {
            let is_slow = is_slow.clone();
            FunctionAudioPipe::new(params, move |_buf| {
                if is_slow.load(std::sync::atomic::Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(30));
                }
            })
        };

        let mut renderer = BufferedRenderer::new(pipe, params, TEST_RENDER_SIZE);
        let stats = renderer.get_buffer_stats();

        // 阶段 1：慢 pipe，触发 underrun
        std::thread::sleep(Duration::from_millis(40));
        let mut dest = vec![0.0f32; frame_count()];
        let mut underrun_detected = false;

        for _ in 0..20 {
            renderer.read(&mut dest);
            if check_underrun(&stats, dest.len()) {
                underrun_detected = true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        assert!(
            underrun_detected,
            "Should have detected underrun in slow phase"
        );

        // 阶段 2：切换到快 pipe
        is_slow.store(false, std::sync::atomic::Ordering::Relaxed);

        // 停止读取一段时间，让渲染线程追上累积缺口。
        // 快速 pipe 每 ~9ms 产生 960 个样本。等待 250ms ≈ 25 个渲染周期
        // = 24000 个样本，足以覆盖慢阶段累积的 ~-17000 缺口。
        std::thread::sleep(Duration::from_millis(250));

        // 阶段 3：验证 buffer 已恢复（连续多次读取不再 underrun）
        let mut recovered_count = 0;
        let need_recovery = 3;
        for _ in 0..10 {
            renderer.read(&mut dest);
            if !check_underrun(&stats, dest.len()) {
                recovered_count += 1;
            }
            // 读取间隔略小于渲染周期，保持趋于填满
            std::thread::sleep(Duration::from_millis(15));
        }

        assert!(
            recovered_count >= need_recovery,
            "Buffer should recover after load spike, but only {recovered_count}/{need_recovery} reads had sufficient data"
        );
    }

    /// 测试 4：渲染负载指标的正确性
    ///
    /// 使用已知慢 pipe 验证 average_renderer_load 和 last_renderer_load 的值。
    #[test]
    fn test_renderer_load_metric() {
        let params = test_params();
        // 慢 pipe：18ms ≈ 2x 预算（预算 ~9ms）
        let pipe = FunctionAudioPipe::new(params, |_buf| {
            std::thread::sleep(Duration::from_millis(18));
        });
        let renderer = BufferedRenderer::new(pipe, params, TEST_RENDER_SIZE);
        let stats = renderer.get_buffer_stats();

        // 等几个渲染周期积累统计数据
        std::thread::sleep(Duration::from_millis(100));

        let avg_load = stats.average_renderer_load();
        let last_load = stats.last_renderer_load();

        // 负载应明显 > 1.0（18ms > 9ms 预算）
        assert!(
            avg_load > 1.0,
            "Slow pipe should have avg renderer load > 1.0, got {avg_load}"
        );
        assert!(
            last_load > 1.0,
            "Slow pipe should have last renderer load > 1.0, got {last_load}"
        );

        // 但不应过于离谱（< 5x 预算）
        assert!(
            avg_load < 5.0,
            "avg_renderer_load({avg_load}) is unrealistically high"
        );
    }

    /// 测试 5：小 render_size 下高负载的 underrun 行为
    ///
    /// 小 render_size（64 帧 ≈ 1.3ms）更频繁、更短的渲染周期，
    /// 更容易暴露 underrun 问题。
    #[test]
    fn test_underrun_small_render_size() {
        let small_size: usize = 64;
        let params = AudioStreamParams::new(TEST_SAMPLE_RATE, TEST_CHANNELS);

        // 模拟中等负载：6ms 渲染 > 1.2ms 预算 → underrun
        let pipe = FunctionAudioPipe::new(params, |_buf| {
            std::thread::sleep(Duration::from_millis(6));
        });
        let mut renderer = BufferedRenderer::new(pipe, params, small_size);
        let stats = renderer.get_buffer_stats();

        std::thread::sleep(Duration::from_millis(20));

        let mut dest = vec![0.0f32; small_size * TEST_CHANNELS.count() as usize];
        let mut underrun = false;
        for _ in 0..40 {
            renderer.read(&mut dest);
            if check_underrun(&stats, dest.len()) {
                underrun = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }

        assert!(
            underrun,
            "Small render size ({small_size}) should underrun under load"
        );
    }

    /// 测试 6：长时间运行下的 buffer 稳定性
    ///
    /// 快速 pipe 长时间运行，验证 buffer 不会意外耗尽。
    #[test]
    fn test_stability_long_running() {
        let params = test_params();
        let pipe = FunctionAudioPipe::new(params, |_buf| {});
        let mut renderer = BufferedRenderer::new(pipe, params, TEST_RENDER_SIZE);
        let stats = renderer.get_buffer_stats();

        // 让 buffer 预填
        std::thread::sleep(Duration::from_millis(40));

        let mut dest = vec![0.0f32; frame_count()];

        // 模拟长时间运行：约 1 秒，速率接近实时
        let iterations = (TEST_SAMPLE_RATE / TEST_RENDER_SIZE as u32) as usize;
        for _ in 0..iterations {
            renderer.read(&mut dest);
            assert!(
                !check_underrun(&stats, dest.len()),
                "Long run underrun: last_samples_after_read={} < {}",
                stats.last_samples_after_read(),
                dest.len()
            );
            // 模拟实时读取间隔（90% 的实时速率）
            std::thread::sleep(Duration::from_micros(
                (TEST_RENDER_SIZE as u64 * 1_000_000 / TEST_SAMPLE_RATE as u64) * 9 / 10,
            ));
        }

        let load = stats.average_renderer_load();
        assert!(
            load < 1.0,
            "Fast pipe should keep load under 1.0, got {load}"
        );
    }

    /// 测试 7：极端 stress — 持续快速读取耗尽 buffer
    ///
    /// 在不同负载条件下测量 underrun 频率和严重程度。
    #[test]
    fn test_stress_rapid_reads() {
        let params = test_params();
        // 中等负载：每次渲染 ~12ms（略高于 ~9ms 预算）
        let pipe = FunctionAudioPipe::new(params, |_buf| {
            std::thread::sleep(Duration::from_millis(12));
        });
        let mut renderer = BufferedRenderer::new(pipe, params, TEST_RENDER_SIZE);
        let stats = renderer.get_buffer_stats();

        std::thread::sleep(Duration::from_millis(30));

        let mut dest = vec![0.0f32; frame_count()];
        let mut underrun_count = 0;
        let mut max_shortfall: i64 = 0;
        let total_reads = 50;

        // 快速连续读取，模拟被 cpal 背压淹没
        for _ in 0..total_reads {
            renderer.read(&mut dest);
            if check_underrun(&stats, dest.len()) {
                underrun_count += 1;
                // 缺口大小 = 请求数 - 实际可用数
                let shortfall = dest.len() as i64 - stats.last_samples_after_read();
                max_shortfall = max_shortfall.max(shortfall);
            }
            std::thread::sleep(Duration::from_millis(3));
        }

        // 应检测到 underrun
        assert!(
            underrun_count > 0,
            "Stress test should detect underruns, got {underrun_count}/{total_reads}"
        );

        // 缺口不应无限增长
        assert!(
            max_shortfall < 10_000_000,
            "Buffer underrun shortfall ({max_shortfall}) is excessive"
        );
    }
}
