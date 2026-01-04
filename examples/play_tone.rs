// examples/play_tone.rs
use auxide::graph::{Graph, NodeType, PortId, Rate};
use auxide::plan::Plan;
use auxide::rt::Runtime;
use auxide_io::stream_controller::StreamController;

fn main() -> anyhow::Result<()> {
    // Build 440Hz sine
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

    let plan = Plan::compile(&graph, 512).unwrap();
    let runtime = Runtime::new(plan, &graph, 192000.0);

    // Play it
    let controller = StreamController::play(runtime)?;
    controller.start()?;
    
    println!("Playing 440Hz tone. Press Enter to stop...");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    
    controller.stop();
    println!("Stopped.");
    
    Ok(())
}