//! CLI tool for testing oggmux with real Ogg files.
//!
//! Reads one or more Ogg Vorbis files, pipes them through the muxer with
//! configurable silence/gapless mode and optional metadata injection, and
//! writes the output to stdout.
//!
//! Usage:
//!   # Single file, pipe to mpv:
//!   cargo run --example pipe_test -- file.ogg | mpv --no-video -
//!
//!   # Multiple files played sequentially with silence gaps:
//!   cargo run --example pipe_test -- track1.ogg track2.ogg track3.ogg | mpv --no-video -
//!
//!   # Gapless mode (no silence between tracks):
//!   cargo run --example pipe_test -- --gapless track1.ogg track2.ogg | mpv --no-video -
//!
//!   # With metadata injection (title = filename):
//!   cargo run --example pipe_test -- --metadata track1.ogg track2.ogg | mpv --no-video -
//!
//!   # Read from stdin:
//!   cat file.ogg | cargo run --example pipe_test | mpv --no-video -
//!
//!   # With debug logging:
//!   RUST_LOG=debug cargo run --example pipe_test -- file.ogg 2>mux.log | mpv --no-video -

use anyhow::Result;
use bytes::Bytes;
use oggmux::{BufferConfig, MuxMode, OggMux};
use std::env;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::time::{Duration, sleep};

struct Config {
    files: Vec<PathBuf>,
    gapless: bool,
    metadata: bool,
    delay_between_ms: u64,
    read_stdin: bool,
}

