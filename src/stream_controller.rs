use crate::buffer_size_adapter::{BufferSizeAdapter, MAX_HOST_FRAMES};
use crate::device_management::default_output_device;
use crate::device_management::DeviceExt;
use crate::error_recovery::handle_process_error;
use crate::stream_state::{AtomicStreamState, StreamState};
use anyhow::Result;
use auxide::rt::{Runtime, RuntimeHandle};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Lock-free diagnostics counters updated from the audio callback.
///
/// All fields use `Ordering::Relaxed` — the counters are monotonic
/// and consumed only on the main thread via `DiagnosticsSnapshot`.
pub struct Diagnostics {
    /// Total number of audio callbacks invoked.
    pub callback_count: AtomicUsize,
    /// Number of overflow events (host buffer > MAX_HOST_FRAMES).
    pub overflow_count: AtomicUsize,
    /// Peak absolute sample value observed (stored as `f32::to_bits`).
    pub peak: AtomicU32,
    /// Most recent reported output latency in nanoseconds (0 = unknown / no stream).
    /// Captured from `cpal::OutputCallbackInfo::timestamp()` each callback.
    pub latency_nanos: AtomicU64,
}

impl Diagnostics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            callback_count: AtomicUsize::new(0),
            overflow_count: AtomicUsize::new(0),
            peak: AtomicU32::new(0),
            latency_nanos: AtomicU64::new(0),
        })
    }

    /// Records the latest output latency (in nanoseconds) from the callback.
    pub fn update_latency(&self, nanos: u64) {
        self.latency_nanos.store(
            (nanos as u128).min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
    }

    /// Atomically updates `peak` if `sample` is larger (lock-free max).
    pub fn update_peak(&self, sample: f32) {
        let bits = sample.to_bits();
        loop {
            let current = self.peak.load(Ordering::Relaxed);
            if bits <= current {
                break;
            }
            if self
                .peak
                .compare_exchange_weak(current, bits, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }
    }
}

/// Read-only snapshot of diagnostics data.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiagnosticsSnapshot {
    pub callback_count: usize,
    pub overflow_count: usize,
    pub peak: f32,
    /// Most recent output latency, if a stream has reported one.
    pub latency: Option<Duration>,
}

/// A point in musical time, sampled once per host buffer.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TransportTime {
    /// Tempo in beats per minute (0.0 when no clock is set / no tempo).
    pub bpm: f32,
    /// Beat phase in `[0.0, 1.0)` (0.0 when no clock is set).
    pub beat_phase: f32,
    /// Absolute sample position at the start of the current buffer.
    pub sample: u64,
}

/// Source of musical time for the audio callback.
///
/// Implementors are queried once per host buffer via
/// [`StreamController::set_transport_clock`]; the most recent value is cached
/// and exposed through [`StreamController::transport_time`].
pub trait TransportClock {
    /// Returns the current transport position. Called once per host buffer.
    fn transport_time(&self) -> TransportTime;
}

/// Default no-op clock: reports zeroed musical time.
///
/// Used implicitly when no clock has been installed (the callback simply never
/// updates the cached value, so [`StreamController::transport_time`] yields
/// zeros and existing callers are unaffected).
pub struct IdentityClock;

impl TransportClock for IdentityClock {
    fn transport_time(&self) -> TransportTime {
        TransportTime {
            bpm: 0.0,
            beat_phase: 0.0,
            sample: 0,
        }
    }
}

/// Shared transport state: an optional clock plus the most recent value it
/// reported. Safe to share across the audio callback and the main thread.
pub struct TransportState {
    clock: Mutex<Option<Box<dyn TransportClock + Send + Sync>>>,
    last: Mutex<TransportTime>,
}

impl TransportState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            clock: Mutex::new(None),
            last: Mutex::new(TransportTime::default()),
        })
    }

    /// Installs (or replaces) the transport clock.
    pub fn set_clock(&self, clock: Box<dyn TransportClock + Send + Sync>) {
        *self.clock.lock().unwrap() = Some(clock);
    }

    /// Samples the installed clock once (no-op when no clock is set) and caches
    /// the resulting value. Call once per host buffer.
    pub fn tick(&self) {
        if let Some(clock) = self.clock.lock().unwrap().as_ref() {
            *self.last.lock().unwrap() = clock.transport_time();
        }
    }

    /// Returns the most recently cached transport value.
    pub fn sample(&self) -> TransportTime {
        *self.last.lock().unwrap()
    }
}

