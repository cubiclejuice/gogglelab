use std::{fmt, io};

use library_protocol::{RuntimeError, RuntimeErrorCode};
use stl_core::zip_bounds::{classify_zip_error, ZipOpenError};
use stl_core::{ResourceStop, StlError};

/// Typed processing failures retained until the terminal presentation boundary.
#[derive(Debug)]
pub enum ProcessingError {
    ResourceStopped(ResourceStop),
    AllocationFailed,
    FundingBusy,
    Ordinary(String),
}

impl fmt::Display for ProcessingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResourceStopped(stop) => fmt::Display::fmt(stop, formatter),
            Self::AllocationFailed => {
                formatter.write_str("Model processing could not allocate memory")
            }
            Self::FundingBusy => {
                formatter.write_str("Model processing funding is temporarily unavailable")
            }
            Self::Ordinary(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ProcessingError {}

impl From<ZipOpenError> for ProcessingError {
    fn from(error: ZipOpenError) -> Self {
        match error {
            ZipOpenError::ResourceStopped(stop) => Self::ResourceStopped(stop),
            ZipOpenError::AllocationFailed => Self::AllocationFailed,
            ZipOpenError::FundingBusy => Self::FundingBusy,
            ZipOpenError::Archive(error) => Self::Ordinary(error.to_string()),
        }
    }
}

impl From<StlError> for ProcessingError {
    fn from(error: StlError) -> Self {
        match error {
            StlError::ResourceStopped(stop) => Self::ResourceStopped(stop),
            StlError::AllocationFailed => Self::AllocationFailed,
            StlError::FundingBusy => Self::FundingBusy,
            ordinary => Self::Ordinary(ordinary.to_string()),
        }
    }
}

impl From<String> for ProcessingError {
    fn from(message: String) -> Self {
        Self::Ordinary(message)
    }
}

impl From<&str> for ProcessingError {
    fn from(message: &str) -> Self {
        Self::Ordinary(message.into())
    }
}

impl ProcessingError {
    pub(crate) fn from_io(error: io::Error) -> Self {
        match classify_zip_error(zip::result::ZipError::Io(error)) {
            // Preserve the existing ordinary IO message, without a ZIP prefix.
            ZipOpenError::Archive(zip::result::ZipError::Io(error)) => {
                Self::Ordinary(error.to_string())
            }
            classified => classified.into(),
        }
    }

    pub(crate) fn is_terminal(&self) -> bool {
        !matches!(self, Self::Ordinary(_))
    }

    pub(crate) fn with_ordinary_message(self, message: impl Into<String>) -> Self {
        match self {
            Self::Ordinary(_) => Self::Ordinary(message.into()),
            terminal => terminal,
        }
    }

    pub(crate) fn into_runtime(self, fallback: RuntimeError) -> RuntimeError {
        let (code, retryable) = match &self {
            Self::Ordinary(_) => return fallback,
            Self::ResourceStopped(ResourceStop::EnvelopeChanged) => (RuntimeErrorCode::Stale, true),
            Self::ResourceStopped(_) => (RuntimeErrorCode::Unsupported, false),
            Self::AllocationFailed => (RuntimeErrorCode::Internal, false),
            Self::FundingBusy => (RuntimeErrorCode::Busy, true),
        };
        RuntimeError {
            code,
            message: self.to_string(),
            retryable,
            current_version: None,
            source_holds: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stl_core::zip_bounds::ZipAllocationFailed;

    fn fallback() -> RuntimeError {
        RuntimeError {
            code: RuntimeErrorCode::InvalidInput,
            message: "existing ordinary presentation".into(),
            retryable: true,
            current_version: Some(19),
            source_holds: Vec::new(),
        }
    }

    #[test]
    fn typed_processing_error_preserves_only_concrete_io_markers() {
        for stop in [
            ResourceStop::InputLimitExceeded,
            ResourceStop::ReadWorkExceeded,
            ResourceStop::EnvelopeChanged,
        ] {
            let error = ProcessingError::from_io(stop.into_io_error());
            assert!(matches!(error, ProcessingError::ResourceStopped(actual) if actual == stop));
            assert!(!ProcessingError::from_io(io::Error::other(stop.to_string())).is_terminal());
        }
        assert!(matches!(
            ProcessingError::from_io(ZipAllocationFailed.into_io_error()),
            ProcessingError::AllocationFailed
        ));
        assert!(!ProcessingError::from_io(io::Error::new(
            io::ErrorKind::OutOfMemory,
            "Model processing could not allocate memory"
        ))
        .is_terminal());
        assert!(matches!(
            ProcessingError::from_io(io::Error::other(zip::read::MetadataLimitExceeded {
                kind: zip::read::MetadataLimitKind::ReadBytes,
                limit: 1,
                attempted: 2,
            })),
            ProcessingError::ResourceStopped(ResourceStop::ReadWorkExceeded)
        ));
    }

    #[test]
    fn funding_busy_preserves_concrete_marker_and_rejects_message_lookalikes() {
        use stl_core::zip_bounds::ZipFundingBusy;
        assert!(matches!(
            ProcessingError::from_io(ZipFundingBusy.into_io_error()),
            ProcessingError::FundingBusy
        ));
        assert!(matches!(
            ProcessingError::from(StlError::FundingBusy),
            ProcessingError::FundingBusy
        ));
        let ordinary = std::io::Error::other("Model processing funding is temporarily unavailable");
        assert!(!ProcessingError::from_io(ordinary).is_terminal());
    }

    #[test]
    fn typed_processing_error_terminal_runtime_mapping_does_not_relabel_ordinary_errors() {
        let cases = [
            (ProcessingError::FundingBusy, RuntimeErrorCode::Busy, true),
            (
                ProcessingError::ResourceStopped(ResourceStop::InputLimitExceeded),
                RuntimeErrorCode::Unsupported,
                false,
            ),
            (
                ProcessingError::ResourceStopped(ResourceStop::ReadWorkExceeded),
                RuntimeErrorCode::Unsupported,
                false,
            ),
            (
                ProcessingError::ResourceStopped(ResourceStop::EnvelopeChanged),
                RuntimeErrorCode::Stale,
                true,
            ),
            (
                ProcessingError::AllocationFailed,
                RuntimeErrorCode::Internal,
                false,
            ),
        ];
        for (error, code, retryable) in cases {
            let message = error.to_string();
            let mapped = error
                .with_ordinary_message("must not replace stop")
                .into_runtime(fallback());
            assert_eq!(mapped.code, code);
            assert_eq!(mapped.retryable, retryable);
            assert_eq!(mapped.message, message);
        }
        assert_eq!(
            ProcessingError::from("ordinary").into_runtime(fallback()),
            fallback()
        );
        assert_eq!(
            ProcessingError::from("old")
                .with_ordinary_message("new")
                .to_string(),
            "new"
        );
        assert!(matches!(
            ProcessingError::from(StlError::AllocationFailed),
            ProcessingError::AllocationFailed
        ));
    }
}
