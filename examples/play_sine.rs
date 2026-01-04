use auxide::graph::{Graph, NodeType, PortId, Rate};
use auxide::plan::Plan;
use auxide::rt::Runtime;
use auxide_io::stream_controller::StreamController;
use std::thread;
use std::time::Duration;

fn main() -> anyhow::Result<()> {
    // Build a simple sine wave graph
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

    let plan = Plan::compile(&graph, 512).unwrap();
    let runtime = Runtime::new(plan, &graph, 192000.0);

    // Create and start the stream
    let controller = StreamController::play(runtime)?;
    controller.start()?;

    println!("Playing 440Hz sine wave for 5 seconds...");
    thread::sleep(Duration::from_secs(5));

    controller.stop();
    println!("Stopped.");

    Ok(())
}
