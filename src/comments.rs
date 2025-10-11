use anyhow::Result;
use bytes::{Bytes, BytesMut};

/// Generate a Vorbis comment packet with the given key-value pairs.
///
/// This creates a properly formatted Vorbis comment header packet that can be
/// injected into an Ogg stream to provide metadata at track boundaries.
///
/// The format follows the Vorbis I specification for comment headers.
pub fn generate_comment_packet(comments: Vec<(String, String)>) -> Result<Vec<u8>> {
    let mut packet = BytesMut::new();

    // Packet type (0x03 = comment header)
    packet.extend_from_slice(&[0x03]);

    // Vorbis identifier
    packet.extend_from_slice(b"vorbis");

    // Vendor string (length + string)
    let vendor = b"oggmux";
    packet.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    packet.extend_from_slice(vendor);

    // User comment list length
    packet.extend_from_slice(&(comments.len() as u32).to_le_bytes());

    // Each comment
    for (key, value) in comments {
        let comment = format!("{}={}", key, value);
        let comment_bytes = comment.as_bytes();
        packet.extend_from_slice(&(comment_bytes.len() as u32).to_le_bytes());
        packet.extend_from_slice(comment_bytes);
    }

    // Framing bit (1)
    packet.extend_from_slice(&[0x01]);

    Ok(packet.to_vec())
}

/// Wrap a Vorbis comment packet in an Ogg page.
///
/// Creates a complete Ogg page containing the comment packet, suitable for
/// injection into an Ogg stream.
pub fn create_comment_page(
    packet: Vec<u8>,
    serial: u32,
    sequence: u32,
    granule_pos: u64,
) -> Result<Bytes> {
    // Build segment table for the packet
    let mut segments = Vec::new();
    let mut remaining = packet.len();
    while remaining > 255 {
        segments.push(255u8);
        remaining -= 255;
    }
    segments.push(remaining as u8);

    // Manually construct the complete Ogg page
    let mut page_data = Vec::new();

    // Write header fields
    page_data.extend_from_slice(b"OggS");  // Capture pattern
    page_data.extend_from_slice(&[0x00]);  // Version
    page_data.extend_from_slice(&[0x00]);  // header_type_flag: normal page
    page_data.extend_from_slice(&granule_pos.to_le_bytes());
    page_data.extend_from_slice(&serial.to_le_bytes());
    page_data.extend_from_slice(&sequence.to_le_bytes());
    page_data.extend_from_slice(&[0, 0, 0, 0]);  // CRC (will calculate later)
    page_data.extend_from_slice(&[segments.len() as u8]);  // Number of segments
    page_data.extend_from_slice(&segments);  // Segment table

    // Write packet data
    page_data.extend_from_slice(&packet);

    // Calculate and insert CRC using the Ogg CRC algorithm
    let crc = calculate_crc(&page_data);
    page_data[22..26].copy_from_slice(&crc.to_le_bytes());

    Ok(Bytes::from(page_data))
}

