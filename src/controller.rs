use crate::silence::SilenceTemplate;
use crate::stream::StreamProcessor;
use anyhow::{Context, Result};
use bytes::Bytes;
use log::{debug, trace};

/// Controls muxing across real and silence streams.
///
/// The MuxController manages the creation and transition between different
/// audio streams, ensuring proper granule position continuity and timing.
/// It handles switching between real audio data and silence frames when needed.
pub struct MuxController {
    silence: SilenceTemplate,
    next_serial: u32,
    current: Option<StreamProcessor>,
    granule_position: u64,
}

impl MuxController {
    /// Create a new MuxController.
    ///
    /// # Arguments
    ///
    /// * `silence` - The silence template to use for generating silence frames.
    /// * `initial_serial` - The initial serial number to use for Ogg streams.
    pub fn new(silence: SilenceTemplate, initial_serial: u32) -> Self {
        debug!("MuxController initialised with serial base: {initial_serial:#x}");
        Self {
            silence,
            next_serial: initial_serial,
            current: None,
            granule_position: 0,
        }
    }

    /// Allocate a new serial number for a stream.
    ///
    /// Each stream in an Ogg file must have a unique serial number.
    /// This method generates a new serial number for each stream.
    fn alloc_serial(&mut self) -> u32 {
        let serial = self.next_serial;
        self.next_serial = self.next_serial.wrapping_add(1);
        debug!("Allocated new serial: {serial:#x}");
        serial
    }

    /// Get the current granule position.
    ///
    /// The granule position represents the current time position in the audio stream,
    /// measured in samples since the beginning of the stream.
    pub fn get_granule_position(&self) -> u64 {
        self.granule_position
    }

    /// Create a new stream.
    ///
    /// For silence streams, we pass the current granule position
    /// so that the new stream starts where the last one left off.
    ///
    /// # Arguments
    ///
    /// * `use_silence` - If true, create a silence stream. Otherwise, create a real audio stream.
    pub fn start_stream(&mut self, use_silence: bool) {
        let serial = self.alloc_serial();
        debug!(
            "Starting new {}stream with serial {serial:#x}",
            if use_silence { "silence " } else { "" }
        );

        let stream = if use_silence {
            self.silence.spawn_stream(serial, self.granule_position)
        } else {
            StreamProcessor::with_serial(serial, self.granule_position)
        };

        self.current = Some(stream);
    }

    /// Process incoming data (either real or silence).
    ///
    /// This method ensures that granule positions are accumulated across streams,
    /// maintaining continuous timing.
    ///
    /// # Arguments
    ///
    /// * `data` - The data to process. If None, process a silence frame.
    ///
    /// # Returns
    ///
    /// The processed Ogg data, or an error if processing failed.
    pub fn process(&mut self, data: Option<Bytes>) -> Result<Bytes> {
        // Drop the current stream if it is finished.
        if self
            .current
            .as_ref()
            .map(|s| s.is_finished())
            .unwrap_or(false)
        {
            self.current = None;
        }

        if self.current.is_none() {
            self.start_stream(data.is_none());
        }

        let result = {
            let stream = self
                .current
                .as_mut()
                .expect("Stream must be active when processing");

            match data {
                Some(ref real_data) => {
                    debug!("Processing real data ({} bytes)", real_data.len());
                    stream.process(real_data).with_context(|| {
                        format!("Failed to process {} bytes of real data", real_data.len())
                    })?
                }
                None => {
                    debug!("Processing silence frame");
                    stream
                        .process(self.silence.raw_bytes())
                        .with_context(|| "Failed to process silence frame")?
                }
            }
        };

        // For silence streams, if no data is returned, finalise immediately.
        if data.is_none() && result.is_empty() {
            let _ = self
                .finalize()
                .with_context(|| "Failed to finalise stream after empty silence processing")?;
        }
        // For real data or non-empty results, update the granule position.
        else if !result.is_empty() {
            if let Some(ref stream) = self.current {
                self.granule_position = stream.get_granule_position();
                debug!("Updated granule position to {}", self.granule_position);
            }
        } else {
            trace!("No data emitted by stream processor, finalising stream");
            let _ = self
                .finalize()
                .with_context(|| "Failed to finalise stream after empty output")?;
        }

        Ok(result)
    }