/// Manages real-time audio streaming with lock-free state management.
///
/// Handles audio device I/O, buffer adaptation, error recovery, and an
/// optional transport clock via atomic flags and shared state.
#[allow(dead_code)]
pub struct StreamController {
    stream: Option<Stream>,
    state: Arc<AtomicStreamState>,
    error_flag: Arc<AtomicBool>,
    recovery_needed: Arc<AtomicBool>,
    diagnostics: Arc<Diagnostics>,
    sample_rate: u32,
    block_size: usize,
    /// Shared handle store: the callback borrows the handle every block;
    /// recover() takes it out to rebuild the stream.
    handle_store: Arc<Mutex<Option<RuntimeHandle>>>,
    /// Optional musical-time clock, sampled once per host buffer.
    transport: Arc<TransportState>,
}

/// Derives the output latency (presentation delay) from a `cpal`
/// [`OutputCallbackInfo`] timestamp.
///
/// Calls the **public** [`cpal::OutputCallbackInfo::timestamp`] method
/// (the `timestamp` *field* is private in cpal 0.15.x — using it
/// would not compile) to obtain the `OutputStreamTimestamp`, then returns
/// the delta between the predicted playback instant and the callback
/// invocation instant. Returns `None` if the timestamp is not later
/// than the callback instant.
pub fn output_latency(ts: &cpal::OutputStreamTimestamp) -> Option<Duration> {
    ts.playback.duration_since(&ts.callback)
}

impl StreamController {
    /// Finds the best supported sample rate from the default output device.
    ///
    /// Attempts to match the requested rate; falls back to standard rates (48000, 44100)
    /// if exact match unavailable. Returns stereo-compatible configs only.
    pub fn get_best_sample_rate(requested_rate: f32) -> Result<f32> {
        let device = default_output_device()?;
        let requested_sample_rate = requested_rate as u32;

        let supported_configs: Vec<_> = device.supported_configs()?.into_iter().collect();

        // First try to find exact match
        if let Some(config) = supported_configs.iter().find(|c| {
            c.sample_rate().0 == requested_sample_rate
                && c.channels() == 2
                && c.sample_format() == SampleFormat::F32
        }) {
            return Ok(config.sample_rate().0 as f32);
        }

        // Find best alternative
        if let Some(config) = supported_configs
            .iter()
            .filter(|c| c.channels() == 2 && c.sample_format() == SampleFormat::F32)
            .min_by_key(|c| {
                let rate = c.sample_rate().0;
                rate.abs_diff(requested_sample_rate)
            })
        {
            return Ok(config.sample_rate().0 as f32);
        }

        // Fallback to any F32 config
        if let Some(config) = supported_configs
            .iter()
            .find(|c| c.sample_format() == SampleFormat::F32)
        {
            return Ok(config.sample_rate().0 as f32);
        }

        // No suitable configuration found
        let config_summary: Vec<String> = supported_configs
            .iter()
            .map(|c| {
                format!(
                    "{}ch @ {}Hz ({})",
                    c.channels(),
                    c.sample_rate().0,
                    match c.sample_format() {
                        SampleFormat::F32 => "F32",
                        SampleFormat::I16 => "I16",
                        SampleFormat::U16 => "U16",
                        _ => "Other",
                    }
                )
            })
            .collect();

        Err(anyhow::anyhow!(
            "No suitable audio configuration found. Requested: {} Hz, F32 format. \
            Available: {}",
            requested_rate,
            if config_summary.is_empty() {
                "none".to_string()
            } else {
                config_summary.join(", ")
            }
        ))
    }

