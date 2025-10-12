# OggMux

A Rust library for muxing Ogg streams with clean silence gaps, suitable for streaming applications like Icecast.

## Features

- **Continuous streaming**: Automatically inserts silence when audio input is not available
- **Clean transitions**: Manages transitions between real audio data and silence
- **Time-accurate**: Maintains proper timing across stream transitions
- **Buffering**: Configurable buffering to handle network jitter
- **Async-first**: Built with Tokio for high-performance asynchronous operation
- **Robust error handling**: Uses anyhow for rich error context and graceful recovery
- **Helper utilities**: Functions for calculating optimal buffer sizes based on bitrate and latency requirements
- **Stream metrics**: Collection and monitoring of stream health statistics

### Basic Example

```rust
use oggmux::{OggMux, BufferConfig, VorbisConfig, VorbisBitrateMode};
use bytes::Bytes;
use tokio::time::sleep;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create a new OggMux with custom configuration
    let mux = OggMux::new()
        .with_buffer_config(BufferConfig {
            buffered_seconds: 10.0,
            channel_capacity: 4096,
        })
        .with_vorbis_config(VorbisConfig {
            sample_rate: 44100,
            bitrate: VorbisBitrateMode::CBR(320),
        });

    // Spawn the muxer and get the channels
    let (input_tx, mut output_rx, shutdown_tx, handle) = mux.spawn();

    // Feed some data (e.g., from a file or network stream)
    let ogg_data = Bytes::from_static(&[/* Ogg data here */]);
    let _ = input_tx.send(ogg_data).await;

    // Read the processed output (e.g., send to Icecast)
    tokio::spawn(async move {
        while let Some(output) = output_rx.recv().await {
            // Send output to your streaming destination
            println!("Got {} bytes of muxed output", output.len());
        }
    });

    // ... do work ...

    // Gracefully shut down when done
    let _ = shutdown_tx.send(()).await;
    handle.await??;  // Wait for clean exit and check for errors

    Ok(())
}
```

## Configuration

### Buffer Configuration

```rust
BufferConfig {
    buffered_seconds: 10.0,    // Target amount of audio to keep buffered
    channel_capacity: 65536,   // Maximum number of chunks buffered in channel
}
```

### Vorbis Configuration

You can configure the vorbis stream in two ways:

#### Constant Bitrate (CBR)

```rust
VorbisConfig {
    sample_rate: 44100,                   // Sample rate in Hz
    bitrate: VorbisBitrateMode::CBR(320), // 320 kbps constant bitrate
}
```

#### Variable Bitrate (VBR)

```rust
VorbisConfig {
    sample_rate: 44100,                        // Sample rate in Hz
    bitrate: VorbisBitrateMode::VBRQuality(6), // Quality level 6 VBR
}
```

## How It Works

OggMux maintains a continuous flow of Ogg data by:

1. Processing incoming audio data when available
2. Inserting silence data when input is empty
3. Managing Ogg page boundaries and granule positions
4. Ensuring proper time synchronisation across stream transitions

This approach is ideal for applications where a continuous stream must be maintained despite irregular input data, such as live streaming or internet radio.

## Requirements

- Rust 2024 edition or later
- Tokio runtime

## Utility Functions

OggMux provides helper functions for calculating optimal buffer sizes:

```rust
use oggmux::{calculate_buffer_size, calculate_buffered_seconds};

// Calculate ideal buffer size based on bitrate and target latency
let buffer_size = calculate_buffer_size(192, 500); // 192kbps, 500ms latency -> 16384 bytes

// Calculate how many seconds of audio a buffer can hold
let seconds = calculate_buffered_seconds(192, 16384); // ~0.68 seconds at 192kbps
```

These utilities help you configure your streaming application for optimal performance.

## Stream Metrics

OggMux can collect metrics about the streaming process, which is helpful for monitoring and debugging:

```rust
use oggmux::{OggMux, MetricsCollector};

// Enable metrics collection
let mux = OggMux::new().with_metrics();

// Get the metrics collector before spawning
let metrics = mux.metrics();

// Spawn the muxer
let (input_tx, output_rx, shutdown_tx, handle) = mux.spawn();

// Use the metrics collector
if let Some(metrics) = metrics {
    // Later, retrieve metrics:
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            
            // Get a snapshot of current metrics
            let snapshot = metrics.snapshot().await;
            
            println!("Stream stats:");
            println!("  Bytes processed: {}", snapshot.bytes_processed);
            println!("  Silence insertions: {}", snapshot.silence_insertions);
            println!("  Silence percentage: {:.1}%", snapshot.silence_percentage());
            println!("  Buffer utilization: {:.1}%", snapshot.buffer_utilization.last);
            println!("  Processing latency: {:.2}ms avg, {:.2}ms max", 
                     snapshot.processing_latency_ms.avg,
                     snapshot.processing_latency_ms.max);
        }
    });
}
```

Available metrics include:
- Bytes processed and silence inserted
- Number of silence insertions and real streams
- Buffer utilization percentage
- Processing latency statistics
- Uptime and stream health indicators

## Limitations

- Currently only supports Ogg Vorbis format
- Silence data must have the same format as the main audio stream
- Best suited for streaming applications rather than file manipulation
