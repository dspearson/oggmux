use anyhow::Result;
use bytes::Bytes;
use oggmux::{BufferConfig, OggMux, VorbisConfig};
use std::time::{Duration, Instant};
use tokio::time::sleep;

/// Helper function to create a new OggMux instance with custom buffer settings
fn create_test_mux() -> OggMux {
    OggMux::new().with_buffer_config(BufferConfig {
        target_buffered_secs: 0.5, // Small target for quicker testing
        max_buffer_secs: 1.0,
        max_chunk_size: 4096,
    })
}

/// Helper function to get a fresh copy of the silence.ogg data
fn get_silence_ogg() -> Bytes {
    Bytes::from(include_bytes!("../resources/silence.ogg").to_vec())
}

/// Helper function to check if a byte slice contains Ogg page signatures
fn contains_ogg_signatures(data: &[u8]) -> bool {
    data.windows(4).any(|window| window == b"OggS")
}

/// Basic test that the OggMux can be constructed and configured
#[tokio::test]
async fn test_oggmux_construction() {
    // Create with default settings
    let mux = OggMux::new();
    let (tx, _rx) = mux.spawn();
    drop(tx); // Close the channel to allow the mux task to complete

    // Create with custom buffer settings
    let mux = OggMux::new().with_buffer_config(BufferConfig {
        target_buffered_secs: 5.0,
        max_buffer_secs: 8.0,
        max_chunk_size: 8192,
    });
    let (tx, _rx) = mux.spawn();
    drop(tx);

    // Create with custom Vorbis settings
    let mux = OggMux::new().with_vorbis_config(VorbisConfig {
        sample_rate: 48000,
        bitrate_bps: 192_000.0,
    });
    let (tx, _rx) = mux.spawn();
    drop(tx);

    // Create with both custom settings
    let mux = OggMux::new()
        .with_buffer_config(BufferConfig {
            target_buffered_secs: 3.0,
            max_buffer_secs: 6.0,
            max_chunk_size: 4096,
        })
        .with_vorbis_config(VorbisConfig {
            sample_rate: 22050,
            bitrate_bps: 64_000.0,
        });
    let (tx, _rx) = mux.spawn();
    drop(tx);
}

/// Test that the OggMux produces silence when no input is provided
#[tokio::test]
async fn test_silence_generation() -> Result<()> {
    let mux = create_test_mux();
    let (tx, mut rx) = mux.spawn();

    // Wait a short time to allow silence generation
    sleep(Duration::from_millis(200)).await;

    // Collect output for a short period
    let start = Instant::now();
    let mut received_packets = 0;
    let mut total_bytes = 0;

    while start.elapsed() < Duration::from_millis(1000) {
        tokio::select! {
            Some(packet) = rx.recv() => {
                received_packets += 1;
                total_bytes += packet.len();

                // Basic validation: check for "OggS" signature
                assert!(contains_ogg_signatures(&packet));
            }
            _ = sleep(Duration::from_millis(50)) => {}
        }

        // Stop if we've received enough data
        if received_packets >= 3 {
            break;
        }
    }

    // We should have received at least one packet of silence
    assert!(received_packets > 0, "No silence packets received");
    assert!(total_bytes > 0, "No bytes received");

    // Close the channel
    drop(tx);
    Ok(())
}

/// Test muxing with Ogg data
#[tokio::test]
async fn test_with_ogg_data() -> Result<()> {
    let mux = create_test_mux();
    let (tx, mut rx) = mux.spawn();

    // Send silence.ogg data
    tx.send(get_silence_ogg()).await?;

    // Collect output for a short time
    let start = Instant::now();
    let mut received_packets = 0;
    let mut received_data = Vec::<u8>::new();

    while start.elapsed() < Duration::from_millis(1000) {
        tokio::select! {
            Some(packet) = rx.recv() => {
                received_packets += 1;
                received_data.extend_from_slice(&packet);
            }
            _ = sleep(Duration::from_millis(50)) => {}
        }

        // Stop if we've received enough data
        if received_packets > 5 {
            break;
        }
    }

    // We should have received at least one packet
    assert!(received_packets > 0, "No packets received");
    assert!(!received_data.is_empty(), "No data received");

    // The output should contain the Ogg page signature
    assert!(
        contains_ogg_signatures(&received_data),
        "Output does not contain Ogg pages"
    );

    // Close the channel
    drop(tx);
    Ok(())
}

