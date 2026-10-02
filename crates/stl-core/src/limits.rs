//! Limits from spec §3. `MAX_FILE_BYTES` is enforced by the Tauri command layer
//! (S4), which stats the file before ever reading it into memory; `parse()` here
//! enforces `MAX_TRIANGLES` because triangle count is intrinsic to STL content
//! and can be checked before allocating the geometry buffers.

pub const MAX_FILE_BYTES: u64 = 100 * 1024 * 1024;
pub const MAX_TRIANGLES: u32 = 2_000_000;
pub const INTEGRITY_MAX_TRIANGLES: u32 = 1_000_000;

/// 3MF model parts are deflated XML and routinely inflate 5-10x. This caps
/// the total bytes the reader will decompress across all parts of one
/// package so a small file cannot expand into an unbounded allocation.
pub const MAX_3MF_INFLATED_BYTES: u64 = 512 * 1024 * 1024;
