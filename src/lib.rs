//! # OggMux
//!
//! OggMux is a library for muxing Ogg audio streams with clean silence gaps,
//! suitable for streaming applications like Icecast.
//!
//! It automatically inserts silence when real audio data is not available,
//! maintains proper timing across stream transitions, and provides configurable
//! buffering to handle network jitter.
//!
//! ## Example
//! ```rust,no_run
//! use oggmux::OggMux;
//! use bytes::Bytes;
//!
//! #[tokio::main]
//! async fn main() {
//!     // Create a new OggMux with default configuration
//!     let mux = OggMux::new();
//!
//!     // Spawn the muxer and get the channels
//!     let (input_tx, mut output_rx) = mux.spawn();
//!
//!     // Feed some Ogg data (e.g., from a file or network stream)
//!     let ogg_data = Bytes::from_static(&[/* Ogg data here */]);
//!     let _ = input_tx.send(ogg_data).await;
//!
//!     // Read the processed output (e.g., send to Icecast)
//!     while let Some(output) = output_rx.recv().await {
//!         println!("Got {} bytes of output", output.len());
//!     }
//! }
//! ```

mod mux;
mod silence;
mod stream;
mod timing;

// Re-export public API
pub use mux::{BufferConfig, OggMux, VorbisConfig};
