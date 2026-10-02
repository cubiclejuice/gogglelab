use crate::parse::ParsedStl;
use crate::vec3::{cross, dot, sub, V3};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bbox {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// Geometry measures for a non-empty mesh (spec §2 — `None` for 0 triangles,
/// since a bbox has no defined value there; that case is `measure()` returning
/// `None`, not an error).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeometryStats {
    pub bbox: Bbox,
    pub dimensions: [f32; 3],
    pub surface_area_mm2: f64,
    pub volume_mm3: f64,
}

/// bbox, dimensions, volume, and area in a single traversal (R-020) — four
/// passes over a triangle list this size is cache-hostile and was flagged in
/// review as duplicated iteration for no reason.
///
/// Volume is the absolute value of the signed-tetrahedron sum (each triangle
/// contributes `dot(v0, cross(v1, v2)) / 6` relative to the origin); this is
/// only physically meaningful for a closed, consistently-wound mesh, which is
/// exactly why spec §2 treats it as provisional until `integrity()` confirms
/// both. `measure()` itself just computes the number.
pub fn measure(parsed: &ParsedStl) -> Option<GeometryStats> {
    if parsed.triangle_count == 0 {
        return None;
    }

    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    let mut signed_volume_sum: f64 = 0.0;
    let mut area_sum: f64 = 0.0;

    for tri in parsed.positions.chunks_exact(9) {
        let v0: V3 = [tri[0], tri[1], tri[2]];
        let v1: V3 = [tri[3], tri[4], tri[5]];
        let v2: V3 = [tri[6], tri[7], tri[8]];

        for v in [v0, v1, v2] {
            for axis in 0..3 {
                min[axis] = min[axis].min(v[axis]);
                max[axis] = max[axis].max(v[axis]);
            }
        }

        signed_volume_sum += dot(v0, cross(v1, v2)) as f64 / 6.0;

        let edge_area = cross(sub(v1, v0), sub(v2, v0));
        area_sum += (dot(edge_area, edge_area) as f64).sqrt() / 2.0;
    }

    let dimensions = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];

    Some(GeometryStats {
        bbox: Bbox { min, max },
        dimensions,
        surface_area_mm2: area_sum,
        volume_mm3: signed_volume_sum.abs(),
    })
}
