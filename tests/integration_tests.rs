use anyhow::Result;
use bytes::Bytes;
use oggmux::{BufferConfig, OggMux, VorbisBitrateMode, VorbisConfig};
use tokio::time::{sleep, Duration};

fn create_test_mux() -> OggMux {
    OggMux::new()
        .with_buffer_config(BufferConfig {
            buffered_seconds: 0.5,
            max_chunk_size: 4096,
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
    let (_tx, mut rx) = mux.spawn();

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
    let (tx, mut rx) = mux.spawn();

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
    let (tx, mut rx) = mux.spawn();

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
    let (tx, mut rx) = mux.spawn();

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
    let (tx, mut rx) = mux.spawn();

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
    let (tx, mut rx) = mux.spawn();

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
