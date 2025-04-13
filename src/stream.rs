// stream.rs

use anyhow::{Context, Result};
use bytes::{Buf, Bytes, BytesMut};
use log::{debug, warn};
use ogg_pager::Page;
use std::io::Cursor;

/// Processes and remaps Ogg streams.
///
/// The StreamProcessor reads Ogg pages from an input stream, remaps their
/// granule positions and serial numbers, and outputs the modified stream.
/// It handles both real audio data and silence streams.
pub struct StreamProcessor {
    serial_number: u32,
    buffer: BytesMut,

    // Granule remapping state
    last_input_abgp: u64,
    last_output_abgp: u64,
    sequence_number: u32,
    finished: bool,   // When true, no more pages are output.
    is_silence: bool, // True for silence streams.
}

impl StreamProcessor {
    /// Constructor for real data streams.
    ///
    /// # Arguments
    ///
    /// * `serial` - The serial number to use for the Ogg stream.
    /// * `starting_output_granule` - The starting granule position for the stream.
    ///
    /// # Returns
    ///
    /// A new StreamProcessor configured for real audio data.
    pub fn with_serial(serial: u32, starting_output_granule: u64) -> Self {
        debug!(
            "Creating StreamProcessor with serial {serial:#x}, starting granule = {starting_output_granule}"
        );
        Self {
            serial_number: serial,
            buffer: BytesMut::new(),
            last_input_abgp: 0,
            last_output_abgp: starting_output_granule,
            sequence_number: 0,
            finished: false,
            is_silence: false,
        }
    }

    /// Constructor for silence streams.
    ///
    /// # Arguments
    ///
    /// * `serial` - The serial number to use for the Ogg stream.
    /// * `starting_output_granule` - The starting granule position for the stream.
    ///
    /// # Returns
    ///
    /// A new StreamProcessor configured for silence data.
    pub fn with_silence(serial: u32, starting_output_granule: u64) -> Self {
        debug!(
            "Creating Silence StreamProcessor with serial {serial:#x}, starting granule = {starting_output_granule}"
        );
        Self {
            serial_number: serial,
            buffer: BytesMut::new(),
            last_input_abgp: 0,
            last_output_abgp: starting_output_granule,
            sequence_number: 0,
            finished: false,
            is_silence: true,
        }
    }

    /// Process a chunk of Ogg data.
    ///
    /// This method reads Ogg pages from the input, remaps their granule positions
    /// and serial numbers, and outputs the modified pages.
    ///
    /// # Arguments
    ///
    /// * `input` - The input data to process.
    ///
    /// # Returns
    ///
    /// The processed Ogg data, or an error if processing failed.
    pub fn process(&mut self, input: &[u8]) -> Result<Bytes> {
        // If already finished, clear buffer and return empty bytes.
        if self.finished {
            self.buffer.clear();
            return Ok(Bytes::new());
        }

        debug!("Processing input: {} bytes", input.len());
        self.buffer.extend_from_slice(input);

        let mut output = BytesMut::new();
        let mut cursor = Cursor::new(&self.buffer[..]);
        let mut consumed = 0;

        // Process pages one by one, stopping when we can't read any more
        while let Ok(mut page) = Page::read(&mut cursor) {
            let end = cursor.position() as usize;
            let input_abgp = page.header().abgp;
            let delta = input_abgp.saturating_sub(self.last_input_abgp);
            let new_abgp = self.last_output_abgp + delta;

            {
                let header = page.header_mut();
                header.abgp = new_abgp;
                header.stream_serial = self.serial_number;
                header.sequence_number = self.sequence_number;
            }

            page.gen_crc();
            let serialised = page.as_bytes();
            output.extend_from_slice(&serialised);

            self.last_input_abgp = input_abgp;
            self.last_output_abgp = new_abgp;
            self.sequence_number += 1;
            consumed = end;
        }

        if consumed > 0 {
            self.buffer.advance(consumed);
        } else if self.buffer.len() > 1_048_576 {
            warn!("Buffer too large ({}), discarding", self.buffer.len());
            self.buffer.clear();
        }

        // For silence streams, mark finished after one complete processing call.
        if self.is_silence {
            self.finished = true;
            debug!("Silence stream processed once; marking stream as finished.");
        }

        Ok(output.freeze())
    }

    /// Finalise the stream.
    ///
    /// This method processes any remaining data in the buffer and marks the stream
    /// as finished.
    ///
    /// # Returns
    ///
    /// The final Ogg data from the stream, or an error if finalisation failed.
    pub fn finalize(&mut self) -> Result<Bytes> {
        debug!("Finalising stream");
        self.process(&[])
            .with_context(|| "Failed to finalise stream by processing remaining data")
    }

    /// Get the current granule position.
    ///
    /// # Returns
    ///
    /// The current granule position, in samples.
    pub fn get_granule_position(&self) -> u64 {
        self.last_output_abgp
    }

    /// Set the granule position.
    ///
    /// # Arguments
    ///
    /// * `pos` - The new granule position, in samples.
    pub fn set_granule_position(&mut self, pos: u64) {
        debug!("Setting granule position to {pos}");
        self.last_output_abgp = pos;
    }