/// Calculate CRC32 for an Ogg page (using the Ogg CRC polynomial).
fn calculate_crc(data: &[u8]) -> u32 {
    let mut crc: u32 = 0;

    for &byte in data {
        let mut value = ((crc >> 24) & 0xFF) ^ (byte as u32);

        for _ in 0..8 {
            if (value & 1) != 0 {
                value = (value >> 1) ^ 0x04C11DB7;
            } else {
                value >>= 1;
            }
        }

        crc = (crc << 8) ^ value;
    }

    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_comment_packet() {
        let comments = vec![
            ("TITLE".to_string(), "Test Song".to_string()),
            ("ARTIST".to_string(), "Test Artist".to_string()),
        ];

        let packet = generate_comment_packet(comments).unwrap();

        // Should start with packet type (0x03) and "vorbis"
        assert_eq!(packet[0], 0x03);
        assert_eq!(&packet[1..7], b"vorbis");

        // Should end with framing bit (0x01)
        assert_eq!(*packet.last().unwrap(), 0x01);
    }

    #[test]
    fn test_generate_comment_packet_empty() {
        let comments = vec![];
        let packet = generate_comment_packet(comments).unwrap();

        // Even with no comments, should have valid structure
        assert_eq!(packet[0], 0x03);
        assert_eq!(&packet[1..7], b"vorbis");
        assert_eq!(*packet.last().unwrap(), 0x01);
    }

    #[test]
    fn test_generate_comment_packet_with_unicode() {
        let comments = vec![
            ("TITLE".to_string(), "Test 日本語 Song".to_string()),
            ("ARTIST".to_string(), "Tëst Ártist".to_string()),
        ];

        let packet = generate_comment_packet(comments).unwrap();

        // Should handle unicode without crashing
        assert_eq!(packet[0], 0x03);
        assert_eq!(&packet[1..7], b"vorbis");
    }

    #[test]
    fn test_create_comment_page() {
        let packet = vec![0x03, b'v', b'o', b'r', b'b', b'i', b's'];
        let page = create_comment_page(packet.clone(), 0x12345678, 42, 1000).unwrap();

        // Should start with "OggS"
        assert_eq!(&page[0..4], b"OggS");

        // Check version (offset 4)
        assert_eq!(page[4], 0x00);

        // Check header type flag (offset 5)
        assert_eq!(page[5], 0x00);

        // Check granule position (offset 6-13, little-endian)
        let granule = u64::from_le_bytes([
            page[6], page[7], page[8], page[9],
            page[10], page[11], page[12], page[13]
        ]);
        assert_eq!(granule, 1000);

        // Check stream serial (offset 14-17, little-endian)
        let serial = u32::from_le_bytes([page[14], page[15], page[16], page[17]]);
        assert_eq!(serial, 0x12345678);

        // Check sequence number (offset 18-21, little-endian)
        let sequence = u32::from_le_bytes([page[18], page[19], page[20], page[21]]);
        assert_eq!(sequence, 42);

        // CRC should be non-zero (offset 22-25)
        let crc = u32::from_le_bytes([page[22], page[23], page[24], page[25]]);
        assert_ne!(crc, 0, "CRC should be calculated");

        // Segment count (offset 26)
        assert_eq!(page[26], 1, "Should have 1 segment for small packet");

        // Segment size (offset 27)
        assert_eq!(page[27], packet.len() as u8);

        // Packet data should follow
        assert_eq!(&page[28..28+packet.len()], &packet[..]);
    }

    #[test]
    fn test_create_comment_page_large_packet() {
        // Create a packet larger than 255 bytes to test multi-segment handling
        let mut packet = vec![0x03];
        packet.extend_from_slice(b"vorbis");
        packet.extend(vec![0u8; 300]); // Pad to > 255 bytes

        let page = create_comment_page(packet.clone(), 0x99999999, 10, 5000).unwrap();

        // Should start with "OggS"
        assert_eq!(&page[0..4], b"OggS");

        // Should have multiple segments
        let segment_count = page[26];
        assert!(segment_count > 1, "Large packet should need multiple segments");

        // First segment should be 255
        assert_eq!(page[27], 255);

        // Last segment should be the remainder
        let last_segment_idx = 27 + segment_count as usize - 1;
        let expected_last = (packet.len() % 255) as u8;
        assert_eq!(page[last_segment_idx], expected_last);
    }

    #[test]
    fn test_create_comment_page_crc_changes() {
        let packet = vec![0x03, b'v', b'o', b'r', b'b', b'i', b's'];

        // Create two pages with different parameters
        let page1 = create_comment_page(packet.clone(), 0x11111111, 1, 100).unwrap();
        let page2 = create_comment_page(packet.clone(), 0x22222222, 1, 100).unwrap();

        // CRCs should be different due to different serial numbers
        let crc1 = u32::from_le_bytes([page1[22], page1[23], page1[24], page1[25]]);
        let crc2 = u32::from_le_bytes([page2[22], page2[23], page2[24], page2[25]]);

        assert_ne!(crc1, crc2, "Different page content should produce different CRCs");
    }
}
