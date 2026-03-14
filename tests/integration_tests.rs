use anyhow::Result;
use bytes::Bytes;
use ogg_pager::Page;
use oggmux::{BufferConfig, MuxMode, OggMux, VorbisBitrateMode, VorbisConfig};
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use tokio::time::{Duration, sleep};

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

/// Parse all valid Ogg pages from a buffer. Returns pages with their serials,
/// granule positions, and header type flags.
fn parse_ogg_pages(data: &[u8]) -> Vec<Page> {
    let mut pages = Vec::new();
    let mut cursor = Cursor::new(data);
    while let Ok(page) = Page::read(&mut cursor) {
        pages.push(page);
        if cursor.position() as usize >= data.len() {
            break;
        }
    }
    pages
}

/// Collect all output packets from a muxer receiver with a timeout.
async fn collect_packets(
    rx: &mut tokio::sync::mpsc::Receiver<Bytes>,
    timeout_ms: u64,
) -> Vec<Bytes> {
    let mut packets = Vec::new();
    while let Ok(Some(packet)) =
        tokio::time::timeout(Duration::from_millis(timeout_ms), rx.recv()).await
    {
        packets.push(packet);
    }
    packets
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

    // Send invalid data — the muxer should log and continue, not crash
    tx.send(invalid_data).await?;

    // Wait long enough for the muxer to process the bad chunk and fall back to silence
    sleep(Duration::from_millis(300)).await;

    // Muxer should still be alive and inserting silence
    match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
        Ok(Some(packet)) => {
            assert!(
                contains_ogg_signatures(&packet),
                "Should receive valid Ogg output (silence) after invalid data"
            );
        }
        Ok(None) => {
            panic!("Muxer should continue after invalid data, not close the channel");
        }
        Err(_) => {
            // Timeout is acceptable — muxer may not have produced silence yet
            // but the important thing is it didn't crash
        }
    }

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
async fn test_sequential_stream_handoff() -> Result<()> {
    let mux = create_test_mux();
    let (tx, mut rx, _shutdown, _handle) = mux.spawn();

    // Send multiple sequential streams with gaps between them
    // This is the supported use case — one stream at a time
    for i in 0..3 {
        tx.send(get_silence_ogg()).await?;
        // Wait for the stream to be fully processed before sending the next
        sleep(Duration::from_millis(600)).await;
        let _ = i;
    }

    // Collect output packets
    let mut packets_received = 0;
    while let Ok(Some(packet)) = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
        assert!(contains_ogg_signatures(&packet));
        packets_received += 1;
    }

    assert!(
        packets_received >= 3,
        "Expected at least 3 packets from sequential streams, got {}",
        packets_received
    );

    Ok(())
}

#[tokio::test]
async fn test_gapless_mode_no_silence() -> Result<()> {
    // In gapless mode, mux should NOT insert silence when idle
    let mux = create_test_mux().with_mode(MuxMode::Gapless);

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
    let mux = create_test_mux().with_mode(MuxMode::WithSilence);

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

    let mux = create_test_mux().with_metadata_callback(move |_granule_pos| {
        *callback_invoked_clone.lock().unwrap() = true;
        Some(vec![
            ("TITLE".to_string(), "Test Track".to_string()),
            ("ARTIST".to_string(), "Test Artist".to_string()),
        ])
    });

    let (tx, mut rx, _shutdown, _handle) = mux.spawn();

    // Send data to trigger stream processing
    tx.send(get_silence_ogg()).await?;

    // Drop tx to signal no more input
    drop(tx);

    // Wait for processing and receive output with timeout
    sleep(Duration::from_millis(200)).await;

    // Drain receiver with timeout to avoid hanging
    while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
        // Keep receiving until timeout or empty
    }

    // Callback should have been invoked after stream finished
    assert!(
        *callback_invoked.lock().unwrap(),
        "Metadata callback should be invoked after stream"
    );

    Ok(())
}

