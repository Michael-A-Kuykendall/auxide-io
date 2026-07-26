use crate::buffer_size_adapter::{BufferSizeAdapter, MAX_HOST_FRAMES};
use crate::channel_router::ChannelMap;
use crate::device_management::default_output_device;
use crate::device_management::DeviceExt;
use crate::error_recovery::handle_process_error;
use crate::recorder::SharedRecorder;
use crate::stream_state::{AtomicStreamState, StreamState};
use anyhow::Result;
use auxide::rt::{Runtime, RuntimeHandle};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Lock-free diagnostics counters updated from the audio callback.
pub struct Diagnostics {
    pub callback_count: AtomicUsize,
    pub overflow_count: AtomicUsize,
    pub peak: AtomicU32,
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

    pub fn update_latency(&self, nanos: u64) {
        self.latency_nanos.store(
            (nanos as u128).min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
    }

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
    pub latency: Option<Duration>,
}

/// A point in musical time, sampled once per host buffer.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TransportTime {
    pub bpm: f32,
    pub beat_phase: f32,
    pub sample: u64,
}

/// Source of musical time for the audio callback.
pub trait TransportClock {
    fn transport_time(&self) -> TransportTime;
}

/// Default no-op clock: reports zeroed musical time.
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

    pub fn set_clock(&self, clock: Box<dyn TransportClock + Send + Sync>) {
        *self.clock.lock().unwrap() = Some(clock);
    }

    pub fn tick(&self) {
        if let Some(clock) = self.clock.lock().unwrap().as_ref() {
            *self.last.lock().unwrap() = clock.transport_time();
        }
    }

    pub fn sample(&self) -> TransportTime {
        *self.last.lock().unwrap()
    }
}

/// Manages real-time audio streaming with lock-free state management.
#[allow(dead_code)]
pub struct StreamController {
    stream: Option<Stream>,
    input_stream: Option<Stream>,
    state: Arc<AtomicStreamState>,
    error_flag: Arc<AtomicBool>,
    recovery_needed: Arc<AtomicBool>,
    diagnostics: Arc<Diagnostics>,
    /// Runtime (graph) sample rate — the rate the audio graph runs at.
    runtime_rate: u32,
    /// Negotiated device sample rate (may differ from `runtime_rate`, in which
    /// case the adapter resamples).
    device_rate: u32,
    block_size: usize,
    channel_map: ChannelMap,
    handle_store: Arc<Mutex<Option<RuntimeHandle>>>,
    transport: Arc<TransportState>,
}

/// Derives the output latency (presentation delay) from a `cpal`
/// [`OutputStreamTimestamp`].
pub fn output_latency(ts: &cpal::OutputStreamTimestamp) -> Option<Duration> {
    ts.playback.duration_since(&ts.callback)
}