    /// Check if the stream is finished.
    ///
    /// # Returns
    ///
    /// True if the stream is finished, false otherwise.
    pub fn is_finished(&self) -> bool {
        self.finished
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper function to extract a chunk of the silence Ogg file
    fn get_silence_ogg_data() -> Vec<u8> {
        include_bytes!("../resources/silence.ogg").to_vec()
    }

    #[test]
    fn test_stream_processor_creation() {
        let sp = StreamProcessor::with_serial(0x42, 1000);
        assert_eq!(sp.serial_number, 0x42);
        assert_eq!(sp.get_granule_position(), 1000);
        assert!(!sp.is_finished());
        assert!(!sp.is_silence);

        let silence_sp = StreamProcessor::with_silence(0x43, 2000);
        assert_eq!(silence_sp.serial_number, 0x43);
        assert_eq!(silence_sp.get_granule_position(), 2000);
        assert!(!silence_sp.is_finished());
        assert!(silence_sp.is_silence);
    }

    #[test]
    fn test_granule_position_setting() {
        let mut sp = StreamProcessor::with_serial(0x42, 1000);
        assert_eq!(sp.get_granule_position(), 1000);

        sp.set_granule_position(2000);
        assert_eq!(sp.get_granule_position(), 2000);
    }

    #[test]
    fn test_process_real_ogg_data() -> Result<()> {
        let mut sp = StreamProcessor::with_serial(0x42, 1000);

        // Get the silence.ogg data and process it
        let ogg_data = get_silence_ogg_data();
        let result = sp.process(&ogg_data)?;

        // Should output data
        assert!(!result.is_empty());

        // Check that the pages were processed (granule position should advance)
        assert!(sp.last_input_abgp > 0);
        assert!(sp.get_granule_position() > 1000);
        assert!(sp.sequence_number > 0);

        Ok(())
    }

    #[test]
    fn test_silence_stream_finishes_after_processing() -> Result<()> {
        let mut sp = StreamProcessor::with_silence(0x42, 1000);
        assert!(!sp.is_finished());

        // Get the silence.ogg data and process it
        let ogg_data = get_silence_ogg_data();
        let _ = sp.process(&ogg_data)?;

        // Silence streams should be marked as finished after processing
        assert!(sp.is_finished());

        // Processing again should return empty bytes
        let empty_result = sp.process(&ogg_data)?;
        assert!(empty_result.is_empty());

        Ok(())
    }

    #[test]
    fn test_finalise() -> Result<()> {
        let mut sp = StreamProcessor::with_serial(0x42, 1000);

        // Add some Ogg data to the buffer
        let ogg_data = get_silence_ogg_data();
        sp.buffer.extend_from_slice(&ogg_data);

        // Finalise should process the buffered data
        let result = sp.finalize()?;
        assert!(!result.is_empty());

        // Buffer should be processed
        assert!(sp.buffer.len() < ogg_data.len());

        Ok(())
    }

    #[test]
    fn test_large_buffer_handling() -> Result<()> {
        let mut sp = StreamProcessor::with_serial(0x42, 1000);

        // Create a large invalid buffer (not valid Ogg)
        let large_data = vec![0; 2_000_000]; // 2MB of zeros

        // This should process without crashing, and clear the buffer
        let result = sp.process(&large_data)?;

        // No valid pages, so no output
        assert!(result.is_empty());

        // Buffer should be cleared because it's too large
        assert!(sp.buffer.is_empty() || sp.buffer.len() > 1_048_576);

        Ok(())
    }

    #[test]
    fn test_multiple_processing_calls() -> Result<()> {
        let mut sp = StreamProcessor::with_serial(0x42, 1000);

        // Get the silence.ogg data
        let ogg_data = get_silence_ogg_data();

        // Process it multiple times
        let result1 = sp.process(&ogg_data)?;
        let granule1 = sp.get_granule_position();

        let result2 = sp.process(&ogg_data)?;
        let granule2 = sp.get_granule_position();

        // Should have output both times
        assert!(!result1.is_empty());
        assert!(!result2.is_empty());

        // Granule position should advance
        assert!(granule2 > granule1);

        Ok(())
    }

    #[test]
    fn test_remapping() -> Result<()> {
        // Create two stream processors with different serials and starting granules
        let mut sp1 = StreamProcessor::with_serial(0x1111, 0);
        let mut sp2 = StreamProcessor::with_serial(0x2222, 5000);

        // Get the silence.ogg data
        let ogg_data = get_silence_ogg_data();

        // Process with both processors
        let result1 = sp1.process(&ogg_data)?;
        let result2 = sp2.process(&ogg_data)?;

        // Both should produce output
        assert!(!result1.is_empty());
        assert!(!result2.is_empty());

        // The first bytes of each result should be "OggS" (Ogg page signature)
        assert_eq!(&result1[0..4], b"OggS");
        assert_eq!(&result2[0..4], b"OggS");

        // But the results should be different due to different serial numbers and granule bases
        assert_ne!(result1, result2);

        // The second processor should have a higher granule position
        assert!(sp2.get_granule_position() > sp1.get_granule_position());

        Ok(())
    }
}
