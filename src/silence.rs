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
    pub fn new_embedded(raw: &'static [u8]) -> Self {
        Self {
            data: Bytes::from_static(raw),
        }
    }

    /// Return the raw Ogg silence bytes.
    pub fn raw_bytes(&self) -> &[u8] {
        &self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_silence_template_creation() {
        let silence_data = include_bytes!("../resources/silence_default.ogg");
        let template = SilenceTemplate::new_embedded(silence_data);
        assert_eq!(template.raw_bytes(), silence_data.as_slice());
    }

    #[test]
    fn test_silence_raw_bytes() {
        // Create a custom small silence template
        let custom_data = b"custom silence data";

        // Can't use new_embedded directly with non-static data,
        // but we can test the raw_bytes method
        let template = SilenceTemplate {
            data: bytes::Bytes::from(custom_data.to_vec()),
        };

        // Check that raw_bytes returns the correct data
        assert_eq!(template.raw_bytes(), custom_data);
    }
}
