//! IPC geometry buffer, v2 (spec §2). Positions only — no normals; the
//! frontend calls `computeVertexNormals()` on the non-indexed geometry, which
//! for non-indexed BufferGeometry assigns each vertex its own face normal
//! (verified against three.js docs, R-10), so shipping normals over IPC would
//! only double the payload to duplicate that.
//!
//! Layout (little-endian throughout):
//!   0   4        magic: the ASCII bytes 'S','T','L','2' in file order
//!   4   4        version = 2 (u32)
//!   8   4        triangle_count = N (u32)
//!   12  4        reserved (0)
//!   16  36*N     positions: f32[N*9]

pub const MAGIC: [u8; 4] = *b"STL2";
pub const VERSION: u32 = 2;

pub fn build(parsed: &stl_core::ParsedStl) -> Vec<u8> {
    build_positions(parsed.triangle_count, &parsed.positions)
}

/// Encode a validated contiguous plate range without copying a second mesh.
pub fn build_positions(triangle_count: u32, positions: &[f32]) -> Vec<u8> {
    debug_assert_eq!(positions.len(), triangle_count as usize * 9);
    let mut buf = Vec::with_capacity(16 + positions.len() * 4);
    buf.extend_from_slice(&MAGIC);
    buf.extend_from_slice(&VERSION.to_le_bytes());
    buf.extend_from_slice(&triangle_count.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes()); // reserved
    for f in positions {
        buf.extend_from_slice(&f.to_le_bytes());
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_bytes_read_in_file_order() {
        let parsed = stl_core::ParsedStl {
            format: stl_core::StlFormat::Binary,
            triangle_count: 1,
            positions: vec![0.0; 9],
            stored_normals: vec![0.0; 3],
        };
        let buf = build(&parsed);
        assert_eq!(
            &buf[0..4],
            b"STL2",
            "magic bytes must read S,T,L,2 in file order"
        );
        assert_eq!(u32::from_le_bytes(buf[4..8].try_into().unwrap()), VERSION);
        assert_eq!(u32::from_le_bytes(buf[8..12].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(buf[12..16].try_into().unwrap()), 0);
        assert_eq!(buf.len(), 16 + 9 * 4);
    }

    #[test]
    fn positions_length_matches_nine_times_triangle_count() {
        let parsed = stl_core::ParsedStl {
            format: stl_core::StlFormat::Binary,
            triangle_count: 3,
            positions: (0..27).map(|i| i as f32).collect(),
            stored_normals: vec![0.0; 9],
        };
        let buf = build(&parsed);
        let n = u32::from_le_bytes(buf[8..12].try_into().unwrap()) as usize;
        let position_bytes = &buf[16..];
        assert_eq!(position_bytes.len() / 4, 9 * n);

        let first = f32::from_le_bytes(buf[16..20].try_into().unwrap());
        assert_eq!(first, 0.0);
        let last = f32::from_le_bytes(buf[buf.len() - 4..].try_into().unwrap());
        assert_eq!(last, 26.0);
    }
}
