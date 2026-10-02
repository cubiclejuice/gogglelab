use crate::{ParsedStl, StlError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plate {
    pub id: u32,
    pub name: String,
    pub triangle_start: u32,
    pub triangle_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPart {
    pub id: String,
    pub name: String,
    pub triangle_start: u32,
    pub triangle_count: u32,
    pub material_id: Option<String>,
    pub filament_slot: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaterialSource {
    ThreeMfBaseMaterial,
    ThreeMfColorGroup,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMaterial {
    pub id: String,
    pub name: Option<String>,
    pub color: Option<String>,
    pub source: MaterialSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFilament {
    pub slot: u32,
    pub name: Option<String>,
    pub material: Option<String>,
    pub color: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedModel {
    pub model: ParsedStl,
    pub plates: Vec<Plate>,
    pub parts: Vec<ParsedPart>,
    pub materials: Vec<ParsedMaterial>,
    pub embedded_filaments: Vec<ParsedFilament>,
    pub part_warning: Option<String>,
    pub plate_metadata: bool,
    pub plate_warning: Option<String>,
}

impl ParsedModel {
    pub fn plate_mesh(&self, id: u32) -> Option<ParsedStl> {
        let plate = self.plates.iter().find(|plate| plate.id == id)?;
        let triangle_start = usize::try_from(plate.triangle_start).ok()?;
        let triangle_count = usize::try_from(plate.triangle_count).ok()?;
        let position_start = triangle_start.checked_mul(9)?;
        let position_end = position_start.checked_add(triangle_count.checked_mul(9)?)?;
        let normal_start = triangle_start.checked_mul(3)?;
        let normal_end = normal_start.checked_add(triangle_count.checked_mul(3)?)?;

        Some(ParsedStl {
            format: self.model.format,
            triangle_count: plate.triangle_count,
            positions: self
                .model
                .positions
                .get(position_start..position_end)?
                .to_vec(),
            stored_normals: self
                .model
                .stored_normals
                .get(normal_start..normal_end)?
                .to_vec(),
        })
    }

    pub fn plate_parts(&self, id: u32) -> Option<Vec<ParsedPart>> {
        let plate = self.plates.iter().find(|plate| plate.id == id)?;
        let plate_end = plate.triangle_start.checked_add(plate.triangle_count)?;
        let mut parts = Vec::new();

        for part in &self.parts {
            let part_end = part.triangle_start.checked_add(part.triangle_count)?;
            if part.triangle_start >= plate.triangle_start && part_end <= plate_end {
                let mut part = part.clone();
                part.triangle_start -= plate.triangle_start;
                parts.push(part);
            } else if part.triangle_start < plate_end && part_end > plate.triangle_start {
                return None;
            }
        }

        Some(parts)
    }
}

pub fn parse_model_with_plates(bytes: &[u8]) -> Result<ParsedModel, StlError> {
    if crate::threemf::is_zip(bytes) {
        crate::threemf::parse_3mf_with_plates(bytes)
    } else {
        let model = crate::parse::parse(bytes)?;
        let triangle_count = model.triangle_count;
        Ok(ParsedModel {
            model,
            plates: vec![Plate {
                id: 0,
                name: "Model".into(),
                triangle_start: 0,
                triangle_count,
            }],
            parts: Vec::new(),
            materials: Vec::new(),
            embedded_filaments: Vec::new(),
            part_warning: None,
            plate_metadata: false,
            plate_warning: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ParsedModel, ParsedPart, Plate};
    use crate::{ParsedStl, StlFormat};

    fn parsed_model(parts: Vec<ParsedPart>) -> ParsedModel {
        ParsedModel {
            model: ParsedStl {
                format: StlFormat::ThreeMf,
                triangle_count: 4,
                positions: vec![0.0; 36],
                stored_normals: vec![0.0; 12],
            },
            plates: vec![
                Plate {
                    id: 1,
                    name: "First".into(),
                    triangle_start: 0,
                    triangle_count: 1,
                },
                Plate {
                    id: 2,
                    name: "Second".into(),
                    triangle_start: 1,
                    triangle_count: 2,
                },
                Plate {
                    id: 3,
                    name: "Third".into(),
                    triangle_start: 3,
                    triangle_count: 1,
                },
            ],
            parts,
            materials: Vec::new(),
            embedded_filaments: Vec::new(),
            part_warning: None,
            plate_metadata: true,
            plate_warning: None,
        }
    }

    fn part(id: &str, triangle_start: u32, triangle_count: u32) -> ParsedPart {
        ParsedPart {
            id: id.into(),
            name: id.into(),
            triangle_start,
            triangle_count,
            material_id: None,
            filament_slot: None,
        }
    }

    #[test]
    fn plate_parts_rebases_contained_ranges() {
        let model = parsed_model(vec![
            part("first", 0, 1),
            part("second-a", 1, 1),
            part("second-b", 2, 1),
            part("third", 3, 1),
        ]);

        assert_eq!(
            model.plate_parts(2),
            Some(vec![part("second-a", 0, 1), part("second-b", 1, 1)])
        );
        assert_eq!(model.plate_parts(99), None);
    }

    #[test]
    fn plate_parts_rejects_a_range_crossing_the_plate_boundary() {
        let model = parsed_model(vec![part("crossing", 0, 2)]);

        assert_eq!(model.plate_parts(2), None);
    }

    #[test]
    fn stl_models_have_no_authored_colorway_metadata() {
        let ascii = b"solid t
facet normal 0 0 1
outer loop
vertex 0 0 0
vertex 1 0 0
vertex 0 1 0
endloop
endfacet
endsolid t
";
        let model = crate::parse_model_with_plates(ascii).unwrap();
        assert!(model.parts.is_empty());
        assert!(model.materials.is_empty());
        assert!(model.embedded_filaments.is_empty());
        assert_eq!(model.part_warning, None);
        let mut binary = vec![0u8; 134];
        binary[80..84].copy_from_slice(&1u32.to_le_bytes());
        let model = crate::parse_model_with_plates(&binary).unwrap();
        assert!(model.parts.is_empty());
        assert!(model.materials.is_empty());
        assert!(model.embedded_filaments.is_empty());
        assert_eq!(model.part_warning, None);
    }
}