    /// Shared audio-callback body used by both `play` and `play_handle`.
    ///
    /// Captures output latency from `info.timestamp()`, guards against host
    /// buffer overflow, and — when running — drives the supplied `fill`
    /// closure (which pulls samples from the graph / `RuntimeHandle`). All
    /// counters are lock-free. Contains no logging (RT-safe).
    #[allow(clippy::too_many_arguments)]
    fn run_callback<F>(
        data: &mut [f32],
        info: &cpal::OutputCallbackInfo,
        adapter: &mut BufferSizeAdapter,
        diagnostics: &Diagnostics,
        state: &AtomicStreamState,
        error_flag: &AtomicBool,
        recovery_needed: &AtomicBool,
        transport: &TransportState,
        fill: &mut F,
    ) where
        F: FnMut(&mut [f32], &mut BufferSizeAdapter) -> Result<()>,
    {
        // Sample the transport clock once per host buffer (no-op if unset).
        transport.tick();

        diagnostics.callback_count.fetch_add(1, Ordering::Relaxed);

        // Report output latency from the cpal callback timestamp.
        // Uses the PUBLIC `OutputCallbackInfo::timestamp()` method
        // (the `timestamp` *field* is private in cpal 0.15.x).
        if let Some(d) = output_latency(&info.timestamp()) {
            diagnostics.update_latency(d.as_nanos().min(u64::MAX as u128) as u64);
        }

        if data.len() > MAX_HOST_FRAMES {
            diagnostics.overflow_count.fetch_add(1, Ordering::Relaxed);
            error_flag.store(true, Ordering::Relaxed);
            recovery_needed.store(true, Ordering::Relaxed);
            handle_process_error(data);
            return;
        }

        match state.get_state() {
            StreamState::Running => {
                if fill(data, adapter).is_err() {
                    error_flag.store(true, Ordering::Relaxed);
                    recovery_needed.store(true, Ordering::Relaxed);
                    handle_process_error(data);
                }

                let max_sample = data.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
                diagnostics.update_peak(max_sample);
            }
            _ => {
                data.fill(0.0);
            }
        }
    }

