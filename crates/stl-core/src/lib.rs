//! Pure STL and 3MF parsing and measurement. No Tauri dependency (spec §1) —
//! this crate is unit-testable with plain `cargo test` and stays portable.
//!
//! Both formats reduce to the same `ParsedStl` triangle soup, so measure(),
//! integrity() and the IPC geometry buffer never know which file they came from.

pub mod error;
pub mod integrity;
pub mod limits;
pub mod measure;
pub mod model;
pub mod parse;
pub mod threemf;
mod vec3;

pub use error::StlError;
pub use integrity::{integrity, Integrity, IntegrityResult};
pub use measure::{measure, Bbox, GeometryStats};
pub use model::{
    parse_model_with_plates, MaterialSource, ParsedFilament, ParsedMaterial, ParsedModel,
    ParsedPart, Plate,
};
pub use parse::{parse, ParsedStl, StlFormat};
pub use threemf::parse_3mf;

/// Parse a model file of either supported format. A 3MF is a zip package, so
/// the local-file-header magic is a reliable discriminator; anything else is
/// handed to the STL parser, which does its own binary/ASCII detection. The
/// file extension is deliberately not consulted: a misnamed file should still
/// open if its bytes are readable, and fail with the right error if not.
pub fn parse_model(bytes: &[u8]) -> Result<ParsedStl, StlError> {
    Ok(parse_model_with_plates(bytes)?.model)
}
