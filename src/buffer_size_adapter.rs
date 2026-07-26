//! Buffer size adaptation between host and runtime block sizes.
//!
//! Audio hosts may provide buffers of arbitrary sizes, while the runtime
//! expects fixed block sizes. This module bridges that gap using a ring buffer.
//! It also owns the channel-routing policy ([`ChannelMap`]) and the optional
//! sample-rate [`LinearResampler`] fallback used when the device rate differs
//! from the runtime rate.

use crate::channel_router::ChannelMap;
use crate::resampler::LinearResampler;
use auxide::rt::{Runtime, RuntimeHandle};

pub const MAX_HOST_FRAMES: usize = 16384;

/// Adapts between host buffer sizes and fixed runtime block sizes.
///
/// Uses a ring buffer to accumulate data from multiple runtime blocks
/// into host buffers, or vice versa, accommodating any size mismatch.
///
/// The adapter also applies the configured [`ChannelMap`] when writing device
/// frames and, when constructed for a mismatched device rate, resamples the
/// mono runtime stream up/down to the device rate.
pub struct BufferSizeAdapter {
    ring_buffer: Vec<f32>,
    read_pos: usize,
    write_pos: usize,
    runtime_block_size: usize,
    block_buffer: Vec<f32>,
    channel_map: ChannelMap,
    /// Active resampler for the runtime to device rate mismatch, if any.
    resampler: Option<LinearResampler>,
    /// Staging buffer for pulling runtime-rate samples before resampling.
    resample_in: Vec<f32>,
    /// Staging buffer for device-rate samples after resampling.
    resample_out: Vec<f32>,
    /// Count of underflow glitches observed while filling host buffers.
    glitch_count: u64,
}

impl BufferSizeAdapter {
    /// Creates a new adapter for the given runtime block size.
    ///
    /// Allocates a 4x ring buffer to handle common host/runtime size mismatches.
    /// Defaults to [`ChannelMap::MonoToStereo`] and no resampling (used when the
    /// device rate equals the runtime rate).
    pub fn new(runtime_block_size: usize) -> Self {
        Self {
            ring_buffer: vec![0.0; 4 * MAX_HOST_FRAMES],
            read_pos: 0,
            write_pos: 0,
            runtime_block_size,
            block_buffer: vec![0.0; runtime_block_size],
            channel_map: ChannelMap::default(),
            resampler: None,
            resample_in: vec![0.0; MAX_HOST_FRAMES * 8 + runtime_block_size],
            resample_out: vec![0.0; MAX_HOST_FRAMES + runtime_block_size],
            glitch_count: 0,
        }
    }

    /// Sets the channel-routing policy (defaults to [`ChannelMap::MonoToStereo`]).
    pub fn with_channel_map(mut self, map: ChannelMap) -> Self {
        self.channel_map = map;
        self
    }

    /// Enables resampling between `input_rate` (runtime) and `output_rate`
    /// (device). A no-op when the rates are equal (the fast passthrough path is
    /// used instead).
    pub fn with_resampling(mut self, input_rate: u32, output_rate: u32) -> Self {
        if input_rate != output_rate && input_rate > 0 && output_rate > 0 {
            self.resampler =
                Some(LinearResampler::new(input_rate, output_rate, self.runtime_block_size));
        }
        self
    }

    /// Returns the number of glitches (ring underflows) observed since
    /// construction. Useful for latency/glitch benchmarking.
    pub fn glitches(&self) -> u64 {
        self.glitch_count
    }

