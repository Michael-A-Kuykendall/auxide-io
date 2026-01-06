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
Stream Auxide's audio graphs to speakers with CPAL, featuring buffer size adaptation, channel routing, and RT-safe operation.

## Auxide Ecosystem
| Crate | Description | Version |
|-------|-------------|---------|
| [auxide](https://github.com/Michael-A-Kuykendall/auxide) | Real-time-safe audio graph kernel | 0.3.0 |
| **[auxide-dsp](https://github.com/Michael-A-Kuykendall/auxide-dsp)** | DSP nodes library | 0.2.0 |
| **[auxide-io](https://github.com/Michael-A-Kuykendall/auxide-io)** | Audio I/O layer | 0.2.0 |
| [auxide-midi](https://github.com/Michael-A-Kuykendall/auxide-midi) | MIDI integration | 0.2.0 |

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
auxide = "0.3"
auxide-io = "0.2"
```

## Example

```rust
use auxide_io::stream_controller::StreamController;
use auxide::graph::Graph;

// Create your audio graph
let graph = Graph::new();

// Set up audio output
let mut controller = StreamController::new(graph)?;
controller.start()?;

// Your graph will now stream to speakers
```

See `examples/` for more usage.

## Community & Support

• 🐛 Bug Reports: [GitHub Issues](https://github.com/Michael-A-Kuykendall/auxide-io/issues)
• 💬 Discussions: [GitHub Discussions](https://github.com/Michael-A-Kuykendall/auxide-io/discussions)
• 📖 Documentation: [docs/](https://github.com/Michael-A-Kuykendall/auxide-io/tree/main/docs)
• 💝 Sponsorship: [GitHub Sponsors](https://github.com/sponsors/Michael-A-Kuykendall)
• 🤝 Contributing: [CONTRIBUTING.md](https://github.com/Michael-A-Kuykendall/auxide-io/blob/main/CONTRIBUTING.md)
• 📜 Governance: [GOVERNANCE.md](https://github.com/Michael-A-Kuykendall/auxide-io/blob/main/GOVERNANCE.md)
• 🔒 Security: [SECURITY.md](https://github.com/Michael-A-Kuykendall/auxide-io/blob/main/SECURITY.md)

## License & Philosophy

MIT License - forever and always.

**Philosophy**: Audio I/O should be invisible. Auxide is infrastructure.

**Testing Philosophy**: Reliability through comprehensive validation and property-based testing.

**Forever maintainer**: Michael A. Kuykendall  
**Promise**: This will never become a paid product  
**Mission**: Making real-time audio I/O simple and reliable
