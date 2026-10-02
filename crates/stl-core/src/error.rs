use thiserror::Error;

/// Mirrors the parse-time subset of `AppError` from spec §2. The Tauri layer
/// (src-tauri) maps this 1:1 into the full `AppError` enum — kept separate so
/// this crate has no Tauri dependency (spec §1).
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
}
