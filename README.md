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

## Features

- **CPAL Integration**: Cross-platform audio I/O with CPAL
- **Buffer Adaptation**: Automatic buffer size matching between graph and hardware
- **Channel Routing**: Flexible channel mapping and routing
- **Error Recovery**: Robust error handling and recovery mechanisms
- **RT-Safe**: Zero allocations in audio processing paths

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
auxide = "0.3"
auxide-io = "0.2"
```

## Example

```rust
use auxide_io::{AudioStream, StreamConfig};

// Configure audio stream
let config = StreamConfig {
    sample_rate: 44100.0,
    channels: 2,
    buffer_size: 512,
};

// Create and start audio stream
let mut stream = AudioStream::new(config)?;
stream.start()?;

// Stream will automatically process auxide graphs
// Audio flows from graph output to speakers
```

## Architecture

Auxide IO provides the bridge between Auxide's audio graphs and system audio hardware:

- **Stream Controller**: Manages audio stream lifecycle
- **Buffer Adapter**: Handles buffer size mismatches
- **Channel Router**: Maps graph outputs to hardware channels
- **Error Recovery**: Handles device errors gracefully

## Status

- ✅ CPAL Integration: Cross-platform audio I/O working
- ✅ Buffer Adaptation: Automatic size matching implemented
- ✅ Channel Routing: Flexible routing system complete
- ✅ Error Recovery: Robust error handling in place
- 📋 Performance: Latency optimization ongoing

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
| [auxide](https://github.com/Michael-A-Kuykendall/auxide) | Real-time-safe audio graph kernel | 0.3.1 |
| [auxide-dsp](https://github.com/Michael-A-Kuykendall/auxide-dsp) | DSP nodes library | 0.2.0 |
| **[auxide-io](https://github.com/Michael-A-Kuykendall/auxide-io)** | Audio I/O layer | 0.1.2 |
| [auxide-midi](https://github.com/Michael-A-Kuykendall/auxide-midi) | MIDI integration | 0.1.1 |
