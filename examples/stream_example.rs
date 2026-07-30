//! Example: streaming an Auxide graph to the default output device.
//!
//! Builds a tiny oscillator graph, runs it through a `RuntimeHandle`, and
//! streams it via `StreamController`. Also shows the transport-clock hook and
//! the recording types. This file exists so the README snippets stay honest:
//! it must compile against the current public API. (Nothing starts without a
//! device; `main` only runs when one is present.)

use auxide::graph::{Graph, NodeType, PortId, Rate};
use auxide::plan::Plan;
use auxide::rt::RuntimeCore;
use auxide_io::{
    ChannelMap, Recorder, SharedRecorder, StreamController, TransportClock, TransportTime,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A trivial transport clock whose sample position advances per buffer.
struct Clock {
    sample: AtomicU64,
    bpm: f32,
}
impl TransportClock for Clock {
    fn transport_time(&self) -> TransportTime {
        let s = self.sample.fetch_add(64, Ordering::Relaxed);
        TransportTime {
            bpm: self.bpm,
            beat_phase: 0.0,
            sample: s,
        }
    }
}

fn main() -> anyhow::Result<()> {
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

    // Preferred path: a RuntimeHandle plus its control channel.
    let (handle, _control) = RuntimeCore::new_with_channels(plan, &graph, 44100.0);
    let sc = StreamController::play_handle(handle)?;

    // The transport clock is sampled once per host buffer.
    sc.set_transport_clock(Box::new(Clock {
        sample: AtomicU64::new(0),
        bpm: 120.0,
    }));

    // Channel mapping and device selection live on the same API:
    let _map = ChannelMap::Explicit(vec![(0, 1)]);

    // Recording input to a WAV file uses a shareable Recorder:
    let _recorder: SharedRecorder = Arc::new(Mutex::new(Recorder::new(44100, 2)));

    sc.start()?;
    std::thread::sleep(Duration::from_millis(50));
    sc.stop();
    Ok(())
}