    /// Builds a cpal output stream that drives the shared [`Self::run_callback`]
    /// with the supplied per-backend `fill` closure.
    #[allow(clippy::too_many_arguments)]
    fn spawn_stream<F>(
        mut fill: F,
        sample_rate: u32,
        block_size: usize,
        diagnostics: Arc<Diagnostics>,
        state: Arc<AtomicStreamState>,
        error_flag: Arc<AtomicBool>,
        recovery_needed: Arc<AtomicBool>,
        transport: Arc<TransportState>,
    ) -> Result<Stream>
    where
        F: FnMut(&mut [f32], &mut BufferSizeAdapter) -> Result<()> + Send + 'static,
    {
        let device = default_output_device()?;
        let config = device
            .supported_configs()?
            .into_iter()
            .find(|c| {
                c.sample_rate().0 == sample_rate
                    && c.channels() == 2
                    && c.sample_format() == SampleFormat::F32
            })
            .ok_or_else(|| anyhow::anyhow!("No suitable config for {}Hz", sample_rate))?;

        let config = config.config();
        let mut adapter = BufferSizeAdapter::new(block_size);
        let error_cb_flag = error_flag.clone();

        let stream = device.build_output_stream(
            &config,
            move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                Self::run_callback(
                    data,
                    info,
                    &mut adapter,
                    &diagnostics,
                    &state,
                    &error_flag,
                    &recovery_needed,
                    &transport,
                    &mut fill,
                );
            },
            move |_| {
                error_cb_flag.store(true, Ordering::Relaxed);
            },
            None,
        )?;
        Ok(stream)
    }

    /// Starts real-time audio streaming from the given runtime.
    ///
    /// Creates a cpal stream, launches the audio callback, and returns a controller
    /// for managing playback state. Returns an error if device enumeration or stream creation fails.
    pub fn play(mut runtime: Runtime) -> Result<Self> {
        AtomicStreamState::verify_lock_free_atomics()?;
        let sample_rate = runtime.sample_rate() as u32;
        let block_size = runtime.plan.block_size;

        let state = Arc::new(AtomicStreamState::new(StreamState::Stopped));
        let error_flag = Arc::new(AtomicBool::new(false));
        let recovery_needed = Arc::new(AtomicBool::new(false));
        let diagnostics = Diagnostics::new();
        let handle_store = Arc::new(Mutex::new(None));
        let transport = TransportState::new();

        let fill = move |data: &mut [f32], adapter: &mut BufferSizeAdapter| {
            adapter
                .fill_host_buffer(data, &mut runtime, 2)
                .map_err(|e| anyhow::anyhow!(e))
        };

        let stream = Self::spawn_stream(
            fill,
            sample_rate,
            block_size,
            diagnostics.clone(),
            state.clone(),
            error_flag.clone(),
            recovery_needed.clone(),
            transport.clone(),
        )?;

        Ok(Self {
            stream: Some(stream),
            state,
            error_flag,
            recovery_needed,
            diagnostics,
            sample_rate,
            block_size,
            handle_store,
            transport,
        })
    }

    /// Starts real-time audio streaming from a RuntimeHandle (new architecture).
    ///
    /// This is the preferred method for new code. It uses the split architecture:
    /// - RuntimeHandle is moved into the audio callback
    /// - Control messages are received via lock-free queue
    /// - Invariant signals are emitted via lock-free queue
    pub fn play_handle(handle: RuntimeHandle) -> Result<Self> {
        AtomicStreamState::verify_lock_free_atomics()?;
        let sample_rate = handle.sample_rate() as u32;
        let block_size = handle.block_size();

        let state = Arc::new(AtomicStreamState::new(StreamState::Stopped));
        let error_flag = Arc::new(AtomicBool::new(false));
        let recovery_needed = Arc::new(AtomicBool::new(false));
        let diagnostics = Diagnostics::new();
        let handle_store = Arc::new(Mutex::new(Some(handle)));
        let transport = TransportState::new();

        let store_clone = handle_store.clone();
        let fill = move |data: &mut [f32], adapter: &mut BufferSizeAdapter| {
            if let Ok(mut guard) = store_clone.lock() {
                if let Some(ref mut h) = *guard {
                    return adapter
                        .fill_host_buffer_handle(data, h, 2)
                        .map_err(|e| anyhow::anyhow!(e));
                }
            }
            Ok(())
        };

        let stream = Self::spawn_stream(
            fill,
            sample_rate,
            block_size,
            diagnostics.clone(),
            state.clone(),
            error_flag.clone(),
            recovery_needed.clone(),
            transport.clone(),
        )?;

        Ok(Self {
            stream: Some(stream),
            state,
            error_flag,
            recovery_needed,
            diagnostics,
            sample_rate,
            block_size,
            handle_store,
            transport,
        })
    }

    /// Attempts to recover from a device error.
    ///
    /// For the `play_handle` path the `RuntimeHandle` is preserved in the
    /// internal store, so recovery **rebuilds and restarts** the stream via
    /// [`Self::restart`]. For the legacy `play` path the `Runtime` was consumed
    /// by the original callback and cannot be recreated; recovery simply clears
    /// the error flags and leaves the controller stopped.
    pub fn recover(&mut self) -> Result<()> {
        self.stream = None;
        self.error_flag.store(false, Ordering::Relaxed);
        self.recovery_needed.store(false, Ordering::Relaxed);

        if self
            .handle_store
            .lock()
            .map_err(|_| anyhow::anyhow!("handle store poisoned"))?
            .is_some()
        {
            self.restart()?;
        } else {
            self.state.set_state(StreamState::Stopped);
        }
        Ok(())
    }

    /// Rebuilds and restarts the underlying cpal stream from the stored
    /// `RuntimeHandle` (the `play_handle` path).
    ///
    /// Returns an error if this controller was built via the legacy `play`,
    /// whose `Runtime` was consumed and cannot be restarted.
    pub fn restart(&mut self) -> Result<()> {
        if self
            .handle_store
            .lock()
            .map_err(|_| anyhow::anyhow!("handle store poisoned"))?
            .is_none()
        {
            return Err(anyhow::anyhow!(
                "restart requires a RuntimeHandle (use play_handle); the legacy play() \
                 path consumes its Runtime and cannot be restarted"
            ));
        }

        let store = self.handle_store.clone();
        let fill = move |data: &mut [f32], adapter: &mut BufferSizeAdapter| {
            if let Ok(mut guard) = store.lock() {
                if let Some(ref mut h) = *guard {
                    return adapter
                        .fill_host_buffer_handle(data, h, 2)
                        .map_err(|e| anyhow::anyhow!(e));
                }
            }
            Ok(())
        };

        let stream = Self::spawn_stream(
            fill,
            self.sample_rate,
            self.block_size,
            self.diagnostics.clone(),
            self.state.clone(),
            self.error_flag.clone(),
            self.recovery_needed.clone(),
            self.transport.clone(),
        )?;
        stream.play()?;
        self.stream = Some(stream);
        self.state.set_state(StreamState::Running);
        Ok(())
    }

    /// Returns true if the error callback flagged that recovery is needed.
    pub fn recovery_needed(&self) -> bool {
        self.recovery_needed.load(Ordering::Relaxed)
    }

    /// Starts the audio stream if not already playing.
    pub fn start(&self) -> Result<()> {
        if let Some(stream) = &self.stream {
            stream.play()?;
            self.state.set_state(StreamState::Running);
        }
        Ok(())
    }

    /// Pauses the audio stream.
    pub fn stop(&self) {
        if let Some(stream) = &self.stream {
            let _ = stream.pause();
        }
        self.state.set_state(StreamState::Stopped);
    }

    /// Returns true if an audio callback error was recorded.
    pub fn has_error(&self) -> bool {
        self.error_flag.load(Ordering::Relaxed)
    }

    /// Clears the error flag.
    pub fn clear_error(&self) {
        self.error_flag.store(false, Ordering::Relaxed);
    }

    /// Installs a transport clock. The callback samples it once per host
    /// buffer; the most recent value is available via [`Self::transport_time`].
    ///
    /// Passing a clock is optional: with none set the controller reports
    /// zeroed musical time and existing callers are unaffected.
    pub fn set_transport_clock(&self, clock: Box<dyn TransportClock + Send + Sync>) {
        self.transport.set_clock(clock);
    }

    /// Returns the most recent transport position reported by the installed
    /// clock, or a zeroed [`TransportTime`] if no clock is set.
    pub fn transport_time(&self) -> TransportTime {
        self.transport.sample()
    }

    /// Returns a snapshot of the lock-free diagnostics counters.
    pub fn diagnostics(&self) -> DiagnosticsSnapshot {
        let nanos = self.diagnostics.latency_nanos.load(Ordering::Relaxed);
        DiagnosticsSnapshot {
            callback_count: self.diagnostics.callback_count.load(Ordering::Relaxed),
            overflow_count: self.diagnostics.overflow_count.load(Ordering::Relaxed),
            peak: f32::from_bits(self.diagnostics.peak.load(Ordering::Relaxed)),
            latency: if nanos == 0 {
                None
            } else {
                Some(Duration::from_nanos(nanos))
            },
        }
    }

    /// Returns the most recent output latency reported by the audio callback,
    /// derived from `cpal::OutputCallbackInfo.timestamp()` (playback instant
    /// minus callback-invocation instant), or `None` if no value has been
    /// captured yet.
    ///
    /// This is **best-effort**: it is `None` before any stream has run, and
    /// also `None` whenever the host's audio backend does not supply a
    /// timestamp for the callback (cpal yields `None` from
    /// `OutputCallbackInfo.timestamp()` in that case). The value reflects
    /// whatever the last callback observed and is not smoothed.
    pub fn latency(&self) -> Option<Duration> {
        let nanos = self.diagnostics.latency_nanos.load(Ordering::Relaxed);
        if nanos == 0 {
            None
        } else {
            Some(Duration::from_nanos(nanos))
        }
    }
}

