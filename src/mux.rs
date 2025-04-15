use anyhow::Result;
use bytes::Bytes;
use log::{debug, error};
use std::time::Instant;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};

use crate::silence::SilenceTemplate;
use crate::stream::StreamProcessor;
use crate::timing::StreamClock;

/// Configuration for buffer management.
#[derive(Clone, Copy)]
pub struct BufferConfig {
    /// Target amount of audio to keep buffered (in seconds)
    pub target_buffered_secs: f64,
    /// Maximum buffer size before throttling (in seconds)
    pub max_buffer_secs: f64,
    /// Maximum chunk size to process at once (in bytes)
    pub max_chunk_size: usize,
}

/// Configuration for Vorbis audio parameters.
#[derive(Clone, Copy)]
pub struct VorbisConfig {
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Bitrate in bits per second
    pub bitrate_bps: f64,
}

/// Single "session" that processes exactly one Ogg stream (real or silence).
struct StreamSession {
    stream_processor: StreamProcessor,
    is_silence: bool,
    silence_template: Option<Bytes>,
}

impl StreamSession {
    /// Create a new session for processing real audio data
    fn new_real(serial: u32, base_granule: u64) -> Self {
        Self {
            stream_processor: StreamProcessor::with_serial(serial, base_granule),
            is_silence: false,
            silence_template: None,
        }
    }

    /// Create a new session for generating silence
    fn new_silence(serial: u32, base_granule: u64, template: Bytes) -> Self {
        Self {
            stream_processor: StreamProcessor::with_silence(serial, base_granule),
            is_silence: true,
            silence_template: Some(template),
        }
    }

    /// Run until the stream is finished (EOS) or no more data.
    ///
    /// For silence, we just process the template once and finalise.
    /// For real data, we process until we reach EOS or run out of input.
    async fn run(
        &mut self,
        input_rx: &mut mpsc::Receiver<Bytes>,
        output_tx: &mpsc::Sender<Bytes>,
        clock: &StreamClock,
        min_buffer: f64,
        max_buffer: f64,
        granule_position_ref: &mut u64,
    ) -> Result<()> {
        if self.is_silence {
            if let Some(ref template) = self.silence_template {
                let out = self.stream_processor.process(template)?;
                if !out.is_empty() {
                    self.maybe_sleep(clock, max_buffer, *granule_position_ref)
                        .await;
                    if output_tx.send(out).await.is_err() {
                        return Ok(());
                    }
                    *granule_position_ref = self.stream_processor.get_granule_position();
                }
            }
            let final_out = self.stream_processor.finalise()?;
            if !final_out.is_empty() {
                self.maybe_sleep(clock, max_buffer, *granule_position_ref)
                    .await;
                let _ = output_tx.send(final_out).await;
            }
            *granule_position_ref = self.stream_processor.get_granule_position();
            return Ok(());
        }

        // Real-data stream processing
        let mut last_action_time = Instant::now();
        let timeout = Duration::from_millis(500); // Timeout for considering input as invalid

        loop {
            let lead = clock.lead_secs(*granule_position_ref);
            if lead >= max_buffer {
                sleep(Duration::from_millis(20)).await;
            }

            match input_rx.try_recv() {
                Ok(data) => {
                    last_action_time = Instant::now();
                    let out = self.stream_processor.process(&data)?;
                    if !out.is_empty() {
                        self.maybe_sleep(clock, max_buffer, *granule_position_ref)
                            .await;
                        if output_tx.send(out).await.is_err() {
                            break;
                        }
                    }
                    *granule_position_ref = self.stream_processor.get_granule_position();
                }
                Err(mpsc::error::TryRecvError::Empty) => {
                    let lead = clock.lead_secs(*granule_position_ref);

                    // Check if we've been waiting too long with no valid output
                    // This could indicate invalid data that doesn't parse properly
                    if !self.stream_processor.has_produced_output()
                        && last_action_time.elapsed() > timeout
                    {
                        debug!("No valid output produced after timeout; breaking to restart with silence");
                        break;
                    }

                    if lead < min_buffer {
                        sleep(Duration::from_millis(10)).await;
                    } else {
                        sleep(Duration::from_millis(10)).await;
                    }
                }
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    break;
                }
            }

            if self.stream_processor.is_finished() {
                break;
            }
        }

