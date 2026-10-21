use serde::Serialize;

/// The full app-level error taxonomy (spec §2). `stl_core::StlError` is the
/// parse-time subset; everything else (I/O, staleness, slicer/launch) is
/// specific to this layer, which is why the mapping lives here rather than in
/// stl-core.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum AppError {
    NotFound,
    NotReadable,
    EmptyFile,
    FileTooLarge {
        size: u64,
        limit: u64,
    },
    NotStl,
    MalformedHeader,
    TruncatedFile {
        expected: u64,
        actual: u64,
    },
    TooManyTriangles {
        count: u32,
        limit: u32,
    },
    NonFiniteCoordinate {
        triangle_index: u32,
    },
    /// Zip package that is not a readable 3MF; `detail` names the problem.
    Malformed3mf {
        detail: String,
    },
    MalformedStep {
        detail: String,
    },
    Fusion {
        message: String,
        setup_required: bool,
    },
    /// A newer `open_model`/`model_geometry` call has superseded this one
    /// (spec §2, R-02) — the backend, not the frontend, is the source of
    /// truth for staleness.
    Superseded,
    NoModelLoaded,
    UnknownPlate,
    Archive {
        message: String,
    },
    Watch {
        message: String,
    },
    /// list_dir was asked for a path outside its OS-supplied root.
    OutsideRoot,
    UnknownSlicer,
    LaunchFailed {
        status: Option<i32>,
    },
    EntitlementRequired {
        feature: String,
    },
    Library {
        message: String,
    },
}

impl From<stl_core::StlError> for AppError {
    fn from(e: stl_core::StlError) -> Self {
        match e {
            stl_core::StlError::EmptyFile => AppError::EmptyFile,
            stl_core::StlError::FileTooLarge { size, limit } => {
                AppError::FileTooLarge { size, limit }
            }
            stl_core::StlError::NotStl => AppError::NotStl,
            stl_core::StlError::MalformedHeader => AppError::MalformedHeader,
            stl_core::StlError::TruncatedFile { expected, actual } => {
                AppError::TruncatedFile { expected, actual }
            }
            stl_core::StlError::TooManyTriangles { count, limit } => {
                AppError::TooManyTriangles { count, limit }
            }
            stl_core::StlError::NonFiniteCoordinate { triangle_index } => {
                AppError::NonFiniteCoordinate { triangle_index }
            }
            stl_core::StlError::Malformed3mf { detail } => AppError::Malformed3mf { detail },
            stl_core::StlError::ResourceStopped(stop) => AppError::Library {
                message: stop.to_string(),
            },
            stl_core::StlError::FundingBusy => AppError::Library {
                message: "Model processing funding is temporarily unavailable".into(),
            },
            stl_core::StlError::AllocationFailed => AppError::Library {
                message: "Model processing could not allocate memory".into(),
            },
        }
    }
}

impl From<library_protocol::RuntimeError> for AppError {
    fn from(error: library_protocol::RuntimeError) -> Self {
        AppError::Library {
            message: error.message,
        }
    }
}

impl From<library_runtime::ProcessingError> for AppError {
    fn from(error: library_runtime::ProcessingError) -> Self {
        AppError::Library {
            message: error.to_string(),
        }
    }
}

impl From<stl_core::zip_bounds::ZipOpenError> for AppError {
    fn from(error: stl_core::zip_bounds::ZipOpenError) -> Self {
        match error {
            stl_core::zip_bounds::ZipOpenError::ResourceStopped(stop) => {
                stl_core::StlError::ResourceStopped(stop).into()
            }
            stl_core::zip_bounds::ZipOpenError::FundingBusy => {
                stl_core::StlError::FundingBusy.into()
            }
            stl_core::zip_bounds::ZipOpenError::AllocationFailed => {
                stl_core::StlError::AllocationFailed.into()
            }
            stl_core::zip_bounds::ZipOpenError::Archive(error) => {
                crate::archive::map_zip_open_error(error)
            }
        }
    }
}

pub fn map_io_error(e: &std::io::Error) -> AppError {
    match e.kind() {
        std::io::ErrorKind::NotFound => AppError::NotFound,
        _ => AppError::NotReadable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_runtime_errors_reach_the_native_boundary_without_changing_shape() {
        for stop in [
            stl_core::ResourceStop::InputLimitExceeded,
            stl_core::ResourceStop::ReadWorkExceeded,
            stl_core::ResourceStop::EnvelopeChanged,
        ] {
            let error = AppError::from(library_runtime::ProcessingError::ResourceStopped(stop));
            assert_eq!(
                serde_json::to_value(error).unwrap(),
                serde_json::json!({ "kind": "Library", "message": stop.to_string() })
            );
        }
        for error in [
            library_runtime::ProcessingError::AllocationFailed,
            library_runtime::ProcessingError::FundingBusy,
            library_runtime::ProcessingError::Ordinary("ordinary failure".into()),
        ] {
            let message = error.to_string();
            assert_eq!(
                serde_json::to_value(AppError::from(error)).unwrap(),
                serde_json::json!({ "kind": "Library", "message": message })
            );
        }
    }

    #[test]
    fn funding_busy_keeps_the_existing_native_library_error_shape() {
        for error in [
            AppError::from(stl_core::StlError::FundingBusy),
            AppError::from(stl_core::zip_bounds::ZipOpenError::FundingBusy),
            AppError::from(library_runtime::ProcessingError::FundingBusy),
        ] {
            assert_eq!(
                serde_json::to_value(error).unwrap(),
                serde_json::json!({
                    "kind": "Library",
                    "message": "Model processing funding is temporarily unavailable"
                })
            );
        }
    }

    #[test]
    fn allocation_failure_keeps_the_existing_native_library_error_shape() {
        let mapped = AppError::from(stl_core::StlError::AllocationFailed);
        assert_eq!(
            serde_json::to_value(mapped).unwrap(),
            serde_json::json!({
                "kind": "Library",
                "message": "Model processing could not allocate memory"
            })
        );
    }

    #[test]
    fn resource_stops_keep_the_existing_native_library_error_shape() {
        for stop in [
            stl_core::ResourceStop::InputLimitExceeded,
            stl_core::ResourceStop::ReadWorkExceeded,
            stl_core::ResourceStop::EnvelopeChanged,
        ] {
            let mapped = AppError::from(stl_core::StlError::ResourceStopped(stop));
            let serialized = serde_json::to_value(mapped).unwrap();
            assert_eq!(
                serialized,
                serde_json::json!({ "kind": "Library", "message": stop.to_string() })
            );
        }
    }
}
