<img src="https://raw.githubusercontent.com/Michael-A-Kuykendall/auxide-io/main/assets/auxide-io-logo.png" alt="Auxide IO Logo" width="400">

[![Crates.io](https://img.shields.io/crates/v/auxide-io.svg)](https://crates.io/crates/auxide-io)
[![Documentation](https://docs.rs/auxide-io/badge.svg)](https://docs.rs/auxide-io)
[![CI](https://github.com/Michael-A-Kuykendall/auxide-io/workflows/CI/badge.svg)](https://github.com/Michael-A-Kuykendall/auxide-io/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

# Auxide IO

**Real-time audio I/O layer for Auxide.**  
Stream Auxide's audio graphs to speakers with CPAL, featuring buffer size adaptation, channel routing, and RT-safe operation.

## The Auxide Audio Stack

Auxide IO is the **output layer** of the Auxide audio ecosystem:

- **[Auxide](https://github.com/Michael-A-Kuykendall/auxide)**: RT-safe DSP graph kernel
- **Auxide IO** (this crate): Hardware audio streaming and I/O

Together, they provide a complete solution for real-time audio synthesis and processing in Rust.

**🔗 [Get Auxide Core](https://github.com/Michael-A-Kuykendall/auxide) | [Crate](https://crates.io/crates/auxide) | [Docs](https://docs.rs/auxide)**

## What is Auxide IO?

Auxide IO bridges the gap between Auxide's deterministic kernel and real-world audio output. It provides:
- **CPAL Integration**: Cross-platform audio I/O via CPAL.
- **Buffer Adaptation**: Handles mismatched buffer sizes between Auxide (fixed) and host (variable).
- **Channel Routing**: Mono-to-stereo duplication, future multi-channel support.
- **RT Safety**: No allocations, locks, or I/O in audio callbacks.
- **Error Recovery**: Graceful handling of stream errors with silence fallback.

### Why Auxide IO?
Auxide handles the DSP graph; Auxide IO handles the hardware. Together, they form a complete audio synthesis stack.

| Feature              | Auxide IO | CPAL | Rodio | SDL2 Audio |
|----------------------|-----------|------|-------|------------|
| RT-Safe             | ✅       | ✅   | ❌    | ❌        |
| Buffer Adaptation   | ✅       | ❌   | ❌    | ❌        |
| Channel Routing     | ✅       | ❌   | ❌    | ❌        |
| Auxide Integration  | ✅       | ❌   | ❌    | ❌        |
| Cross-Platform      | ✅       | ✅   | ✅    | ✅        |

## Governance

Auxide IO is open source but not open contribution. The project is maintained by @Michael-A-Kuykendall; unsolicited PRs are closed by default. See CONTRIBUTING.md and GOVERNANCE.md for the collaboration policy.

## Usage

Auxide IO requires [Auxide](https://github.com/Michael-A-Kuykendall/auxide) for the DSP graph. Add both to your `Cargo.toml`:

```toml
[dependencies]
auxide = "0.1"
auxide-io = "0.1"
```

Basic playback:
```rust
use auxide::graph::{Graph, NodeType, PortId, Rate};
use auxide::plan::Plan;
use auxide::rt::Runtime;
use auxide_io::StreamController;

fn main() -> anyhow::Result<()> {
    // Build a sine wave graph
    let mut graph = Graph::new();
    let osc = graph.add_node(NodeType::SineOsc { freq: 440.0 });
    let sink = graph.add_node(NodeType::OutputSink);
    graph.add_edge(auxide::graph::Edge {
        from_node: osc,
        from_port: PortId(0),
        to_node: sink,
        to_port: PortId(0),
        rate: Rate::Audio,
    })?;

    let plan = Plan::compile(&graph, 512)?;
    let runtime = Runtime::new(plan, &graph, 44100.0);

    // Play it
    let controller = StreamController::play(runtime)?;
    controller.start()?;
    // Audio plays...
    controller.stop();

    Ok(())
}
```

> **💡 Complex Graphs**: This works with any Auxide graph! Try the [Auxide examples](https://github.com/Michael-A-Kuykendall/auxide/tree/master/examples) like AM synthesis, filter chains, or sequencers—just replace the offline `runtime.process_block()` calls with `StreamController::play()`.

## Architecture

Auxide IO's components:
- **StreamController**: Manages CPAL streams, state, and lifecycle.
- **BufferSizeAdapter**: Ring buffer for size mismatches.
- **ChannelRouter**: Mono-to-stereo conversion.
- **StreamState**: Atomic state management for RT communication.
- **ErrorRecovery**: Silent failure on errors.

All operations are RT-safe: fixed-size buffers, atomic ops, no heap allocation in callbacks.

## Contributing

See CONTRIBUTING.md. Auxide IO follows the same rules as Auxide: high standards, RT safety first.

## Learn More

- **[Auxide Core](https://github.com/Michael-A-Kuykendall/auxide)**: Build complex DSP graphs, custom nodes, and audio processing chains
- **Examples**: 
  - `play_sine.rs` - Basic sine wave playback
  - `play_tone.rs` - Interactive tone player  
  - `mixer.rs` - Mixing two oscillators for chords
- **Documentation**: Full API docs on [docs.rs](https://docs.rs/auxide-io)

## License

MIT License. See LICENSE.

## Sponsors

See SPONSORS.md.