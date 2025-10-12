//! Utility functions for OggMux.

/// Calculate ideal buffer size based on bitrate and target latency.
///
/// This helps users determine the optimal buffer configuration based
/// on their audio quality settings and desired latency.
///
/// # Arguments
///
/// * `bitrate_kbps` - The bitrate in kbps (e.g., 192, 320)
/// * `target_latency_ms` - The desired latency in milliseconds
///
/// # Returns
///
/// The recommended buffer size in bytes
///
/// # Example
///
/// ```
/// use oggmux::utils::calculate_buffer_size;
///
/// // For 192kbps audio with 500ms latency
/// let buffer_size = calculate_buffer_size(192, 500);
/// ```
pub fn calculate_buffer_size(bitrate_kbps: u32, target_latency_ms: u32) -> usize {
    // Convert latency from ms to seconds
    let latency_seconds = target_latency_ms as f64 / 1000.0;

    // Calculate bytes per second (bitrate is in kilobits)
    // 8 bits = 1 byte, 1000 bits = 1 kilobit
    let bytes_per_second = (bitrate_kbps * 1000) / 8;

    // Calculate buffer size with a small safety margin (1.1x)
    let buffer_size = (bytes_per_second as f64 * latency_seconds * 1.1) as usize;

    // Round to nearest power of 2 for optimal memory alignment
    round_to_power_of_2(buffer_size)
}

/// Calculate buffered seconds based on bitrate and buffer size.
///
/// This helps users understand how much audio time their current
/// buffer configuration represents.
///
/// # Arguments
///
/// * `bitrate_kbps` - The bitrate in kbps (e.g., 192, 320)
/// * `buffer_size` - The buffer size in bytes
///
/// # Returns
///
/// The amount of audio time (in seconds) that fits in the buffer
///
/// # Example
///
/// ```
/// use oggmux::utils::calculate_buffered_seconds;
///
/// // For 192kbps audio with a 16KB buffer
/// let seconds = calculate_buffered_seconds(192, 16384);
/// ```
pub fn calculate_buffered_seconds(bitrate_kbps: u32, buffer_size: usize) -> f64 {
    // Calculate bytes per second (bitrate is in kilobits)
    let bytes_per_second = (bitrate_kbps * 1000) / 8;

    // Calculate seconds of audio that fit in the buffer
    buffer_size as f64 / bytes_per_second as f64
}

/// Round a number to the nearest power of 2.
///
/// This is useful for optimizing buffer sizes for memory allocation.
///
/// # Arguments
///
/// * `n` - The number to round
///
/// # Returns
///
/// The nearest power of 2 (rounded up)
fn round_to_power_of_2(n: usize) -> usize {
    if n <= 1 {
        return 1;
    }

    // Find the next power of 2
    let mut power = 1;
    while power < n {
        power *= 2;
    }

    // Check if we should round down instead
    let previous_power = power / 2;
    if n - previous_power < power - n {
        previous_power
    } else {
        power
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round_to_power_of_2() {
        assert_eq!(round_to_power_of_2(1), 1);
        assert_eq!(round_to_power_of_2(2), 2);
        assert_eq!(round_to_power_of_2(3), 4);
        assert_eq!(round_to_power_of_2(4), 4);
        assert_eq!(round_to_power_of_2(5), 4);
        assert_eq!(round_to_power_of_2(7), 8);
        assert_eq!(round_to_power_of_2(9), 8);
        assert_eq!(round_to_power_of_2(12), 16);
        assert_eq!(round_to_power_of_2(17), 16);
        assert_eq!(round_to_power_of_2(32768), 32768);
        assert_eq!(round_to_power_of_2(32769), 32768);
    }

    #[test]
    fn test_calculate_buffer_size() {
        // For 192kbps with 500ms latency:
        // (192 * 1000 / 8) bytes/sec * 0.5 sec * 1.1 safety = 13,200 bytes
        // Nearest power of 2 is 16,384
        assert_eq!(calculate_buffer_size(192, 500), 16384);

        // Test a few other combinations
        assert_eq!(calculate_buffer_size(320, 100), 4096);
        assert_eq!(calculate_buffer_size(64, 1000), 8192);
    }

    #[test]
    fn test_calculate_buffered_seconds() {
        // 192kbps = 24,000 bytes per second
        // 16,384 bytes / 24,000 bytes/sec = ~0.683 seconds
        let seconds = calculate_buffered_seconds(192, 16384);
        assert!((seconds - 0.683).abs() < 0.001);
    }
}
