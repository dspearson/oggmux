# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2025-10-11

### Added
- **Graceful shutdown mechanism**: `spawn()` now returns a shutdown channel to cleanly terminate the muxer
- **Error exposure**: `spawn()` now returns `JoinHandle<Result<()>>` to detect and handle muxer errors
- **Backpressure handling**: 5-second timeout on all output sends prevents indefinite blocking when consumer is slow
- **Configuration validation**: Added validation to `BufferConfig` and `VorbisConfig` to prevent runtime panics
  - Validates `buffered_seconds > 0`
  - Validates `channel_capacity > 0`
  - Validates `sample_rate > 0` (prevents division by zero)
  - Validates `CBR bitrate > 0`
- **Warning logs**: Added warning when falling back to default silence template
- **Documentation**: Added detailed comments explaining buffer overflow threshold (1MB safety valve)
- **New test**: Added `test_mux_graceful_shutdown()` to verify shutdown behavior

### Changed
- **BREAKING**: `spawn()` signature changed from returning `(Sender, Receiver)` to `(Sender, Receiver, Sender<()>, JoinHandle<Result<()>>)`
  - Users must now destructure 4 values instead of 2
  - See migration guide below
- **BREAKING**: Renamed `BufferConfig::max_chunk_size` to `channel_capacity` for API clarity
  - Old name suggested chunk size but actually controlled channel buffer capacity
- **BREAKING**: Output sends now timeout after 5 seconds instead of blocking indefinitely
  - Logs warning and drops packet on timeout
  - Continues processing (more resilient to slow consumers)
- Updated all examples in README, lib.rs, and tests to use new API

### Removed
- Removed duplicate `spawn()` docstring

### Fixed
- Fixed misleading field naming (`max_chunk_size` → `channel_capacity`)
- Improved error messages for configuration validation
- Better handling of slow/stuck output consumers

### Migration Guide (0.1.x → 0.2.0)

**Old code (0.1.x):**
```rust
let mux = OggMux::new().with_buffer_config(BufferConfig {
    buffered_seconds: 10.0,
    max_chunk_size: 4096,  // OLD NAME
});
let (input_tx, output_rx) = mux.spawn();  // 2 values
```

**New code (0.2.0):**
```rust
let mux = OggMux::new().with_buffer_config(BufferConfig {
    buffered_seconds: 10.0,
    channel_capacity: 4096,  // NEW NAME
});
let (input_tx, output_rx, shutdown_tx, handle) = mux.spawn();  // 4 values

// ... use the muxer ...

// Gracefully shut down
let _ = shutdown_tx.send(()).await;
handle.await??;  // Check for errors
```

## [0.1.2] - 2025-10-11

### Added
- Metrics module for monitoring stream health and performance
- `MetricsCollector` with statistics tracking:
  - Bytes processed and silence inserted
  - Silence insertions count
  - Real streams processed
  - Buffer utilization percentage
  - Processing latency statistics
- Utility functions: `calculate_buffer_size()` and `calculate_buffered_seconds()`
- `with_metrics()` builder method to enable metrics collection

### Changed
- Improved error handling with better context messages
- Enhanced error propagation throughout the codebase

### Removed
- Removed redundant tests that provided no value

## [0.1.1] - 2024-04-23

### Fixed
- Corrected handling of granule positions in Ogg headers
- Improved granule position tracking across stream boundaries

## [0.1.0] - 2024-04-17

### Added
- Initial release of OggMux
- Automatic silence insertion when audio input unavailable
- Clean transitions between real audio and silence
- Time-accurate stream management
- Configurable buffering for network jitter handling
- Async-first design with Tokio
- Support for multiple silence templates (44.1kHz and 48kHz)
- Support for both CBR and VBR quality modes
- Comprehensive Ogg Vorbis stream muxing
- CI pipeline with GitHub Actions

[Unreleased]: https://github.com/dspearson/oggmux/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/dspearson/oggmux/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/dspearson/oggmux/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/dspearson/oggmux/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/dspearson/oggmux/releases/tag/v0.1.0
