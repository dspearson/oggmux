use bytes::Bytes;
use log::{debug, error};
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};

use crate::controller::MuxController;
use crate::silence::SilenceTemplate;
use crate::timing::StreamClock;

/// Configuration for buffer management.
///
/// Controls how much audio should be buffered and the maximum chunk size for
/// processing.
#[derive(Clone, Copy)]
pub struct BufferConfig {
    /// Target buffered time in seconds.
    ///
    /// The muxer will aim to keep this much audio buffered at all times.
    /// When the buffer falls below this level, silence will be inserted.
    pub target_buffered_secs: f64,

    /// Maximum buffer size in seconds.
    ///
    /// When the buffer exceeds this size, the muxer will throttle output
    /// until the buffer decreases.
    pub max_buffer_secs: f64,

    /// Maximum chunk size to read at once in bytes.
    pub max_chunk_size: usize,
}

/// Configuration for Vorbis audio parameters.
///
/// Used for timing calculations and stream rate estimation.
#[derive(Clone, Copy)]
pub struct VorbisConfig {
    /// Sample rate in Hz.
    pub sample_rate: u32,

    /// Bitrate in bits per second.
    pub bitrate_bps: f64,
}

/// The main OggMux controller.
///
/// OggMux provides a simple API for muxing Ogg audio streams with silence gaps.
/// It automatically inserts silence when real audio data is not available,
/// maintains proper timing across stream transitions, and provides configurable
/// buffering to handle network jitter.
pub struct OggMux {
    buffer_config: BufferConfig,
    vorbis_config: VorbisConfig,
    silence: SilenceTemplate,
    initial_serial: u32,
}

impl OggMux {
    /// Create a new OggMux with default configuration.
    ///
    /// Default settings:
    /// - `target_buffered_secs`: 10.0
    /// - `max_buffer_secs`: 10.0
    /// - `max_chunk_size`: 65536
    /// - `sample_rate`: 44100
    /// - `bitrate_bps`: 320_000.0
    /// - `initial_serial`: 0xfeed_0000
    pub fn new() -> Self {
        Self {
            buffer_config: BufferConfig {
                target_buffered_secs: 10.0,
                max_buffer_secs: 10.0,
                max_chunk_size: 65536,
            },
            vorbis_config: VorbisConfig {
                sample_rate: 44100,
                bitrate_bps: 320_000.0,
            },
            silence: SilenceTemplate::new_embedded(include_bytes!("../resources/silence.ogg")),
            initial_serial: 0xfeed_0000,
        }
    }

    /// Configure buffer parameters.
    ///
    /// # Arguments
    ///
    /// * `config` - The buffer configuration to use.
    pub fn with_buffer_config(mut self, config: BufferConfig) -> Self {
        self.buffer_config = config;
        self
    }

    /// Configure Vorbis parameters.
    ///
    /// # Arguments
    ///
    /// * `config` - The Vorbis configuration to use.
    pub fn with_vorbis_config(mut self, config: VorbisConfig) -> Self {
        self.vorbis_config = config;
        self
    }

    /// Spawn the OggMux processor.
    ///
    /// Returns a pair of channels:
    /// - A sender for raw Ogg input
    /// - A receiver for processed Ogg output
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use oggmux::OggMux;
    /// use bytes::Bytes;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let mux = OggMux::new();
    ///     let (input_tx, mut output_rx) = mux.spawn();
    ///
    ///     // Send some Ogg data
    ///     let ogg_data = Bytes::from_static(&[/* Ogg data here */]);
    ///     let _ = input_tx.send(ogg_data).await;
    ///
    ///     // Receive processed output
    ///     if let Some(output) = output_rx.recv().await {
    ///         println!("Got {} bytes of output", output.len());
    ///     }
    /// }
    /// ```
    pub fn spawn(self) -> (mpsc::Sender<Bytes>, mpsc::Receiver<Bytes>) {
        debug!("Spawning OggMux");

        let (input_tx, mut input_rx) = mpsc::channel(self.buffer_config.max_chunk_size);
        let (output_tx, output_rx) = mpsc::channel(self.buffer_config.max_chunk_size);

        let mut controller = MuxController::new(self.silence, self.initial_serial);
        let clock = StreamClock::new(self.vorbis_config.sample_rate);

        let min_buffer = self.buffer_config.target_buffered_secs;
        let max_buffer = self.buffer_config.max_buffer_secs;

        tokio::spawn(async move {
            debug!("OggMux started");

            loop {
                // Check how many seconds (based on granule position) we are ahead of wall‑clock.
                let lead = clock.lead_secs(controller.get_granule_position());

                // If the lead exceeds max_buffer (e.g. 10 seconds), sleep before outputting more pages.
                if lead >= max_buffer {
                    sleep(Duration::from_millis(20)).await;
                    continue;
                }

                match input_rx.try_recv() {
                    Ok(data) => match controller.process(Some(data)) {
                        Ok(output) if !output.is_empty() => {
                            if output_tx.send(output).await.is_err() {
                                break;
                            }
                        }
                        Ok(_) => {}
                        Err(e) => error!("Error processing real input: {e:?}"),
                    },
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                        // If there isn't enough data buffered (lead is below our target),
                        // process a silence frame to fill up the gap.
                        if lead < min_buffer {
                            match controller.process(None) {
                                Ok(output) if !output.is_empty() => {
                                    if output_tx.send(output).await.is_err() {
                                        break;
                                    }
                                }
                                Ok(_) => {}
                                Err(e) => error!("Error processing silence: {e:?}"),
                            }
                        } else {
                            sleep(Duration::from_millis(10)).await;
                        }
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        match controller.finalize() {
                            Ok(output) if !output.is_empty() => {
                                let _ = output_tx.send(output).await;
                            }
                            Err(e) => error!("Error finalising stream: {e:?}"),
                            _ => {}
                        }
                        break;
                    }
                }
            }
        });

        (input_tx, output_rx)
    }
}

impl Default for OggMux {
    fn default() -> Self {
        Self::new()
    }
}
