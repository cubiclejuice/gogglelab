use std::collections::HashMap;

use crate::limits::INTEGRITY_MAX_TRIANGLES;
use crate::parse::ParsedStl;
use crate::vec3::{cross, dot, sub, V3};

/// Vertex weld tolerance (spec §2): coordinates within this distance on each
/// axis are treated as the same vertex when building canonical edge keys.
/// STL is triangle soup — exact float equality across triangles sharing a
/// vertex is not guaranteed by any exporter.
pub const WELD_EPS: f64 = 1e-4;

/// A triangle is degenerate if its area is at or below this threshold. Not a
/// spec-pinned constant (the spec table doesn't fix a value) — chosen as a
/// tolerance far below anything a legitimate triangle in a printable model
/// would have, so it only fires on true zero/near-zero-area faces.
pub const AREA_EPS: f64 = 1e-9;

/// Angle threshold, in degrees, above which a stored normal is considered to
/// disagree with the triangle's winding-order normal. Not spec-pinned; chosen
/// to absorb ordinary f32 export rounding while still catching a genuinely
/// wrong normal.
pub const NORMAL_DISAGREEMENT_DEGREES: f32 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Integrity {
    pub boundary_edges: u32,
    pub non_manifold_edges: u32,
    pub inconsistent_orientation: u32,
    pub degenerate_triangles: u32,
    pub distinct_vertices: u32,
    pub zero_normals: u32,
    pub normal_disagreements: u32,
}

impl Integrity {
    pub fn watertight(&self) -> bool {
        self.boundary_edges == 0 && self.non_manifold_edges == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityResult {
    Computed(Integrity),
    SkippedTooLarge { triangle_count: u32, limit: u32 },
}

type VKey = (i64, i64, i64);

fn vkey(v: V3) -> VKey {
    (
        (v[0] as f64 / WELD_EPS).round() as i64,
        (v[1] as f64 / WELD_EPS).round() as i64,
        (v[2] as f64 / WELD_EPS).round() as i64,
    )
}

/// Off the critical path (spec §2/§5) — called from a background thread after
/// the model summary has already rendered. Above `INTEGRITY_MAX_TRIANGLES`,
/// skips rather than allocating an unbounded edge map (R-007).
pub fn integrity(parsed: &ParsedStl) -> IntegrityResult {
    if parsed.triangle_count > INTEGRITY_MAX_TRIANGLES {
        return IntegrityResult::SkippedTooLarge {
            triangle_count: parsed.triangle_count,
            limit: INTEGRITY_MAX_TRIANGLES,
        };
    }

    let mut result = Integrity::default();
    let mut vertex_keys: std::collections::HashSet<VKey> = std::collections::HashSet::new();

    // Per canonical undirected edge: (occurrence count, sum of directions).
    // Direction is +1 when a triangle traverses the edge from its
    // lexicographically-smaller endpoint to its larger one, -1 otherwise. For
    // an edge shared by exactly two triangles, a consistently-wound closed
    // mesh always traverses it in opposite directions, so the sum is 0; if
    // both traversed it the same way, the sum is +-2 and one face is flipped
    // relative to the other (R-04 — this, not a stored-normal comparison,
    // is what actually detects a flipped face).
    let mut edges: HashMap<(VKey, VKey), (u32, i32)> = HashMap::new();

    for (i, tri) in parsed.positions.chunks_exact(9).enumerate() {
        let verts: [V3; 3] = [
            [tri[0], tri[1], tri[2]],
            [tri[3], tri[4], tri[5]],
            [tri[6], tri[7], tri[8]],
        ];
        let keys: [VKey; 3] = [vkey(verts[0]), vkey(verts[1]), vkey(verts[2])];
        for k in keys {
            vertex_keys.insert(k);
        }

        let edge_normal = cross(sub(verts[1], verts[0]), sub(verts[2], verts[0]));
        let area = (dot(edge_normal, edge_normal) as f64).sqrt() / 2.0;
        if area <= AREA_EPS {
            result.degenerate_triangles += 1;
        }

        let stored = [
            parsed.stored_normals[i * 3],
            parsed.stored_normals[i * 3 + 1],
            parsed.stored_normals[i * 3 + 2],
        ];
        if stored == [0.0, 0.0, 0.0] {
            result.zero_normals += 1;
        } else {
            let winding_len = (dot(edge_normal, edge_normal) as f64).sqrt();
            if winding_len > 0.0 {
                let winding_unit = [
                    (edge_normal[0] as f64 / winding_len) as f32,
                    (edge_normal[1] as f64 / winding_len) as f32,
                    (edge_normal[2] as f64 / winding_len) as f32,
                ];
                let stored_len = dot(stored, stored).sqrt();
                if stored_len > 0.0 {
                    let stored_unit = [
                        stored[0] / stored_len,
                        stored[1] / stored_len,
                        stored[2] / stored_len,
                    ];
                    let cos_angle = dot(winding_unit, stored_unit).clamp(-1.0, 1.0);
                    let angle_deg = cos_angle.acos().to_degrees();
                    if angle_deg > NORMAL_DISAGREEMENT_DEGREES {
                        result.normal_disagreements += 1;
                    }
                }
            }
        }

        for e in 0..3 {
            let a = keys[e];
            let b = keys[(e + 1) % 3];
            let (canon, dir) = if a <= b {
                ((a, b), 1i32)
            } else {
                ((b, a), -1i32)
            };
            let entry = edges.entry(canon).or_insert((0, 0));
            entry.0 += 1;
            entry.1 += dir;
        }
    }

    for (count, dir_sum) in edges.values() {
        match count {
            1 => result.boundary_edges += 1,
            2 => {
                if *dir_sum != 0 {
                    result.inconsistent_orientation += 1;
                }
            }
            _ => result.non_manifold_edges += 1,
        }
    }

    result.distinct_vertices = vertex_keys.len() as u32;

    IntegrityResult::Computed(result)
}
