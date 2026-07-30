# Changelog

## [0.1.3] - 2026-07-29
- **Glitch detection**: Added glitch_count to Diagnostics for real-time buffer underflow tracking
- **BufferSizeAdapter**: Added with_diagnostics method; all stream paths (output_core, play_duplex, restart) wire shared glitch counting
- **Ecosystem docs**: Updated AGENTS.md with full 7-crate Auxide ecosystem overview

## [0.1.2] - 2026-01-07
- **Documentation improvements**: Complete crate-level and module documentation
- **Error handling enhancements**: Improved error messages for better debugging
- **Auxide 0.2.1 compatibility**: Updated for latest auxide kernel
- **RT safety verification**: Confirmed lock-free and allocation-free audio paths
- **Testing**: All unit tests passing with enhanced coverage

**Requires**: [auxide >= 0.2.1](https://github.com/Michael-A-Kuykendall/auxide/releases/tag/v0.2.1)  
**Compatible with**: [auxide-dsp 0.1.1](https://github.com/Michael-A-Kuykendall/auxide-dsp/releases/tag/v0.1.1), [auxide-midi 0.1.1](https://github.com/Michael-A-Kuykendall/auxide-midi/releases/tag/v0.1.1)

## [0.2.0] - 2026-01-05
- **RT-safety fix** - Removed RT-unsafe println! from audio callback path
- **Auxide 0.3.0 compatibility** - Updated for compatibility with latest auxide kernel changes
- **Improved error handling** - Enhanced error recovery and state management
- **Example improvements** - Renamed mixer.rs to audio_mixer.rs for clarity

## [0.1.1] - 2026-01-03
- **RT-safety fix** - Removed RT-unsafe println! from audio callback path
- **Auxide 0.2.0 compatibility** - Updated for compatibility with latest auxide kernel changes
- **Improved error handling** - Enhanced error recovery and state management

## [0.1.0] - 2026-01-03
- **Initial release** of auxide-io, the RT-safe audio I/O layer for Auxide.
- **CPAL integration**: Cross-platform audio streaming with hardware device enumeration.
- **Buffer adaptation**: Ring buffer system handles size mismatches between Auxide's fixed blocks (512 samples) and variable host buffers (up to 16384 samples).
- **Channel routing**: Mono-to-stereo duplication with bit-exact copying.
- **RT safety**: No allocations, locks, or I/O in audio callbacks. Atomic state management only.
- **Error recovery**: Graceful failure with silence on stream errors, no panics in RT path.
- **Stream control**: Start/stop/pause audio streams with thread-safe atomic state.
- **Lock-free verification**: Platform atomic support checking at initialization.
- **Examples**: `play_tone.rs` and `play_sine.rs` demonstrate real-time audio output.
- **Comprehensive testing**: Unit tests cover all components, fuzz testing for buffer adaptation.
<parameter name="filePath">c:/Users/micha/repos/auxide/CHANGELOG.md