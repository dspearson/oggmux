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
    fn test_clock_elapsed() {
        let clock = StreamClock::new(44100);
        thread::sleep(Duration::from_millis(50));
        assert!(clock.elapsed_secs() >= 0.05);
    }

    #[test]
    fn test_granule_conversion_accuracy() {
        let clock = StreamClock::new(48000);
        assert!((clock.granule_to_secs(24000) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_lead_secs_positive_negative() {
        let clock = StreamClock::new(44100);
        thread::sleep(Duration::from_millis(100));
        let elapsed_granule = (clock.elapsed_secs() * 44100.0) as u64;

        assert!(clock.lead_secs(elapsed_granule) < 0.05);
        assert!(clock.lead_secs(elapsed_granule + 44100) > 0.95);
    }
}
