// examples/mixer.rs
//! Real-time Mixer Example
//!
//! Demonstrates mixing two oscillators using Auxide + Auxide IO.
//! Plays a chord created by mixing 440Hz and 554Hz (A and C#).
//! Run with: cargo run --example mixer

use auxide::graph::{Graph, NodeType, PortId, Rate};
use auxide::plan::Plan;
use auxide::rt::Runtime;
use auxide_io::stream_controller::StreamController;
use std::thread;
use std::time::Duration;

fn main() -> anyhow::Result<()> {
    println!("Auxide Mixer Demo");
    println!("Playing a chord: 440Hz (A) + 554Hz (C#)...");

    // Create graph: two oscillators + mixer + output
    let mut graph = Graph::new();

    // Oscillator 1: 440Hz (A note)
    let osc1 = graph.add_node(NodeType::SineOsc { freq: 440.0 });

    // Oscillator 2: 554Hz (C# note) - creates a major sixth interval
    let osc2 = graph.add_node(NodeType::SineOsc { freq: 554.0 });

    // Mixer to combine the two oscillators
    let mix = graph.add_node(NodeType::Mix);

    // Output sink
    let sink = graph.add_node(NodeType::OutputSink);

    // Connect: osc1 -> mix (input 0)
    graph
        .add_edge(auxide::graph::Edge {
            from_node: osc1,
            from_port: PortId(0),
            to_node: mix,
            to_port: PortId(0),
            rate: Rate::Audio,
        })
        .unwrap();

    // Connect: osc2 -> mix (input 1)
    graph
        .add_edge(auxide::graph::Edge {
            from_node: osc2,
            from_port: PortId(0),
            to_node: mix,
            to_port: PortId(1),
            rate: Rate::Audio,
        })
        .unwrap();

    // Connect: mix -> output
    graph
        .add_edge(auxide::graph::Edge {
            from_node: mix,
            from_port: PortId(0),
            to_node: sink,
            to_port: PortId(0),
            rate: Rate::Audio,
        })
        .unwrap();

    let plan = Plan::compile(&graph, 512).unwrap();
    let runtime = Runtime::new(plan, &graph, 44100.0);

    // Play the mixed oscillators
    let controller = StreamController::play(runtime)?;
    controller.start()?;

    println!("Playing for 10 seconds...");
    thread::sleep(Duration::from_secs(10));

    controller.stop();
    println!("Mixer demo complete!");

    Ok(())
}
