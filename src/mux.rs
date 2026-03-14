use crate::metrics::MetricsCollector;
use anyhow::{Context, Result};
use bytes::Bytes;
use log::{debug, warn};
use std::time::Instant;
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep, timeout};

use crate::comments::{create_comment_page, generate_comment_packet};
use crate::silence::SilenceTemplate;
use crate::stream::StreamProcessor;
use crate::timing::StreamClock;

/// Timeout for output sends - prevents indefinite blocking if consumer is slow
const OUTPUT_SEND_TIMEOUT: Duration = Duration::from_secs(5);

/// Configuration for buffer management.
#[derive(Clone, Copy)]
pub struct BufferConfig {
    /// Target amount of audio to keep buffered (in seconds)
    pub buffered_seconds: f64,

    /// Maximum number of chunks that can be buffered in the channel
    pub channel_capacity: usize,
}

impl BufferConfig {
    /// Validate the buffer configuration.
    ///
    /// # Panics
    ///
    /// Panics if configuration values are invalid:
    /// - `buffered_seconds` must be positive
    /// - `channel_capacity` must be non-zero
    fn validate(&self) {
        assert!(
            self.buffered_seconds > 0.0,
            "BufferConfig: buffered_seconds must be positive, got {}",
            self.buffered_seconds
        );
        assert!(
            self.channel_capacity > 0,
            "BufferConfig: channel_capacity must be non-zero"
        );
    }
}

/// Bitrate mode for Vorbis encoding or template selection
#[derive(Clone, Copy, Debug)]
pub enum VorbisBitrateMode {
    /// Constant Bitrate (CBR)
    CBR(u32),
    /// Variable Bitrate (VBR) with quality level
    VBRQuality(u8),
}

/// Configuration for Vorbis audio parameters.
#[derive(Clone, Copy, Debug)]
pub struct VorbisConfig {
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Bitrate mode
    pub bitrate: VorbisBitrateMode,
}

impl VorbisConfig {
    /// Generate a key string for selecting a matching silence template
    pub fn silence_key(&self) -> String {
        match self.bitrate {
            VorbisBitrateMode::CBR(kbps) => format!("{}_{}", self.sample_rate, kbps),
            VorbisBitrateMode::VBRQuality(q) => format!("{}_q{}", self.sample_rate, q),
        }
    }

    /// Validate the Vorbis configuration.
    ///
    /// # Panics
    ///
    /// Panics if configuration values are invalid:
    /// - `sample_rate` must be non-zero (would cause division by zero in timing calculations)
    /// - For CBR mode, bitrate must be non-zero
    fn validate(&self) {
        assert!(
            self.sample_rate > 0,
            "VorbisConfig: sample_rate must be non-zero, got {}",
            self.sample_rate
        );
        match self.bitrate {
            VorbisBitrateMode::CBR(kbps) => {
                assert!(kbps > 0, "VorbisConfig: CBR bitrate must be non-zero");
            }
            VorbisBitrateMode::VBRQuality(_) => {
                // Quality levels 0-10 are all valid
            }
        }
    }
}

/// Operating mode for the muxer
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MuxMode {
    /// Insert silence between streams (original behavior for Icecast).
    ///
    /// Silence uses a fixed-duration template (embedded .ogg file). The granule
    /// position is remapped to ensure timing continuity across stream boundaries,
    /// but the actual audio silence duration is fixed per template regardless of
    /// how long the input gap lasts. Multiple silence segments may be inserted
    /// back-to-back for longer gaps.
    WithSilence,
    /// Direct stream concatenation without silence gaps (gapless playback)
    Gapless,
}