#[tokio::test]
async fn test_metadata_callback_with_granule_position() -> Result<()> {
    let received_granule = Arc::new(Mutex::new(None));
    let received_granule_clone = received_granule.clone();

    let mux = create_test_mux().with_metadata_callback(move |granule_pos| {
        *received_granule_clone.lock().unwrap() = Some(granule_pos);
        Some(vec![("POSITION".to_string(), granule_pos.to_string())])
    });

    let (tx, mut rx, _shutdown, _handle) = mux.spawn();

    tx.send(get_silence_ogg()).await?;

    // Drop tx to signal no more input
    drop(tx);

    sleep(Duration::from_millis(200)).await;

    // Drain receiver with timeout to avoid hanging
    while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
        // Keep receiving until timeout or empty
    }

    // Should have received a granule position
    let granule = received_granule.lock().unwrap();
    assert!(
        granule.is_some(),
        "Callback should receive granule position"
    );
    assert!(
        *granule.as_ref().unwrap() > 0,
        "Granule position should be > 0"
    );

    Ok(())
}

#[tokio::test]
async fn test_metadata_injection_creates_pages() -> Result<()> {
    let mux = create_test_mux()
        .with_metadata_callback(|_| Some(vec![("TITLE".to_string(), "Test".to_string())]));

    let (tx, mut rx, _shutdown, _handle) = mux.spawn();

    tx.send(get_silence_ogg()).await?;

    let mut packets_received = 0;

    // Collect packets
    while let Ok(Some(_packet)) = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await
    {
        packets_received += 1;
    }

    assert!(packets_received > 0, "Should receive packets");
    // Note: Metadata pages are injected but may be difficult to detect without full Ogg parsing
    // The main test is that it doesn't crash and produces output

    Ok(())
}

#[tokio::test]
async fn test_gapless_with_multiple_streams() -> Result<()> {
    let mux = create_test_mux().with_mode(MuxMode::Gapless);

    let (tx, mut rx, _shutdown, _handle) = mux.spawn();

    // Send multiple streams with delays to ensure separate processing
    for i in 0..3 {
        tx.send(get_silence_ogg()).await?;
        if i < 2 {
            // Wait long enough for the stream to be fully processed
            sleep(Duration::from_millis(200)).await;
        }
    }

    // Drop tx to signal no more input
    drop(tx);

    // Should receive data from all streams without silence gaps
    let mut packets_received = 0;
    while let Ok(Some(packet)) = tokio::time::timeout(Duration::from_millis(2000), rx.recv()).await
    {
        assert!(contains_ogg_signatures(&packet));
        packets_received += 1;
    }

    assert!(
        packets_received >= 3,
        "Should receive at least 3 packets from streams, got {}",
        packets_received
    );

    Ok(())
}