/// Test multiple pushes of audio data
#[tokio::test]
async fn test_multiple_pushes() -> Result<()> {
    let mux = create_test_mux();
    let (tx, mut rx) = mux.spawn();

    // Push silence.ogg multiple times with small delays
    for i in 0..3 {
        tx.send(get_silence_ogg()).await?;
        println!("Pushed silence chunk {}", i + 1);

        // Small delay between pushes
        sleep(Duration::from_millis(100)).await;
    }

    // Collect output for a short time
    let start = Instant::now();
    let mut received_packets = 0;
    let mut total_bytes = 0;

    while start.elapsed() < Duration::from_millis(1500) {
        tokio::select! {
            Some(packet) = rx.recv() => {
                received_packets += 1;
                total_bytes += packet.len();
            }
            _ = sleep(Duration::from_millis(50)) => {}
        }

        // Stop if we've received enough packets
        if received_packets > 5 {
            break;
        }
    }

    // We should have received multiple packets
    assert!(
        received_packets > 1,
        "Not enough packets received: {}",
        received_packets
    );
    assert!(
        total_bytes > 0,
        "Not enough bytes received: {}",
        total_bytes
    );

    // Close the channel
    drop(tx);
    Ok(())
}

/// Test transitions between silence and audio
#[tokio::test]
async fn test_silence_to_audio_transition() -> Result<()> {
    let mux = create_test_mux();
    let (tx, mut rx) = mux.spawn();

    // Wait for silence to be generated
    sleep(Duration::from_millis(300)).await;

    // Now push real audio
    tx.send(get_silence_ogg()).await?;

    // Collect output to see the transition
    let start = Instant::now();
    let mut received_packets = 0;

    while start.elapsed() < Duration::from_millis(1000) {
        tokio::select! {
            Some(packet) = rx.recv() => {
                received_packets += 1;

                // Basic validation: check for "OggS" signature
                assert!(contains_ogg_signatures(&packet));
            }
            _ = sleep(Duration::from_millis(50)) => {}
        }

        // Stop if we've received enough data
        if received_packets >= 5 {
            break;
        }
    }

    // We should have received multiple packets
    assert!(
        received_packets > 1,
        "Not enough packets received: {}",
        received_packets
    );

    // Close the channel
    drop(tx);
    Ok(())
}

/// Test rapid alternating between silence and audio
#[tokio::test]
async fn test_alternating_silence_and_audio() -> Result<()> {
    let mux = create_test_mux();
    let (tx, mut rx) = mux.spawn();

    // Alternating pattern: wait for silence, then push audio
    for _ in 0..3 {
        // Wait for silence to be generated
        sleep(Duration::from_millis(300)).await;

        // Push real audio
        tx.send(get_silence_ogg()).await?;

        // Collect some packets
        for _ in 0..2 {
            if let Some(packet) = rx.recv().await {
                // Basic validation: check for "OggS" signature
                assert!(contains_ogg_signatures(&packet));
            }
        }
    }

    // Close the channel
    drop(tx);
    Ok(())
}

/// Test with invalid Ogg data (the muxer should handle this gracefully)
#[tokio::test]
async fn test_invalid_ogg_data() -> Result<()> {
    // Create some invalid data (not a valid Ogg stream)
    let invalid_data = Bytes::from(b"This is not a valid Ogg stream".to_vec());

    let mux = create_test_mux();
    let (tx, mut rx) = mux.spawn();

    // Push the invalid data
    tx.send(invalid_data).await?;

    // Wait a bit to allow processing
    sleep(Duration::from_millis(500)).await;

    // We should still get silence output despite the invalid input
    let mut received_packets = 0;
    let start = Instant::now();

    while start.elapsed() < Duration::from_millis(1000) {
        tokio::select! {
            Some(packet) = rx.recv() => {
                received_packets += 1;

                // Basic validation: check for "OggS" signature
                assert!(contains_ogg_signatures(&packet));
            }
            _ = sleep(Duration::from_millis(50)) => {}
        }

        if received_packets >= 3 {
            break;
        }
    }

    // We should have received at least one packet (silence generation)
    assert!(
        received_packets > 0,
        "No packets received after invalid input"
    );

    // Close the channel
    drop(tx);
    Ok(())
}

