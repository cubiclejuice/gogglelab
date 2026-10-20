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

/// A part borrowed from a validated plate selection, with a plate-local start.
#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub struct PlatePartRef<'a> {
    source: &'a ParsedPart,
    triangle_start: u32,
}
impl<'a> PlatePartRef<'a> {
    pub fn source(&self) -> &'a ParsedPart {
        self.source
    }
    pub fn triangle_start(&self) -> u32 {
        self.triangle_start
    }
}

/// Iterates only after every part range has been checked for this selection.
#[doc(hidden)]
pub struct PlateParts<'a> {
    parts: std::slice::Iter<'a, ParsedPart>,
    start: u32,
    end: u32,
    remaining: usize,
}
impl<'a> Iterator for PlateParts<'a> {
    type Item = PlatePartRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        for part in self.parts.by_ref() {
            // The immutable ranges were checked before this iterator was created.
            let end = part.triangle_start + part.triangle_count;
            if part.triangle_start >= self.start && end <= self.end {
                self.remaining -= 1;
                return Some(PlatePartRef {
                    source: part,
                    triangle_start: part.triangle_start - self.start,
                });
            }
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}
impl ExactSizeIterator for PlateParts<'_> {}
impl std::iter::FusedIterator for PlateParts<'_> {}

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

    #[doc(hidden)]
    pub fn plate_part_refs(&self, id: u32) -> Option<PlateParts<'_>> {
        let plate = self.plates.iter().find(|plate| plate.id == id)?;
        let end = plate.triangle_start.checked_add(plate.triangle_count)?;
        let mut remaining = 0;
        for part in &self.parts {
            let part_end = part.triangle_start.checked_add(part.triangle_count)?;
            if part.triangle_start >= plate.triangle_start && part_end <= end {
                remaining += 1;
            } else if part.triangle_start < end && part_end > plate.triangle_start {
                return None;
            }
        }
        Some(PlateParts {
            parts: self.parts.iter(),
            start: plate.triangle_start,
            end,
            remaining,
        })
    }

    pub fn plate_parts(&self, id: u32) -> Option<Vec<ParsedPart>> {
        Some(
            self.plate_part_refs(id)?
                .map(|view| {
                    let mut part = view.source().clone();
                    part.triangle_start = view.triangle_start();
                    part
                })
                .collect(),
        )
    }
}

pub fn parse_model_with_plates(bytes: &[u8]) -> Result<ParsedModel, StlError> {
    let mut context = crate::package_bounds::PackageProcessingContext::new(
        crate::zip_bounds::ZipResourceObserver::new(),
    );
    parse_model_with_plates_in_context(bytes, &mut context)
}

#[doc(hidden)]
pub fn parse_model_with_plates_in_context(
    bytes: &[u8],
    context: &mut crate::package_bounds::PackageProcessingContext,
) -> Result<ParsedModel, StlError> {
    let observer = context.observer();
    observer.check()?;
    let result = (|| {
        if crate::threemf::is_zip(bytes) {
            crate::threemf::parse_3mf_with_plates_in_context(bytes, context)
        } else {
            let model = crate::parse::parse_observed(bytes, &observer)?;
            observer.check()?;
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
    })();
    observer.finish(result)
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
    fn borrowed_plate_parts_preserve_backing_order_and_boundary_rows() {
        let model = parsed_model(vec![
            part("end-zero", 3, 0),
            part("second-b", 2, 1),
            part("first", 0, 1),
            part("start-zero", 1, 0),
            part("second-a", 1, 1),
            part("outside-model", 20, 1),
        ]);
        let mut views = model.plate_part_refs(2).unwrap();
        assert_eq!(views.len(), 4);
        for (index, start) in [(0, 2), (1, 1), (3, 0), (4, 0)] {
            let view = views.next().unwrap();
            assert!(std::ptr::eq(view.source(), &model.parts[index]));
            assert_eq!(view.source().id.as_ptr(), model.parts[index].id.as_ptr());
            assert_eq!(view.triangle_start(), start);
        }
        assert_eq!(views.len(), 0);
        assert!(views.next().is_none());
        assert!(views.next().is_none());
        assert_eq!(
            model.plate_parts(2).unwrap(),
            vec![
                part("end-zero", 2, 0),
                part("second-b", 1, 1),
                part("start-zero", 0, 0),
                part("second-a", 0, 1)
            ]
        );
        assert!(model.plate_part_refs(99).is_none());
    }

    #[test]
    fn borrowed_plate_parts_validate_late_and_outside_ranges_before_yielding() {
        for invalid in [
            part("late-crossing", 2, 2),
            part("outside-overflow", u32::MAX, 1),
        ] {
            let model = parsed_model(vec![part("contained", 1, 1), invalid]);
            assert!(model.plate_part_refs(2).is_none());
            assert!(model.plate_parts(2).is_none());
        }
        let model = parsed_model(vec![part("overlap-a", 1, 2), part("overlap-b", 2, 1)]);
        assert_eq!(model.plate_part_refs(2).unwrap().len(), 2);
        let mut overflow = parsed_model(Vec::new());
        overflow.plates[1].triangle_start = u32::MAX;
        assert!(overflow.plate_part_refs(2).is_none());
        let empty = parsed_model(Vec::new());
        assert_eq!(empty.plate_part_refs(2).unwrap().len(), 0);
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
    #[test]
    fn model_context_stl_dispatch_preserves_existing_errors_and_preexisting_terminal_causes() {
        use crate::package_bounds::PackageProcessingContext;
        use crate::zip_bounds::ZipResourceObserver;
        use crate::{ResourceStop, StlError};
        let bytes = b"ordinary invalid STL";
        let mut ordinary = PackageProcessingContext::new(ZipResourceObserver::new());
        assert_eq!(
            super::parse_model_with_plates_in_context(bytes, &mut ordinary).unwrap_err(),
            super::parse_model_with_plates(bytes).unwrap_err()
        );
        for stop in [
            Some(ResourceStop::InputLimitExceeded),
            Some(ResourceStop::ReadWorkExceeded),
            Some(ResourceStop::EnvelopeChanged),
            None,
        ] {
            let observer = ZipResourceObserver::new();
            let expected = match stop {
                Some(reason) => {
                    let _ = observer.stop_io(reason);
                    StlError::ResourceStopped(reason)
                }
                None => {
                    let _ = observer.allocation_failed_io();
                    StlError::AllocationFailed
                }
            };
            let mut context = PackageProcessingContext::new(observer);
            assert_eq!(
                super::parse_model_with_plates_in_context(bytes, &mut context).unwrap_err(),
                expected
            );
            assert_eq!(
                crate::parse_model_in_context(bytes, &mut context).unwrap_err(),
                expected
            );
        }
    }
}
