use std::time::Instant;

/// Tracks timing between wall clock and stream time.
///
/// The StreamClock is used to maintain proper timing relationships between
/// the real-time wall clock and the audio stream's granule position.
/// This ensures that audio playback maintains the correct speed and
/// doesn't buffer too much or too little data.
#[derive(Debug, Clone)]
pub struct StreamClock {
    start: Instant,
    sample_rate: u32,
}

impl StreamClock {
    /// Create a new StreamClock.
    ///
    /// # Arguments
    ///
    /// * `sample_rate` - The sample rate of the audio stream in Hz.
    ///
    /// # Returns
    ///
    /// A new StreamClock initialised with the current time.
    pub fn new(sample_rate: u32) -> Self {
        Self {
            start: Instant::now(),
            sample_rate,
        }
    }

    /// Get the elapsed wall-clock time in seconds.
    ///
    /// # Returns
    ///
    /// The number of seconds elapsed since the StreamClock was created.
    pub fn elapsed_secs(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Convert a granule position to seconds.
    ///
    /// # Arguments
    ///
    /// * `granule` - The granule position to convert.
    ///
    /// # Returns
    ///
    /// The equivalent time in seconds.
    pub fn granule_to_secs(&self, granule: u64) -> f64 {
        granule as f64 / self.sample_rate as f64
    }

    /// Calculate how far ahead the stream is compared to wall-clock time.
    ///
    /// This is used to determine if we need to add silence (when the lead
    /// is too small) or throttle output (when the lead is too large).
    ///
    /// # Arguments
    ///
    /// * `granule` - The current granule position of the stream.
    ///
    /// # Returns
    ///
    /// The lead time in seconds (positive if the stream is ahead of wall-clock).
    pub fn lead_secs(&self, granule: u64) -> f64 {
        self.granule_to_secs(granule) - self.elapsed_secs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_clock_creation() {
        let clock = StreamClock::new(44100);
        assert_eq!(clock.sample_rate, 44100);
    }

    #[test]
    fn test_granule_to_secs() {
        let clock = StreamClock::new(44100);

        // 44100 samples at 44100Hz = 1 second
        assert_eq!(clock.granule_to_secs(44100), 1.0);

        // 22050 samples at 44100Hz = 0.5 seconds
        assert_eq!(clock.granule_to_secs(22050), 0.5);

        // 0 samples = 0 seconds
        assert_eq!(clock.granule_to_secs(0), 0.0);
    }

    #[test]
    fn test_elapsed_secs() {
        let clock = StreamClock::new(44100);

        // Sleep briefly
        thread::sleep(std::time::Duration::from_millis(50));

        // Elapsed time should be approximately 0.05 seconds
        let elapsed = clock.elapsed_secs();
        assert!(
            elapsed >= 0.04 && elapsed <= 0.1,
            "Elapsed time was {}",
            elapsed
        );
    }

    #[test]
    fn test_lead_secs() {
        let clock = StreamClock::new(44100);

        // A granule position of 0 should have a negative lead (behind wall-clock)
        thread::sleep(std::time::Duration::from_millis(50));
        let lead = clock.lead_secs(0);
        assert!(lead < 0.0, "Lead time should be negative, got {}", lead);

        // A very large granule position should have a positive lead (ahead of wall-clock)
        let lead = clock.lead_secs(44100 * 10); // 10 seconds of audio
        assert!(
            lead > 9.0,
            "Lead time should be around 10 seconds, got {}",
            lead
        );
    }
}
