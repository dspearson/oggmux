use crate::stream::StreamProcessor;
use bytes::Bytes;

/// Preloaded silence template for generating silence streams.
///
/// This struct holds the raw Ogg data for a silence stream, which is typically
/// embedded at compile time and used as a template for generating silence frames.
pub struct SilenceTemplate {
    data: Bytes,
}

impl SilenceTemplate {
    /// Create a new SilenceTemplate from embedded data.
    ///
    /// # Arguments
    ///
    /// * `raw` - The raw Ogg data for a silence stream.
    pub fn new_embedded(raw: &'static [u8]) -> Self {
        Self {
            data: Bytes::from_static(raw),
        }
    }

    /// Spawn a remuxable silence stream with a new serial and granule base.
    ///
    /// This method creates a new StreamProcessor specifically configured for
    /// silence data, using the provided serial number and starting from the
    /// specified granule position.
    ///
    /// # Arguments
    ///
    /// * `serial` - The serial number to use for the Ogg stream.
    /// * `base_granule` - The starting granule position for the stream.
    ///
    /// # Returns
    ///
    /// A new StreamProcessor configured for silence data.
    pub fn spawn_stream(&self, serial: u32, base_granule: u64) -> StreamProcessor {
        StreamProcessor::with_silence(serial, base_granule)
    }

    /// Get the raw bytes of the silence template.
    ///
    /// # Returns
    ///
    /// The raw Ogg data for the silence template.
    pub fn raw_bytes(&self) -> &[u8] {
        &self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_silence_template_creation() {
        // Get embedded silence
        let silence_data = include_bytes!("../resources/silence.ogg");

        // Create a silence template
        let template = SilenceTemplate::new_embedded(silence_data);

        // Check that the data is stored correctly
        assert_eq!(template.raw_bytes(), silence_data.as_slice());
    }

    #[test]
    fn test_spawn_stream() {
        // Get embedded silence
        let silence_data = include_bytes!("../resources/silence.ogg");

        // Create a silence template
        let template = SilenceTemplate::new_embedded(silence_data);

        // Spawn a stream
        let stream = template.spawn_stream(0x12345678, 1000);

        // Check that the stream has the right properties
        assert_eq!(stream.get_granule_position(), 1000);
        assert!(!stream.is_finished());
    }
}
