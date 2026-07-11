use std::ops::RangeInclusive;
pub use xsynth_core::{
    channel::ChannelInitOptions,
    channel_group::{SynthFormat, ThreadCount},
};

/// Controls the rendering strategy used by the realtime synthesizer.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum RealtimeRenderMode {
    /// Render all channels synchronously inside the buffered render thread using
    /// `ChannelGroup`. This avoids the 16 per-channel OS threads and the blocking
    /// collect phase that caused audio dropouts on macOS.
    ChannelGroup,

    /// Legacy threaded rendering with one OS thread per MIDI channel.
    Threaded,
}

impl Default for RealtimeRenderMode {
    fn default() -> Self {
        RealtimeRenderMode::ChannelGroup
    }
}

/// Options for initializing a new RealtimeSynth.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(default)
)]
pub struct XSynthRealtimeConfig {
    /// Channel initialization options (same for all channels).
    /// See the `ChannelInitOptions` documentation for more information.
    pub channel_init_options: ChannelInitOptions,

    /// The length of the buffer reader in ms.
    ///
    /// Default: `10.0`
    pub render_window_ms: f64,

    /// Defines the format that the synthesizer will use. See the `SynthFormat`
    /// documentation for more information.
    ///
    /// Default: `SynthFormat::Midi`
    pub format: SynthFormat,

    /// Controls the multithreading used for rendering per-voice audio for all
    /// the voices stored in a key for a channel. See the `ThreadCount` documentation
    /// for the available options.
    ///
    /// Default: `ThreadCount::None`
    pub multithreading: ThreadCount,

    /// A range of velocities that will not be played.
    ///
    /// Default: `0..=0`
    pub ignore_range: RangeInclusive<u8>,

    /// Controls the realtime rendering strategy.
    ///
    /// `ChannelGroup` renders all channels synchronously inside the buffered
    /// render thread, eliminating the 16 per-channel OS threads and the blocking
    /// collect phase that caused dropouts on macOS. `Threaded` keeps the legacy
    /// behavior.
    ///
    /// Default: `RealtimeRenderMode::ChannelGroup`
    pub render_mode: RealtimeRenderMode,

    /// Maximum NPS (notes per second) before the realtime synthesizer starts
    /// dropping notes based on velocity. `u64::MAX` effectively disables the
    /// limiter, ensuring every NoteOn event is sent to the render pipeline.
    ///
    /// Default: `u64::MAX` (no limit)
    pub max_nps: u64,

    /// Render duration warning threshold in milliseconds. If a single render
    /// call (flush + render) exceeds this threshold, a warning is logged.
    /// Set to 0 to disable the warning.
    ///
    /// Recommended: `5.0` (half a 10 ms render window).
    ///
    /// Default: `5.0`
    pub render_warn_threshold_ms: f64,
}

impl Default for XSynthRealtimeConfig {
    fn default() -> Self {
        Self {
            channel_init_options: Default::default(),
            render_window_ms: 10.0,
            format: Default::default(),
            multithreading: ThreadCount::None,
            ignore_range: 0..=0,
            render_mode: RealtimeRenderMode::default(),
            max_nps: u64::MAX,
            render_warn_threshold_ms: 5.0,
        }
    }
}
