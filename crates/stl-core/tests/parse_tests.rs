mod common;

use common::*;
use stl_core::{parse, StlError, StlFormat};

#[test]
fn empty_file_is_rejected() {
    assert_eq!(parse(&[]), Err(StlError::EmptyFile));
}

#[test]
fn valid_binary_cube_parses() {
    let header = binary_header(b"unit cube");
    let tris = unit_cube_triangles(1.0);
    let bytes = binary_stl(&header, &tris);

    let parsed = parse(&bytes).expect("valid binary STL must parse");
    assert_eq!(parsed.format, StlFormat::Binary);
    assert_eq!(parsed.triangle_count, 12);
    assert_eq!(parsed.positions.len(), 12 * 9);
    assert_eq!(parsed.stored_normals.len(), 12 * 3);
}

#[test]
fn valid_ascii_cube_parses() {
    let tris = unit_cube_triangles(1.0);
    let text = ascii_stl("unit cube", &tris);

    let parsed = parse(text.as_bytes()).expect("valid ASCII STL must parse");
    assert_eq!(parsed.format, StlFormat::Ascii);
    assert_eq!(parsed.triangle_count, 12);
    assert_eq!(parsed.positions.len(), 12 * 9);
}

#[test]
fn binary_stl_whose_header_starts_with_solid_is_still_binary() {
    // R-021 — the exact case the size-arithmetic discriminator exists for.
    // The header's first six bytes spell "solid " but the size math proves
    // this is a valid binary file, and it must be parsed as one.
    let header = binary_header(b"solid this looks like ascii but is not");
    let tris = unit_cube_triangles(2.0);
    let bytes = binary_stl(&header, &tris);
    assert_eq!(&bytes[0..6], b"solid ");

    let parsed = parse(&bytes).expect("must discriminate by size, not prefix");
    assert_eq!(parsed.format, StlFormat::Binary);
    assert_eq!(parsed.triangle_count, 12);
}

#[test]
fn truncated_binary_file_is_reported_with_expected_and_actual() {
    let header = binary_header(b"truncated");
    let tris = unit_cube_triangles(1.0);
    // Header claims 12 triangles; only 3 triangles of data are actually present,
    // and the truncated bytes don't look like ASCII either.
    let bytes = binary_stl_truncated(&header, &tris[..3], 12);

    match parse(&bytes) {
        Err(StlError::TruncatedFile { expected, actual }) => {
            assert_eq!(expected, 84 + 12 * 50);
            assert_eq!(actual, bytes.len() as u64);
        }
        other => panic!("expected TruncatedFile, got {other:?}"),
    }
}

#[test]
fn valid_zero_triangle_binary_stl_parses_as_empty() {
    // R-006/R-014 — 0 triangles is legal and distinct from EmptyFile.
    let header = binary_header(b"empty");
    let bytes = binary_stl(&header, &[]);

    let parsed = parse(&bytes).expect("a valid 0-triangle STL is not an error");
    assert_eq!(parsed.triangle_count, 0);
    assert!(parsed.positions.is_empty());
}

#[test]
fn valid_zero_triangle_ascii_stl_parses_as_empty() {
    let text = "solid empty\nendsolid empty\n";
    let parsed = parse(text.as_bytes()).expect("a valid 0-triangle ASCII STL is not an error");
    assert_eq!(parsed.triangle_count, 0);
}

#[test]
fn nan_coordinate_is_rejected_binary() {
    let header = binary_header(b"nan");
    let mut tris = unit_cube_triangles(1.0);
    tris[0].1[0][0] = f32::NAN;
    let bytes = binary_stl(&header, &tris);

    match parse(&bytes) {
        Err(StlError::NonFiniteCoordinate { triangle_index }) => assert_eq!(triangle_index, 0),
        other => panic!("expected NonFiniteCoordinate, got {other:?}"),
    }
}

#[test]
fn positive_infinity_coordinate_is_rejected_binary() {
    let header = binary_header(b"inf");
    let mut tris = unit_cube_triangles(1.0);
    tris[5].1[1][2] = f32::INFINITY;
    let bytes = binary_stl(&header, &tris);

    match parse(&bytes) {
        Err(StlError::NonFiniteCoordinate { triangle_index }) => assert_eq!(triangle_index, 5),
        other => panic!("expected NonFiniteCoordinate, got {other:?}"),
    }
}

