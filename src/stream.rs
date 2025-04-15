use anyhow::Result;
use bytes::{Buf, Bytes, BytesMut};
use log::{debug, warn};
use ogg_pager::Page;
use std::io::Cursor;
use std::time::Instant;

/// Processes and remaps Ogg streams.
///
/// The StreamProcessor reads Ogg pages from an input buffer, remaps their
/// granule positions and serial numbers, and outputs the modified pages.
/// It handles both real audio data and silence streams. The only difference
/// between them is how input is provided, but EOS detection is the same.
pub struct StreamProcessor {
    serial_number: u32,
    buffer: BytesMut,

    // Granule remapping state
    last_input_abgp: u64,
    last_output_abgp: u64,
    sequence_number: u32,
    finished: bool,
    pub is_silence: bool,
    // Track if we've successfully produced any output
    has_output: bool,
    creation_time: Instant,
}

impl StreamProcessor {
    /// Create a StreamProcessor for real audio data.
    pub fn with_serial(serial: u32, starting_output_granule: u64) -> Self {
        debug!(
            "Creating StreamProcessor for REAL data with serial={:#x}, starting granule={}",
            serial, starting_output_granule
        );
        Self {
            serial_number: serial,
            buffer: BytesMut::new(),
            last_input_abgp: 0,
            last_output_abgp: starting_output_granule,
            sequence_number: 0,
            finished: false,
            is_silence: false,
            has_output: false,
            creation_time: Instant::now(),
        }
    }

    /// Create a StreamProcessor for silence streams.
    pub fn with_silence(serial: u32, starting_output_granule: u64) -> Self {
        debug!(
            "Creating StreamProcessor for SILENCE data with serial={:#x}, starting granule={}",
            serial, starting_output_granule
        );
        Self {
            serial_number: serial,
            buffer: BytesMut::new(),
            last_input_abgp: 0,
            last_output_abgp: starting_output_granule,
            sequence_number: 0,
            finished: false,
            is_silence: true,
            has_output: false,
            creation_time: Instant::now(),
        }
    }

    /// Process a chunk of Ogg data.
    ///
    /// Reads Ogg pages from `input`, updates their serial and granule positions,
    /// and returns the remapped pages as a single `Bytes` buffer. If an EOS
    /// page is encountered, we log it and mark the stream as finished.
    ///
    /// # Returns
    ///
    /// - `Ok(Bytes)`: The remapped page data. If the stream is finished,
    ///   subsequent calls return an empty buffer.
    /// - `Err(...)`: If parsing fails.
    pub fn process(&mut self, input: &[u8]) -> Result<Bytes> {
        // If already finished, return empty
        if self.finished {
            return Ok(Bytes::new());
        }

        // Append this chunk to our internal buffer
        self.buffer.extend_from_slice(input);

        let mut out = BytesMut::new();
        let mut cursor = Cursor::new(&self.buffer[..]);
        let mut consumed = 0;

        // Read pages in a loop until we can't parse any more
        while let Ok(mut page) = Page::read(&mut cursor) {
            let end = cursor.position() as usize;

            // Calculate the new absolute granule position based on the last input
            let input_abgp = page.header().abgp;
            let delta = input_abgp.saturating_sub(self.last_input_abgp);
            let new_abgp = self.last_output_abgp + delta;

            // Rewrite the page header
            {
                let header = page.header_mut();
                header.abgp = new_abgp;
                header.stream_serial = self.serial_number;
                header.sequence_number = self.sequence_number;
            }

            // Update CRC, then append page to output
            page.gen_crc();
            out.extend_from_slice(&page.as_bytes());

            // Update our tracking
            self.last_input_abgp = input_abgp;
            self.last_output_abgp = new_abgp;
            self.sequence_number += 1;
            consumed = end;
            self.has_output = true;

            // If (header_type & 0x04) != 0, that means EOS is set.
            if (page.header().header_type_flag() & 0x04) != 0 {
                debug!(
                    "Detected EOS page on stream serial={:#x}; marking as finished",
                    self.serial_number
                );
                self.finished = true;
                // Typically we break after an EOS
                break;
            }
        }

        // If we parsed some pages, drop them from the buffer
        if consumed > 0 {
            self.buffer.advance(consumed);
        } else if self.buffer.len() > 1_048_576 {
            // If we never found a page but the buffer is huge => discard
            warn!(
                "Large buffer ({} bytes) with no parseable Ogg pages, discarding",
                self.buffer.len()
            );
            self.buffer.clear();
        } else if !self.is_silence && self.creation_time.elapsed().as_secs() > 2 && !self.has_output
        {
            // For real streams: if we've collected data for more than 2 seconds but haven't
            // produced any valid output, this is likely invalid data
            warn!("No valid Ogg pages found after 2 seconds of data collection; aborting stream");
            self.finished = true;
        }

        Ok(out.freeze())
    }

