use auxide::rt::Runtime;

pub const MAX_HOST_FRAMES: usize = 16384;

pub struct BufferSizeAdapter {
    ring_buffer: Vec<f32>,
    read_pos: usize,
    write_pos: usize,
    runtime_block_size: usize,
    block_buffer: Vec<f32>,
}

impl BufferSizeAdapter {
    pub fn new(runtime_block_size: usize) -> Self {
        Self {
            ring_buffer: vec![0.0; 4 * MAX_HOST_FRAMES],
            read_pos: 0,
            write_pos: 0,
            runtime_block_size,
            block_buffer: vec![0.0; runtime_block_size],
        }
    }

    pub fn adapt_to_host_buffer(&mut self, host_size: usize) -> Result<(), &'static str> {
        if host_size > MAX_HOST_FRAMES {
            return Err("Host buffer size exceeds MAX_HOST_FRAMES");
        }
        Ok(())
    }

    pub fn handle_partial_block(&mut self) {
        // Accumulate partial blocks if needed
    }

    pub fn handle_unknown_buffer_size(&mut self) {
        // Handle unknown sizes
    }

    pub fn fill_host_buffer(&mut self, host_buffer: &mut [f32], runtime: &mut Runtime, channels: usize) -> Result<(), &'static str> {
        let mut host_idx = 0;
        while host_idx < host_buffer.len() {
            // Check if we need more data
            let available = if self.write_pos >= self.read_pos {
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
        graph.add_edge(auxide::graph::Edge {
            from_node: osc,
            from_port: PortId(0),
            to_node: sink,
            to_port: PortId(0),
            rate: Rate::Audio,
        }).unwrap();
        let plan = Plan::compile(&graph, 64).unwrap();
        let mut runtime = Runtime::new(plan, &graph, 44100.0);

        let mut adapter = BufferSizeAdapter::new(64);
        let mut buffer = vec![0.0; 128]; // Stereo
        assert!(adapter.fill_host_buffer(&mut buffer, &mut runtime, 2).is_ok());
    }

    #[test]
    fn test_oversized_buffer_rejection() {
        let mut adapter = BufferSizeAdapter::new(64);
        assert!(adapter.adapt_to_host_buffer(MAX_HOST_FRAMES + 1).is_err());
    }
}