/// Test with truncated Ogg data
#[tokio::test]
async fn test_truncated_ogg_data() -> Result<()> {
    // Get the silence.ogg data and truncate it
    let mut truncated_ogg = include_bytes!("../resources/silence.ogg").to_vec();
    truncated_ogg.truncate(truncated_ogg.len() / 2);

    let mux = create_test_mux();
    let (tx, mut rx) = mux.spawn();

    // Push the truncated data
    tx.send(Bytes::from(truncated_ogg)).await?;

    // Wait a bit to allow processing
    sleep(Duration::from_millis(500)).await;

    // We should still get output
    let mut received_packets = 0;
    let start = Instant::now();

    while start.elapsed() < Duration::from_millis(1000) {
        tokio::select! {
            Some(packet) = rx.recv() => {
                received_packets += 1;

                // Basic validation: check for "OggS" signature
                assert!(contains_ogg_signatures(&packet));
            }
            _ = sleep(Duration::from_millis(50)) => {}
        }

        if received_packets >= 3 {
            break;
        }
    }

    // We should have received at least one packet
    assert!(
        received_packets > 0,
        "No packets received after truncated input"
    );

    // Close the channel
    drop(tx);
    Ok(())
}

/// Test large buffer handling
#[tokio::test]
async fn test_large_buffer() -> Result<()> {
    let mux = OggMux::new().with_buffer_config(BufferConfig {
        target_buffered_secs: 5.0,
        max_buffer_secs: 10.0,
        max_chunk_size: 65536,
    });

    let (tx, mut rx) = mux.spawn();

    // Push multiple copies to fill the buffer
    for _ in 0..10 {
        tx.send(get_silence_ogg()).await?;
    }

    // Collect output for a longer time
    let start = Instant::now();
    let mut received_packets = 0;
    let mut total_bytes = 0;

    while start.elapsed() < Duration::from_millis(2000) {
        tokio::select! {
            Some(packet) = rx.recv() => {
                received_packets += 1;
                total_bytes += packet.len();
            }
            _ = sleep(Duration::from_millis(100)) => {}
        }

        // Stop if we've received enough data
        if received_packets >= 10 {
            break;
        }
    }

    // We should have received multiple packets
    assert!(
        received_packets > 1,
        "Not enough packets received: {}",
        received_packets
    );
    assert!(
        total_bytes > 0,
        "Not enough bytes received: {}",
        total_bytes
    );

    // Close the channel
    drop(tx);
    Ok(())
}

/// Test with a large number of small chunks
#[tokio::test]
async fn test_many_small_chunks() -> Result<()> {
    // Get the silence.ogg data and split it into small chunks
    let silence_ogg = include_bytes!("../resources/silence.ogg").to_vec();
    let chunk_size = 64; // Small chunk size
    let chunks: Vec<_> = silence_ogg
        .chunks(chunk_size)
        .map(|c| Bytes::from(c.to_vec()))
        .collect();

    let mux = create_test_mux();
    let (tx, mut rx) = mux.spawn();

    // Push all the small chunks
    for chunk in chunks {
        tx.send(chunk).await?;

        // Small delay to avoid overwhelming the channel
        sleep(Duration::from_millis(10)).await;
    }

    // Collect output
    let start = Instant::now();
    let mut received_packets = 0;
    let mut total_bytes = 0;

    while start.elapsed() < Duration::from_millis(1500) {
        tokio::select! {
            Some(packet) = rx.recv() => {
                received_packets += 1;
                total_bytes += packet.len();
            }
            _ = sleep(Duration::from_millis(50)) => {}
        }

        if received_packets >= 5 {
            break;
        }
    }

    // We should have received some packets
    assert!(received_packets > 0, "No packets received");
    assert!(total_bytes > 0, "No bytes received");

    // Close the channel
    drop(tx);
    Ok(())
}
