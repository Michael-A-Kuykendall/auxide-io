//! Latency / throughput benchmark for the [`BufferSizeAdapter`].
//!
//! Documents its budget: filling one runtime block (64 frames) to a 256-frame
//! stereo host buffer should stay well under 1 ms/block on commodity hardware.
//! This replaces the earlier "fuzz" latency claims with a measured harness.

use auxide::graph::{Graph, NodeType, PortId, Rate};
use auxide::plan::Plan;
use auxide::rt::Runtime;
use auxide_io::buffer_size_adapter::BufferSizeAdapter;
use criterion::{criterion_group, criterion_main, Criterion};

fn make_runtime() -> Runtime {
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
    Runtime::new(plan, &graph, 44100.0)
}

fn bench_latency(c: &mut Criterion) {
    let mut runtime = make_runtime();
    let mut adapter = BufferSizeAdapter::new(64);

    // Budget: < 1 ms/block @ 64 (documented). The measured median should be
    // comfortably under this on commodity hardware.
    c.bench_function("fill_256_stereo_frames", |b| {
        b.iter(|| {
            let mut buf = vec![0.0f32; 512];
            adapter.fill_host_buffer(&mut buf, &mut runtime, 2).unwrap();
        });
    });

    for &frames in &[64usize, 256, 1024] {
        let mut buf = vec![0.0f32; frames * 2];
        c.bench_function(&format!("fill_{}_stereo_frames", frames), |b| {
            b.iter(|| {
                adapter.fill_host_buffer(&mut buf, &mut runtime, 2).unwrap();
            });
        });
    }
}

criterion_group!(benches, bench_latency);
criterion_main!(benches);