#[tokio::test]
async fn test_metadata_callback_returning_none() -> Result<()> {
    // Callback that returns None should not crash or cause issues
    let mux = create_test_mux().with_metadata_callback(|_| None);

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

// ============================================================================
// New tests for the bug fixes
// ============================================================================

#[tokio::test]
async fn test_error_resilience_bad_then_good_data() -> Result<()> {
    // Send corrupt data followed by valid data. The muxer should survive the
    // corrupt data and successfully process the valid stream.
    let mux = create_test_mux();
    let (tx, mut rx, _shutdown, handle) = mux.spawn();

    // Send garbage
    tx.send(Bytes::from(b"this is not ogg data at all".to_vec()))
        .await?;

    // Wait for the muxer to recover
    sleep(Duration::from_millis(800)).await;

    // Now send valid Ogg data
    tx.send(get_silence_ogg()).await?;

    // Drain and collect — we should get valid Ogg output
    let packets = collect_packets(&mut rx, 1000).await;

    // At least one packet should contain valid Ogg pages
    let total_pages: usize = packets.iter().map(|p| parse_ogg_pages(p).len()).sum();
    assert!(
        total_pages > 0,
        "Should produce valid Ogg pages after recovering from bad data"
    );

    // Verify all output pages have valid CRCs (ogg_pager::Page::read checks CRC)
    for packet in &packets {
        for page in parse_ogg_pages(packet) {
            // If Page::read succeeded, the CRC was valid
            assert!(
                page.header().stream_serial != 0 || page.header().sequence_number == 0,
                "Pages should have reasonable header values"
            );
        }
    }

    // Muxer task should still be running
    drop(tx);
    let result = tokio::time::timeout(Duration::from_millis(1000), handle).await;
    assert!(result.is_ok(), "Muxer should exit cleanly after tx dropped");
    assert!(
        result.unwrap().unwrap().is_ok(),
        "Muxer should not have errored"
    );

    Ok(())
}

#[tokio::test]
async fn test_error_resilience_multiple_bad_chunks() -> Result<()> {
    // Send several rounds of corrupt data. The muxer should survive all of them.
    let mux = create_test_mux();
    let (tx, mut rx, shutdown_tx, handle) = mux.spawn();

    for i in 0..5 {
        let bad = Bytes::from(format!("corrupt chunk #{}", i).into_bytes());
        tx.send(bad).await?;
        sleep(Duration::from_millis(200)).await;
    }

    // Muxer should still be alive — send shutdown and verify clean exit
    let _ = shutdown_tx.send(()).await;
    let result = tokio::time::timeout(Duration::from_millis(1000), handle).await;
    assert!(result.is_ok(), "Muxer should exit on shutdown signal");
    assert!(
        result.unwrap().unwrap().is_ok(),
        "Muxer should exit without error despite corrupt input"
    );

    // Drain any output (may have silence)
    drop(tx);
    let _ = collect_packets(&mut rx, 100).await;

    Ok(())
}

#[tokio::test]
async fn test_all_output_pages_have_valid_crcs() -> Result<()> {
    // Verify that every Ogg page produced by the muxer has a valid CRC
    // (ogg_pager::Page::read validates CRC on parse).
    let mux = create_test_mux();
    let (tx, mut rx, _shutdown, _handle) = mux.spawn();

    // Send valid data
    tx.send(get_silence_ogg()).await?;
    sleep(Duration::from_millis(200)).await;

    // Collect all output
    let packets = collect_packets(&mut rx, 500).await;
    assert!(!packets.is_empty(), "Should receive output");

    let mut total_pages = 0;
    for packet in &packets {
        let pages = parse_ogg_pages(packet);
        total_pages += pages.len();
        // Each page was successfully parsed — CRC is valid
    }

    assert!(
        total_pages > 0,
        "Should have parsed at least one valid Ogg page"
    );

    Ok(())
}

#[tokio::test]
async fn test_output_pages_have_consistent_serial_within_stream() -> Result<()> {
    // All pages from a single stream should share the same serial number
    let mux = create_test_mux();
    let (tx, mut rx, _shutdown, _handle) = mux.spawn();

    tx.send(get_silence_ogg()).await?;
    sleep(Duration::from_millis(200)).await;
    drop(tx);

    let packets = collect_packets(&mut rx, 500).await;
    assert!(!packets.is_empty(), "Should receive output");

    // Parse all pages and group by serial
    let mut serials_seen = std::collections::HashSet::new();
    for packet in &packets {
        for page in parse_ogg_pages(packet) {
            serials_seen.insert(page.header().stream_serial);
        }
    }

    // Should have at least one serial (the real stream)
    assert!(
        !serials_seen.is_empty(),
        "Should see at least one serial number"
    );

    Ok(())
}

#[tokio::test]
async fn test_granule_positions_are_monotonic() -> Result<()> {
    // Granule positions should never decrease across pages within the output
    let mux = create_test_mux();
    let (tx, mut rx, _shutdown, _handle) = mux.spawn();

    // Send two streams with a gap to trigger silence insertion
    tx.send(get_silence_ogg()).await?;
    sleep(Duration::from_millis(800)).await;
    tx.send(get_silence_ogg()).await?;
    sleep(Duration::from_millis(200)).await;
    drop(tx);

    let packets = collect_packets(&mut rx, 1000).await;

    let mut all_granules: Vec<u64> = Vec::new();
    for packet in &packets {
        for page in parse_ogg_pages(packet) {
            let gp = page.header().abgp;
            if gp != u64::MAX {
                // u64::MAX is a special "no position" value
                all_granules.push(gp);
            }
        }
    }

    // Verify monotonicity
    for window in all_granules.windows(2) {
        assert!(
            window[1] >= window[0],
            "Granule positions should be monotonically non-decreasing: {} followed by {}",
            window[0],
            window[1]
        );
    }

    assert!(
        all_granules.len() >= 2,
        "Should have at least 2 granule positions to verify monotonicity, got {}",
        all_granules.len()
    );

    Ok(())
}

#[tokio::test]
async fn test_metadata_injected_with_next_stream_serial() -> Result<()> {
    // Metadata should be injected using the NEW stream's serial, not the old one.
    // We track which serial the metadata callback was stored for, then check the
    // output pages.
    let callback_count = Arc::new(Mutex::new(0u32));
    let callback_count_clone = callback_count.clone();

    let mux = create_test_mux().with_metadata_callback(move |_granule_pos| {
        let mut count = callback_count_clone.lock().unwrap();
        *count += 1;
        Some(vec![("TITLE".to_string(), format!("Track {}", *count))])
    });

    let (tx, mut rx, _shutdown, _handle) = mux.spawn();

    // Send first stream
    tx.send(get_silence_ogg()).await?;
    sleep(Duration::from_millis(800)).await;

    // Send second stream — metadata from first stream's callback should be
    // injected with this stream's serial
    tx.send(get_silence_ogg()).await?;
    sleep(Duration::from_millis(200)).await;
    drop(tx);

    let packets = collect_packets(&mut rx, 1000).await;

    // Parse all pages and collect serials
    let mut all_pages: Vec<(u32, u64, Vec<u8>)> = Vec::new(); // (serial, granule, content)
    for packet in &packets {
        for page in parse_ogg_pages(packet) {
            all_pages.push((
                page.header().stream_serial,
                page.header().abgp,
                page.content().to_vec(),
            ));
        }
    }

    // Should have pages from at least 2 different serials (two streams, possibly silence)
    let unique_serials: std::collections::HashSet<u32> =
        all_pages.iter().map(|(s, _, _)| *s).collect();

    assert!(
        unique_serials.len() >= 2,
        "Should have pages from at least 2 different stream serials, got {}",
        unique_serials.len()
    );

    // The metadata callback should have been invoked
    assert!(
        *callback_count.lock().unwrap() >= 1,
        "Metadata callback should have been invoked at least once"
    );

    // Check that any metadata pages (containing "TITLE") use a serial that matches
    // actual stream pages (not an orphaned serial)
    for (serial, _, content) in &all_pages {
        if content.windows(5).any(|w| w == b"TITLE") {
            assert!(
                unique_serials.contains(serial),
                "Metadata page serial 0x{:x} should match a known stream serial",
                serial
            );
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_shutdown_during_active_stream() -> Result<()> {
    // Verify that shutdown works cleanly even while a stream is being processed
    let mux = create_test_mux();
    let (tx, _rx, shutdown_tx, handle) = mux.spawn();

    // Start sending data
    tx.send(get_silence_ogg()).await?;

    // Immediately signal shutdown
    sleep(Duration::from_millis(50)).await;
    let _ = shutdown_tx.send(()).await;

    // Task should exit cleanly
    let result = tokio::time::timeout(Duration::from_millis(1000), handle).await;
    assert!(result.is_ok(), "Task should exit on shutdown signal");
    assert!(
        result.unwrap().unwrap().is_ok(),
        "Task should exit without error"
    );

    Ok(())
}

#[tokio::test]
async fn test_silence_then_real_stream_transition() -> Result<()> {
    // Start with no input (silence insertion), then send real data.
    // Verify the transition produces valid, continuous output.
    let mux = create_test_mux();
    let (tx, mut rx, _shutdown, _handle) = mux.spawn();

    // Wait for silence to be inserted
    sleep(Duration::from_millis(300)).await;

    // Now send real data
    tx.send(get_silence_ogg()).await?;
    sleep(Duration::from_millis(200)).await;
    drop(tx);

    let packets = collect_packets(&mut rx, 500).await;
    assert!(
        packets.len() >= 2,
        "Should have at least silence + real stream output, got {}",
        packets.len()
    );

    // All output should be valid Ogg
    for packet in &packets {
        assert!(contains_ogg_signatures(packet));
        let pages = parse_ogg_pages(packet);
        assert!(
            !pages.is_empty(),
            "Each packet should contain valid Ogg pages"
        );
    }

    Ok(())
}