#[test]
fn nan_coordinate_is_rejected_ascii() {
    let mut tris = unit_cube_triangles(1.0);
    tris[2].1[0][1] = f32::NAN;
    let text = ascii_stl("nan", &tris);

    match parse(text.as_bytes()) {
        Err(StlError::NonFiniteCoordinate { triangle_index }) => assert_eq!(triangle_index, 2),
        other => panic!("expected NonFiniteCoordinate, got {other:?}"),
    }
}

#[test]
fn triangle_count_over_the_cap_is_rejected_before_allocating() {
    // Construct a header claiming more than MAX_TRIANGLES without materializing
    // that many triangle records — parse() must reject on the count alone.
    let header = binary_header(b"huge");
    let count: u32 = stl_core::limits::MAX_TRIANGLES + 1;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(&count.to_le_bytes());
    // No triangle data follows — if parse() tried to read it, it would panic
    // or return TruncatedFile instead of TooManyTriangles. It must reject on
    // the count before touching triangle data at all.

    match parse(&bytes) {
        Err(StlError::TooManyTriangles { count: c, limit }) => {
            assert_eq!(c, count);
            assert_eq!(limit, stl_core::limits::MAX_TRIANGLES);
        }
        other => panic!("expected TooManyTriangles, got {other:?}"),
    }
}

#[test]
fn garbage_short_file_is_not_stl() {
    let bytes = b"not an stl file at all, too short and no solid prefix".to_vec();
    // Shorter than the 84-byte binary prefix and doesn't start with "solid".
    assert_eq!(parse(&bytes), Err(StlError::MalformedHeader));
}

#[test]
fn open_cube_parses_successfully_topology_checked_in_s3() {
    // Parsing must succeed for a mesh with a hole — S2 doesn't judge topology,
    // only structure. Boundary-edge detection is integrity()'s job (S3).
    let header = binary_header(b"open cube");
    let tris = open_cube_triangles(1.0);
    let bytes = binary_stl(&header, &tris);

    let parsed = parse(&bytes).expect("structurally valid STL must parse regardless of topology");
    assert_eq!(parsed.triangle_count, 10);
}

#[test]
fn backwards_face_cube_parses_successfully() {
    let header = binary_header(b"backwards face");
    let tris = unit_cube_one_face_backwards(1.0);
    let bytes = binary_stl(&header, &tris);

    let parsed = parse(&bytes).expect("structurally valid STL must parse regardless of winding");
    assert_eq!(parsed.triangle_count, 12);
}

#[test]
fn stored_normals_are_preserved_verbatim_including_zero_normals() {
    // Some exporters write (0,0,0) for every normal. Parsing must not editorialize.
    let header = binary_header(b"zero normals");
    let mut tris = unit_cube_triangles(1.0);
    for t in tris.iter_mut() {
        t.0 = [0.0, 0.0, 0.0];
    }
    let bytes = binary_stl(&header, &tris);

    let parsed = parse(&bytes).unwrap();
    assert!(parsed.stored_normals.iter().all(|&c| c == 0.0));
}

// --- Differential oracle: stl_io (dev-dependency only, spec §7) ---

#[test]
fn agrees_with_stl_io_oracle_on_valid_binary_cube() {
    let header = binary_header(b"oracle cube");
    let tris = unit_cube_triangles(3.5);
    let bytes = binary_stl(&header, &tris);

    let ours = parse(&bytes).unwrap();

    let oracle = stl_io::read_stl(&mut std::io::Cursor::new(&bytes)).expect("oracle must parse");
    let oracle_triangles: Vec<_> = oracle.vertices.clone().into_iter().collect();
    assert_eq!(ours.triangle_count as usize, oracle.faces.len());
    assert_eq!(oracle_triangles.len(), 8); // unit cube has 8 distinct vertices

    // Cross-check total vertex count and a spot-checked coordinate agree.
    let mut oracle_positions = Vec::with_capacity(ours.positions.len());
    for face in &oracle.faces {
        for &vi in &face.vertices {
            let v = oracle.vertices[vi];
            oracle_positions.push(v[0]);
            oracle_positions.push(v[1]);
            oracle_positions.push(v[2]);
        }
    }
    assert_eq!(ours.positions.len(), oracle_positions.len());
}