        let final_out = self.stream_processor.finalise()?;
        if !final_out.is_empty() {
            self.maybe_sleep(clock, max_buffer, *granule_position_ref)
                .await;
            let _ = output_tx.send(final_out).await;
        }
        *granule_position_ref = self.stream_processor.get_granule_position();
        Ok(())
    }

    /// Sleep if we're generating output faster than real-time
    async fn maybe_sleep(&self, clock: &StreamClock, max_buffer: f64, current_granule: u64) {
        let lead = clock.lead_secs(current_granule);
        if lead >= max_buffer {
            while clock.lead_secs(current_granule) >= max_buffer {
                sleep(Duration::from_millis(20)).await;
            }
        }
    }
}

/// The main OggMux.
///
/// This struct handles the muxing of Ogg streams, automatically inserting
/// silence when no real audio data is available, and ensuring proper timing
/// across stream transitions.
pub struct OggMux {
    buffer_config: BufferConfig,
    vorbis_config: VorbisConfig,
    silence: SilenceTemplate,
    initial_serial: u32,
}

impl OggMux {
    /// Create a new OggMux with default configuration.
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

    /// Configure buffer settings for the OggMux.
    pub fn with_buffer_config(mut self, config: BufferConfig) -> Self {
        self.buffer_config = config;
        self
    }

    /// Configure Vorbis audio parameters for the OggMux.
    pub fn with_vorbis_config(mut self, config: VorbisConfig) -> Self {
        self.vorbis_config = config;
        self
    }

    /// Generate a new serial number each time we spawn a stream.
    fn next_serial(&mut self) -> u32 {
        let s = self.initial_serial;
        self.initial_serial = self.initial_serial.wrapping_add(1);
        s
    }

    /// Spawn the main muxer loop: returns (input_tx, output_rx).
    ///
    /// - Send real Ogg data into `input_tx`.
    /// - Muxed output arrives on `output_rx`.
    ///
    /// The muxer automatically inserts silence when no input is available,
    /// and manages transitions between real audio and silence to maintain
    /// proper timing.
    pub fn spawn(mut self) -> (mpsc::Sender<Bytes>, mpsc::Receiver<Bytes>) {
        let (input_tx, mut input_rx) = mpsc::channel::<Bytes>(self.buffer_config.max_chunk_size);
        let (output_tx, output_rx) = mpsc::channel::<Bytes>(self.buffer_config.max_chunk_size);

        let clock = StreamClock::new(self.vorbis_config.sample_rate);
        let min_buffer = self.buffer_config.target_buffered_secs;
        let max_buffer = self.buffer_config.max_buffer_secs;

        let mut global_granule_position: u64 = 0;

        tokio::spawn(async move {
            debug!("OggMux main loop started");

            loop {
                // Attempt to read once from input
                match input_rx.try_recv() {
                    Ok(first_chunk) => {
                        // We have real data => start a real session
                        let serial = self.next_serial();
                        debug!("Starting REAL stream, serial=0x{:x}", serial);

                        let mut session = StreamSession::new_real(serial, global_granule_position);
                        // Feed the chunk we just pulled
                        match session.stream_processor.process(&first_chunk) {
                            Ok(out) => {
                                if !out.is_empty() {
                                    session
                                        .maybe_sleep(&clock, max_buffer, global_granule_position)
                                        .await;
                                    let _ = output_tx.send(out).await;
                                }
                                global_granule_position =
                                    session.stream_processor.get_granule_position();
                            }
                            Err(e) => {
                                error!("Error processing first chunk: {:?}", e);
                                continue;
                            }
                        }

                        // Now let the session run
                        if let Err(e) = session
                            .run(
                                &mut input_rx,
                                &output_tx,
                                &clock,
                                min_buffer,
                                max_buffer,
                                &mut global_granule_position,
                            )
                            .await
                        {
                            error!("Session run error: {:?}", e);
                        }
                    }
                    Err(mpsc::error::TryRecvError::Empty) => {
                        // No data => do silence
                        let serial = self.next_serial();
                        debug!("Starting SILENCE stream, serial=0x{:x}", serial);

                        let silence_data = self.silence.raw_bytes().to_vec().into();
                        let mut session = StreamSession::new_silence(
                            serial,
                            global_granule_position,
                            silence_data,
                        );

                        if let Err(e) = session
                            .run(
                                &mut input_rx,
                                &output_tx,
                                &clock,
                                min_buffer,
                                max_buffer,
                                &mut global_granule_position,
                            )
                            .await
                        {
                            error!("Silence session error: {:?}", e);
                        }
                    }
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        debug!("Input disconnected; finishing");
                        break;
                    }
                }
            }

            debug!("OggMux main loop ended");
        });

        (input_tx, output_rx)
    }
}

impl Default for OggMux {
    fn default() -> Self {
        Self::new()
    }
}
