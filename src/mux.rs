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
    pub buffered_seconds: f64,

    pub max_chunk_size: usize,
}

/// Bitrate mode for Vorbis encoding or template selection
#[derive(Clone, Copy, Debug)]
pub enum VorbisBitrateMode {
    /// Constant Bitrate (CBR)
    CBR(u32), // e.g., 192 (kbps)
    /// Variable Bitrate (VBR) with quality level
    VBRQuality(u8), // e.g., q6
}

/// Configuration for Vorbis audio parameters.
#[derive(Clone, Copy, Debug)]
pub struct VorbisConfig {
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Bitrate in bits per second
    pub bitrate: VorbisBitrateMode,
}

impl VorbisConfig {
    pub fn silence_key(&self) -> String {
        match self.bitrate {
            VorbisBitrateMode::CBR(kbps) => format!("{}_{}", self.sample_rate, kbps),
            VorbisBitrateMode::VBRQuality(q) => format!("{}_q{}", self.sample_rate, q),
        }
    }
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
        buffered_seconds: f64,
        granule_position_ref: &mut u64,
    ) -> Result<()> {
        if self.is_silence {
            if let Some(ref template) = self.silence_template {
                let out = self.stream_processor.process(template)?;
                if !out.is_empty() {
                    self.maybe_sleep(clock, buffered_seconds, *granule_position_ref)
                        .await;
                    if output_tx.send(out).await.is_err() {
                        return Ok(());
                    }
                    *granule_position_ref = self.stream_processor.get_granule_position();
                }
            }
            let final_out = self.stream_processor.finalise()?;
            if !final_out.is_empty() {
                self.maybe_sleep(clock, buffered_seconds, *granule_position_ref)
                    .await;
                let _ = output_tx.send(final_out).await;
            }
            *granule_position_ref = self.stream_processor.get_granule_position();
            return Ok(());
        }

        let mut last_action_time = Instant::now();
        let timeout = Duration::from_millis(500); // Timeout for considering input as invalid

        loop {
            let lead = clock.lead_secs(*granule_position_ref);
            if lead >= buffered_seconds {
                sleep(Duration::from_millis(20)).await;
            }

            match input_rx.try_recv() {
                Ok(data) => {
                    last_action_time = Instant::now();
                    let out = self.stream_processor.process(&data)?;
                    if !out.is_empty() {
                        self.maybe_sleep(clock, buffered_seconds, *granule_position_ref)
                            .await;
                        if output_tx.send(out).await.is_err() {
                            break;
                        }
                    }
                    *granule_position_ref = self.stream_processor.get_granule_position();
                }
                Err(mpsc::error::TryRecvError::Empty) => {
                    // Check if we've been waiting too long with no valid output
                    // This could indicate invalid data that doesn't parse properly
                    if !self.stream_processor.has_produced_output()
                        && last_action_time.elapsed() > timeout
                    {
                        debug!("No valid output produced after timeout; breaking to restart with silence");
                        break;
                    }
                    // Sleep for a fixed duration regardless of the lead value.
                    sleep(Duration::from_millis(10)).await;
                }
                Err(mpsc::error::TryRecvError::Disconnected) => break,
            }

            if self.stream_processor.is_finished() {
                break;
            }
        }

        let final_out = self.stream_processor.finalise()?;
        if !final_out.is_empty() {
            self.maybe_sleep(clock, buffered_seconds, *granule_position_ref)
                .await;
            let _ = output_tx.send(final_out).await;
        }
        *granule_position_ref = self.stream_processor.get_granule_position();
        Ok(())
    }

    /// Sleep if we're generating output faster than real-time
    async fn maybe_sleep(&self, clock: &StreamClock, buffered_seconds: f64, current_granule: u64) {
        let lead = clock.lead_secs(current_granule);
        if lead >= buffered_seconds {
            while clock.lead_secs(current_granule) >= buffered_seconds {
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
        let vorbis_config = VorbisConfig {
            sample_rate: 44100,
            bitrate: VorbisBitrateMode::CBR(320),
        };

        let silence = Self::load_default_silence(&vorbis_config);

        Self {
            buffer_config: BufferConfig {
                buffered_seconds: 10.0,
                max_chunk_size: 65536,
            },
            vorbis_config,
            silence,
            initial_serial: 0xfeed_0000,
        }
    }

    /// Configure buffer settings for the OggMux.
    fn load_default_silence(config: &VorbisConfig) -> SilenceTemplate {
        match config.silence_key().as_str() {
            "44100_192" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_44100_192.ogg"))
            }
            "44100_128" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_44100_128.ogg"))
            }
            "44100_320" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_44100_320.ogg"))
            }
            "48000_q6" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_48000_q6.ogg"))
            }
            _ => SilenceTemplate::new_embedded(include_bytes!("../resources/silence_default.ogg")),
        }
    }

    pub fn with_buffer_config(mut self, config: BufferConfig) -> Self {
        self.buffer_config = config;
        self
    }

    /// Configure Vorbis audio parameters for the OggMux.
    pub fn with_vorbis_config(mut self, config: VorbisConfig) -> Self {
        self.vorbis_config = config;
        self.silence = Self::load_default_silence(&self.vorbis_config);
        self
    }

    pub fn with_silence_template(mut self, silence: SilenceTemplate) -> Self {
        self.silence = silence;
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
        let buffered_seconds = self.buffer_config.buffered_seconds;

        let mut global_granule_position: u64 = 0;

        tokio::spawn(async move {
            debug!("OggMux main loop started");

            loop {
                // Shutdown if output is closed
                if output_tx.is_closed() {
                    debug!("Output channel closed; exiting mux loop");
                    break;
                }

                tokio::select! {
                    maybe_input = input_rx.recv() => {
                        match maybe_input {
                            Some(first_chunk) => {
                                let serial = self.next_serial();
                                debug!("Starting REAL stream, serial=0x{:x}", serial);

                                let mut session = StreamSession::new_real(serial, global_granule_position);

                                match session.stream_processor.process(&first_chunk) {
                                    Ok(out) => {
                                        if !out.is_empty() && !output_tx.is_closed() {
                                            session.maybe_sleep(&clock, buffered_seconds, global_granule_position).await;
                                            let _ = output_tx.send(out).await;
                                        }
                                        global_granule_position = session.stream_processor.get_granule_position();
                                    }
                                    Err(e) => {
                                        error!("Error processing first chunk: {:?}", e);
                                        continue;
                                    }
                                }

                                if let Err(e) = session.run(
                                    &mut input_rx,
                                    &output_tx,
                                    &clock,
                                    buffered_seconds,
                                    &mut global_granule_position
                                ).await {
                                    error!("Session run error: {:?}", e);
                                }
                            }
                            None => {
                                debug!("Input channel closed; exiting mux loop");
                                break;
                            }
                        }
                    }

                    // Optional timeout to insert silence if input is idle
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {
                        let serial = self.next_serial();
                        debug!("Inserting SILENCE stream, serial=0x{:x}", serial);

                        let silence_data = self.silence.raw_bytes().to_vec().into();
                        let mut session = StreamSession::new_silence(
                            serial,
                            global_granule_position,
                            silence_data,
                        );

                        if let Err(e) = session.run(
                            &mut input_rx,
                            &output_tx,
                            &clock,
                            buffered_seconds,
                            &mut global_granule_position
                        ).await {
                            error!("Silence session error: {:?}", e);
                        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vorbis_config_silence_key_cbr() {
        let cfg = VorbisConfig {
            sample_rate: 44100,
            bitrate: VorbisBitrateMode::CBR(192),
        };
        assert_eq!(cfg.silence_key(), "44100_192");
    }

    #[test]
    fn test_vorbis_config_silence_key_vbr() {
        let cfg = VorbisConfig {
            sample_rate: 48000,
            bitrate: VorbisBitrateMode::VBRQuality(6),
        };
        assert_eq!(cfg.silence_key(), "48000_q6");
    }

    #[test]
    fn test_default_mux_uses_expected_key() {
        let mux = OggMux::new();
        let key = mux.vorbis_config.silence_key();
        assert_eq!(key, "44100_320");
    }

    #[tokio::test]
    async fn test_mux_shutdown_behavior() {
        let mux = OggMux::new();
        let (input_tx, output_rx) = mux.spawn();
        drop(output_rx);
        let _ = input_tx.send(Bytes::from_static(b"example")).await;
        sleep(Duration::from_millis(200)).await;
        let result = input_tx.send(Bytes::from_static(b"after close")).await;
        assert!(result.is_err(), "Expected send to fail after shutdown");
    }
}
