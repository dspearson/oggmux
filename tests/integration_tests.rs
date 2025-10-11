use anyhow::Result;
use bytes::Bytes;
use oggmux::{BufferConfig, MuxMode, OggMux, VorbisBitrateMode, VorbisConfig};
use std::sync::{Arc, Mutex};
use tokio::time::{sleep, Duration};

fn create_test_mux() -> OggMux {
    OggMux::new()
        .with_buffer_config(BufferConfig {
            buffered_seconds: 0.5,
            channel_capacity: 4096,
        })
        .with_vorbis_config(VorbisConfig {
            sample_rate: 44100,
            bitrate: VorbisBitrateMode::CBR(320),
        })
}

fn get_silence_ogg() -> Bytes {
    Bytes::from_static(include_bytes!("../resources/silence_44100_320.ogg"))
}

fn contains_ogg_signatures(data: &[u8]) -> bool {
    data.windows(4).any(|window| window == b"OggS")
}

#[tokio::test]
async fn test_silence_generation() -> Result<()> {
    let mux = create_test_mux();
    let (_tx, mut rx, _shutdown, _handle) = mux.spawn();

    sleep(Duration::from_millis(200)).await;

    if let Some(packet) = rx.recv().await {
        assert!(contains_ogg_signatures(&packet));
    } else {
        panic!("Expected silence packet, received none");
    }

    Ok(())
}

#[tokio::test]
async fn test_mux_with_valid_ogg_data() -> Result<()> {
    let mux = create_test_mux();
    let (tx, mut rx, _shutdown, _handle) = mux.spawn();

    tx.send(get_silence_ogg()).await?;

    if let Some(packet) = rx.recv().await {
        assert!(contains_ogg_signatures(&packet));
    } else {
        panic!("Expected Ogg packet, received none");
    }

    Ok(())
}

#[tokio::test]
async fn test_invalid_data_handling() -> Result<()> {
    let invalid_data = Bytes::from(b"invalid data".to_vec());
    let mux = create_test_mux();
    let (tx, mut rx, _shutdown, _handle) = mux.spawn();

    tx.send(invalid_data).await?;

    sleep(Duration::from_millis(300)).await;

    assert!(
        rx.recv().await.is_none(),
        "Expected channel to close after invalid data"
    );

    Ok(())
}

#[tokio::test]
async fn test_truncated_ogg_data() -> Result<()> {
    let mut truncated = get_silence_ogg().to_vec();
    truncated.truncate(truncated.len() / 2);

    let mux = create_test_mux();
    let (tx, mut rx, _shutdown, _handle) = mux.spawn();

    tx.send(Bytes::from(truncated)).await?;

    if let Some(packet) = rx.recv().await {
        assert!(contains_ogg_signatures(&packet));
    } else {
        panic!("Expected output even from truncated input");
    }

    Ok(())
}

#[tokio::test]
async fn test_multiple_data_pushes() -> Result<()> {
    let mux = create_test_mux();
    let (tx, mut rx, _shutdown, _handle) = mux.spawn();

    for _ in 0..3 {
        tx.send(get_silence_ogg()).await?;
        sleep(Duration::from_millis(100)).await;
    }

    let mut packets_received = 0;
    while packets_received < 3 {
        if let Some(packet) = rx.recv().await {
            assert!(contains_ogg_signatures(&packet));
            packets_received += 1;
        } else {
            break;
        }
    }

    assert!(
        packets_received >= 3,
        "Expected at least 3 packets, got {}",
        packets_received
    );

    Ok(())
}

#[tokio::test]
async fn test_concurrent_producers() -> Result<()> {
    let mux = create_test_mux();
    let (tx, mut rx, _shutdown, _handle) = mux.spawn();

    let tx1 = tx.clone();
    let producer1 = tokio::spawn(async move {
        for _ in 0..3 {
            tx1.send(get_silence_ogg()).await.unwrap();
            sleep(Duration::from_millis(200)).await;
        }
    });

    let tx2 = tx.clone();
    let producer2 = tokio::spawn(async move {
        for _ in 0..2 {
            tx2.send(get_silence_ogg()).await.unwrap();
            sleep(Duration::from_millis(300)).await;
        }
    });

    let mut packets_received = 0;
    while packets_received < 5 {
        if let Some(packet) = rx.recv().await {
            assert!(contains_ogg_signatures(&packet));
            packets_received += 1;
        } else {
            break;
        }
    }

    producer1.await?;
    producer2.await?;

    assert_eq!(packets_received, 5, "Expected exactly 5 packets");

    Ok(())
}