/// Callback type for metadata injection at track boundaries.
///
/// The callback receives the current granule position and returns an optional
/// vector of (key, value) comment pairs to inject as a Vorbis comment packet.
pub type MetadataCallback = Box<dyn Fn(u64) -> Option<Vec<(String, String)>> + Send + Sync>;

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
    ///
    /// If `initial_data` is provided, it is processed as the first chunk before
    /// entering the main receive loop.
    #[allow(clippy::too_many_arguments)]
    async fn run(
        &mut self,
        input_rx: &mut mpsc::Receiver<Bytes>,
        output_tx: &mpsc::Sender<Bytes>,
        clock: &StreamClock,
        buffered_seconds: f64,
        granule_position_ref: &mut u64,
        metrics_collector: Option<MetricsCollector>,
        initial_data: Option<Bytes>,
    ) -> Result<()> {
        if self.is_silence {
            if let Some(ref template) = self.silence_template {
                let out = self
                    .stream_processor
                    .process(template)
                    .context("processing silence template")?;
                if !out.is_empty() {
                    self.maybe_sleep(clock, buffered_seconds, *granule_position_ref)
                        .await;
                    match timeout(OUTPUT_SEND_TIMEOUT, output_tx.send(out.clone())).await {
                        Ok(Ok(())) => {}
                        Ok(Err(_)) => return Err(anyhow::anyhow!("output channel closed")),
                        Err(_) => {
                            warn!("Output send timeout - consumer too slow, dropping packet");
                        }
                    }

                    if let Some(ref mc) = metrics_collector {
                        mc.add_bytes_processed(out.len()).await;
                    }

                    *granule_position_ref = self.stream_processor.get_granule_position();
                }
            }
            let final_out = self
                .stream_processor
                .finalise()
                .context("finalising silence stream")?;
            if !final_out.is_empty() {
                self.maybe_sleep(clock, buffered_seconds, *granule_position_ref)
                    .await;
                match timeout(OUTPUT_SEND_TIMEOUT, output_tx.send(final_out.clone())).await {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) => return Err(anyhow::anyhow!("output channel closed")),
                    Err(_) => {
                        warn!(
                            "Output send timeout - consumer too slow, dropping final silence packet"
                        );
                    }
                }

                if let Some(ref mc) = metrics_collector {
                    mc.add_bytes_processed(final_out.len()).await;
                }
            }
            *granule_position_ref = self.stream_processor.get_granule_position();
            return Ok(());
        }

        // Process initial data if provided (first chunk from main loop)
        if let Some(data) = initial_data {
            self.process_chunk(
                &data,
                output_tx,
                clock,
                buffered_seconds,
                granule_position_ref,
                &metrics_collector,
            )
            .await?;
        }

        loop {
            if self.stream_processor.is_finished() {
                break;
            }

            // Throttle if ahead of real-time
            let lead = clock.lead_secs(*granule_position_ref);
            if lead >= buffered_seconds {
                sleep(Duration::from_millis(20)).await;
                continue;
            }

            tokio::select! {
                biased;
                maybe_data = input_rx.recv() => {
                    match maybe_data {
                        Some(data) => {
                            self.process_chunk(&data, output_tx, clock, buffered_seconds, granule_position_ref, &metrics_collector).await?;
                        }
                        None => break, // input closed
                    }
                }
                _ = sleep(Duration::from_millis(500)) => {
                    if !self.stream_processor.has_produced_output() {
                        debug!("No valid output after timeout; breaking to restart with silence");
                    }
                    // Current stream likely done — no new data for 500ms
                    break;
                }
            }
        }

        match self.stream_processor.finalise() {
            Ok(final_out) => {
                if !final_out.is_empty() {
                    self.maybe_sleep(clock, buffered_seconds, *granule_position_ref)
                        .await;
                    match timeout(OUTPUT_SEND_TIMEOUT, output_tx.send(final_out.clone())).await {
                        Ok(Ok(())) => {}
                        Ok(Err(_)) => return Err(anyhow::anyhow!("output channel closed")),
                        Err(_) => {
                            warn!(
                                "Output send timeout - consumer too slow, dropping final real audio packet"
                            );
                        }
                    }

                    if let Some(mc) = &metrics_collector {
                        mc.add_bytes_processed(final_out.len()).await;
                    }
                }
            }
            Err(e) => {
                warn!("Error finalising real stream: {}", e);
            }
        }
        *granule_position_ref = self.stream_processor.get_granule_position();
        Ok(())
    }

    /// Process a single chunk of real audio data, sending output and recording metrics.
    async fn process_chunk(
        &mut self,
        data: &[u8],
        output_tx: &mpsc::Sender<Bytes>,
        clock: &StreamClock,
        buffered_seconds: f64,
        granule_position_ref: &mut u64,
        metrics_collector: &Option<MetricsCollector>,
    ) -> Result<()> {
        let process_start = Instant::now();
        let out = match self.stream_processor.process(data) {
            Ok(out) => out,
            Err(e) => {
                warn!("Error processing chunk in stream session: {}", e);
                return Ok(()); // Non-fatal: skip this chunk
            }
        };

        if let Some(mc) = metrics_collector {
            let latency = process_start.elapsed().as_secs_f64() * 1000.0;
            mc.record_processing_latency(latency).await;
        }

        if !out.is_empty() {
            self.maybe_sleep(clock, buffered_seconds, *granule_position_ref)
                .await;
            match timeout(OUTPUT_SEND_TIMEOUT, output_tx.send(out.clone())).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => return Err(anyhow::anyhow!("output channel closed")),
                Err(_) => {
                    warn!("Output send timeout - consumer too slow, dropping real audio packet");
                }
            }

            if let Some(mc) = metrics_collector {
                mc.add_bytes_processed(out.len()).await;
            }
        }
        *granule_position_ref = self.stream_processor.get_granule_position();

        if let Some(mc) = metrics_collector {
            let lead = clock.lead_secs(*granule_position_ref);
            let utilization = (lead / buffered_seconds) * 100.0;
            mc.record_buffer_utilization(utilization).await;
        }

        Ok(())
    }

    /// Sleep if we're generating output faster than real-time
    async fn maybe_sleep(&self, clock: &StreamClock, buffered_seconds: f64, current_granule: u64) {
        while clock.lead_secs(current_granule) >= buffered_seconds {
            sleep(Duration::from_millis(20)).await;
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
    metrics_collector: Option<crate::metrics::MetricsCollector>,
    mode: MuxMode,
    metadata_callback: Option<MetadataCallback>,
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
                channel_capacity: 65536,
            },
            vorbis_config,
            silence,
            initial_serial: 0xfeed_0000,
            metrics_collector: None,
            mode: MuxMode::WithSilence,
            metadata_callback: None,
        }
    }

    /// Configure buffer settings for the OggMux.
    ///
    /// # Panics
    ///
    /// Panics if the configuration is invalid (e.g., non-positive buffered_seconds,
    /// zero channel_capacity).
    pub fn with_buffer_config(mut self, config: BufferConfig) -> Self {
        config.validate();
        self.buffer_config = config;
        self
    }

    /// Configure Vorbis audio parameters for the OggMux.
    ///
    /// # Panics
    ///
    /// Panics if the configuration is invalid (e.g., zero sample_rate which would
    /// cause division by zero in timing calculations, zero CBR bitrate).
    pub fn with_vorbis_config(mut self, config: VorbisConfig) -> Self {
        config.validate();
        self.vorbis_config = config;
        self.silence = Self::load_default_silence(&self.vorbis_config);
        self
    }

    /// Override the silence template used for silence streams.
    pub fn with_silence_template(mut self, silence: SilenceTemplate) -> Self {
        self.silence = silence;
        self
    }

    /// Add metrics collection to the OggMux.
    ///
    /// Enables collecting performance metrics such as buffer utilization,
    /// latency, and silence insertion statistics.
    pub fn with_metrics(mut self) -> Self {
        self.metrics_collector = Some(crate::metrics::MetricsCollector::new());
        self
    }

    /// Get access to the metrics collector, if enabled.
    ///
    /// Returns the metrics collector instance if it was enabled
    /// with `with_metrics()`, or None if metrics collection is disabled.
    pub fn metrics(&self) -> Option<crate::metrics::MetricsCollector> {
        self.metrics_collector.clone()
    }

    /// Set the operating mode for the muxer.
    ///
    /// - `MuxMode::WithSilence`: Insert silence between streams (default, for Icecast)
    /// - `MuxMode::Gapless`: Direct concatenation without silence gaps (for gapless playback)
    pub fn with_mode(mut self, mode: MuxMode) -> Self {
        self.mode = mode;
        self
    }

    /// Set a callback that will be invoked at track boundaries to inject metadata.
    ///
    /// The callback receives the current granule position and returns an optional
    /// vector of (key, value) comment pairs to inject into the stream as a Vorbis
    /// comment packet.
    ///
    /// # Example
    /// ```
    /// use oggmux::{OggMux, MuxMode};
    ///
    /// let mux = OggMux::new()
    ///     .with_mode(MuxMode::Gapless)
    ///     .with_metadata_callback(|granule_pos| {
    ///         Some(vec![
    ///             ("TITLE".to_string(), "My Song".to_string()),
    ///             ("ARTIST".to_string(), "My Artist".to_string()),
    ///             ("CUSTOM_GRANULE".to_string(), granule_pos.to_string()),
    ///         ])
    ///     });
    /// ```
    pub fn with_metadata_callback<F>(mut self, callback: F) -> Self
    where
        F: Fn(u64) -> Option<Vec<(String, String)>> + Send + Sync + 'static,
    {
        self.metadata_callback = Some(Box::new(callback));
        self
    }

    /// Load the default embedded silence template based on the Vorbis config.
    fn load_default_silence(config: &VorbisConfig) -> SilenceTemplate {
        let key = config.silence_key();
        match key.as_str() {
            // 44.1 kHz CBR templates
            "44100_64" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_44100_64.ogg"))
            }
            "44100_96" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_44100_96.ogg"))
            }
            "44100_128" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_44100_128.ogg"))
            }
            "44100_160" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_44100_160.ogg"))
            }
            "44100_192" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_44100_192.ogg"))
            }
            "44100_256" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_44100_256.ogg"))
            }
            "44100_320" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_44100_320.ogg"))
            }
            // 44.1 kHz VBR templates
            "44100_q2" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_44100_q2.ogg"))
            }
            "44100_q4" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_44100_q4.ogg"))
            }
            "44100_q5" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_44100_q5.ogg"))
            }
            "44100_q6" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_44100_q6.ogg"))
            }
            "44100_q8" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_44100_q8.ogg"))
            }
            // 48 kHz CBR templates
            "48000_64" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_48000_64.ogg"))
            }
            "48000_96" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_48000_96.ogg"))
            }
            "48000_128" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_48000_128.ogg"))
            }
            "48000_160" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_48000_160.ogg"))
            }
            "48000_192" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_48000_192.ogg"))
            }
            "48000_256" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_48000_256.ogg"))
            }
            "48000_320" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_48000_320.ogg"))
            }
            // 48 kHz VBR templates
            "48000_q2" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_48000_q2.ogg"))
            }
            "48000_q4" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_48000_q4.ogg"))
            }
            "48000_q5" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_48000_q5.ogg"))
            }
            "48000_q6" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_48000_q6.ogg"))
            }
            "48000_q8" => {
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_48000_q8.ogg"))
            }
            _ => {
                warn!(
                    "No silence template for config '{}' (sample_rate={}, bitrate={:?}), using default (44100_320)",
                    key, config.sample_rate, config.bitrate
                );
                SilenceTemplate::new_embedded(include_bytes!("../resources/silence_default.ogg"))
            }
        }
    }

    /// Generate a new serial number each time we spawn a stream.
    fn next_serial(&mut self) -> u32 {
        let s = self.initial_serial;
        self.initial_serial = self.initial_serial.wrapping_add(1);
        s
    }

    /// Spawn the main muxer loop.
    ///
    /// Returns `(input_tx, output_rx, shutdown_tx, handle)` where:
    /// - `input_tx`: Send real Ogg data chunks to be muxed
    /// - `output_rx`: Receive muxed output (real audio + silence as needed)
    /// - `shutdown_tx`: Send `()` to gracefully shut down the muxer
    /// - `handle`: JoinHandle to await task completion and detect errors
    ///
    /// The muxer automatically inserts silence if input is idle,
    /// and manages transitions between real audio and silence to maintain
    /// proper timing.
    ///
    /// # Graceful Shutdown
    ///
    /// To shut down cleanly:
    /// ```no_run
    /// # use oggmux::OggMux;
    /// # #[tokio::main]
    /// # async fn main() {
    /// let (input_tx, output_rx, shutdown_tx, handle) = OggMux::new().spawn();
    ///
    /// // ... use the muxer ...
    ///
    /// // Signal shutdown
    /// let _ = shutdown_tx.send(()).await;
    ///
    /// // Wait for clean exit
    /// match handle.await {
    ///     Ok(Ok(())) => println!("Muxer exited cleanly"),
    ///     Ok(Err(e)) => eprintln!("Muxer error: {:?}", e),
    ///     Err(e) => eprintln!("Task panicked: {:?}", e),
    /// }
    /// # }
    /// ```
    pub fn spawn(
        mut self,
    ) -> (
        mpsc::Sender<Bytes>,
        mpsc::Receiver<Bytes>,
        mpsc::Sender<()>,
        tokio::task::JoinHandle<Result<()>>,
    ) {
        let (input_tx, mut input_rx) = mpsc::channel::<Bytes>(self.buffer_config.channel_capacity);
        let (output_tx, output_rx) = mpsc::channel::<Bytes>(self.buffer_config.channel_capacity);
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
        let clock = StreamClock::new(self.vorbis_config.sample_rate);
        let buffered_seconds = self.buffer_config.buffered_seconds;
        let mut global_granule_position = 0u64;
        let metrics_collector = self.metrics_collector.clone();

        let handle = tokio::spawn(async move {
            // Wrap in Result for `?`
            let run_res: Result<()> = async {
                debug!("OggMux main loop started");
                let mut pending_metadata: Option<Vec<(String, String)>> = None;
                loop {
                    // Shutdown if output is closed
                    if output_tx.is_closed() {
                        debug!("Output channel closed; exiting mux loop");
                        break;
                    }

                    tokio::select! {
                        biased;
                        _ = shutdown_rx.recv() => {
                            debug!("Shutdown signal received; exiting mux loop");
                            break;
                        }
                        maybe_input = input_rx.recv() => {
                            match maybe_input {
                                Some(first_chunk) => {
                                    let serial = self.next_serial();
                                    debug!("Starting REAL stream, serial=0x{:x}", serial);

                                    let mut session = StreamSession::new_real(serial, global_granule_position);

                                    // Inject pending metadata with this new stream's serial
                                    if let Some(comments) = pending_metadata.take() {
                                        debug!("Injecting pending metadata into new stream serial=0x{:x}", serial);
                                        match generate_comment_packet(comments) {
                                            Ok(packet) => {
                                                match create_comment_page(packet, serial, 0, global_granule_position) {
                                                    Ok(page) => {
                                                        match timeout(OUTPUT_SEND_TIMEOUT, output_tx.send(page)).await {
                                                            Ok(Ok(())) => {},
                                                            Ok(Err(_)) => return Err(anyhow::anyhow!("output channel closed")),
                                                            Err(_) => {
                                                                warn!("Output send timeout - dropping metadata page");
                                                            }
                                                        }
                                                    }
                                                    Err(e) => warn!("Failed to create metadata page: {}", e),
                                                }
                                            }
                                            Err(e) => warn!("Failed to generate metadata packet: {}", e),
                                        }
                                    }

                                    // Record real stream in metrics
                                    if let Some(ref mc) = metrics_collector {
                                        mc.increment_real_streams().await;
                                    }

                                    // Run session with first chunk as initial data
                                    session.run(
                                        &mut input_rx, &output_tx, &clock, buffered_seconds,
                                        &mut global_granule_position, metrics_collector.clone(),
                                        Some(first_chunk),
                                    ).await?;

                                    // Store pending metadata for injection at the start of the next stream
                                    if let Some(callback) = &self.metadata_callback
                                        && let Some(comments) = callback(global_granule_position)
                                    {
                                        debug!("Storing pending metadata for next stream (granule={})", global_granule_position);
                                        pending_metadata = Some(comments);
                                    }
                                }
                                None => {
                                    debug!("Input channel closed; exiting mux loop");
                                    break;
                                }
                            }
                        }
                        // Optional timeout to insert silence if input is idle
                        _ = sleep(Duration::from_millis(100)) => {
                            // Only insert silence if not in gapless mode
                            if self.mode == MuxMode::WithSilence {
                                let serial = self.next_serial();
                                debug!("Inserting SILENCE stream, serial=0x{:x}", serial);

                                let silence_data: Bytes = self.silence.raw_bytes().to_vec().into();
                                let mut session = StreamSession::new_silence(
                                    serial,
                                    global_granule_position,
                                    silence_data.clone(),
                                );

                                // Record silence insertion in metrics
                                if let Some(ref mc) = metrics_collector {
                                    mc.add_silence_bytes(silence_data.len()).await;
                                }

                                session.run(&mut input_rx, &output_tx, &clock, buffered_seconds, &mut global_granule_position, metrics_collector.clone(), None).await?;
                            } else {
                                debug!("Gapless mode: waiting for input without silence");
                            }
                        }
                    }
                }
                debug!("OggMux main loop ended");
                Ok(())
            }
            .await;

            run_res
        });

        (input_tx, output_rx, shutdown_tx, handle)
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
    use tokio::time::Duration;

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

    #[tokio::test]
    async fn test_mux_shutdown_behavior() {
        let mux = OggMux::new();
        let (_input_tx, output_rx, _shutdown_tx, handle) = mux.spawn();
        drop(output_rx);

        // Task should exit cleanly when output is dropped
        let result = tokio::time::timeout(Duration::from_millis(200), handle).await;
        assert!(result.is_ok(), "Task should exit when output dropped");
        assert!(result.unwrap().is_ok(), "Task should not panic");
    }

    #[tokio::test]
    async fn test_mux_graceful_shutdown() {
        let mux = OggMux::new();
        let (_input_tx, _output_rx, shutdown_tx, handle) = mux.spawn();

        // Signal shutdown
        shutdown_tx
            .send(())
            .await
            .expect("Failed to send shutdown signal");

        // Task should exit cleanly
        let result = tokio::time::timeout(Duration::from_millis(200), handle).await;
        assert!(result.is_ok(), "Task should exit on shutdown signal");
        let task_result = result.unwrap().expect("Task should not panic");
        assert!(task_result.is_ok(), "Task should exit without error");
    }
}
