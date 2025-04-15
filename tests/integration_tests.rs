use anyhow::Result;
use bytes::Bytes;
use oggmux::{BufferConfig, OggMux, VorbisConfig};
use std::time::{Duration, Instant};
use tokio::time::sleep;

/// Helper function to create a new OggMux instance with custom buffer settings
fn create_test_mux() -> OggMux {
    OggMux::new().with_buffer_config(BufferConfig {
        buffered_seconds: 0.5, // Small target for quicker testing
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
        buffered_seconds: 5.0,
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
            buffered_seconds: 3.0,
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

#[tokio::test]
async fn test_invalid_ogg_data() -> Result<()> {
    // Create some invalid data (not a valid Ogg stream)
    let invalid_data = Bytes::from(b"This is not a valid Ogg stream".to_vec());

    let mux = create_test_mux();
    let (tx, mut rx) = mux.spawn();

    // Push the invalid data
    tx.send(invalid_data).await?;

    // Wait a bit longer to allow the timeout mechanism to trigger
    sleep(Duration::from_millis(1000)).await;

    // We should still get silence output despite the invalid input
    let mut received_packets = 0;
    let start = Instant::now();

    while start.elapsed() < Duration::from_millis(2000) {
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
        buffered_seconds: 5.0,
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

/// Test recovery after invalid data
/// This test ensures the muxer can process valid data after receiving invalid data
#[tokio::test]
async fn test_recovery_after_invalid_data() -> Result<()> {
    let mux = create_test_mux();
    let (tx, mut rx) = mux.spawn();

    // First send invalid data
    let invalid_data = Bytes::from(b"This is not a valid Ogg stream".to_vec());
    tx.send(invalid_data).await?;

    // Wait for the timeout to trigger
    sleep(Duration::from_millis(1000)).await;

    // Now send valid Ogg data
    tx.send(get_silence_ogg()).await?;

    // Collect output to verify we receive valid Ogg pages
    let start = Instant::now();
    let mut received_packets = 0;
    let mut received_valid_data = false;

    while start.elapsed() < Duration::from_millis(2000) {
        tokio::select! {
            Some(packet) = rx.recv() => {
                received_packets += 1;
                if contains_ogg_signatures(&packet) {
                    received_valid_data = true;
                }
            }
            _ = sleep(Duration::from_millis(50)) => {}
        }

        if received_packets >= 5 && received_valid_data {
            break;
        }
    }

    assert!(
        received_valid_data,
        "No valid Ogg data received after recovery"
    );
    assert!(received_packets > 0, "No packets received after recovery");

    drop(tx);
    Ok(())
}

/// Test with extreme buffer configuration values
#[tokio::test]
async fn test_extreme_buffer_configuration() -> Result<()> {
    // Very small buffer
    let small_buffer_mux = OggMux::new().with_buffer_config(BufferConfig {
        buffered_seconds: 0.1,
        max_chunk_size: 1024,
    });

    let (small_tx, mut small_rx) = small_buffer_mux.spawn();
    small_tx.send(get_silence_ogg()).await?;

    // Very large buffer
    let large_buffer_mux = OggMux::new().with_buffer_config(BufferConfig {
        buffered_seconds: 30.0,
        max_chunk_size: 1_048_576,
    });

    let (large_tx, mut large_rx) = large_buffer_mux.spawn();
    large_tx.send(get_silence_ogg()).await?;

    // Verify both produce output
    let mut small_buffer_received = false;
    let mut large_buffer_received = false;

    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(2000) {
        tokio::select! {
            Some(packet) = small_rx.recv() => {
                assert!(contains_ogg_signatures(&packet));
                small_buffer_received = true;
            }
            Some(packet) = large_rx.recv() => {
                assert!(contains_ogg_signatures(&packet));
                large_buffer_received = true;
            }
            _ = sleep(Duration::from_millis(50)) => {}
        }

        if small_buffer_received && large_buffer_received {
            break;
        }
    }

    assert!(
        small_buffer_received,
        "Small buffer configuration did not produce output"
    );
    assert!(
        large_buffer_received,
        "Large buffer configuration did not produce output"
    );

    drop(small_tx);
    drop(large_tx);
    Ok(())
}

/// Test with various sample rates and bitrates
#[tokio::test]
async fn test_various_audio_configurations() -> Result<()> {
    // Test with different sample rates and bitrates
    let configurations = vec![
        VorbisConfig {
            sample_rate: 8000,
            bitrate_bps: 64_000.0,
        },
        VorbisConfig {
            sample_rate: 16000,
            bitrate_bps: 96_000.0,
        },
        VorbisConfig {
            sample_rate: 48000,
            bitrate_bps: 256_000.0,
        },
    ];

    for config in configurations {
        let mux = OggMux::new().with_vorbis_config(config);
        let (tx, mut rx) = mux.spawn();

        // Send valid data
        tx.send(get_silence_ogg()).await?;

        // Verify we receive output
        let mut received_output = false;
        let start = Instant::now();

        while start.elapsed() < Duration::from_millis(1000) {
            tokio::select! {
                Some(packet) = rx.recv() => {
                    assert!(contains_ogg_signatures(&packet));
                    received_output = true;
                    break;
                }
                _ = sleep(Duration::from_millis(50)) => {}
            }
        }

        assert!(
            received_output,
            "No output received for sample_rate={}, bitrate={}",
            config.sample_rate, config.bitrate_bps
        );

        drop(tx);
    }

    Ok(())
}

/// Test concurrent inputs from multiple producers
#[tokio::test]
async fn test_concurrent_producers() -> Result<()> {
    let mux = create_test_mux();
    let (tx, mut rx) = mux.spawn();

    // Create multiple producer tasks
    let tx1 = tx.clone();
    let tx2 = tx.clone();

    // Producer 1: Send data every 300ms
    let producer1 = tokio::spawn(async move {
        for _ in 0..3 {
            tx1.send(get_silence_ogg()).await.unwrap();
            sleep(Duration::from_millis(300)).await;
        }
    });

    // Producer 2: Send data every 500ms
    let producer2 = tokio::spawn(async move {
        sleep(Duration::from_millis(150)).await; // Offset the timing
        for _ in 0..2 {
            tx2.send(get_silence_ogg()).await.unwrap();
            sleep(Duration::from_millis(500)).await;
        }
    });

    // Collect and verify output
    let mut received_packets = 0;
    let start = Instant::now();

    while start.elapsed() < Duration::from_millis(2000) {
        tokio::select! {
            Some(packet) = rx.recv() => {
                assert!(contains_ogg_signatures(&packet));
                received_packets += 1;
            }
            _ = sleep(Duration::from_millis(50)) => {}
        }

        if received_packets >= 10 {
            break;
        }
    }

    // Wait for producers to complete
    let _ = tokio::join!(producer1, producer2);

    assert!(
        received_packets > 0,
        "No packets received from concurrent producers"
    );

    drop(tx);
    Ok(())
}

/// Test long-running stability (this is a longer test)
#[tokio::test]
async fn test_long_running_stability() -> Result<()> {
    // This test will run for a longer time to ensure stability
    let test_duration = Duration::from_secs(6); // Extended to 6 seconds

    // Use a mux with slightly faster output for testing
    let mux = OggMux::new().with_buffer_config(BufferConfig {
        buffered_seconds: 0.3, // Smaller buffer for more frequent output
        max_chunk_size: 4096,
    });

    let (tx, mut rx) = mux.spawn();

    // Simulate intermittent data with periods of silence
    let producer = tokio::spawn(async move {
        for i in 0..5 {
            // Send data
            tx.send(get_silence_ogg()).await.unwrap();

            // Wait a varying amount of time
            let wait_time = match i % 3 {
                0 => 200, // Shorter waits
                1 => 500,
                _ => 800,
            };

            sleep(Duration::from_millis(wait_time)).await;
        }

        // Keep the channel open for the rest of the test
        sleep(Duration::from_secs(4)).await;
        tx
    });

    // Give a moment for the muxer to start up
    sleep(Duration::from_millis(100)).await;

    // Collect output for the full test duration
    let start = Instant::now();
    let mut received_packets = 0;
    let mut last_packet_time = Instant::now();
    let mut max_gap_ms = 0;

    while start.elapsed() < test_duration {
        tokio::select! {
            Some(packet) = rx.recv() => {
                assert!(contains_ogg_signatures(&packet));

                // Track the timing gap between packets
                let gap = last_packet_time.elapsed().as_millis();
                if gap > max_gap_ms {
                    max_gap_ms = gap;
                }

                last_packet_time = Instant::now();
                received_packets += 1;

                // Debug: Print when packets are received
                println!("Received packet #{}, size: {} bytes", received_packets, packet.len());
            }
            _ = sleep(Duration::from_millis(50)) => {} // Shorter polling interval
        }
    }

    // Recover the channel and close it
    let tx = producer.await?;
    drop(tx);

    println!("Total packets received: {}", received_packets);
    println!("Maximum gap between packets: {}ms", max_gap_ms);

    // With our configuration, we should get at least 5 packets
    // (More conservative than before)
    assert!(
        received_packets >= 5,
        "Not enough packets received in long running test"
    );

    // The maximum gap between packets should be reasonable
    // If silence is properly generated, we should get regular packets
    assert!(
        max_gap_ms < 2000, // More forgiving gap check
        "Gap between packets too large: {}ms",
        max_gap_ms
    );

    Ok(())
}

/// Test error propagation for a channel that's closed while processing
#[tokio::test]
async fn test_channel_close_during_processing() -> Result<()> {
    let mux = create_test_mux();
    let (tx, rx) = mux.spawn();

    // Send some valid data
    tx.send(get_silence_ogg()).await?;

    // Immediately drop both channels
    drop(tx);
    drop(rx);

    // Wait a bit to allow the background task to detect and handle the closure
    sleep(Duration::from_millis(500)).await;

    // This test passes if we don't panic or crash
    // The background task should handle the channel closure gracefully

    Ok(())
}

/// Test with empty Ogg data
#[tokio::test]
async fn test_empty_data() -> Result<()> {
    let mux = create_test_mux();
    let (tx, mut rx) = mux.spawn();

    // Send an empty buffer
    tx.send(Bytes::new()).await?;

    // Wait a bit
    sleep(Duration::from_millis(500)).await;

    // We should still get silence output
    let mut received_packets = 0;
    let start = Instant::now();

    while start.elapsed() < Duration::from_millis(1000) {
        tokio::select! {
            Some(packet) = rx.recv() => {
                received_packets += 1;
                assert!(contains_ogg_signatures(&packet));
            }
            _ = sleep(Duration::from_millis(50)) => {}
        }

        if received_packets >= 3 {
            break;
        }
    }

    assert!(received_packets > 0, "No packets received after empty data");

    drop(tx);
    Ok(())
}
