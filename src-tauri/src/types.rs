use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlicerPaletteLayout {
    Bambu,
    Elegoo,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StlFormatJson {
    Binary,
    Ascii,
    #[serde(rename = "3mf")]
    ThreeMf,
}

impl From<stl_core::StlFormat> for StlFormatJson {
    fn from(f: stl_core::StlFormat) -> Self {
        match f {
            stl_core::StlFormat::Binary => StlFormatJson::Binary,
            stl_core::StlFormat::Ascii => StlFormatJson::Ascii,
            stl_core::StlFormat::ThreeMf => StlFormatJson::ThreeMf,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BboxJson {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

#[derive(Debug, Clone, Serialize)]
pub struct GeometryStatsJson {
    pub bbox: BboxJson,
    pub dimensions: [f32; 3],
    pub surface_area_mm2: f64,
    pub volume_mm3: f64,
}

impl From<stl_core::GeometryStats> for GeometryStatsJson {
    fn from(g: stl_core::GeometryStats) -> Self {
        GeometryStatsJson {
            bbox: BboxJson {
                min: g.bbox.min,
                max: g.bbox.max,
            },
            dimensions: g.dimensions,
            surface_area_mm2: g.surface_area_mm2,
            volume_mm3: g.volume_mm3,
        }
    }
}

/// Phase 1 result (spec §2/§5) — everything needed for first frame. No
/// `integrity` field: that arrives later via the `integrity` event, off
/// the critical path (R-013).
#[derive(Debug, Clone, Serialize)]
pub struct PartSummary {
    pub id: String,
    pub name: String,
    pub triangle_start: u32,
    pub triangle_count: u32,
    pub material_id: Option<String>,
    pub filament_slot: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub enum MaterialSourceJson {
    #[serde(rename = "3mf_base_material")]
    ThreeMfBaseMaterial,
    #[serde(rename = "3mf_color_group")]
    ThreeMfColorGroup,
}

impl From<stl_core::MaterialSource> for MaterialSourceJson {
    fn from(source: stl_core::MaterialSource) -> Self {
        match source {
            stl_core::MaterialSource::ThreeMfBaseMaterial => Self::ThreeMfBaseMaterial,
            stl_core::MaterialSource::ThreeMfColorGroup => Self::ThreeMfColorGroup,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MaterialSummary {
    pub id: String,
    pub name: Option<String>,
    pub color: Option<String>,
    pub source: MaterialSourceJson,
}

#[derive(Debug, Clone, Serialize)]
pub struct EmbeddedFilamentSummary {
    pub slot: u32,
    pub name: Option<String>,
    pub material: Option<String>,
    pub color: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlateSummary {
    pub id: u32,
    pub name: String,
    pub triangle_count: u32,
    pub geometry: Option<GeometryStatsJson>,
    pub parts: Vec<PartSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelSummary {
    pub generation: u64,
    pub file_name: String,
    pub file_size_bytes: u64,
    pub format: StlFormatJson,
    pub triangle_count: u32,
    pub geometry: Option<GeometryStatsJson>,
    pub parse_ms: u32,
    pub plates: Vec<PlateSummary>,
    pub materials: Vec<MaterialSummary>,
    pub embedded_filaments: Vec<EmbeddedFilamentSummary>,
    pub part_warning: Option<String>,
    pub plate_metadata: bool,
    pub plate_warning: Option<String>,
    pub source_identity: String,
    pub source_digest: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct IntegrityJson {
    pub boundary_edges: u32,
    pub non_manifold_edges: u32,
    pub inconsistent_orientation: u32,
    pub degenerate_triangles: u32,
    pub distinct_vertices: u32,
    pub zero_normals: u32,
    pub normal_disagreements: u32,
    pub watertight: bool,
}

impl From<stl_core::Integrity> for IntegrityJson {
    fn from(i: stl_core::Integrity) -> Self {
        IntegrityJson {
            boundary_edges: i.boundary_edges,
            non_manifold_edges: i.non_manifold_edges,
            inconsistent_orientation: i.inconsistent_orientation,
            degenerate_triangles: i.degenerate_triangles,
            distinct_vertices: i.distinct_vertices,
            zero_normals: i.zero_normals,
            normal_disagreements: i.normal_disagreements,
            watertight: i.watertight(),
        }
    }
}

/// Payload of the `integrity` event (spec §2/§5). Carries `generation` so a
/// late result from a superseded load can be told apart from the current one.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status")]
pub enum IntegrityReportJson {
    Computed {
        generation: u64,
        plate_id: u32,
        integrity: IntegrityJson,
    },
    SkippedTooLarge {
        generation: u64,
        plate_id: u32,
        triangle_count: u32,
        limit: u32,
    },
}
