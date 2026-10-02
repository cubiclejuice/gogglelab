mod common;

use common::*;
use stl_core::{measure, parse};

#[test]
fn zero_triangle_stl_measures_to_none() {
    let bytes = binary_stl(&binary_header(b"empty"), &[]);
    let parsed = parse(&bytes).unwrap();
    assert_eq!(measure(&parsed), None);
}

#[test]
fn unit_cube_measures_correctly() {
    let bytes = binary_stl(&binary_header(b"unit cube"), &unit_cube_triangles(1.0));
    let parsed = parse(&bytes).unwrap();
    let stats = measure(&parsed).expect("non-empty mesh must measure");

    assert!(
        (stats.volume_mm3 - 1.0).abs() < 1e-6,
        "volume: {}",
        stats.volume_mm3
    );
    assert!(
        (stats.surface_area_mm2 - 6.0).abs() < 1e-6,
        "area: {}",
        stats.surface_area_mm2
    );
    assert_eq!(stats.bbox.min, [0.0, 0.0, 0.0]);
    assert_eq!(stats.bbox.max, [1.0, 1.0, 1.0]);
    assert_eq!(stats.dimensions, [1.0, 1.0, 1.0]);
}

#[test]
fn scaled_cube_measures_correctly() {
    let size = 20.0f32;
    let bytes = binary_stl(&binary_header(b"cube20"), &unit_cube_triangles(size));
    let parsed = parse(&bytes).unwrap();
    let stats = measure(&parsed).unwrap();

    assert!(
        (stats.volume_mm3 - 8000.0).abs() < 1e-3,
        "volume: {}",
        stats.volume_mm3
    );
    assert!(
        (stats.surface_area_mm2 - 2400.0).abs() < 1e-3,
        "area: {}",
        stats.surface_area_mm2
    );
}

#[test]
fn regular_tetrahedron_measures_correctly() {
    // A right-angle-corner tetrahedron with legs of length `a` along the axes
    // has an exact, easy-to-verify volume of a^3 / 6 and a known surface area.
    let a = 3.0f32;
    let v = [[0.0, 0.0, 0.0], [a, 0.0, 0.0], [0.0, a, 0.0], [0.0, 0.0, a]];
    // Outward-facing normals for a right-corner tetrahedron at the origin.
    let tris = vec![
        ([0.0, 0.0, -1.0], [v[0], v[2], v[1]]), // base (z=0), facing down
        ([-1.0, 0.0, 0.0], [v[0], v[3], v[2]]), // x=0 face
        ([0.0, -1.0, 0.0], [v[0], v[1], v[3]]), // y=0 face
        ([1.0, 1.0, 1.0], [v[1], v[2], v[3]]), // slanted face (not unit, direction only matters for sign check we don't assert)
    ];
    let bytes = binary_stl(&binary_header(b"tetra"), &tris);
    let parsed = parse(&bytes).unwrap();
    let stats = measure(&parsed).unwrap();

    let expected_volume = (a * a * a / 6.0) as f64;
    assert!(
        (stats.volume_mm3 - expected_volume).abs() < 1e-4,
        "volume: {} expected {}",
        stats.volume_mm3,
        expected_volume
    );
}

#[test]
fn planar_model_has_zero_extent_on_one_axis() {
    // A flat (z=0) triangle — the fixture that would divide-by-zero the
    // camera framing in the frontend (F-014) if dimensions weren't clamped
    // there. measure() itself must report the true zero extent honestly;
    // the clamp is the viewport's job (S5), not measure()'s.
    let tris = vec![(
        [0.0, 0.0, 1.0],
        [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0]],
    )];
    let bytes = binary_stl(&binary_header(b"flat"), &tris);
    let parsed = parse(&bytes).unwrap();
    let stats = measure(&parsed).unwrap();

    assert_eq!(stats.dimensions[2], 0.0);
    assert_eq!(stats.volume_mm3, 0.0);
}
