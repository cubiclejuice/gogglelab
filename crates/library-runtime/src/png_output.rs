use std::io::{self, Write};

#[derive(Clone, Copy)]
enum Failure {
    Limit,
    Allocation,
}

impl Failure {
    fn error(self) -> io::Error {
        match self {
            Self::Limit => {
                io::Error::new(io::ErrorKind::InvalidData, "PNG output exceeds its limit")
            }
            Self::Allocation => io::Error::other("PNG output allocation failed"),
        }
    }
}

/// Limits encoded output only; compressor workspace and input remain separate.
pub(crate) struct BoundedPngOutput {
    bytes: Vec<u8>,
    limit: usize,
    failure: Option<Failure>,
}

impl BoundedPngOutput {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            failure: None,
        }
    }

    pub(crate) fn limit_exceeded(&self) -> bool {
        matches!(self.failure, Some(Failure::Limit))
    }

    pub(crate) fn into_bytes(self) -> io::Result<Vec<u8>> {
        match self.failure {
            Some(failure) => Err(failure.error()),
            None => Ok(self.bytes),
        }
    }

    fn reserve_for_write(&mut self, additional: usize) -> io::Result<()> {
        if let Some(failure) = self.failure {
            return Err(failure.error());
        }
        let required = self.bytes.len().checked_add(additional);
        if required.is_none_or(|required| required > self.limit) {
            self.failure = Some(Failure::Limit);
            return Err(Failure::Limit.error());
        }
        let required = required.expect("checked output length");
        if required <= self.bytes.capacity() {
            return Ok(());
        }
        // Bound growth explicitly rather than allowing Vec's automatic growth
        // to request capacity beyond the cap. Allocator overhead is separate.
        let target = required
            .max(self.bytes.capacity().saturating_mul(2))
            .min(self.limit);
        if self
            .bytes
            .try_reserve_exact(target - self.bytes.len())
            .is_err()
        {
            self.failure = Some(Failure::Allocation);
            return Err(Failure::Allocation.error());
        }
        Ok(())
    }
}

impl Write for BoundedPngOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.reserve_for_write(bytes.len())?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.failure {
            Some(failure) => Err(failure.error()),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_fit_succeeds_and_overflow_never_appends_a_partial_write() {
        let mut output = BoundedPngOutput::new(4);
        assert_eq!(output.write(b"ab").unwrap(), 2);
        assert_eq!(output.write(b"cd").unwrap(), 2);
        assert_eq!(output.into_bytes().unwrap(), b"abcd");

        let mut output = BoundedPngOutput::new(4);
        output.write_all(b"ab").unwrap();
        assert!(output.write(b"cde").is_err());
        assert_eq!(output.bytes, b"ab");
        assert!(output.limit_exceeded());
        assert!(output.write(b"c").is_err());
        assert!(output.write(b"").is_err());
        assert!(output.flush().is_err());
        assert_eq!(output.bytes, b"ab");
        assert!(output.into_bytes().is_err());
    }

    #[test]
    fn allocation_failure_is_sticky_without_retaining_output() {
        let mut output = BoundedPngOutput::new(usize::MAX);
        // Vec cannot represent this capacity; exercise the fallible reservation
        // without actually allocating a large buffer.
        assert!(output.reserve_for_write(usize::MAX).is_err());
        assert!(!output.limit_exceeded());
        assert!(output.bytes.is_empty());
        assert!(output.write(b"x").is_err());
        assert!(output.into_bytes().is_err());
    }

    #[test]
    fn png_finish_reports_an_iend_only_overflow() {
        let pixels = [127; 8 * 4];
        let mut baseline = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut baseline, 8, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&pixels).unwrap();
            writer.finish().unwrap();
        }
        let mut output = BoundedPngOutput::new(baseline.len() - 1);
        {
            let mut encoder = png::Encoder::new(&mut output, 8, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&pixels).unwrap();
            assert!(writer.finish().is_err());
        }
        assert!(output.limit_exceeded());
        assert!(output.into_bytes().is_err());
    }
}
