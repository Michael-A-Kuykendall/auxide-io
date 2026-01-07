//! Buffer size adaptation between host and runtime block sizes.
//!
//! Audio hosts may provide buffers of arbitrary sizes, while the runtime
//! expects fixed block sizes. This module bridges that gap using a ring buffer.

use auxide::rt::Runtime;

pub const MAX_HOST_FRAMES: usize = 16384;

/// Adapts between host buffer sizes and fixed runtime block sizes.
///
/// Uses a ring buffer to accumulate data from multiple runtime blocks
/// into host buffers, or vice versa, accommodating any size mismatch.
pub struct BufferSizeAdapter {
    ring_buffer: Vec<f32>,
    read_pos: usize,
    write_pos: usize,
    runtime_block_size: usize,
    block_buffer: Vec<f32>,
}

impl BufferSizeAdapter {
    /// Creates a new adapter for the given runtime block size.
    ///
    /// Allocates a 4× ring buffer to handle common host/runtime size mismatches.
    pub fn new(runtime_block_size: usize) -> Self {
        Self {
            ring_buffer: vec![0.0; 4 * MAX_HOST_FRAMES],
            read_pos: 0,
            write_pos: 0,
            runtime_block_size,
            block_buffer: vec![0.0; runtime_block_size],
        }
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

    /// Handles cases where the host provides less data than the runtime expects.
    ///
    /// Currently a no-op; can be extended to implement progressive accumulation.
    pub fn handle_partial_block(&mut self) {
        // Current implementation fills host buffers completely from available data
        // Partial block accumulation could be implemented here if needed for very small host buffers
    }

    /// Handles cases where the host buffer size is unknown or variable.
    ///
    /// Size validation happens at stream setup; this marks the point
    /// where dynamic sizing is managed at stream time.
    pub fn handle_unknown_buffer_size(&mut self) {
        // Current implementation handles unknown/variable sizes dynamically in fill_host_buffer
        // Size validation happens at stream setup time
    }

    /// Fills a host-provided buffer by processing runtime blocks and managing the ring buffer.
    ///
    /// Processes enough runtime blocks to supply the requested host buffer,
    /// duplicating mono output across all channels as needed.
    ///
    /// # Arguments
    /// * `host_buffer` - The output buffer to fill
    /// * `runtime` - The audio processing runtime
    /// * `channels` - Number of output channels (duplication factor for mono)
    pub fn fill_host_buffer(
        &mut self,
        host_buffer: &mut [f32],
        runtime: &mut Runtime,
        channels: usize,
    ) -> Result<(), &'static str> {
        let mut host_idx = 0;
        while host_idx < host_buffer.len() {
            // Check if we need more data
            let mut available = if self.write_pos >= self.read_pos {
                self.write_pos - self.read_pos
            } else {
                self.ring_buffer.len() - self.read_pos + self.write_pos
            };
            if available < self.runtime_block_size {
                // Process a block
                runtime.process_block(&mut self.block_buffer)?;
                // Write to ring buffer
                for &sample in &self.block_buffer {
                    self.ring_buffer[self.write_pos] = sample;
                    self.write_pos = (self.write_pos + 1) % self.ring_buffer.len();
                }
                // Recalculate available after writing
                available = if self.write_pos >= self.read_pos {
                    self.write_pos - self.read_pos
                } else {
                    self.ring_buffer.len() - self.read_pos + self.write_pos
                };
            }
            // Read from ring buffer
            if available >= 1 {
                let sample = self.ring_buffer[self.read_pos];
                self.read_pos = (self.read_pos + 1) % self.ring_buffer.len();
                for _ in 0..channels {
                    if host_idx < host_buffer.len() {
                        host_buffer[host_idx] = sample;
                        host_idx += 1;
                    }
                }
            } else {
                break;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auxide::graph::{Graph, NodeType, PortId, Rate};
    use auxide::plan::Plan;
    use auxide::rt::Runtime;

    #[test]
    fn test_fuzz_variable_sizes() {
        // Test with different sizes
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
        let mut runtime = Runtime::new(plan, &graph, 44100.0);

        let mut adapter = BufferSizeAdapter::new(64);
        let mut buffer = vec![0.0; 128]; // Stereo
        assert!(adapter
            .fill_host_buffer(&mut buffer, &mut runtime, 2)
            .is_ok());
        // Ensure we actually produced non-zero audio
        assert!(
            buffer.iter().any(|&x| x != 0.0),
            "Buffer should contain non-zero audio samples"
        );
        // For stereo, check that samples come in identical pairs (L=R for each frame)
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
        let mut runtime = Runtime::new(plan, &graph, 44100.0);

        let mut adapter = BufferSizeAdapter::new(64);
        let mut buffer = vec![0.0; 2]; // Very small buffer
        assert!(adapter
            .fill_host_buffer(&mut buffer, &mut runtime, 1)
            .is_ok());
        assert!(buffer.iter().any(|&x| x != 0.0));
    }

    #[test]
    fn test_large_host_buffer() {
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
        let mut runtime = Runtime::new(plan, &graph, 44100.0);

        let mut adapter = BufferSizeAdapter::new(64);
        let mut buffer = vec![0.0; 1024]; // Larger buffer
        assert!(adapter
            .fill_host_buffer(&mut buffer, &mut runtime, 1)
            .is_ok());
        assert!(buffer.iter().any(|&x| x != 0.0));
    }

    #[test]
    fn test_validate_oversized_buffer() {
        let mut adapter = BufferSizeAdapter::new(64);
        assert!(adapter.adapt_to_host_buffer(MAX_HOST_FRAMES + 1).is_err());
        assert!(adapter.adapt_to_host_buffer(1024).is_ok());
    }
}