    /// Validates host buffer size against the maximum allowed.
    ///
    /// Returns an error if the host buffer exceeds `MAX_HOST_FRAMES`.
    pub fn adapt_to_host_buffer(&mut self, host_size: usize) -> Result<(), &'static str> {
        if host_size > MAX_HOST_FRAMES {
            return Err("Host buffer size exceeds MAX_HOST_FRAMES");
        }
        Ok(())
    }

    /// Fills a host-provided buffer by processing runtime blocks and managing
    /// the ring buffer, routing through the configured [`ChannelMap`].
    ///
    /// `host_buffer` is interleaved with `channels` device channels per frame.
    /// `pull` produces one runtime block of mono samples at the runtime rate.
    fn fill_inner<F>(
        &mut self,
        host_buffer: &mut [f32],
        channels: usize,
        pull: &mut F,
    ) -> Result<(), &'static str>
    where
        F: FnMut(&mut [f32]) -> Result<(), &'static str>,
    {
        if channels == 0 {
            return Err("device channel count must be > 0");
        }
        let host_frames = host_buffer.len() / channels;

        // Fast path: device rate == runtime rate, no resampling.
        if self.resampler.is_none() {
            return self.fill_passthrough(host_buffer, channels, host_frames, pull);
        }

        // Resampling path: produce `host_frames` device-rate mono samples.
        let resampler = self
            .resampler
            .as_mut()
            .expect("resampler present in resampling path");
        let need = resampler.input_needed_for(host_frames);
        let mut got = 0usize;
        while got < need {
            pull(&mut self.block_buffer)?;
            let end = (got + self.runtime_block_size).min(self.resample_in.len());
            self.resample_in[got..end].copy_from_slice(&self.block_buffer[..end - got]);
            got = end;
        }
        if got > 0 {
            resampler.push(&self.resample_in[..got]);
        }
        if host_frames > self.resample_out.len() {
            return Err("host buffer exceeds resampler staging capacity");
        }
        resampler.pull(host_frames, &mut self.resample_out[..host_frames]);

        for f in 0..host_frames {
            let base = f * channels;
            for c in 0..channels {
                host_buffer[base + c] = 0.0;
            }
            self.channel_map
                .apply(self.resample_out[f], &mut host_buffer[base..base + channels]);
        }
        Ok(())
    }

    /// Passthrough fill used when runtime and device rates match.
    fn fill_passthrough<F>(
        &mut self,
        host_buffer: &mut [f32],
        channels: usize,
        host_frames: usize,
        pull: &mut F,
    ) -> Result<(), &'static str>
    where
        F: FnMut(&mut [f32]) -> Result<(), &'static str>,
    {
        let mut frame = 0usize;
        while frame < host_frames {
            if self.available() < 1 {
                pull(&mut self.block_buffer)?;
                for &sample in &self.block_buffer {
                    self.ring_buffer[self.write_pos] = sample;
                    self.write_pos = (self.write_pos + 1) % self.ring_buffer.len();
                }
                if self.available() < 1 {
                    self.glitch_count += 1;
                    host_buffer[frame * channels..].fill(0.0);
                    return Ok(());
                }
            }

            let sample = self.ring_buffer[self.read_pos];
            self.read_pos = (self.read_pos + 1) % self.ring_buffer.len();

            let base = frame * channels;
            for c in 0..channels {
                host_buffer[base + c] = 0.0;
            }
            self.channel_map
                .apply(sample, &mut host_buffer[base..base + channels]);
            frame += 1;
        }
        Ok(())
    }

    /// Number of samples currently buffered in the ring.
    fn available(&self) -> usize {
        if self.write_pos >= self.read_pos {
            self.write_pos - self.read_pos
        } else {
            self.ring_buffer.len() - self.read_pos + self.write_pos
        }
    }

    /// Fills a host-provided buffer using a borrowed [`Runtime`].
    pub fn fill_host_buffer(
        &mut self,
        host_buffer: &mut [f32],
        runtime: &mut Runtime,
        channels: usize,
    ) -> Result<(), &'static str> {
        let block_size = self.runtime_block_size;
        let mut block = vec![0.0f32; block_size];
        let mut pull = |out: &mut [f32]| -> Result<(), &'static str> {
            runtime.process_block(&mut block)?;
            out.copy_from_slice(&block);
            Ok(())
        };
        self.fill_inner(host_buffer, channels, &mut pull)
    }

    /// Fills a host-provided buffer using a [`RuntimeHandle`].
    pub fn fill_host_buffer_handle(
        &mut self,
        host_buffer: &mut [f32],
        handle: &mut RuntimeHandle,
        channels: usize,
    ) -> Result<(), &'static str> {
        let block_size = self.runtime_block_size;
        let mut block = vec![0.0f32; block_size];
        let mut pull = |out: &mut [f32]| -> Result<(), &'static str> {
            handle.process_block(&mut block)?;
            out.copy_from_slice(&block);
            Ok(())
        };
        self.fill_inner(host_buffer, channels, &mut pull)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auxide::graph::{Graph, NodeType, PortId, Rate};
    use auxide::plan::Plan;
    use auxide::rt::Runtime;

    fn make_runtime(rate: f32) -> Runtime {
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
        Runtime::new(plan, &graph, rate)
    }

    #[test]
    fn test_variable_host_sizes() {
        let mut runtime = make_runtime(44100.0);
        let mut adapter = BufferSizeAdapter::new(64);
        let mut buffer = vec![0.0; 128];
        assert!(adapter.fill_host_buffer(&mut buffer, &mut runtime, 2).is_ok());
        assert!(
            buffer.iter().any(|&x| x != 0.0),
            "Buffer should contain non-zero audio samples"
        );
        for i in (0..buffer.len()).step_by(2) {
            if i + 1 < buffer.len() {
                assert_eq!(
                    buffer[i],
                    buffer[i + 1],
                    "Stereo channels should be identical for mono input"
                );
            }
        }
    }

    #[test]
    fn test_oversized_buffer_rejection() {
        let mut adapter = BufferSizeAdapter::new(64);
        assert!(adapter.adapt_to_host_buffer(MAX_HOST_FRAMES + 1).is_err());
    }

    #[test]
    fn test_small_host_buffer() {
        let mut runtime = make_runtime(44100.0);
        let mut adapter = BufferSizeAdapter::new(64);
        let mut buffer = vec![0.0; 2];
        assert!(adapter.fill_host_buffer(&mut buffer, &mut runtime, 1).is_ok());
        assert!(buffer.iter().any(|&x| x != 0.0));
    }

    #[test]
    fn test_large_host_buffer() {
        let mut runtime = make_runtime(44100.0);
        let mut adapter = BufferSizeAdapter::new(64);
        let mut buffer = vec![0.0; 1024];
        assert!(adapter.fill_host_buffer(&mut buffer, &mut runtime, 1).is_ok());
        assert!(buffer.iter().any(|&x| x != 0.0));
    }

    #[test]
    fn test_validate_oversized_buffer() {
        let mut adapter = BufferSizeAdapter::new(64);
        assert!(adapter.adapt_to_host_buffer(MAX_HOST_FRAMES + 1).is_err());
        assert!(adapter.adapt_to_host_buffer(1024).is_ok());
    }

    #[test]
    fn test_channel_map_explicit_routing() {
        let map = ChannelMap::Explicit(vec![(0, 3)]);
        let mut adapter = BufferSizeAdapter::new(64).with_channel_map(map);
        let mut runtime = make_runtime(44100.0);
        let mut buffer = vec![0.0; 16];
        assert!(adapter.fill_host_buffer(&mut buffer, &mut runtime, 4).is_ok());
        // Mapped channel 3 must carry signal; all other channels stay silent.
        for f in 0..4usize {
            assert_eq!(buffer[f * 4], 0.0, "channel 0 must stay silent");
            assert_eq!(buffer[f * 4 + 1], 0.0, "channel 1 must stay silent");
            assert_eq!(buffer[f * 4 + 2], 0.0, "channel 2 must stay silent");
        }
        let energy: f32 = buffer.iter().map(|x| x.abs()).sum();
        assert!(energy > 0.0, "mapped channel 3 must carry signal");
    }

    #[test]
    fn no_glitches_normal_sizing() {
        let mut runtime = make_runtime(44100.0);
        let mut adapter = BufferSizeAdapter::new(64);
        for _ in 0..1000 {
            let mut buffer = vec![0.0; 512];
            adapter
                .fill_host_buffer(&mut buffer, &mut runtime, 2)
                .expect("fill must succeed for normal sizing");
            assert!(
                buffer.iter().any(|&x| x != 0.0),
                "normal sizing must keep producing audio"
            );
        }
        assert_eq!(
            adapter.glitches(),
            0,
            "normal sizing must not produce any glitches"
        );
    }

    #[test]
    fn test_resampling_path_produces_output() {
        // Force a rate mismatch so the adapter engages the LinearResampler
        // fallback (device 48000 vs runtime 44100); output must still be
        // non-zero and correctly sized after resampling.
        let mut runtime = make_runtime(44100.0);
        let mut adapter = BufferSizeAdapter::new(64).with_resampling(44100, 48000);
        let mut buffer = vec![0.0; 512]; // 256 stereo frames at the device rate
        assert!(adapter.fill_host_buffer(&mut buffer, &mut runtime, 2).is_ok());
        assert!(
            buffer.iter().any(|&x| x != 0.0),
            "resampling path must still produce audio"
        );
    }
}
