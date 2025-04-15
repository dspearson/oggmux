use std::time::Instant;

/// Tracks timing between wall clock and stream time.
///
/// This struct helps maintain synchronisation between the system's real-time
/// clock and the logical time in the audio stream (measured in samples).
#[derive(Debug, Clone)]
pub struct StreamClock {
    start: Instant,
    sample_rate: u32,
}

impl StreamClock {
    /// Create a new StreamClock with the specified sample rate.
    pub fn new(sample_rate: u32) -> Self {
        Self {
            start: Instant::now(),
            sample_rate,
        }
    }

    /// Get the elapsed time in seconds since this clock was created.
    pub fn elapsed_secs(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Convert a granule position to seconds.
    ///
    /// In Ogg Vorbis, granule positions represent the number of
    /// samples, so dividing by the sample rate gives us seconds.
    pub fn granule_to_secs(&self, granule: u64) -> f64 {
        granule as f64 / self.sample_rate as f64
    }

    /// Calculate how far ahead the stream is from the wall clock.
    ///
    /// A positive value means the stream is ahead (buffering).
    /// A negative value means the stream is behind (potential underrun).
    pub fn lead_secs(&self, granule: u64) -> f64 {
        self.granule_to_secs(granule) - self.elapsed_secs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_clock() {
        let clock = StreamClock::new(44100);
        thread::sleep(std::time::Duration::from_millis(20));
        let e = clock.elapsed_secs();
        assert!(e >= 0.02);
    }

    #[test]
    fn test_granule_conversions() {
        // Test conversion from granule positions to seconds at different sample rates
        let clock_44k = StreamClock::new(44100);
        let clock_48k = StreamClock::new(48000);
        let clock_8k = StreamClock::new(8000);

        // 1 second of audio at each sample rate
        assert_eq!(clock_44k.granule_to_secs(44100), 1.0);
        assert_eq!(clock_48k.granule_to_secs(48000), 1.0);
        assert_eq!(clock_8k.granule_to_secs(8000), 1.0);

        // Test fractional seconds
        assert!((clock_44k.granule_to_secs(22050) - 0.5).abs() < 0.001);
        assert!((clock_48k.granule_to_secs(24000) - 0.5).abs() < 0.001);

        // Test large values
        let hour_44k = 44100 * 3600;
        assert_eq!(clock_44k.granule_to_secs(hour_44k), 3600.0);
    }

    #[test]
    fn test_lead_calculation() {
        // Create a clock
        let clock = StreamClock::new(44100);

        // Initial lead should be negative (we're behind real-time)
        assert!(clock.lead_secs(0) < 0.0);

        // Lead should be approximately zero if granule position matches elapsed time
        thread::sleep(Duration::from_millis(100)); // Sleep 100ms
        let elapsed = clock.elapsed_secs();
        let granule = (elapsed * 44100.0) as u64;

        // Lead should be close to zero, but might be slightly negative due to timing
        let lead = clock.lead_secs(granule);
        assert!(lead.abs() < 0.05); // Within 50ms

        // Lead should be positive if granule position is ahead
        let future_granule = granule + 44100; // 1 second ahead
        assert!(clock.lead_secs(future_granule) > 0.9); // Should be ~1 second lead
    }

    #[test]
    fn test_extreme_sample_rates() {
        // Test with unconventional sample rates

        // Very low sample rate
        let low_clock = StreamClock::new(1000);
        assert_eq!(low_clock.granule_to_secs(1000), 1.0);

        // Very high sample rate
        let high_clock = StreamClock::new(192000);
        assert_eq!(high_clock.granule_to_secs(192000), 1.0);

        // Extreme but valid sample rate
        let extreme_clock = StreamClock::new(384000);
        assert_eq!(extreme_clock.granule_to_secs(384000), 1.0);
    }
}