    /// Finalise the current stream.
    ///
    /// This call takes the current stream out, finalises it, and—crucially—updates
    /// the controller's granule_position to the stream's final output granule so
    /// that the next stream will start with an increasing timestamp.
    ///
    /// # Returns
    ///
    /// The final Ogg data from the stream, or an error if finalisation failed.
    pub fn finalize(&mut self) -> Result<Bytes> {
        if let Some(mut stream) = self.current.take() {
            debug!(
                "Finalising current stream with (previous) granule position {}",
                self.granule_position
            );
            // Set the stream's starting granule based on the controller.
            stream.set_granule_position(self.granule_position);

            let result = stream
                .finalize()
                .with_context(|| "Failed to finalise stream")?;

            // Update the global granule position from the finalised stream.
            self.granule_position = stream.get_granule_position();
            debug!(
                "Updated global granule position to {}",
                self.granule_position
            );
            Ok(result)
        } else {
            debug!("Finalise called with no active stream");
            Ok(Bytes::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    #[test]
    fn test_controller_creation() {
        let silence = SilenceTemplate::new_embedded(include_bytes!("../resources/silence.ogg"));
        let controller = MuxController::new(silence, 0x12345678);

        assert_eq!(controller.get_granule_position(), 0);
        assert_eq!(controller.next_serial, 0x12345678);
        assert!(controller.current.is_none());
    }

    #[test]
    fn test_start_stream() {
        let silence = SilenceTemplate::new_embedded(include_bytes!("../resources/silence.ogg"));
        let mut controller = MuxController::new(silence, 0x12345678);

        // Start a regular stream
        controller.start_stream(false);
        assert!(controller.current.is_some());
        assert_eq!(controller.next_serial, 0x12345679); // Serial should be incremented

        // Get the stream and check its properties
        let stream = controller.current.as_ref().unwrap();
        assert_eq!(stream.get_granule_position(), 0);
        assert!(!stream.is_finished());

        // Start a silence stream
        controller.start_stream(true);
        assert!(controller.current.is_some());
        assert_eq!(controller.next_serial, 0x1234567A); // Serial incremented again

        // Get the stream and check its properties
        let stream = controller.current.as_ref().unwrap();
        assert_eq!(stream.get_granule_position(), 0);
        assert!(!stream.is_finished());
    }

    #[tokio::test]
    async fn test_process_silence() -> Result<()> {
        let silence = SilenceTemplate::new_embedded(include_bytes!("../resources/silence.ogg"));
        let mut controller = MuxController::new(silence, 0x12345678);

        // Process silence (None data)
        let result = controller.process(None)?;

        // Should have some output
        assert!(!result.is_empty());

        // The silence stream should be finished after processing
        assert!(controller.current.is_none() || controller.current.as_ref().unwrap().is_finished());

        Ok(())
    }

    #[tokio::test]
    async fn test_process_real_data() -> Result<()> {
        // Create new SilenceTemplate for the controller
        let silence = SilenceTemplate::new_embedded(include_bytes!("../resources/silence.ogg"));
        let mut controller = MuxController::new(silence, 0x12345678);

        // Get silence ogg data to use as our test input
        let ogg_data = include_bytes!("../resources/silence.ogg").to_vec();

        // Process real data
        let result = controller.process(Some(Bytes::from(ogg_data)))?;

        // Should have some output
        assert!(!result.is_empty());

        // The granule position should have been updated
        assert!(controller.get_granule_position() > 0);

        Ok(())
    }

    #[tokio::test]
    async fn test_finalize() -> Result<()> {
        let silence = SilenceTemplate::new_embedded(include_bytes!("../resources/silence.ogg"));
        let mut controller = MuxController::new(silence, 0x12345678);

        // Start a stream
        controller.start_stream(false);

        // Finalise it
        let _result = controller.finalize()?;

        // Since no data was processed, the result might be empty
        // but the controller should have no current stream
        assert!(controller.current.is_none());

        // Finalising with no active stream should work
        let empty_result = controller.finalize()?;
        assert!(empty_result.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_alternating_streams() -> Result<()> {
        let silence = SilenceTemplate::new_embedded(include_bytes!("../resources/silence.ogg"));
        let mut controller = MuxController::new(silence, 0x12345678);

        // Process silence
        let _ = controller.process(None)?;
        let silence_granule = controller.get_granule_position();
        assert!(silence_granule > 0);

        // Get silence ogg data to use as our test input for real data
        let ogg_data = include_bytes!("../resources/silence.ogg").to_vec();

        // Process real data
        let _ = controller.process(Some(Bytes::from(ogg_data)))?;
        let real_granule = controller.get_granule_position();
        assert!(real_granule > silence_granule);

        // Process silence again
        let _ = controller.process(None)?;
        let final_granule = controller.get_granule_position();
        assert!(final_granule > real_granule);

        Ok(())
    }
}
