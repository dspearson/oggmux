# OggMux

A Rust library for muxing Ogg streams with clean silence gaps, suitable for streaming applications like Icecast.

## Features

- **Continuous streaming**: Automatically inserts silence when audio input is not available
- **Clean transitions**: Manages transitions between real audio data and silence
- **Time-accurate**: Maintains proper timing across stream transitions
- **Buffering**: Configurable buffering to handle network jitter
- **Async-first**: Built with Tokio for high-performance asynchronous operation
- **Robust error handling**: Uses anyhow for rich error context and graceful recovery

### Basic Example

```rust
use oggmux::{OggMux, BufferConfig, VorbisConfig, VorbisBitrateMode};
use bytes::Bytes;
use tokio::time::sleep;
use std::time::Duration;

#[tokio::main]
async fn main() {
    // Create a new OggMux with custom configuration
    let mux = OggMux::new()
        .with_buffer_config(BufferConfig {
            buffered_seconds: 10.0,
            max_chunk_size: 4096,
        })
        .with_vorbis_config(VorbisConfig {
            sample_rate: 44100,
            bitrate: VorbisBitrateMode::CBR(320),
        });

    // Spawn the muxer and get the channels
    let (input_tx, mut output_rx) = mux.spawn();

    // Feed some data (e.g., from a file or network stream)
    let ogg_data = Bytes::from_static(&[/* Ogg data here */]);
    let _ = input_tx.send(ogg_data).await;

    // Read the processed output (e.g., send to Icecast)
    while let Some(output) = output_rx.recv().await {
        // Send output to your streaming destination
        println!("Got {} bytes of muxed output", output.len());
    }
}
```

## Configuration

### Buffer Configuration

```rust
BufferConfig {
    buffered_seconds: 10.0, // Target amount of audio to keep buffered
    max_chunk_size: 65536,  // Maximum chunk size to process at once
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

- Rust 2021 edition or later
- Tokio runtime

## Limitations

- Currently only supports Ogg Vorbis format
- Silence data must have the same format as the main audio stream
- Best suited for streaming applications rather than file manipulation
