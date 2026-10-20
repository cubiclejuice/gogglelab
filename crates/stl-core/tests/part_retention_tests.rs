//! Public production-default compatibility for optional part-boundary retention.
//! One cached triangle/1024-1025 instances; no private limits or giant mesh.
use std::io::{Cursor, Write};

use stl_core::{parse_model_with_plates, StlError};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

const METHODS: [CompressionMethod; 2] = [CompressionMethod::Stored, CompressionMethod::Deflated];
const PART_LIMIT: usize = 1024;
const WARNING: &str =
    "This 3MF has too many or unusable part boundaries; model-wide color is available.";
const TRIANGLE: &str = "<mesh><vertices><vertex x='0' y='0' z='0'/><vertex x='1' y='0' z='0'/><vertex x='0' y='1' z='0'/></vertices><triangles><triangle v1='0' v2='1' v3='2'/></triangles></mesh>";

fn root(extra_objects: &str, build: &str) -> String {
    format!("<model unit='millimeter' xmlns='http://schemas.microsoft.com/3dmanufacturing/core/2015/02'><resources><object id='1' type='model' name='One triangle'>{TRIANGLE}</object>{extra_objects}</resources><build>{build}</build></model>")
}

fn repeated_build(count: usize) -> String {
    "<item objectid='1'/>".repeat(count)
}

fn package(model: &str, settings: Option<&str>, method: CompressionMethod) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(method);
    zip.start_file("3D/3dmodel.model", options).unwrap();
    zip.write_all(model.as_bytes()).unwrap();
    if let Some(settings) = settings {
        zip.start_file("Metadata/model_settings.config", options)
            .unwrap();
        zip.write_all(settings.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn plate(id: u32, name: &str, instances: std::ops::Range<usize>) -> String {
    let assignments = instances.map(|instance| format!("<model_instance><metadata key='object_id' value='1'/><metadata key='instance_id' value='{instance}'/></model_instance>")).collect::<String>();
    format!("<plate><metadata key='plater_id' value='{id}'/><metadata key='plater_name' value='{name}'/>{assignments}</plate>")
}

#[test]
fn public_part_retention_exact_limit_survives_later_empty_disabled_and_unprinted_items() {
    let extra = format!("<object id='2' type='model'><mesh><vertices/><triangles/></mesh></object><object id='3' type='other'>{TRIANGLE}</object>");
    let build = format!(
        "{}<item objectid='2'/><item objectid='1' printable='0'/><item objectid='3'/>",
        repeated_build(PART_LIMIT)
    );
    let model = root(&extra, &build);
    for method in METHODS {
        let parsed = parse_model_with_plates(&package(&model, None, method)).unwrap();
        assert_eq!(parsed.model.triangle_count, PART_LIMIT as u32);
        assert_eq!(parsed.model.positions.len(), PART_LIMIT * 9);
        assert_eq!(parsed.parts.len(), PART_LIMIT);
        assert!(parsed.part_warning.is_none());
        assert_eq!(parsed.parts[0].triangle_start, 0);
        assert_eq!(
            parsed.parts[PART_LIMIT - 1].triangle_start,
            PART_LIMIT as u32 - 1
        );
        assert_eq!(parsed.parts[PART_LIMIT - 1].id, "object:1:instance:1023");
        assert!(parsed
            .parts
            .iter()
            .all(|part| part.triangle_count == 1 && part.name == "One triangle"));
    }
}

#[test]
fn public_part_retention_overflow_is_global_across_authored_plate_order_and_ranges() {
    let mut build = String::new();
    for instance in 0..=PART_LIMIT {
        match instance {
            0 => build.push_str("<item objectid='1' transform='1 0 0 0 1 0 0 0 1 10 0 0'/>"),
            512 => build.push_str("<item objectid='1' transform='1 0 0 0 1 0 0 0 1 20 0 0'/>"),
            _ => build.push_str("<item objectid='1'/>"),
        }
    }
    let model = root("", &build);
    let settings = format!(
        "<config>{}{}</config>",
        plate(20, "Later instances", 512..PART_LIMIT + 1),
        plate(10, "Earlier instances", 0..512)
    );
    for method in METHODS {
        let parsed = parse_model_with_plates(&package(&model, Some(&settings), method)).unwrap();
        assert_eq!(parsed.model.triangle_count, PART_LIMIT as u32 + 1);
        assert!(parsed.parts.is_empty());
        assert_eq!(parsed.part_warning.as_deref(), Some(WARNING));
        assert!(parsed.plate_metadata);
        assert!(parsed.plate_warning.is_none());
        assert_eq!(parsed.plates.len(), 2);
        assert_eq!(
            (
                parsed.plates[0].id,
                parsed.plates[0].name.as_str(),
                parsed.plates[0].triangle_start,
                parsed.plates[0].triangle_count
            ),
            (0, "Later instances", 0, 513)
        );
        assert_eq!(
            (
                parsed.plates[1].id,
                parsed.plates[1].name.as_str(),
                parsed.plates[1].triangle_start,
                parsed.plates[1].triangle_count
            ),
            (1, "Earlier instances", 513, 512)
        );
        // The geometry follows authored display order, not original build order.
        assert_eq!(parsed.model.positions[0], 20.0);
        assert_eq!(parsed.model.positions[513 * 9], 10.0);
    }
}

#[test]
fn public_part_retention_overflow_does_not_hide_later_missing_object() {
    let model = root(
        "",
        &format!("{}<item objectid='99'/>", repeated_build(PART_LIMIT + 1)),
    );
    for method in METHODS {
        assert_eq!(
            parse_model_with_plates(&package(&model, None, method)),
            Err(StlError::Malformed3mf {
                detail: "object 99 not found in 3D/3dmodel.model".into()
            })
        );
    }
}

#[test]
fn public_part_retention_overflow_does_not_hide_later_emitted_nonfinite_coordinate() {
    let nonfinite = TRIANGLE.replacen("x='0'", "x='NaN'", 1);
    let extra = format!("<object id='2' type='model'>{nonfinite}</object>");
    let model = root(
        &extra,
        &format!("{}<item objectid='2'/>", repeated_build(PART_LIMIT + 1)),
    );
    for method in METHODS {
        assert_eq!(
            parse_model_with_plates(&package(&model, None, method)),
            Err(StlError::NonFiniteCoordinate {
                triangle_index: PART_LIMIT as u32 + 1
            })
        );
    }
}