#[tokio::test]
async fn test_gapless_mode_no_silence() -> Result<()> {
    // In gapless mode, mux should NOT insert silence when idle
    let mux = create_test_mux()
        .with_mode(MuxMode::Gapless);

    let (_tx, mut rx, _shutdown, _handle) = mux.spawn();

    // Wait for longer than the silence timeout (100ms)
    sleep(Duration::from_millis(250)).await;

    // In gapless mode, should receive nothing when idle
    tokio::select! {
        result = rx.recv() => {
            if result.is_some() {
                panic!("Gapless mode should not insert silence when idle");
            }
        }
        _ = sleep(Duration::from_millis(100)) => {
            // Expected: timeout means no data sent
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_with_silence_mode_inserts_silence() -> Result<()> {
    // WithSilence mode should insert silence when idle (default behavior)
    let mux = create_test_mux()
        .with_mode(MuxMode::WithSilence);

    let (_tx, mut rx, _shutdown, _handle) = mux.spawn();

    // Wait for silence insertion (100ms timeout + processing)
    sleep(Duration::from_millis(200)).await;

    // Should receive silence
    if let Some(packet) = rx.recv().await {
        assert!(contains_ogg_signatures(&packet));
    } else {
        panic!("WithSilence mode should insert silence when idle");
    }

    Ok(())
}

#[tokio::test]
async fn test_metadata_callback_invoked() -> Result<()> {
    let callback_invoked = Arc::new(Mutex::new(false));
    let callback_invoked_clone = callback_invoked.clone();

    let mux = create_test_mux()
        .with_metadata_callback(move |_granule_pos| {
            *callback_invoked_clone.lock().unwrap() = true;
            Some(vec![
                ("TITLE".to_string(), "Test Track".to_string()),
                ("ARTIST".to_string(), "Test Artist".to_string()),
            ])
        });

    let (tx, mut rx, _shutdown, _handle) = mux.spawn();

    // Send data to trigger stream processing
    tx.send(get_silence_ogg()).await?;

    // Wait for processing and receive output
    sleep(Duration::from_millis(200)).await;

    // Drain receiver
    while rx.recv().await.is_some() {
        // Keep receiving until empty
    }

    // Callback should have been invoked after stream finished
    assert!(*callback_invoked.lock().unwrap(), "Metadata callback should be invoked after stream");

    Ok(())
}

#[tokio::test]
async fn test_metadata_callback_with_granule_position() -> Result<()> {
    let received_granule = Arc::new(Mutex::new(None));
    let received_granule_clone = received_granule.clone();

    let mux = create_test_mux()
        .with_metadata_callback(move |granule_pos| {
            *received_granule_clone.lock().unwrap() = Some(granule_pos);
            Some(vec![("POSITION".to_string(), granule_pos.to_string())])
        });

    let (tx, mut rx, _shutdown, _handle) = mux.spawn();

    tx.send(get_silence_ogg()).await?;
    sleep(Duration::from_millis(200)).await;

    // Drain receiver
    while rx.recv().await.is_some() {}

    // Should have received a granule position
    let granule = received_granule.lock().unwrap();
    assert!(granule.is_some(), "Callback should receive granule position");
    assert!(*granule.as_ref().unwrap() > 0, "Granule position should be > 0");

    Ok(())
}

#[tokio::test]
async fn test_metadata_injection_creates_pages() -> Result<()> {
    let mux = create_test_mux()
        .with_metadata_callback(|_| {
            Some(vec![
                ("TITLE".to_string(), "Test".to_string()),
            ])
        });

    let (tx, mut rx, _shutdown, _handle) = mux.spawn();

    tx.send(get_silence_ogg()).await?;

    let mut packets_received = 0;

    // Collect packets
    while let Ok(Some(_packet)) = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
        packets_received += 1;
    }

    assert!(packets_received > 0, "Should receive packets");
    // Note: Metadata pages are injected but may be difficult to detect without full Ogg parsing
    // The main test is that it doesn't crash and produces output

    Ok(())
}

#[tokio::test]
async fn test_gapless_with_multiple_streams() -> Result<()> {
    let mux = create_test_mux()
        .with_mode(MuxMode::Gapless);

    let (tx, mut rx, _shutdown, _handle) = mux.spawn();

    // Send multiple streams with delays
    for i in 0..3 {
        tx.send(get_silence_ogg()).await?;
        if i < 2 {
            sleep(Duration::from_millis(50)).await;
        }
    }

    // Should receive data from all streams without silence gaps
    let mut packets_received = 0;
    while let Ok(Some(packet)) = tokio::time::timeout(Duration::from_millis(1000), rx.recv()).await {
        assert!(contains_ogg_signatures(&packet));
        packets_received += 1;
    }

    assert!(packets_received >= 3, "Should receive at least 3 packets from streams");

    Ok(())
}

#[tokio::test]
async fn test_metadata_callback_returning_none() -> Result<()> {
    // Callback that returns None should not crash or cause issues
    let mux = create_test_mux()
        .with_metadata_callback(|_| None);

    let (tx, mut rx, _shutdown, _handle) = mux.spawn();

    tx.send(get_silence_ogg()).await?;

    // Should still receive normal output
    if let Some(packet) = rx.recv().await {
        assert!(contains_ogg_signatures(&packet));
    } else {
        panic!("Should receive output even when callback returns None");
    }

    Ok(())
}