impl Drop for StreamController {
    /// Best-effort graceful teardown: pause the underlying cpal stream when
    /// the controller is dropped. (cpal stops the stream on drop regardless;
    /// this makes the intent explicit and keeps the state flag consistent.)
    fn drop(&mut self) {
        if let Some(stream) = &self.stream {
            let _ = stream.pause();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auxide::graph::{Graph, NodeType, PortId, Rate};
    use auxide::plan::Plan;

    #[test]
    fn test_error_flag() {
        // Test error flag functionality (without creating actual streams)
        let _state = Arc::new(AtomicStreamState::new(StreamState::Stopped));
        let error_flag = Arc::new(AtomicBool::new(false));

        // Simulate the error flag behavior
        assert!(!error_flag.load(Ordering::Relaxed));
        error_flag.store(true, Ordering::Relaxed);
        assert!(error_flag.load(Ordering::Relaxed));
        error_flag.store(false, Ordering::Relaxed);
        assert!(!error_flag.load(Ordering::Relaxed));
    }

    #[test]
    fn test_play_without_device() {
        // Test that play fails gracefully when no audio device is available
        let mut graph = Graph::new();
        let osc = graph.add_node(NodeType::SineOsc { freq: 440.0 });
        let sink = graph.add_node(NodeType::OutputSink);
        graph
            .add_edge(auxide::graph::Edge {
                from_node: osc,
                from_port: PortId(0),
                to_node: sink,
                to_port: PortId(0),
                rate: Rate::Audio,
            })
            .unwrap();
        let plan = Plan::compile(&graph, 64).unwrap();
        let runtime = Runtime::new(plan, &graph, 44100.0);

        // This should fail in test environment (no audio device)
        let result = StreamController::play(runtime);
        assert!(result.is_err());
    }

    #[test]
    fn test_controller_methods_with_no_stream() {
        let handle_store = Arc::new(Mutex::new(None));
        let controller = StreamController {
            stream: None,
            state: Arc::new(AtomicStreamState::new(StreamState::Stopped)),
            error_flag: Arc::new(AtomicBool::new(false)),
            recovery_needed: Arc::new(AtomicBool::new(false)),
            diagnostics: Diagnostics::new(),
            sample_rate: 44100,
            block_size: 64,
            handle_store,
            transport: TransportState::new(),
        };

        // start should not change state since no stream
        assert_eq!(controller.state.get_state(), StreamState::Stopped);
        let _ = controller.start();
        assert_eq!(controller.state.get_state(), StreamState::Stopped);

        // stop should set state to Stopped
        controller.state.set_state(StreamState::Running);
        controller.stop();
        assert_eq!(controller.state.get_state(), StreamState::Stopped);

        // error flag tests
        assert!(!controller.has_error());
        controller.error_flag.store(true, Ordering::Relaxed);
        assert!(controller.has_error());
        controller.clear_error();
        assert!(!controller.has_error());

        // diagnostics accessor
        let diag = controller.diagnostics();
        assert_eq!(diag.callback_count, 0);
        assert_eq!(diag.overflow_count, 0);
        assert_eq!(diag.peak, 0.0);
    }

    #[test]
    fn test_diagnostics_counters() {
        let d = Diagnostics::new();
        assert_eq!(
            d.callback_count.load(Ordering::Relaxed),
            0,
            "fresh diagnostics should have zero callback_count"
        );
        assert_eq!(
            d.overflow_count.load(Ordering::Relaxed),
            0,
            "fresh diagnostics should have zero overflow_count"
        );
        assert_eq!(d.peak.load(Ordering::Relaxed), 0, "fresh peak should be 0");

        d.callback_count.fetch_add(1, Ordering::Relaxed);
        d.overflow_count.fetch_add(1, Ordering::Relaxed);
        d.update_peak(0.5);

        assert_eq!(d.callback_count.load(Ordering::Relaxed), 1);
        assert_eq!(d.overflow_count.load(Ordering::Relaxed), 1);
        assert_eq!(f32::from_bits(d.peak.load(Ordering::Relaxed)), 0.5);
    }

    #[test]
    fn test_diagnostics_peak_monotonic() {
        let d = Diagnostics::new();
        d.update_peak(0.5);
        assert_eq!(f32::from_bits(d.peak.load(Ordering::Relaxed)), 0.5);
        d.update_peak(0.3);
        assert_eq!(
            f32::from_bits(d.peak.load(Ordering::Relaxed)),
            0.5,
            "peak should remain 0.5 (higher)"
        );
        d.update_peak(0.9);
        assert_eq!(f32::from_bits(d.peak.load(Ordering::Relaxed)), 0.9);
    }

    #[test]
    fn test_contract_stream_controller() {
        // Contract test: ensure buffer size validation works correctly
        let mut adapter = BufferSizeAdapter::new(64);
        // Call with valid size
        assert!(adapter.adapt_to_host_buffer(1024).is_ok());
        // Call with oversized buffer - should fail
        assert!(adapter.adapt_to_host_buffer(MAX_HOST_FRAMES + 1).is_err());
    }

    #[test]
    fn test_latency_reported_from_callback() {
        // Before any stream runs the latency is unknown.
        let handle_store = Arc::new(Mutex::new(None));
        let controller = StreamController {
            stream: None,
            state: Arc::new(AtomicStreamState::new(StreamState::Stopped)),
            error_flag: Arc::new(AtomicBool::new(false)),
            recovery_needed: Arc::new(AtomicBool::new(false)),
            diagnostics: Diagnostics::new(),
            sample_rate: 44100,
            block_size: 64,
            handle_store,
            transport: TransportState::new(),
        };
        assert!(controller.latency().is_none());

        use crate::device_management::default_output_device;
        use auxide::rt::Runtime;
        use std::thread::sleep;

        // The genuine latency can only be obtained inside a real cpal
        // callback: cpal keeps `OutputCallbackInfo.timestamp` AND
        // `StreamInstant`'s fields private, so the value cannot be
        // constructed/faked in a unit test. We therefore exercise the
        // real `output_latency(info.timestamp())` path on a live device
        // (guarded, matching the rest of this crate's hardware tests).
        if let Ok(device) = default_output_device() {
            // cpal requires an *exact* sample-rate match against the device's
            // supported configs, so pick one the real device actually offers
            // instead of assuming 44100 Hz.
            let sample_rate = device
                .supported_configs()
                .expect("device exposes supported configs")
                .into_iter()
                .find(|c| c.channels() == 2 && c.sample_format() == SampleFormat::F32)
                .map(|c| c.sample_rate().0)
                .expect("device offers a 2-channel F32 config");

            let mut graph = Graph::new();
            let osc = graph.add_node(NodeType::SineOsc { freq: 440.0 });
            let sink = graph.add_node(NodeType::OutputSink);
            graph
                .add_edge(auxide::graph::Edge {
                    from_node: osc,
                    from_port: PortId(0),
                    to_node: sink,
                    to_port: PortId(0),
                    rate: Rate::Audio,
                })
                .unwrap();
            let plan = Plan::compile(&graph, 64).unwrap();
            let runtime = Runtime::new(plan, &graph, sample_rate as f32);
            let sc = StreamController::play(runtime).expect("stream starts with a device");
            sc.start().expect("stream should start on a live device");
            let mut saw_some = false;
            for _ in 0..200 {
                if let Some(d) = sc.latency() {
                    assert!(d > Duration::ZERO, "reported latency must be non-zero");
                    saw_some = true;
                    break;
                }
                sleep(Duration::from_millis(1));
            }
            sc.stop();
            assert!(saw_some, "a real stream must report a non-zero latency");
        }
    }

    #[test]
    fn test_recover_clears_error_flag() {
        let handle_store = Arc::new(Mutex::new(None));
        let mut controller = StreamController {
            stream: None,
            state: Arc::new(AtomicStreamState::new(StreamState::Stopped)),
            error_flag: Arc::new(AtomicBool::new(true)),
            recovery_needed: Arc::new(AtomicBool::new(true)),
            diagnostics: Diagnostics::new(),
            sample_rate: 44100,
            block_size: 64,
            handle_store,
            transport: TransportState::new(),
        };

        assert!(controller.has_error());
        assert!(controller.recovery_needed());

        controller.recover().unwrap();

        assert!(!controller.has_error());
        assert!(!controller.recovery_needed());
        // Legacy play() path: the Runtime was consumed, so recovery cannot
        // rebuild a stream and leaves the controller stopped.
        assert_eq!(controller.state.get_state(), StreamState::Stopped);
    }

    #[test]
    fn test_restart_rebuilds_handle_stream() {
        use crate::device_management::default_output_device;
        use auxide::rt::RuntimeCore;
        use std::thread::sleep;

        if let Ok(device) = default_output_device() {
            // Pick a sample rate the real device actually supports.
            let sample_rate = device
                .supported_configs()
                .expect("device exposes supported configs")
                .into_iter()
                .find(|c| c.channels() == 2 && c.sample_format() == SampleFormat::F32)
                .map(|c| c.sample_rate().0)
                .expect("device offers a 2-channel F32 config");

            let mut graph = Graph::new();
            let osc = graph.add_node(NodeType::SineOsc { freq: 440.0 });
            let sink = graph.add_node(NodeType::OutputSink);
            graph
                .add_edge(auxide::graph::Edge {
                    from_node: osc,
                    from_port: PortId(0),
                    to_node: sink,
                    to_port: PortId(0),
                    rate: Rate::Audio,
                })
                .unwrap();
            let plan = Plan::compile(&graph, 64).unwrap();

            let (handle, _control) =
                RuntimeCore::new_with_channels(plan, &graph, sample_rate as f32);
            let mut sc =
                StreamController::play_handle(handle).expect("stream starts with a device");
            sc.start().expect("stream should start on a live device");
            sleep(Duration::from_millis(20));

            // Recover (which rebuilds + restarts the handle stream) and keep
            // using it afterwards — proves the teardown/restart path works.
            sc.recover()
                .expect("recover/restart should rebuild the handle stream");
            sleep(Duration::from_millis(20));
            assert!(
                sc.latency().is_some(),
                "restarted stream should report latency"
            );
            sc.stop();
        }
    }

    #[test]
    fn test_transport_advances() {
        use crate::device_management::default_output_device;
        use auxide::rt::RuntimeCore;
        use std::sync::atomic::{AtomicU64, Ordering as O};
        use std::thread::sleep;

        // A test clock whose sample position advances by the block size each
        // callback and whose beat phase wraps at 1.0.
        struct TestClock {
            sample: AtomicU64,
            bpm: f32,
        }
        impl TransportClock for TestClock {
            fn transport_time(&self) -> TransportTime {
                let s = self.sample.fetch_add(64, O::Relaxed);
                let seconds_per_beat = 60.0 / self.bpm;
                let beat_phase = (s as f32 / 44100.0 / seconds_per_beat) % 1.0;
                TransportTime {
                    bpm: self.bpm,
                    beat_phase,
                    sample: s,
                }
            }
        }

        if let Ok(device) = default_output_device() {
            let sample_rate = device
                .supported_configs()
                .expect("device exposes supported configs")
                .into_iter()
                .find(|c| c.channels() == 2 && c.sample_format() == SampleFormat::F32)
                .map(|c| c.sample_rate().0)
                .expect("device offers a 2-channel F32 config");

            let mut graph = Graph::new();
            let obs = graph.add_node(NodeType::SineOsc { freq: 440.0 });
            let sink = graph.add_node(NodeType::OutputSink);
            graph
                .add_edge(auxide::graph::Edge {
                    from_node: obs,
                    from_port: PortId(0),
                    to_node: sink,
                    to_port: PortId(0),
                    rate: Rate::Audio,
                })
                .unwrap();
            let plan = Plan::compile(&graph, 64).unwrap();

            let (handle, _control) =
                RuntimeCore::new_with_channels(plan, &graph, sample_rate as f32);
            let sc = StreamController::play_handle(handle).expect("stream starts with a device");
            sc.set_transport_clock(Box::new(TestClock {
                sample: AtomicU64::new(0),
                bpm: 120.0,
            }));
            sc.start().expect("stream should start on a live device");

            // Let a few buffers elapse so the callback samples the clock.
            sleep(Duration::from_millis(40));
            let first = sc.transport_time();
            assert!(
                first.sample >= 64,
                "transport sample should advance past the first block"
            );

            sleep(Duration::from_millis(40));
            let second = sc.transport_time();
            assert!(
                second.sample > first.sample,
                "transport sample must advance across buffers"
            );
            assert!(
                second.beat_phase >= 0.0 && second.beat_phase < 1.0,
                "beat phase must wrap within [0, 1)"
            );

            // With no clock installed the controller reports zeroed time.
            sc.stop();
        }
    }
}
