//! Shared fixture builders for stl-core's integration tests (spec §7).
//! Building STL bytes in code (rather than shipping binary fixture files)
//! keeps every fixture's exact defect visible in the test that uses it.

pub fn binary_header(name: &[u8]) -> [u8; 80] {
    let mut h = [0u8; 80];
    let n = name.len().min(80);
    h[..n].copy_from_slice(&name[..n]);
    h
}

/// Build a well-formed binary STL from a list of (normal, [v0, v1, v2]) triangles.
pub fn binary_stl(header: &[u8; 80], triangles: &[([f32; 3], [[f32; 3]; 3])]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(header);
    out.extend_from_slice(&(triangles.len() as u32).to_le_bytes());
    for (normal, verts) in triangles {
        for c in normal {
            out.extend_from_slice(&c.to_le_bytes());
        }
        for v in verts {
            for c in v {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
        out.extend_from_slice(&[0u8, 0u8]); // attribute byte count
    }
    out
}

/// Binary STL with a header claiming `false_count` triangles but only
/// `actual_count` triangles' worth of data actually present — a truncated file.
pub fn binary_stl_truncated(
    header: &[u8; 80],
    triangles: &[([f32; 3], [[f32; 3]; 3])],
    false_count: u32,
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(header);
    out.extend_from_slice(&false_count.to_le_bytes());
    for (normal, verts) in triangles {
        for c in normal {
            out.extend_from_slice(&c.to_le_bytes());
        }
        for v in verts {
            for c in v {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
        out.extend_from_slice(&[0u8, 0u8]);
    }
    out
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Two triangles for a planar quad `a,b,c,d` (in cyclic boundary order), with
/// winding corrected as needed so the winding-order normal actually agrees
/// with the given outward `normal` — computed, not hand-derived, so the
/// fixture can't silently carry the wrong winding for a face (as a hand-
/// verified version of this once did).
fn quad_face(
    normal: [f32; 3],
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
    d: [f32; 3],
) -> [([f32; 3], [[f32; 3]; 3]); 2] {
    let winding = cross3(sub3(b, a), sub3(c, a));
    let (t1, t2) = if dot3(winding, normal) > 0.0 {
        ([a, b, c], [a, c, d])
    } else {
        ([a, c, b], [a, d, c])
    };
    [(normal, t1), (normal, t2)]
}

/// A valid, closed, consistently-wound unit cube (12 triangles), edge length `s`.
/// Outward-facing normals, winding verified against each normal by `quad_face`.
pub fn unit_cube_triangles(s: f32) -> Vec<([f32; 3], [[f32; 3]; 3])> {
    let v = [
        [0.0, 0.0, 0.0],
        [s, 0.0, 0.0],
        [s, s, 0.0],
        [0.0, s, 0.0],
        [0.0, 0.0, s],
        [s, 0.0, s],
        [s, s, s],
        [0.0, s, s],
    ];
    let mut tris = Vec::with_capacity(12);
    tris.extend(quad_face([0.0, 0.0, -1.0], v[0], v[1], v[2], v[3])); // bottom
    tris.extend(quad_face([0.0, 0.0, 1.0], v[4], v[5], v[6], v[7])); // top
    tris.extend(quad_face([0.0, -1.0, 0.0], v[0], v[1], v[5], v[4])); // front
    tris.extend(quad_face([1.0, 0.0, 0.0], v[1], v[2], v[6], v[5])); // right
    tris.extend(quad_face([0.0, 1.0, 0.0], v[2], v[3], v[7], v[6])); // back
    tris.extend(quad_face([-1.0, 0.0, 0.0], v[3], v[0], v[4], v[7])); // left
    tris
}

/// The unit cube with one face's winding reversed (still watertight, but one
/// edge is now traversed the same direction by both adjacent triangles —
/// isolates `inconsistent_orientation` from `normal_disagreements`, R-04).
pub fn unit_cube_one_face_backwards(s: f32) -> Vec<([f32; 3], [[f32; 3]; 3])> {
    let mut tris = unit_cube_triangles(s);
    let (n, [a, b, c]) = tris[0];
    tris[0] = (n, [a, c, b]); // reverse winding, keep the (now-wrong) stored normal
    tris
}

/// The unit cube missing one face — a boundary (hole), not non-manifold.
pub fn open_cube_triangles(s: f32) -> Vec<([f32; 3], [[f32; 3]; 3])> {
    let mut tris = unit_cube_triangles(s);
    tris.truncate(10); // drop the last face's 2 triangles
    tris
}

pub fn ascii_stl(name: &str, triangles: &[([f32; 3], [[f32; 3]; 3])]) -> String {
    let mut s = format!("solid {name}\n");
    for (n, verts) in triangles {
        s += &format!("  facet normal {} {} {}\n", n[0], n[1], n[2]);
        s += "    outer loop\n";
        for v in verts {
            s += &format!("      vertex {} {} {}\n", v[0], v[1], v[2]);
        }
        s += "    endloop\n  endfacet\n";
    }
    s += &format!("endsolid {name}\n");
    s
}
