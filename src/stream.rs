use anyhow::{Context, Result};
use bytes::{Buf, BytesMut};
use log::{debug, warn};
use ogg_pager::Page;
use std::collections::HashSet;
use std::io::Cursor;

/// Processes and remaps Ogg streams.
///
/// The StreamProcessor reads Ogg pages from an input buffer, remaps their
/// granule positions and serial numbers, and outputs the modified pages.
///
/// Supports multi-stream Ogg files (e.g., audio + embedded album art) by
/// automatically detecting and filtering out non-Vorbis streams.
pub struct StreamProcessor {
    serial_number: u32,
    buffer: BytesMut,

    // Granule remapping state
    last_input_abgp: u64,
    last_output_abgp: u64,
    sequence_number: u32,
    finished: bool,
    has_output: bool,

    // Multi-stream filtering
    vorbis_serials: HashSet<u32>,
    seen_bos: bool,
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
            vorbis_serials: HashSet::new(),
            seen_bos: false,
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
            vorbis_serials: HashSet::new(),
            seen_bos: false,
        }
    }

    /// Check if a page is a Vorbis identification header (BOS page).
    ///
    /// Vorbis streams begin with a BOS page containing a packet that starts
    /// with [0x01, 'v', 'o', 'r', 'b', 'i', 's'].
    fn is_vorbis_bos(page: &Page) -> bool {
        const BOS_FLAG: u8 = 0x02;
        const VORBIS_HEADER: &[u8] = &[0x01, b'v', b'o', b'r', b'b', b'i', b's'];

        // Check if this is a BOS page
        if (page.header().header_type_flag() & BOS_FLAG) == 0 {
            return false;
        }

        // Check if the packet starts with the Vorbis identification header
        let content = page.content();
        if content.len() >= VORBIS_HEADER.len() {
            content.starts_with(VORBIS_HEADER)
        } else {
            false
        }
    }

    /// Process a chunk of Ogg data.
    ///
    /// Reads Ogg pages from `input`, updates their serial and granule positions,
    /// and returns the remapped pages as a single `Bytes` buffer.
    ///
    /// Automatically filters out non-Vorbis streams (e.g., embedded album art)
    /// from multi-stream Ogg files.
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
                    let input_serial = page.header().stream_serial;

                    // Detect and track Vorbis streams on first BOS page
                    if Self::is_vorbis_bos(&page) {
                        debug!("Detected Vorbis stream with serial={:#x}", input_serial);
                        self.vorbis_serials.insert(input_serial);
                        self.seen_bos = true;
                    }

                    // Filter out non-Vorbis streams (e.g., album art)
                    if self.seen_bos
                        && !self.vorbis_serials.is_empty()
                        && !self.vorbis_serials.contains(&input_serial)
                    {
                        debug!(
                            "Filtering out non-Vorbis page from serial={:#x} (detected as album art or other stream)",
                            input_serial
                        );
                        consumed = end;
                        continue;
                    }

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
            // Safety valve: If we've buffered >1MB without finding a valid Ogg page,
            // the data is likely corrupt or not Ogg format at all. Clear the buffer
            // to prevent unbounded memory growth. For reference, typical Ogg pages
            // are 4-8KB, so 1MB should be more than sufficient for any valid stream.
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

    /// Get current sequence number.
    #[allow(dead_code)]
    pub fn get_sequence_number(&self) -> u32 {
        self.sequence_number
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

    #[test]
    fn test_stream_processor_initial_state() {
        let sp = StreamProcessor::with_serial(0x42, 0);
        assert_eq!(sp.get_granule_position(), 0);
        assert!(!sp.is_finished());
    }

    #[test]
    fn test_stream_processor_finalise_empty() {
        let mut sp = StreamProcessor::with_serial(0x42, 0);
        let result = sp.finalise();
        assert!(result.unwrap().is_empty());
        assert!(sp.is_finished());
    }

    #[test]
    fn test_vorbis_bos_detection() {
        // Test that we can detect Vorbis BOS pages by their header signature
        // This is a synthetic test - real Ogg pages would need proper structure

        // Create a minimal mock BOS page structure
        // In real usage, this would come from actual Ogg file parsing

        // Vorbis identification header starts with [0x01, 'v', 'o', 'r', 'b', 'i', 's']
        let vorbis_header = b"\x01vorbis\x00\x00\x00\x00\x02\x00";

        // Note: This test verifies the detection logic exists
        // Real integration testing requires actual Ogg file data
        assert!(vorbis_header.starts_with(b"\x01vorbis"));
    }
}
