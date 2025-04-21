use anyhow::{Context, Result};
use bytes::{Buf, BytesMut};
use log::{debug, warn};
use ogg_pager::Page;
use std::io::Cursor;

/// Processes and remaps Ogg streams.
///
/// The StreamProcessor reads Ogg pages from an input buffer, remaps their
/// granule positions and serial numbers, and outputs the modified pages.
pub struct StreamProcessor {
    serial_number: u32,
    buffer: BytesMut,

    // Granule remapping state
    last_input_abgp: u64,
    last_output_abgp: u64,
    sequence_number: u32,
    finished: bool,
    has_output: bool,
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
            has_output: false,
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
            has_output: false,
        }
    }

    /// Process a chunk of Ogg data.
    ///
    /// Reads Ogg pages from `input`, updates their serial and granule positions,
    /// and returns the remapped pages as a single `Bytes` buffer.
    pub fn process(&mut self, input: &[u8]) -> Result<bytes::Bytes> {
        if self.finished {
            return Ok(bytes::Bytes::new());
        }

        // Append incoming data
        self.buffer.extend_from_slice(input);
        let mut out = BytesMut::new();
        let mut cursor = Cursor::new(&self.buffer[..]);
        let mut consumed = 0;

        loop {
            match Page::read(&mut cursor) {
                Ok(mut page) => {
                    let end = cursor.position() as usize;
                    // Remap granule position
                    let input_abgp = page.header().abgp;
                    let new_abgp = if input_abgp == u64::MAX {
                        self.last_output_abgp
                    } else {
                        let delta = input_abgp.saturating_sub(self.last_input_abgp);
                        let pos = self.last_output_abgp + delta;
                        self.last_input_abgp = input_abgp;
                        self.last_output_abgp = pos;
                        pos
                    };
                    {
                        let header = page.header_mut();
                        header.abgp = new_abgp;
                        header.stream_serial = self.serial_number;
                        header.sequence_number = self.sequence_number;
                    }
                    page.gen_crc();
                    out.extend_from_slice(&page.as_bytes());
                    self.sequence_number = self.sequence_number.wrapping_add(1);
                    consumed = end;
                    self.has_output = true;

                    // End of stream detection
                    if (page.header().header_type_flag() & 0x04) != 0 {
                        debug!("Detected EOS on serial={:#x}", self.serial_number);
                        self.finished = true;
                    }
                }
                Err(e) => {
                    // Distinguish incomplete vs malformed
                    if (cursor.position() as usize) < self.buffer.len() {
                        return Err(e).context(format!(
                            "parsing Ogg page (serial=0x{:x})",
                            self.serial_number
                        ));
                    } else {
                        break; // incomplete page, await more data
                    }
                }
            }
            if (cursor.position() as usize) >= self.buffer.len() {
                break;
            }
        }

        // Drop consumed bytes
        if consumed > 0 {
            self.buffer.advance(consumed);
        } else if self.buffer.len() > 1_048_576 {
            warn!(
                "Large buffer ({} bytes) with no pages; clearing",
                self.buffer.len()
            );
            self.buffer.clear();
        }

        Ok(out.freeze())
    }

    /// Finalise the stream, processing leftover data.
    pub fn finalise(&mut self) -> Result<bytes::Bytes> {
        debug!("Finalising serial={:#x}", self.serial_number);
        let leftover = self.process(&[]).context("processing leftover data")?;
        self.finished = true;
        Ok(leftover)
    }

    /// Get current granule position.
    pub fn get_granule_position(&self) -> u64 {
        self.last_output_abgp
    }

    /// Check if finished.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Check if any output produced.
    pub fn has_produced_output(&self) -> bool {
        self.has_output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use bytes::Bytes;
    use std::io::Cursor;

    fn get_silence_ogg_data() -> Vec<u8> {
        include_bytes!("../resources/silence_default.ogg").to_vec()
    }

    #[test]
    fn test_stream_processor_real_without_eos() -> Result<()> {
        let mut sp = StreamProcessor::with_serial(0x42, 1000);
        let data = get_silence_ogg_data();
        let out = sp.process(&data)?;
        assert!(!out.is_empty());
        assert!(sp.has_produced_output());
        Ok(())
    }

    #[test]
    fn test_stream_processor_silence_eos() -> Result<()> {
        let mut sp = StreamProcessor::with_silence(0x99, 0);
        let data = get_silence_ogg_data();
        let out = sp.process(&data)?;
        assert!(!out.is_empty());
        Ok(())
    }

    #[test]
    fn test_finalise_exhausts_leftover_pages() -> Result<()> {
        let mut sp = StreamProcessor::with_serial(0xAB, 5000);
        let data = get_silence_ogg_data();
        sp.buffer.extend_from_slice(&data);
        let leftover = sp.finalise()?;
        assert!(!leftover.is_empty());
        assert!(sp.is_finished());
        Ok(())
    }

    #[test]
    fn test_invalid_data_detection() -> Result<()> {
        let mut sp = StreamProcessor::with_serial(0xCD, 0);
        let invalid_data = b"not ogg";
        let result = sp.process(invalid_data);
        assert!(
            result.is_err(),
            "Expected an error when processing invalid Ogg data"
        );
        Ok(())
    }

    #[test]
    fn test_page_splitting() -> Result<()> {
        let mut sp = StreamProcessor::with_serial(0xAA, 0);
        let data = get_silence_ogg_data();
        let split = data.len() / 3;
        let out1 = sp.process(&data[..split])?;
        let out2 = sp.process(&data[split..])?;
        assert!(!out1.is_empty() || !out2.is_empty());
        Ok(())
    }

    #[test]
    fn test_tiny_chunks() -> Result<()> {
        let mut sp = StreamProcessor::with_serial(0xBB, 1000);
        let data = get_silence_ogg_data();
        let mut total = 0;
        for chunk in data.chunks(10) {
            let out = sp.process(chunk)?;
            total += out.len();
        }
        let final_out = sp.finalise()?;
        total += final_out.len();
        assert!(total > 0);
        Ok(())
    }

    #[test]
    fn test_granule_position_remapping() -> Result<()> {
        let base = 44100 * 10;
        let mut sp = StreamProcessor::with_serial(0xCC, base);
        let _ = sp.process(&get_silence_ogg_data())?;
        assert!(sp.get_granule_position() >= base);
        Ok(())
    }

    #[test]
    fn test_large_buffer_handling() -> Result<()> {
        let mut sp = StreamProcessor::with_serial(0xDD, 0);
        let silence = get_silence_ogg_data();
        let mut large = Vec::new();
        for _ in 0..20 {
            large.extend_from_slice(&silence);
        }
        let out = sp.process(&large)?;
        assert!(!out.is_empty());
        Ok(())
    }

    #[test]
    fn test_sequential_serial_numbers() -> Result<()> {
        let silence = get_silence_ogg_data();
        let mut sp1 = StreamProcessor::with_serial(1000, 0);
        let mut sp2 = StreamProcessor::with_serial(1001, 0);
        let out1 = sp1.process(&silence)?;
        let out2 = sp2.process(&silence)?;
        assert!(!out1.is_empty() && !out2.is_empty());
        Ok(())
    }

    #[test]
    fn test_is_finished_state() -> Result<()> {
        let mut sp = StreamProcessor::with_serial(0xEE, 0);
        assert!(!sp.is_finished());
        sp.finalise()?;
        assert!(sp.is_finished());
        let out = sp.process(&get_silence_ogg_data())?;
        assert!(out.is_empty());
        Ok(())
    }

    #[test]
    fn test_stream_processor_replays_last_granule_on_neg_one() -> Result<()> {
        let data = get_silence_ogg_data();
        let mut cursor = Cursor::new(&data);
        let mut page = Page::read(&mut cursor)?;
        let end = cursor.position() as usize;
        let first = &data[..end];
        let mut sp = StreamProcessor::with_serial(0x01, 100);
        let out1 = sp.process(first)?;
        assert!(!out1.is_empty());
        let baseline = sp.get_granule_position();
        {
            let hdr = page.header_mut();
            hdr.abgp = u64::MAX;
            page.gen_crc();
        }
        let neg = page.as_bytes();
        let out2 = sp.process(&neg)?;
        assert!(!out2.is_empty());
        assert_eq!(sp.get_granule_position(), baseline);
        Ok(())
    }

    #[test]
    fn test_stream_processor_advances_on_valid_granule() -> Result<()> {
        let data = get_silence_ogg_data();
        let mut sp = StreamProcessor::with_serial(0x02, 200);
        let out = sp.process(&data)?;
        assert!(!out.is_empty());
        assert!(sp.get_granule_position() > 200);
        Ok(())
    }
}
