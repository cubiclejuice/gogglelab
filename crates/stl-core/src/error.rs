use std::io;

use thiserror::Error;

/// Terminal processing stops, distinct from malformed optional metadata.
///
/// Limits describe deterministic per-input allowances. Temporary runtime permit
/// contention and allocator failures are separate operational errors. This type
/// carries no source paths or unbounded input details and enforces no limit itself.
#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub enum ResourceStop {
    #[error("Model exceeds supported parsing limits")]
    InputLimitExceeded,
    #[error("Model exceeds supported read-work limits")]
    ReadWorkExceeded,
    #[error("Model source changed during processing")]
    EnvelopeChanged,
}

impl ResourceStop {
    /// Preserve the concrete stop across APIs that accept only `io::Error`.
    /// The error kind alone is never sufficient to recognize a stop.
    pub fn into_io_error(self) -> io::Error {
        io::Error::other(self)
    }

    /// Recover only our concrete IO payload, never a matching message or kind.
    /// Library wrappers must first expose their original IO error.
    pub fn from_io_error(error: &io::Error) -> Option<Self> {
        error.get_ref()?.downcast_ref::<Self>().copied()
    }
}

/// Parse-time errors without a Tauri dependency. The native layer maps resource
/// stops into its existing Library error presentation at the terminal boundary.
#[derive(Debug, Error, PartialEq)]
pub enum StlError {
    #[error("file is empty")]
    EmptyFile,
    #[error("file exceeds the {limit}-byte size limit ({size} bytes)")]
    FileTooLarge { size: u64, limit: u64 },
    #[error("not a recognizable STL file")]
    NotStl,
    #[error("malformed STL header or structure")]
    MalformedHeader,
    #[error("file is truncated: header declares {expected} bytes, file has {actual}")]
    TruncatedFile { expected: u64, actual: u64 },
    #[error("triangle count {count} exceeds limit {limit}")]
    TooManyTriangles { count: u32, limit: u32 },
    #[error("non-finite coordinate in triangle {triangle_index}")]
    NonFiniteCoordinate { triangle_index: u32 },
    /// The bytes are a zip package but not a readable 3MF: missing model
    /// part, malformed XML, dangling object reference, bad vertex index, and
    /// so on. `detail` names the specific problem for the error overlay.
    #[error("malformed 3MF: {detail}")]
    Malformed3mf { detail: String },
    /// Operational allocator failure, separate from deterministic parsing limits.
    #[error("Model processing could not allocate memory")]
    AllocationFailed,
    #[error(transparent)]
    ResourceStopped(#[from] ResourceStop),
}

impl From<crate::zip_bounds::ZipOpenError> for StlError {
    fn from(error: crate::zip_bounds::ZipOpenError) -> Self {
        match error {
            crate::zip_bounds::ZipOpenError::ResourceStopped(stop) => Self::ResourceStopped(stop),
            crate::zip_bounds::ZipOpenError::AllocationFailed => Self::AllocationFailed,
            crate::zip_bounds::ZipOpenError::Archive(error) => Self::Malformed3mf {
                detail: format!("not a readable zip package ({error})"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    const STOPS: [ResourceStop; 3] = [
        ResourceStop::InputLimitExceeded,
        ResourceStop::ReadWorkExceeded,
        ResourceStop::EnvelopeChanged,
    ];

    #[test]
    fn resource_stop_io_roundtrip_preserves_each_concrete_reason() {
        for stop in STOPS {
            let error = stop.into_io_error();
            assert_eq!(ResourceStop::from_io_error(&error), Some(stop));
            assert_eq!(error.to_string(), stop.to_string());
            let parse_error: StlError = stop.into();
            assert_eq!(parse_error, StlError::ResourceStopped(stop));
            assert_eq!(parse_error.to_string(), stop.to_string());
        }
    }

    #[test]
    fn resource_stop_never_recognizes_ordinary_io_by_kind_or_matching_text() {
        for stop in STOPS {
            let ordinary = io::Error::other(stop.to_string());
            assert_eq!(ResourceStop::from_io_error(&ordinary), None);
        }
        for kind in [
            io::ErrorKind::InvalidData,
            io::ErrorKind::Interrupted,
            io::ErrorKind::UnexpectedEof,
        ] {
            assert_eq!(ResourceStop::from_io_error(&io::Error::from(kind)), None);
        }
    }

    #[test]
    fn resource_stop_survives_locked_zip_and_xml_io_wrappers() {
        for stop in STOPS {
            let zip_error = zip::result::ZipError::Io(stop.into_io_error());
            let zip::result::ZipError::Io(io_error) = zip_error else {
                panic!("expected ZIP IO wrapper");
            };
            assert_eq!(ResourceStop::from_io_error(&io_error), Some(stop));

            let xml_error = quick_xml::Error::from(stop.into_io_error());
            let quick_xml::Error::Io(io_error) = xml_error else {
                panic!("expected XML IO wrapper");
            };
            assert_eq!(ResourceStop::from_io_error(io_error.as_ref()), Some(stop));
        }
    }

    struct StoppedReader(ResourceStop);

    impl Read for StoppedReader {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(self.0.into_io_error())
        }
    }

    #[test]
    fn resource_stop_json_reader_requires_recovery_of_original_io_error() {
        for stop in STOPS {
            let error =
                serde_json::from_reader::<_, serde_json::Value>(StoppedReader(stop)).unwrap_err();
            assert!(error.is_io());
            // serde_json's consuming IO conversion returns the original error.
            // Do not assume its borrowed source chain exposes the IO payload.
            let io_error: io::Error = error.into();
            assert_eq!(ResourceStop::from_io_error(&io_error), Some(stop));
        }
        let malformed = serde_json::from_slice::<serde_json::Value>(b"{").unwrap_err();
        assert!(!malformed.is_io());
        let io_error: io::Error = malformed.into();
        assert_eq!(ResourceStop::from_io_error(&io_error), None);
    }

    #[test]
    fn resource_stop_messages_are_fixed_without_input_details() {
        assert_eq!(
            ResourceStop::InputLimitExceeded.to_string(),
            "Model exceeds supported parsing limits"
        );
        assert_eq!(
            ResourceStop::ReadWorkExceeded.to_string(),
            "Model exceeds supported read-work limits"
        );
        assert_eq!(
            ResourceStop::EnvelopeChanged.to_string(),
            "Model source changed during processing"
        );
    }
}
