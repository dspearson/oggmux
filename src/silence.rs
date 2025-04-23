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
        let silence_data = b"test silence data";
        let template = SilenceTemplate {
            data: bytes::Bytes::from_static(silence_data),
        };
        assert_eq!(template.raw_bytes(), silence_data);
    }
}
