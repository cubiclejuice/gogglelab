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
        }
    }
}

pub fn map_io_error(e: &std::io::Error) -> AppError {
    match e.kind() {
        std::io::ErrorKind::NotFound => AppError::NotFound,
        _ => AppError::NotReadable,
    }
}