impl StreamController {
    /// Finds the best supported sample rate from the default output device.
    ///
    /// Attempts to match the requested rate; falls back to the nearest
    /// supported rate (and finally any F32 config). This is the negotiation
    /// step required by `auxide-io-rfi` and is wired into `play`/`play_handle`.
    pub fn get_best_sample_rate(requested_rate: f32) -> Result<f32> {
        let device = default_output_device()?;
        let requested_sample_rate = requested_rate as u32;

        let supported_configs: Vec<_> = device.supported_configs()?.into_iter().collect();

        if let Some(config) = supported_configs.iter().find(|c| {
            c.sample_rate().0 == requested_sample_rate
                && c.channels() == 2
                && c.sample_format() == SampleFormat::F32
        }) {
            return Ok(config.sample_rate().0 as f32);
        }

        if let Some(config) = supported_configs
            .as_slice()
            .iter()
            .filter(|c| c.channels() == 2 && c.sample_format() == SampleFormat::F32)
            .min_by_key(|c| c.sample_rate().0.abs_diff(requested_sample_rate))
        {
            return Ok(config.sample_rate().0 as f32);
        }

        if let Some(config) = supported_configs
            .iter()
            .find(|c| c.sample_format() == SampleFormat::F32)
        {
            return Ok(config.sample_rate().0 as f32);
        }

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
    /// Captures output latency, guards against host buffer overflow, and —
    /// when running — drives the supplied `fill` closure. All counters are
    /// lock-free. Contains no logging (RT-safe).
    #[allow(clippy::too_many_arguments)]
    fn run_callback<F>(
        data: &mut [f32],
        latency: Option<Duration>,
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
        transport.tick();
        diagnostics.callback_count.fetch_add(1, Ordering::Relaxed);

        if let Some(d) = latency {
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
    fn build_output_stream<F>(
        device: cpal::Device,
        device_rate: u32,
        adapter: BufferSizeAdapter,
        diagnostics: Arc<Diagnostics>,
        state: Arc<AtomicStreamState>,
        error_flag: Arc<AtomicBool>,
        recovery_needed: Arc<AtomicBool>,
        transport: Arc<TransportState>,
        mut fill: F,
    ) -> Result<Stream>
    where
        F: FnMut(&mut [f32], &mut BufferSizeAdapter) -> Result<()> + Send + 'static,
    {
        let config = device
            .supported_configs()?
            .into_iter()
            .find(|c| {
                c.sample_rate().0 == device_rate
                    && c.channels() == 2
                    && c.sample_format() == SampleFormat::F32
            })
            .ok_or_else(|| anyhow::anyhow!("No suitable config for {}Hz", device_rate))?
            .config();
        let mut adapter = adapter;
        let error_cb_flag = error_flag.clone();
        let stream = device.build_output_stream(
            &config,
            move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                let latency = output_latency(&info.timestamp());
                Self::run_callback(
                    data,
                    latency,
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

    /// Builds a cpal input stream that forwards captured frames into `recorder`.
    #[allow(clippy::too_many_arguments)]
    fn build_input_stream(
        device: cpal::Device,
        sample_rate: u32,
        channels: usize,
        diagnostics: Arc<Diagnostics>,
        error_flag: Arc<AtomicBool>,
        recorder: SharedRecorder,
    ) -> Result<Stream> {
        let channels = channels.max(1);
        let config = device
            .supported_input_configs()?
            .map(|r| r.with_max_sample_rate())
            .find(|c| {
                c.sample_rate().0 == sample_rate
                    && c.channels() as usize == channels
                    && c.sample_format() == SampleFormat::F32
            })
            .or_else(|| {
                let d = device.clone();
                d.supported_input_configs()
                    .ok()
                    .and_then(|it| {
                        it.map(|r| r.with_max_sample_rate())
                            .find(|c| c.sample_format() == SampleFormat::F32)
                    })
            })
            .ok_or_else(|| anyhow::anyhow!("No suitable input config"))?
            .config();
        let error_cb_flag = error_flag.clone();
        let stream = device.build_input_stream(
            &config,
            move |data: &[f32], _info: &cpal::InputCallbackInfo| {
                diagnostics.callback_count.fetch_add(1, Ordering::Relaxed);
                if let Ok(mut g) = recorder.lock() {
                    g.push_block(data);
                }
            },
            move |_| {
                error_cb_flag.store(true, Ordering::Relaxed);
            },
            None,
        )?;
        Ok(stream)
    }

    /// Shared construction of an output stream controller.
    fn output_core<F>(
        device: cpal::Device,
        runtime_rate: u32,
        block_size: usize,
        channel_map: ChannelMap,
        handle_store: Arc<Mutex<Option<RuntimeHandle>>>,
        fill: F,
    ) -> Result<Self>
    where
        F: FnMut(&mut [f32], &mut BufferSizeAdapter) -> Result<()> + Send + 'static,
    {
        AtomicStreamState::verify_lock_free_atomics()?;
        let device_rate = Self::get_best_sample_rate(runtime_rate as f32)
            .map(|r| r as u32)
            .unwrap_or(runtime_rate);
        let adapter = BufferSizeAdapter::new(block_size)
            .with_channel_map(channel_map.clone())
            .with_resampling(runtime_rate, device_rate);
        let diagnostics = Diagnostics::new();
        let state = Arc::new(AtomicStreamState::new(StreamState::Stopped));
        let error_flag = Arc::new(AtomicBool::new(false));
        let recovery_needed = Arc::new(AtomicBool::new(false));
        let transport = TransportState::new();
        let stream = Self::build_output_stream(
            device,
            device_rate,
            adapter,
            diagnostics.clone(),
            state.clone(),
            error_flag.clone(),
            recovery_needed.clone(),
            transport.clone(),
            fill,
        )?;
        Ok(Self {
            stream: Some(stream),
            input_stream: None,
            state,
            error_flag,
            recovery_needed,
            diagnostics,
            runtime_rate,
            device_rate,
            block_size,
            channel_map,
            handle_store,
            transport,
        })
    }

    /// Builds a fresh adapter from the controller's stored rates and channel map.
    fn make_adapter(&self) -> BufferSizeAdapter {
        BufferSizeAdapter::new(self.block_size)
            .with_channel_map(self.channel_map.clone())
            .with_resampling(self.runtime_rate, self.device_rate)
    }

    /// Starts real-time audio streaming from the given runtime (legacy path).
    pub fn play(mut runtime: Runtime) -> Result<Self> {
        let device = default_output_device()?;
        let runtime_rate = runtime.sample_rate() as u32;
        let block_size = runtime.plan.block_size;
        let handle_store = Arc::new(Mutex::new(None));
        let fill = move |data: &mut [f32], adapter: &mut BufferSizeAdapter| {
            adapter
                .fill_host_buffer(data, &mut runtime, 2)
                .map_err(|e| anyhow::anyhow!(e))
        };
        Self::output_core(
            device,
            runtime_rate,
            block_size,
            ChannelMap::default(),
            handle_store,
            fill,
        )
    }

    /// Like [`Self::play`] but with an explicit channel map.
    pub fn play_with_channel_map(mut runtime: Runtime, channel_map: ChannelMap) -> Result<Self> {
        let device = default_output_device()?;
        let runtime_rate = runtime.sample_rate() as u32;
        let block_size = runtime.plan.block_size;
        let handle_store = Arc::new(Mutex::new(None));
        let fill = move |data: &mut [f32], adapter: &mut BufferSizeAdapter| {
            adapter
                .fill_host_buffer(data, &mut runtime, 2)
                .map_err(|e| anyhow::anyhow!(e))
        };
        Self::output_core(
            device,
            runtime_rate,
            block_size,
            channel_map,
            handle_store,
            fill,
        )
    }

    /// Starts streaming on the output device at the given enumeration index.
    pub fn play_on_device(index: usize, runtime: Runtime) -> Result<Self> {
        let device = crate::device_management::enumerate_output_devices()
            .into_iter()
            .nth(index)
            .ok_or_else(|| anyhow::anyhow!("No output device at index {}", index))?;
        Self::play_on(runtime, device)
    }

    /// Internal: play on an already-selected device.
    fn play_on(mut runtime: Runtime, device: cpal::Device) -> Result<Self> {
        let runtime_rate = runtime.sample_rate() as u32;
        let block_size = runtime.plan.block_size;
        let handle_store = Arc::new(Mutex::new(None));
        let fill = move |data: &mut [f32], adapter: &mut BufferSizeAdapter| {
            adapter
                .fill_host_buffer(data, &mut runtime, 2)
                .map_err(|e| anyhow::anyhow!(e))
        };
        Self::output_core(
            device,
            runtime_rate,
            block_size,
            ChannelMap::default(),
            handle_store,
            fill,
        )
    }

    /// Starts streaming on the output device whose name matches `name`.
    pub fn play_on_device_by_name(name: &str, runtime: Runtime) -> Result<Self> {
        let device = crate::device_management::select_output_device(name)
            .ok_or_else(|| anyhow::anyhow!("No output device named '{}'", name))?;
        Self::play_on(runtime, device)
    }

    /// Starts real-time audio streaming from a RuntimeHandle (preferred path).
    pub fn play_handle(handle: RuntimeHandle) -> Result<Self> {
        let device = default_output_device()?;
        let runtime_rate = handle.sample_rate() as u32;
        let block_size = handle.block_size();
        let handle_store = Arc::new(Mutex::new(Some(handle)));
        let store = handle_store.clone();
        let fill = move |data: &mut [f32], adapter: &mut BufferSizeAdapter| {
            if let Ok(mut g) = store.lock() {
                if let Some(ref mut h) = *g {
                    return adapter
                        .fill_host_buffer_handle(data, h, 2)
                        .map_err(|e| anyhow::anyhow!(e));
                }
            }
            Ok(())
        };
        Self::output_core(
            device,
            runtime_rate,
            block_size,
            ChannelMap::default(),
            handle_store,
            fill,
        )
    }

    /// Like [`Self::play_handle`] but with an explicit channel map.
    pub fn play_handle_with_channel_map(handle: RuntimeHandle, channel_map: ChannelMap) -> Result<Self> {
        let device = default_output_device()?;
        let runtime_rate = handle.sample_rate() as u32;
        let block_size = handle.block_size();
        let handle_store = Arc::new(Mutex::new(Some(handle)));
        let store = handle_store.clone();
        let fill = move |data: &mut [f32], adapter: &mut BufferSizeAdapter| {
            if let Ok(mut g) = store.lock() {
                if let Some(ref mut h) = *g {
                    return adapter
                        .fill_host_buffer_handle(data, h, 2)
                        .map_err(|e| anyhow::anyhow!(e));
                }
            }
            Ok(())
        };
        Self::output_core(
            device,
            runtime_rate,
            block_size,
            channel_map,
            handle_store,
            fill,
        )
    }

    /// Starts a RuntimeHandle stream on the output device at `index`.
    pub fn play_handle_on_device(index: usize, handle: RuntimeHandle) -> Result<Self> {
        let device = crate::device_management::enumerate_output_devices()
            .into_iter()
            .nth(index)
            .ok_or_else(|| anyhow::anyhow!("No output device at index {}", index))?;
        let runtime_rate = handle.sample_rate() as u32;
        let block_size = handle.block_size();
        let handle_store = Arc::new(Mutex::new(Some(handle)));
        let store = handle_store.clone();
        let fill = move |data: &mut [f32], adapter: &mut BufferSizeAdapter| {
            if let Ok(mut g) = store.lock() {
                if let Some(ref mut h) = *g {
                    return adapter
                        .fill_host_buffer_handle(data, h, 2)
                        .map_err(|e| anyhow::anyhow!(e));
                }
            }
            Ok(())
        };
        Self::output_core(
            device,
            runtime_rate,
            block_size,
            ChannelMap::default(),
            handle_store,
            fill,
        )
    }

    /// Starts an input (recording) stream on `device`, forwarding captured
    /// frames into `recorder`. The recorder is shared so callers can read or
    /// [`crate::recorder::Recorder::save_wav`] it after the stream stops.
    pub fn play_input(
        device: cpal::Device,
        sample_rate: u32,
        channels: usize,
        recorder: SharedRecorder,
    ) -> Result<Self> {
        AtomicStreamState::verify_lock_free_atomics()?;
        let diagnostics = Diagnostics::new();
        let error_flag = Arc::new(AtomicBool::new(false));
        let recovery_needed = Arc::new(AtomicBool::new(false));
        let stream = Self::build_input_stream(
            device,
            sample_rate,
            channels,
            diagnostics.clone(),
            error_flag.clone(),
            recorder,
        )?;
        Ok(Self {
            stream: None,
            input_stream: Some(stream),
            state: Arc::new(AtomicStreamState::new(StreamState::Running)),
            error_flag,
            recovery_needed,
            diagnostics,
            runtime_rate: sample_rate,
            device_rate: sample_rate,
            block_size: 0,
            channel_map: ChannelMap::default(),
            handle_store: Arc::new(Mutex::new(None)),
            transport: TransportState::new(),
        })
    }

    /// Starts a full-duplex stream: `runtime` drives the output while captured
    /// input frames are forwarded into `recorder`.
    /// Starts a full-duplex stream: `runtime` drives the output while captured
    /// input frames are forwarded into `recorder`. cpal 0.15 exposes no single
    /// duplex builder, so this runs a coordinated output stream and input
    /// stream on the same device (both share `recorder`).
    pub fn play_duplex(
        device: cpal::Device,
        _sample_rate: u32,
        channels: usize,
        recorder: SharedRecorder,
        mut runtime: Runtime,
    ) -> Result<Self> {
        AtomicStreamState::verify_lock_free_atomics()?;
        let runtime_rate = runtime.sample_rate() as u32;
        let block_size = runtime.plan.block_size;
        let device_rate = Self::get_best_sample_rate(runtime_rate as f32)
            .map(|r| r as u32)
            .unwrap_or(runtime_rate);
        let adapter = BufferSizeAdapter::new(block_size)
            .with_channel_map(ChannelMap::default())
            .with_resampling(runtime_rate, device_rate);
        let diagnostics = Diagnostics::new();
        let state = Arc::new(AtomicStreamState::new(StreamState::Running));
        let error_flag = Arc::new(AtomicBool::new(false));
        let recovery_needed = Arc::new(AtomicBool::new(false));
        let transport = TransportState::new();
        let fill = move |data: &mut [f32], adapter: &mut BufferSizeAdapter| {
            adapter
                .fill_host_buffer(data, &mut runtime, 2)
                .map_err(|e| anyhow::anyhow!(e))
        };
        let out_stream = Self::build_output_stream(
            device.clone(),
            device_rate,
            adapter,
            diagnostics.clone(),
            state.clone(),
            error_flag.clone(),
            recovery_needed.clone(),
            transport.clone(),
            fill,
        )?;
        let in_stream = Self::build_input_stream(
            device,
            device_rate,
            channels,
            diagnostics.clone(),
            error_flag.clone(),
            recorder,
        )?;
        Ok(Self {
            stream: Some(out_stream),
            input_stream: Some(in_stream),
            state,
            error_flag,
            recovery_needed,
            diagnostics,
            runtime_rate,
            device_rate,
            block_size,
            channel_map: ChannelMap::default(),
            handle_store: Arc::new(Mutex::new(None)),
            transport,
        })
    }

    /// Attempts to recover from a device error.
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
            if let Ok(mut g) = store.lock() {
                if let Some(ref mut h) = *g {
                    return adapter
                        .fill_host_buffer_handle(data, h, 2)
                        .map_err(|e| anyhow::anyhow!(e));
                }
            }
            Ok(())
        };
        let adapter = self.make_adapter();
        let device = default_output_device()?;
        let stream = Self::build_output_stream(
            device,
            self.device_rate,
            adapter,
            self.diagnostics.clone(),
            self.state.clone(),
            self.error_flag.clone(),
            self.recovery_needed.clone(),
            self.transport.clone(),
            fill,
        )?;
        stream.play()?;
        self.stream = Some(stream);
        self.state.set_state(StreamState::Running);
        Ok(())
    }

    /// Sets the channel map used when (re)building the output stream.
    ///
    /// Takes effect on the next [`Self::restart`] / [`Self::recover`]. For an
    /// immediate change on a freshly started stream, use
    /// [`Self::play_with_channel_map`] instead.
    pub fn with_channel_map(mut self, channel_map: ChannelMap) -> Self {
        self.channel_map = channel_map;
        self
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
        if let Some(s) = &self.input_stream {
            let _ = s.play();
        }
        Ok(())
    }

    /// Pauses the audio stream (emits silence until restarted).
    pub fn stop(&self) {
        if let Some(stream) = &self.stream {
            let _ = stream.pause();
        }
        if let Some(s) = &self.input_stream {
            let _ = s.pause();
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

    /// Installs a transport clock sampled once per host buffer.
    pub fn set_transport_clock(&self, clock: Box<dyn TransportClock + Send + Sync>) {
        self.transport.set_clock(clock);
    }

    /// Returns the most recent transport position, or a zeroed value.
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
    /// or `None` if no value has been captured yet.
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
    fn drop(&mut self) {
        if let Some(stream) = 
        &self.stream {
            let _ = stream.pause();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auxide::graph::{Graph, NodeType, PortId, Rate};
    use auxide::plan::Plan;
    use auxide::rt::{Runtime, RuntimeCore};
    use std::thread::sleep;

    /// Builds a controller with all required fields populated (no live stream).
    fn bare_controller() -> StreamController {
        StreamController {
            stream: None,
            input_stream: None,
            state: Arc::new(AtomicStreamState::new(StreamState::Stopped)),
            error_flag: Arc::new(AtomicBool::new(false)),
            recovery_needed: Arc::new(AtomicBool::new(false)),
            diagnostics: Diagnostics::new(),
            runtime_rate: 44100,
            device_rate: 44100,
            block_size: 64,
            channel_map: ChannelMap::default(),
            handle_store: Arc::new(Mutex::new(None)),
            transport: TransportState::new(),
        }
    }

    fn build_graph(rate: f32) -> (Runtime, usize) {
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
        let block_size = plan.block_size;
        (Runtime::new(plan, &graph, rate), block_size)
    }

    #[test]
    fn test_error_flag() {
        let error_flag = Arc::new(AtomicBool::new(false));
        assert!(!error_flag.load(Ordering::Relaxed));
        error_flag.store(true, Ordering::Relaxed);
        assert!(error_flag.load(Ordering::Relaxed));
        error_flag.store(false, Ordering::Relaxed);
        assert!(!error_flag.load(Ordering::Relaxed));
    }

    #[test]
    fn test_play_without_device() {
        let (runtime, _bs) = build_graph(44100.0);
        // With a device present play succeeds; without one it errors. Both are
        // valid outcomes; we assert it resolves (does not panic) and, when a
        // device exists, that it actually starts.
        let result = StreamController::play(runtime);
        if crate::device_management::default_output_device().is_ok() {
            assert!(result.is_ok(), "play should succeed when a device exists");
        } else {
            assert!(result.is_err(), "play should fail gracefully without a device");
        }
    }

    #[test]
    fn test_controller_methods_with_no_stream() {
        let controller = bare_controller();
        assert_eq!(controller.state.get_state(), StreamState::Stopped);
        let _ = controller.start();
        assert_eq!(controller.state.get_state(), StreamState::Stopped);

        controller.state.set_state(StreamState::Running);
        controller.stop();
        assert_eq!(controller.state.get_state(), StreamState::Stopped);

        assert!(!controller.has_error());
        controller.error_flag.store(true, Ordering::Relaxed);
        assert!(controller.has_error());
        controller.clear_error();
        assert!(!controller.has_error());

        let diag = controller.diagnostics();
        assert_eq!(diag.callback_count, 0);
        assert_eq!(diag.overflow_count, 0);
        assert_eq!(diag.peak, 0.0);
    }

    #[test]
    fn test_diagnostics_counters() {
        let d = Diagnostics::new();
        assert_eq!(d.callback_count.load(Ordering::Relaxed), 0);
        assert_eq!(d.overflow_count.load(Ordering::Relaxed), 0);
        assert_eq!(d.peak.load(Ordering::Relaxed), 0);
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
        assert_eq!(f32::from_bits(d.peak.load(Ordering::Relaxed)), 0.5);
        d.update_peak(0.9);
        assert_eq!(f32::from_bits(d.peak.load(Ordering::Relaxed)), 0.9);
    }

    #[test]
    fn test_contract_stream_controller() {
        let mut adapter = BufferSizeAdapter::new(64);
        assert!(adapter.adapt_to_host_buffer(1024).is_ok());
        assert!(adapter.adapt_to_host_buffer(MAX_HOST_FRAMES + 1).is_err());
    }

    #[test]
    fn test_recover_clears_error_flag() {
        let mut controller = bare_controller();
        controller.error_flag.store(true, Ordering::Relaxed);
        controller.recovery_needed.store(true, Ordering::Relaxed);
        assert!(controller.has_error());
        assert!(controller.recovery_needed());
        controller.recover().unwrap();
        assert!(!controller.has_error());
        assert!(!controller.recovery_needed());
        assert_eq!(controller.state.get_state(), StreamState::Stopped);
    }

    fn build_handle(rate: f32) -> (RuntimeHandle, auxide::rt::RuntimeControl) {
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
        RuntimeCore::new_with_channels(plan, &graph, rate)
    }

    #[test]
    fn test_latency_reported_from_callback() {
        use crate::device_management::default_output_device;
        let controller = bare_controller();
        assert!(controller.latency().is_none());

        if let Ok(device) = default_output_device() {
            let sample_rate = device
                .supported_configs()
                .expect("device exposes supported configs")
                .into_iter()
                .find(|c| c.channels() == 2 && c.sample_format() == SampleFormat::F32)
                .map(|c| c.sample_rate().0)
                .expect("device offers a 2-channel F32 config");
            let (runtime, _bs) = build_graph(sample_rate as f32);
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
    fn test_restart_rebuilds_handle_stream() {
        use crate::device_management::default_output_device;
        if let Ok(device) = default_output_device() {
            let sample_rate = device
                .supported_configs()
                .expect("device exposes supported configs")
                .into_iter()
                .find(|c| c.channels() == 2 && c.sample_format() == SampleFormat::F32)
                .map(|c| c.sample_rate().0)
                .expect("device offers a 2-channel F32 config");
            let (handle, _control) = build_handle(sample_rate as f32);
            let mut sc =
                StreamController::play_handle(handle).expect("stream starts with a device");
            sc.start().expect("stream should start on a live device");
            sleep(Duration::from_millis(20));
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
        use std::sync::atomic::{AtomicU64, Ordering as O};
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
            let (handle, _control) = build_handle(sample_rate as f32);
            let sc = StreamController::play_handle(handle).expect("stream starts with a device");
            sc.set_transport_clock(Box::new(TestClock {
                sample: AtomicU64::new(0),
                bpm: 120.0,
            }));
            sc.start().expect("stream should start on a live device");
            sleep(Duration::from_millis(40));
            let first = sc.transport_time();
            assert!(first.sample >= 64, "transport sample should advance");
            sleep(Duration::from_millis(40));
            let second = sc.transport_time();
            assert!(second.sample > first.sample, "transport sample must advance");
            sc.stop();
        }
    }

    #[test]
    fn negotiation_wires_best_sample_rate() {
        use crate::device_management::default_output_device;
        if let Ok(device) = default_output_device() {
            let negotiated = StreamController::get_best_sample_rate(99999.0)
                .expect("negotiation should pick a supported rate");
            // The negotiated rate must be one the device actually supports.
            let supported = device
                .supported_configs()
                .expect("configs")
                .into_iter()
                .any(|c| {
                    c.sample_rate().0 as f32 == negotiated
                        && c.channels() == 2
                        && c.sample_format() == SampleFormat::F32
                });
            assert!(supported, "negotiated rate must be device-supported");
            // play at the negotiated rate must start (proves wiring).
            let (runtime, _bs) = build_graph(negotiated);
            let sc = StreamController::play(runtime).expect("play with negotiated rate");
            sc.start().expect("start");
            sleep(Duration::from_millis(20));
            sc.stop();
        }
    }

    #[test]
    fn device_selection_api() {
        use crate::device_management::enumerate_output_devices;
        let devices = enumerate_output_devices();
        if devices.is_empty() {
            return;
        }
        // Out-of-range index must error gracefully.
        let (runtime, _bs) = build_graph(44100.0);
        assert!(StreamController::play_on_device(usize::MAX, runtime).is_err());

        // Name lookup should select the first enumerated device.
        if let Ok(name) = devices[0].name() {
            if !name.is_empty() {
                let (runtime2, _bs) = build_graph(44100.0);
                let res = StreamController::play_on_device_by_name(&name, runtime2);
                assert!(res.is_ok(), "name lookup should select device '{}'", name);
            }
        }
    }

    #[test]
    fn input_duplex_records() {
        use crate::device_management::default_output_device;
        use crate::recorder::Recorder;
        if let Ok(device) = default_output_device() {
            let sample_rate = device
                .supported_configs()
                .expect("configs")
                .into_iter()
                .find(|c| c.channels() == 2 && c.sample_format() == SampleFormat::F32)
                .map(|c| c.sample_rate().0)
                .expect("rate");

            let recorder: SharedRecorder = Arc::new(Mutex::new(Recorder::new(sample_rate, 2)));
            if let Ok(sc) = StreamController::play_input(
                device.clone(),
                sample_rate,
                2,
                recorder.clone(),
            ) {
                sc.start().expect("start input");
                sleep(Duration::from_millis(30));
                assert!(
                    recorder.lock().unwrap().len_frames() > 0,
                    "recorder should capture input frames"
                );
            }

            let recorder2: SharedRecorder = Arc::new(Mutex::new(Recorder::new(sample_rate, 2)));
            let (runtime, _bs) = build_graph(sample_rate as f32);
            if let Ok(sc) =
                StreamController::play_duplex(device, sample_rate, 2, recorder2.clone(), runtime)
            {
                sleep(Duration::from_millis(30));
                assert!(
                    recorder2.lock().unwrap().len_frames() > 0,
                    "duplex recorder should capture input frames"
                );
                sc.stop();
            }
        }
    }
}