fn parse_args() -> Config {
    let args: Vec<String> = env::args().skip(1).collect();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        eprintln!("Usage: pipe_test [OPTIONS] [FILE...]");
        eprintln!();
        eprintln!("Options:");
        eprintln!("  --gapless       Use gapless mode (no silence between tracks)");
        eprintln!("  --metadata      Inject track title metadata (filename)");
        eprintln!("  --delay MS      Delay between tracks in ms (default: 1000)");
        eprintln!("  -h, --help      Show this help");
        eprintln!();
        eprintln!("If no files are given, reads from stdin.");
        eprintln!();
        eprintln!("Examples:");
        eprintln!("  cargo run --example pipe_test -- song.ogg | mpv --no-video -");
        eprintln!("  cargo run --example pipe_test -- --gapless a.ogg b.ogg | mpv -");
        eprintln!("  cat song.ogg | cargo run --example pipe_test | mpv --no-video -");
        std::process::exit(0);
    }

    let mut config = Config {
        files: Vec::new(),
        gapless: false,
        metadata: false,
        delay_between_ms: 1000,
        read_stdin: false,
    };

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--gapless" => config.gapless = true,
            "--metadata" => config.metadata = true,
            "--delay" => {
                i += 1;
                if i < args.len() {
                    config.delay_between_ms = args[i].parse().unwrap_or(1000);
                }
            }
            other => config.files.push(PathBuf::from(other)),
        }
        i += 1;
    }

    if config.files.is_empty() {
        config.read_stdin = true;
    }

    config
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialise logging to stderr so it doesn't interfere with piped output
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .target(env_logger::Target::Stderr)
        .init();

    let config = parse_args();

    // Use a very large buffer so the muxer dumps output as fast as possible
    // rather than throttling to real-time. This is appropriate for file-to-pipe
    // usage but NOT for live streaming.
    let mut mux = OggMux::new().with_buffer_config(BufferConfig {
        buffered_seconds: 86400.0, // 24 hours — effectively unbounded
        channel_capacity: 65536,
    });

    if config.gapless {
        mux = mux.with_mode(MuxMode::Gapless);
        eprintln!("[pipe_test] Mode: gapless");
    } else {
        eprintln!("[pipe_test] Mode: with silence gaps");
    }

    // Track index for metadata
    let track_index = Arc::new(Mutex::new(0usize));
    let file_names: Arc<Vec<String>> = Arc::new(
        config
            .files
            .iter()
            .map(|p| {
                p.file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".to_string())
            })
            .collect(),
    );

    if config.metadata {
        let names = file_names.clone();
        let idx = track_index.clone();
        mux = mux.with_metadata_callback(move |granule_pos| {
            let mut i = idx.lock().unwrap();
            let name = if *i < names.len() {
                names[*i].clone()
            } else {
                format!("Track {}", *i + 1)
            };
            *i += 1;
            eprintln!(
                "[pipe_test] Metadata callback: TITLE={} (granule={})",
                name, granule_pos
            );
            Some(vec![("TITLE".to_string(), name)])
        });
        eprintln!("[pipe_test] Metadata injection: enabled");
    }

    let (input_tx, mut output_rx, shutdown_tx, handle) = mux.spawn();

    // Output writer — writes to stdout
    let output_task = tokio::spawn(async move {
        let mut total_bytes = 0u64;
        let mut packet_count = 0u64;

        while let Some(data) = output_rx.recv().await {
            total_bytes += data.len() as u64;
            packet_count += 1;
            // Lock stdout briefly per packet to avoid holding it across await
            let write_result = {
                let stdout = io::stdout();
                let mut stdout = stdout.lock();
                stdout.write_all(&data).and_then(|_| stdout.flush())
            };
            if let Err(e) = write_result {
                if e.kind() == io::ErrorKind::BrokenPipe {
                    eprintln!("[pipe_test] Broken pipe (consumer closed)");
                    break;
                }
                eprintln!("[pipe_test] Write error: {}", e);
                break;
            }
        }

        eprintln!(
            "[pipe_test] Output: {} packets, {} bytes total",
            packet_count, total_bytes
        );
    });

    // Input feeder
    if config.read_stdin {
        eprintln!("[pipe_test] Reading from stdin...");
        let data = tokio::task::spawn_blocking(|| {
            let mut buf = Vec::new();
            io::stdin().read_to_end(&mut buf).map(|_| buf)
        })
        .await??;
        eprintln!("[pipe_test] Read {} bytes from stdin", data.len());
        let _ = input_tx.send(Bytes::from(data)).await;
    } else {
        for (i, path) in config.files.iter().enumerate() {
            eprintln!(
                "[pipe_test] Sending file {}/{}: {}",
                i + 1,
                config.files.len(),
                path.display()
            );

            let data = std::fs::read(path)?;
            eprintln!("[pipe_test]   {} bytes", data.len());

            if input_tx.send(Bytes::from(data)).await.is_err() {
                eprintln!("[pipe_test] Muxer closed, stopping");
                break;
            }

            // Delay between tracks so the muxer detects stream boundaries.
            // The muxer's session uses a 500ms timeout to detect end-of-stream,
            // so we need to wait longer than that between files.
            if i + 1 < config.files.len() {
                eprintln!(
                    "[pipe_test]   Waiting {}ms before next track...",
                    config.delay_between_ms
                );
                sleep(Duration::from_millis(config.delay_between_ms)).await;
            }
        }
    }

    // Drop input sender — the muxer will detect this and exit its main loop
    // once the current session finishes.
    drop(input_tx);
    eprintln!("[pipe_test] All input sent, waiting for muxer to drain...");

    // Wait for the muxer to finish naturally. It will exit when:
    // - input_rx.recv() returns None (input_tx dropped), OR
    // - the output channel is closed (consumer gone)
    // Do NOT send shutdown prematurely — the muxer needs time to flush.
    match handle.await {
        Ok(Ok(())) => eprintln!("[pipe_test] Muxer exited cleanly"),
        Ok(Err(e)) => eprintln!("[pipe_test] Muxer error: {:?}", e),
        Err(e) => eprintln!("[pipe_test] Muxer task panicked: {:?}", e),
    }

    // Now shut down the shutdown channel (it's unused but we hold it to
    // prevent the muxer from seeing a spurious shutdown signal)
    drop(shutdown_tx);

    let _ = output_task.await;
    Ok(())
}