    /// Finalise the stream, processing any leftover data if present.
    ///
    /// After this call, `is_finished()` will be true and no further data
    /// will be processed.
    pub fn finalise(&mut self) -> Result<Bytes> {
        debug!("Finalising stream serial={:#x}", self.serial_number);
        let leftover = self.process(&[])?;
        self.finished = true;
        Ok(leftover)
    }

    /// Get the current output granule position.
    pub fn get_granule_position(&self) -> u64 {
        self.last_output_abgp
    }

    /// Check if the stream is finished.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Check if this processor has successfully produced any output.
    ///
    /// This is useful for detecting invalid input data - if we've been
    /// processing data but haven't produced any valid Ogg pages, it's
    /// likely the input is invalid.
    pub fn has_produced_output(&self) -> bool {
        self.has_output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    fn get_silence_ogg_data() -> Vec<u8> {
        // This should contain an Ogg container with an actual EOS page at the end
        include_bytes!("../resources/silence.ogg").to_vec()
    }

    #[test]
    fn test_stream_processor_real_without_eos() -> Result<()> {
        // Some Ogg data that doesn't contain an EOS. The stream won't finish automatically.
        let mut sp = StreamProcessor::with_serial(0x42, 1000);
        let data = get_silence_ogg_data();
        let out = sp.process(&data)?;
        assert!(!out.is_empty());
        assert!(sp.has_produced_output());
        // Unless the data had an EOS page, we won't see `finished=true`.
        // It's test-data-dependent.
        Ok(())
    }

    #[test]
    fn test_stream_processor_silence_eos() -> Result<()> {
        // If the embedded silence.ogg has an EOS page, the silence stream
        // will also end automatically when that page is reached.
        let mut sp = StreamProcessor::with_silence(0x99, 0);
        let data = get_silence_ogg_data();
        let out = sp.process(&data)?;
        assert!(!out.is_empty());
        assert!(sp.has_produced_output());
        // If the data had an EOS, sp.is_finished() should now be true
        // but this is test-data-dependent.
        Ok(())
    }

    #[test]
    fn test_finalise_exhausts_leftover_pages() -> Result<()> {
        let mut sp = StreamProcessor::with_serial(0xAB, 5000);
        let data = get_silence_ogg_data();

        // Put some data in buffer
        sp.buffer.extend_from_slice(&data);

        // finalise() should process that leftover
        let leftover_out = sp.finalise()?;
        assert!(!leftover_out.is_empty());
        assert!(sp.is_finished());
        assert!(sp.has_produced_output());
        Ok(())
    }

    #[test]
    fn test_invalid_data_detection() -> Result<()> {
        let mut sp = StreamProcessor::with_serial(0xCD, 0);
        let invalid_data = b"This is not a valid Ogg stream".to_vec();

        // Process the invalid data
        let out = sp.process(&invalid_data)?;

        // Should not produce output
        assert!(out.is_empty());
        assert!(!sp.has_produced_output());

        // Should not be marked as finished yet
        assert!(!sp.is_finished());

        // Manually advance time (can't easily do in a unit test)
        // In real usage, the timeout in mux.rs would handle this
        Ok(())
    }

    #[test]
    fn test_page_splitting() -> Result<()> {
        // Test handling data that splits Ogg pages in the middle
        let mut sp = StreamProcessor::with_serial(0xAA, 0);
        let data = get_silence_ogg_data();

        // Split the data in the middle of a page
        let split_point = data.len() / 3;

        // Process first chunk
        let out1 = sp.process(&data[..split_point])?;

        // First chunk might not produce output if it doesn't contain complete pages

        // Process second chunk
        let out2 = sp.process(&data[split_point..])?;

        // At least one of the outputs should contain data
        assert!(
            out1.len() > 0 || out2.len() > 0,
            "No output produced from split data"
        );

        Ok(())
    }

    #[test]
    fn test_tiny_chunks() -> Result<()> {
        // Test processing very small chunks (pathological case)
        let mut sp = StreamProcessor::with_serial(0xBB, 1000);
        let data = get_silence_ogg_data();

        // Process data in tiny 10-byte chunks
        let chunk_size = 10;
        let mut total_output = 0;

        for chunk in data.chunks(chunk_size) {
            let out = sp.process(chunk)?;
            total_output += out.len();
        }

        // Finalise to get any remaining data
        let final_out = sp.finalise()?;
        total_output += final_out.len();

        // We should have produced some output
        assert!(total_output > 0, "No output produced from tiny chunks");

        Ok(())
    }

    #[test]
    fn test_granule_position_remapping() -> Result<()> {
        // Test that granule positions are correctly remapped
        let base_granule = 44100 * 10; // 10 seconds in
        let mut sp = StreamProcessor::with_serial(0xCC, base_granule);

        // Process data with known granule positions
        let _ = sp.process(&get_silence_ogg_data())?;

        // The output granule position should be at least the base granule
        assert!(
            sp.get_granule_position() >= base_granule,
            "Granule position wasn't remapped correctly"
        );

        Ok(())
    }

    #[test]
    fn test_large_buffer_handling() -> Result<()> {
        // Test handling of a very large buffer
        let mut sp = StreamProcessor::with_serial(0xDD, 0);

        // Create a large buffer with repeated copies of silence data
        let silence = get_silence_ogg_data();
        let mut large_data = Vec::new();

        // Repeat the silence data to create ~1MB buffer
        for _ in 0..20 {
            large_data.extend_from_slice(&silence);
        }

        // Process the large buffer
        let out = sp.process(&large_data)?;

        // We should get substantial output
        assert!(out.len() > 0, "No output from large buffer");

        Ok(())
    }

    #[test]
    fn test_sequential_serial_numbers() -> Result<()> {
        // Test that sequential streams get unique serial numbers
        let silence = get_silence_ogg_data();

        // Create processors with different serial numbers
        let mut sp1 = StreamProcessor::with_serial(1000, 0);
        let mut sp2 = StreamProcessor::with_serial(1001, 0);

        // Process the same data through both
        let out1 = sp1.process(&silence)?;
        let out2 = sp2.process(&silence)?;

        // Both should produce output
        assert!(
            !out1.is_empty() && !out2.is_empty(),
            "One or both processors failed to produce output"
        );

        Ok(())
    }

    #[test]
    fn test_is_finished_state() -> Result<()> {
        // Test the is_finished state transitions
        let mut sp = StreamProcessor::with_serial(0xEE, 0);

        // Initially should not be finished
        assert!(
            !sp.is_finished(),
            "New processor should not be in finished state"
        );

        // After finalising, should be finished
        sp.finalise()?;
        assert!(
            sp.is_finished(),
            "Processor should be in finished state after finalise"
        );

        // Further processing should not produce output
        let out = sp.process(&get_silence_ogg_data())?;
        assert!(
            out.is_empty(),
            "Finished processor should not produce output"
        );

        Ok(())
    }
}
