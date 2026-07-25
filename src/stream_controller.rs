use crate::buffer_size_adapter::{BufferSizeAdapter, MAX_HOST_FRAMES};
use crate::device_management::default_output_device;
use crate::device_management::DeviceExt;
use crate::error_recovery::handle_process_error;
use crate::stream_state::{AtomicStreamState, StreamState};
use anyhow::Result;
use auxide::rt::{Runtime, RuntimeHandle};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

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
}

impl Diagnostics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            callback_count: AtomicUsize::new(0),
            overflow_count: AtomicUsize::new(0),
            peak: AtomicU32::new(0),
        })
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
}

/// Manages real-time audio streaming with lock-free state management.
///
/// Handles audio device I/O, buffer adaptation, and error recovery via atomic flags.
pub struct StreamController {
    stream: Option<Stream>,
    state: Arc<AtomicStreamState>,
    error_flag: Arc<AtomicBool>,
    diagnostics: Arc<Diagnostics>,
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

    /// Starts real-time audio streaming from the given runtime.
    ///
    /// Creates a cpal stream, launches the audio callback, and returns a controller
    /// for managing playback state. Returns an error if device enumeration or stream creation fails.
    pub fn play(mut runtime: Runtime) -> Result<Self> {
        AtomicStreamState::verify_lock_free_atomics()?;
        let device = default_output_device()?;
        let sample_rate = runtime.sample_rate() as u32;

        // Find a supported configuration that matches our runtime's sample rate
        let config = device
            .supported_configs()?
            .into_iter()
            .find(|c| {
                c.sample_rate().0 == sample_rate
                    && c.channels() == 2
                    && c.sample_format() == SampleFormat::F32
            })
            .ok_or_else(|| anyhow::anyhow!("No suitable config for {}Hz", sample_rate))?;

        let sample_format = config.sample_format();
        let config = config.config();

        let state = Arc::new(AtomicStreamState::new(StreamState::Stopped));
        let error_flag = Arc::new(AtomicBool::new(false));
        let diagnostics = Diagnostics::new();
        let state_clone = state.clone();
        let error_flag_clone = error_flag.clone();
        let error_flag_clone2 = error_flag.clone();
        let diag_clone = diagnostics.clone();
        let mut adapter = BufferSizeAdapter::new(runtime.plan.block_size);

        let stream = match sample_format {
            SampleFormat::F32 => device.build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    diag_clone.callback_count.fetch_add(1, Ordering::Relaxed);

                    if data.len() > MAX_HOST_FRAMES {
                        diag_clone.overflow_count.fetch_add(1, Ordering::Relaxed);
                        error_flag_clone.store(true, Ordering::Relaxed);
                        handle_process_error(data);
                        return;
                    }

                    match state_clone.get_state() {
                        StreamState::Running => {
                            if adapter.fill_host_buffer(data, &mut runtime, 2).is_err() {
                                error_flag_clone.store(true, Ordering::Relaxed);
                                handle_process_error(data);
                            }

                            let max_sample = data.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
                            diag_clone.update_peak(max_sample);
                        }
                        _ => {
                            data.fill(0.0);
                        }
                    }
                },
                move |_| {
                    error_flag_clone2.store(true, Ordering::Relaxed);
                },
                None,
            )?,
            _ => return Err(anyhow::anyhow!("Unsupported sample format")),
        };

        Ok(Self {
            stream: Some(stream),
            state,
            error_flag,
            diagnostics,
        })
    }

    /// Starts real-time audio streaming from a RuntimeHandle (new architecture).
    ///
    /// This is the preferred method for new code. It uses the split architecture:
    /// - RuntimeHandle is moved into the audio callback
    /// - Control messages are received via lock-free queue
    /// - Invariant signals are emitted via lock-free queue
    ///
    /// # Example
    ///
    /// ```ignore
    /// use auxide::rt::RuntimeCore;
    /// use auxide_io::StreamController;
    ///
    /// let (handle, control) = RuntimeCore::new_with_channels(plan, &graph, 44100.0);
    /// let stream = StreamController::play_handle(handle)?;
    /// stream.start()?;
    ///
    /// // Control from main thread
    /// control.set_gain(node_id, 0.5)?;
    /// ```
    pub fn play_handle(mut handle: RuntimeHandle) -> Result<Self> {
        AtomicStreamState::verify_lock_free_atomics()?;
        let device = default_output_device()?;
        let sample_rate = handle.sample_rate() as u32;
        let block_size = handle.block_size();

        // Find a supported configuration that matches our runtime's sample rate
        let config = device
            .supported_configs()?
            .into_iter()
            .find(|c| {
                c.sample_rate().0 == sample_rate
                    && c.channels() == 2
                    && c.sample_format() == SampleFormat::F32
            })
            .ok_or_else(|| anyhow::anyhow!("No suitable config for {}Hz", sample_rate))?;

        let sample_format = config.sample_format();
        let config = config.config();

        let state = Arc::new(AtomicStreamState::new(StreamState::Stopped));
        let error_flag = Arc::new(AtomicBool::new(false));
        let diagnostics = Diagnostics::new();
        let state_clone = state.clone();
        let error_flag_clone = error_flag.clone();
        let error_flag_clone2 = error_flag.clone();
        let diag_clone = diagnostics.clone();
        let mut adapter = BufferSizeAdapter::new(block_size);

        let stream = match sample_format {
            SampleFormat::F32 => device.build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    diag_clone.callback_count.fetch_add(1, Ordering::Relaxed);

                    if data.len() > MAX_HOST_FRAMES {
                        diag_clone.overflow_count.fetch_add(1, Ordering::Relaxed);
                        error_flag_clone.store(true, Ordering::Relaxed);
                        handle_process_error(data);
                        return;
                    }

                    match state_clone.get_state() {
                        StreamState::Running => {
                            // Use fill_host_buffer_handle which processes control messages
                            // and emits invariant signals
                            if adapter
                                .fill_host_buffer_handle(data, &mut handle, 2)
                                .is_err()
                            {
                                error_flag_clone.store(true, Ordering::Relaxed);
                                handle_process_error(data);
                            }

                            let max_sample = data.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
                            diag_clone.update_peak(max_sample);
                        }
                        _ => {
                            data.fill(0.0);
                        }
                    }
                },
                move |_| {
                    error_flag_clone2.store(true, Ordering::Relaxed);
                },
                None,
            )?,
            _ => return Err(anyhow::anyhow!("Unsupported sample format")),
        };

        Ok(Self {
            stream: Some(stream),
            state,
            error_flag,
            diagnostics,
        })
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

    /// Returns a snapshot of the lock-free diagnostics counters.
    pub fn diagnostics(&self) -> DiagnosticsSnapshot {
        DiagnosticsSnapshot {
            callback_count: self.diagnostics.callback_count.load(Ordering::Relaxed),
            overflow_count: self.diagnostics.overflow_count.load(Ordering::Relaxed),
            peak: f32::from_bits(self.diagnostics.peak.load(Ordering::Relaxed)),
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
        // Test methods on a controller with no stream
        let controller = StreamController {
            stream: None,
            state: Arc::new(AtomicStreamState::new(StreamState::Stopped)),
            error_flag: Arc::new(AtomicBool::new(false)),
            diagnostics: Diagnostics::new(),
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
}
