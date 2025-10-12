//! Example: Multi-stream Ogg file filtering
//!
//! Demonstrates oggmux's ability to handle Ogg files with embedded album art
//! or other non-audio streams. The muxer automatically filters out non-Vorbis
//! streams (like MJPEG/PNG album art).
//!
//! Usage:
//!   cargo run --example multistream_filter -- <path/to/file.ogg>
//!
//! This will process an Ogg file that may contain multiple logical streams
//! (audio + album art) and output only the Vorbis audio stream.

use anyhow::Result;
use bytes::Bytes;
use oggmux::OggMux;
use std::env;
use std::fs;
use tokio::time::{Duration, sleep};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <input.ogg>", args[0]);
        eprintln!("\nThis example demonstrates multi-stream filtering.");
        eprintln!("It will process an Ogg file with embedded album art and");
        eprintln!("automatically filter out the non-audio streams.");
        std::process::exit(1);
    }

    let input_path = &args[1];
    println!("Reading Ogg file: {}", input_path);

    // Read the input file
    let input_data = fs::read(input_path)?;
    println!("Read {} bytes from input file", input_data.len());

    // Create muxer with debug logging enabled
    println!("\nCreating OggMux with multi-stream filtering...");
    let mux = OggMux::new();
    let (input_tx, mut output_rx, shutdown_tx, handle) = mux.spawn();

    // Spawn output consumer
    let output_handle = tokio::spawn(async move {
        let mut total_output = 0;
        let mut packet_count = 0;
        while let Some(output) = output_rx.recv().await {
            total_output += output.len();
            packet_count += 1;
        }
        println!("\nOutput summary:");
        println!("  Total packets: {}", packet_count);
        println!("  Total bytes: {}", total_output);
        Ok::<_, anyhow::Error>(total_output)
    });

    // Send the input data
    println!("Sending input data to muxer...");
    input_tx.send(Bytes::from(input_data)).await?;
    drop(input_tx);

    // Wait a bit for processing
    sleep(Duration::from_millis(100)).await;

    // Shutdown
    let _ = shutdown_tx.send(()).await;

    // Wait for completion
    match handle.await {
        Ok(Ok(())) => println!("\n✓ Muxer completed successfully"),
        Ok(Err(e)) => eprintln!("\n✗ Muxer error: {:?}", e),
        Err(e) => eprintln!("\n✗ Task panicked: {:?}", e),
    }

    match output_handle.await {
        Ok(Ok(_total)) => {
            println!("✓ Output consumer completed");
            println!("\nIf your input file had album art, check the debug logs");
            println!("(run with RUST_LOG=debug) to see the filtering in action.");
        }
        Ok(Err(e)) => eprintln!("✗ Output error: {:?}", e),
        Err(e) => eprintln!("✗ Output task panicked: {:?}", e),
    }

    Ok(())
}
