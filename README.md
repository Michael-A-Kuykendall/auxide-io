<img src="https://raw.githubusercontent.com/Michael-A-Kuykendall/auxide-io/main/assets/auxide-io-logo.png" alt="Auxide IO Logo" width="400">

[![Crates.io](https://img.shields.io/crates/v/auxide-io.svg)](https://crates.io/crates/auxide-io)
[![Documentation](https://docs.rs/auxide-io/badge.svg)](https://docs.rs/auxide-io)
[![CI](https://github.com/Michael-A-Kuykendall/auxide-io/workflows/CI/badge.svg)](https://github.com/Michael-A-Kuykendall/auxide-io/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

## 💝 Support Auxide's Growth

🚀 If Auxide helps you build amazing audio tools, consider [sponsoring](https://github.com/sponsors/Michael-A-Kuykendall) — 100% of support goes to keeping it free forever.

• $5/month: Coffee tier ☕ - Eternal gratitude + sponsor badge
• $25/month: Bug prioritizer 🐛 - Priority support + name in [SPONSORS.md](https://github.com/Michael-A-Kuykendall/auxide-io/blob/main/SPONSORS.md)
• $100/month: Corporate backer 🏢 - Logo placement + monthly office hours
• $500/month: Infrastructure partner 🚀 - Direct support + roadmap input

**[🎯 Become a Sponsor](https://github.com/sponsors/Michael-A-Kuykendall)** | See our amazing [sponsors](https://github.com/Michael-A-Kuykendall/auxide-io/blob/main/SPONSORS.md) 🙏

# Auxide IO

**Real-time audio I/O layer for Auxide.**  
Stream Auxide's audio graphs to speakers (and capture from microphones) with CPAL, featuring buffer-size adaptation, sample-rate negotiation with a resampling fallback, channel routing, transport-linked timing, and RT-safe operation.

## Features

- **CPAL Integration**: Cross-platform audio I/O with CPAL.
- **Sample-Rate Negotiation**: `get_best_sample_rate` picks a device-supported rate; when the runtime rate still differs, a linear `LinearResampler` fallback bridges the gap.
- **Buffer Adaptation**: Automatic buffer-size matching between graph and hardware via a ring buffer.
- **Channel Routing**: Flexible routing via `ChannelMap` (mono→stereo by default, or an explicit source→destination map).
- **Transport Timing**: Install a `TransportClock` sampled once per host buffer.
- **Input & Duplex**: `play_input` / `play_duplex` capture into a `Recorder` (WAV export via `hound`).
- **Error Recovery**: Robust error handling and recovery mechanisms (`recover` / `restart`).
- **RT-Safe**: `#![forbid(unsafe_code)]`; the real-time audio **callback** performs no heap allocation (buffers are pre-allocated at stream setup; lock-free atomics carry diagnostics), so it is safe to call from the audio thread.

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
auxide = "0.3"
auxide-io = "0.2"
```

A minimal, compiling example lives in [`examples/stream_example.rs`](examples/stream_example.rs). The short version:

```rust
use auxide::graph::{Graph, NodeType, PortId, Rate};
use auxide::plan::Plan;
use auxide::rt::RuntimeCore;
use auxide_io::{ChannelMap, StreamController, TransportClock, TransportTime};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

struct Clock { sample: AtomicU64, bpm: f32 }
impl TransportClock for Clock {
    fn transport_time(&self) -> TransportTime {
        let s = self.sample.fetch_add(64, Ordering::Relaxed);
        TransportTime { bpm: self.bpm, beat_phase: 0.0, sample: s }
    }
}

fn main() -> anyhow::Result<()> {
    let mut graph = Graph::new();
    let osc = graph.add_node(NodeType::SineOsc { freq: 440.0 });
    let sink = graph.add_node(NodeType::OutputSink);
    graph.add_edge(auxide::graph::Edge {
        from_node: osc, from_port: PortId(0),
        to_node: sink, to_port: PortId(0), rate: Rate::Audio,
    }).unwrap();
    let plan = Plan::compile(&graph, 64).unwrap();

    // Preferred path: RuntimeHandle + control channel.
    let (handle, _control) = RuntimeCore::new_with_channels(plan, &graph, 44100.0);
    let sc = StreamController::play_handle(handle)?;
    sc.set_transport_clock(Box::new(Clock { sample: AtomicU64::new(0), bpm: 120.0 }));
    sc.start()?;
    std::thread::sleep(Duration::from_millis(50));
    sc.stop();
    Ok(())
}
```

## Architecture

Auxide IO bridges Auxide's audio graphs and system audio hardware:

- **`StreamController`**: Manages audio stream lifecycle (play / play_handle / play_on_device / play_input / play_duplex), diagnostics, transport clock, and error recovery.
- **`BufferSizeAdapter`**: Handles buffer-size mismatches and applies the active `ChannelMap`; owns the optional resampler fallback.
- **`ChannelMap`**: Maps the mono runtime output onto device channels (default `MonoToStereo`, or `Explicit(Vec<(src, dst)>)`).
- **`LinearResampler`**: Linear interpolation between runtime and device sample rates.
- **`Recorder`**: Accumulates captured input and exports WAV (`hound`).
- **`device_management`**: Device enumeration, selection, and supported-config queries.

## API Quick Reference

| Constructor | Purpose |
|-------------|---------|
| `StreamController::play(Runtime)` | Legacy path (no restart after error). |
| `StreamController::play_handle(RuntimeHandle)` | Preferred; restartable after recovery. |
| `play_on_device(index, …)` / `play_handle_on_device(index, …)` | Target a device by enumeration index. |
| `play_on_device_by_name(name, …)` | Target a device by name. |
| `play_with_channel_map(…, ChannelMap)` | Custom channel routing. |
| `play_input(device, rate, channels, Recorder)` | Capture input to a `Recorder`. |
| `play_duplex(device, rate, channels, Recorder, Runtime)` | Simultaneous output + input capture. |

- `get_best_sample_rate(requested)` — negotiates a device-supported rate.
- `set_transport_clock(clock)` / `transport_time()` — musical-time sampling.
- `latency()` / `diagnostics()` — lock-free diagnostics (latency from `OutputCallbackInfo::timestamp`).
- `recover()` / `restart()` — rebuild the stream after a device error.

## Status

- ✅ CPAL Integration: Cross-platform audio I/O working
- ✅ Buffer Adaptation: Automatic size matching implemented
- ✅ Channel Routing: `ChannelMap` (MonoToStereo + explicit) implemented
- ✅ Sample-Rate Negotiation + Resampling: `get_best_sample_rate` wired; `LinearResampler` fallback
- ✅ Input / Duplex Recording: `Recorder` + `play_input` / `play_duplex`
- ✅ Device Selection: `play_on_device` / `play_on_device_by_name`
- ✅ Error Recovery: Robust handling in place
- 📋 Performance: Latency/glitch benchmark harness in `benches/`

## Community & Support
• 🐛 Bug Reports: [GitHub Issues](https://github.com/Michael-A-Kuykendall/auxide-io/issues)
• 💬 Discussions: [GitHub Discussions](https://github.com/Michael-A-Kuykendall/auxide-io/discussions)
• 📖 Documentation: [docs.rs](https://docs.rs/auxide-io)
• 💝 Sponsorship: [GitHub Sponsors](https://github.com/sponsors/Michael-A-Kuykendall)
• 🤝 Contributing: [CONTRIBUTING.md](https://github.com/Michael-A-Kuykendall/auxide-io/blob/main/CONTRIBUTING.md)
• 📜 Governance: [GOVERNANCE.md](https://github.com/Michael-A-Kuykendall/auxide-io/blob/main/GOVERNANCE.md)
• 🔒 Security: [SECURITY.md](https://github.com/Michael-A-Kuykendall/auxide-io/blob/main/SECURITY.md)

## License & Philosophy
MIT License - forever and always.

**Philosophy**: Audio I/O should be invisible. Auxide is infrastructure.

**Testing Philosophy**: Reliability through comprehensive validation.

**Forever maintainer**: Michael A. Kuykendall  
**Promise**: This will never become a paid product  
**Mission**: Making real-time audio I/O simple and reliable

## Auxide Ecosystem
| Crate | Description | Version |
|-------|-------------|---------|
| [auxide](https://github.com/Michael-A-Kuykendall/auxide) | Real-time-safe audio graph kernel | 0.3.2 |
| [auxide-dsp](https://github.com/Michael-A-Kuykendall/auxide-dsp) | DSP nodes library | 0.2.1 |
| **[auxide-io](https://github.com/Michael-A-Kuykendall/auxide-io)** | Audio I/O layer | 0.1.3 |
| [auxide-midi](https://github.com/Michael-A-Kuykendall/auxide-midi) | MIDI integration | 0.1.2 |
