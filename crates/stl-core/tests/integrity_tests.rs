mod common;

use common::*;
use stl_core::integrity::{integrity, IntegrityResult};
use stl_core::{parse, Integrity};

fn computed(bytes: &[u8]) -> Integrity {
    let parsed = parse(bytes).unwrap();
    match integrity(&parsed) {
        IntegrityResult::Computed(i) => i,
        other => panic!("expected Computed, got {other:?}"),
    }
}

#[test]
fn watertight_unit_cube_has_no_boundary_or_non_manifold_edges() {
    let bytes = binary_stl(&binary_header(b"cube"), &unit_cube_triangles(1.0));
    let i = computed(&bytes);

    assert_eq!(i.boundary_edges, 0);
    assert_eq!(i.non_manifold_edges, 0);
    assert_eq!(i.inconsistent_orientation, 0);
    assert_eq!(i.degenerate_triangles, 0);
    assert_eq!(i.distinct_vertices, 8);
    assert!(i.watertight());
}

#[test]
fn open_cube_reports_boundary_edges_not_non_manifold() {
    // Removing one face leaves exactly 4 boundary edges (the missing face's
    // perimeter) and must NOT be reported as non-manifold — a hole and a
    // branching surface are different defects (R-04's central point).
    let bytes = binary_stl(&binary_header(b"open cube"), &open_cube_triangles(1.0));
    let i = computed(&bytes);

    assert_eq!(i.boundary_edges, 4);
    assert_eq!(i.non_manifold_edges, 0);
    assert!(!i.watertight());
}

#[test]
fn backwards_face_cube_is_watertight_but_orientation_inconsistent() {
    // The fixture that separates inconsistent_orientation from
    // normal_disagreements (R-04): reversing one face's winding does not
    // remove or add any edges (still watertight), but every edge that
    // triangle shares now agrees in direction with its neighbor instead of
    // opposing it.
    let bytes = binary_stl(
        &binary_header(b"backwards face"),
        &unit_cube_one_face_backwards(1.0),
    );
    let i = computed(&bytes);

    assert!(
        i.watertight(),
        "reversing a face's winding must not create edges"
    );
    assert_eq!(
        i.inconsistent_orientation, 3,
        "the reversed triangle shares 3 edges, each now co-directional with its neighbor"
    );
}

#[test]
fn three_triangles_sharing_an_edge_is_non_manifold() {
    // A "tent": two triangles forming a square (sharing a diagonal edge, as
    // in half of unit_cube) plus a third triangle glued onto that same
    // diagonal — that shared diagonal is now used by 3 faces.
    let a = [0.0, 0.0, 0.0];
    let b = [1.0, 0.0, 0.0];
    let c = [1.0, 1.0, 0.0];
    let d = [0.0, 1.0, 0.0];
    let e = [0.0, 0.0, 1.0]; // a third triangle glued onto edge a-c
    let tris = vec![
        ([0.0, 0.0, 1.0], [a, b, c]),
        ([0.0, 0.0, 1.0], [a, c, d]),
        ([0.0, 1.0, 0.0], [a, c, e]),
    ];
    let bytes = binary_stl(&binary_header(b"tent"), &tris);
    let i = computed(&bytes);

    assert_eq!(i.non_manifold_edges, 1);
}

#[test]
fn zero_area_triangle_is_degenerate() {
    let tris = vec![(
        [0.0, 0.0, 1.0],
        [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]], // collinear -> zero area
    )];
    let bytes = binary_stl(&binary_header(b"degenerate"), &tris);
    let i = computed(&bytes);

    assert_eq!(i.degenerate_triangles, 1);
}

#[test]
fn all_zero_stored_normals_are_reported_and_excluded_from_disagreements() {
    let mut tris = unit_cube_triangles(1.0);
    for t in tris.iter_mut() {
        t.0 = [0.0, 0.0, 0.0];
    }
    let bytes = binary_stl(&binary_header(b"zero normals"), &tris);
    let i = computed(&bytes);

    assert_eq!(i.zero_normals, 12);
    assert_eq!(
        i.normal_disagreements, 0,
        "a missing normal is a fact about the exporter, not a disagreement"
    );
}

#[test]
fn stored_normal_pointing_the_wrong_way_is_a_disagreement() {
    let mut tris = unit_cube_triangles(1.0);
    // Flip the first triangle's stored normal to point the opposite way from
    // its actual winding-order normal, without touching the winding itself.
    tris[0].0 = [-tris[0].0[0], -tris[0].0[1], -tris[0].0[2]];
    let bytes = binary_stl(&binary_header(b"wrong normal"), &tris);
    let i = computed(&bytes);

    assert_eq!(i.normal_disagreements, 1);
    // Winding itself is untouched, so topology is unaffected.
    assert!(i.watertight());
    assert_eq!(i.inconsistent_orientation, 0);
}

#[test]
fn distinct_vertices_welds_near_duplicate_coordinates() {
    // Two vertices that differ by far less than WELD_EPS must weld to the
    // same canonical key — this is what makes distinct_vertices meaningful
    // for triangle soup, where exact float equality across triangles from
    // different exporters is not guaranteed.
    let tris = vec![
        (
            [0.0, 0.0, 1.0],
            [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        ),
        (
            [0.0, 0.0, 1.0],
            // Same triangle, third vertex nudged by 1e-7 -- far below WELD_EPS (1e-4).
            [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0000001, 1.0, 0.0]],
        ),
    ];
    let bytes = binary_stl(&binary_header(b"near dup"), &tris);
    let i = computed(&bytes);

    assert_eq!(i.distinct_vertices, 3);
}

#[test]
fn integrity_is_skipped_above_the_triangle_cap() {
    let bytes = binary_stl(&binary_header(b"tiny"), &unit_cube_triangles(1.0));
    let mut parsed = parse(&bytes).unwrap();
    // Simulate a mesh over INTEGRITY_MAX_TRIANGLES without actually allocating
    // that many triangles — only triangle_count is read by the cap check.
    parsed.triangle_count = stl_core::limits::INTEGRITY_MAX_TRIANGLES + 1;

    match integrity(&parsed) {
        IntegrityResult::SkippedTooLarge {
            triangle_count,
            limit,
        } => {
            assert_eq!(
                triangle_count,
                stl_core::limits::INTEGRITY_MAX_TRIANGLES + 1
            );
            assert_eq!(limit, stl_core::limits::INTEGRITY_MAX_TRIANGLES);
        }
        other => panic!("expected SkippedTooLarge, got {other:?}"),
    }
}
