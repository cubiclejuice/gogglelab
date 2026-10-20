//! Public compatibility controls for the later streamed plate/map migration.
//! These exercise existing diagnostics and membership, without private quotas.
use std::io::{Cursor, Write};

use stl_core::parse_model_with_plates;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

const METHODS: [CompressionMethod; 2] = [CompressionMethod::Stored, CompressionMethod::Deflated];
const TRIANGLE: &str = "<object id='1' type='model' name='Triangle'><mesh><vertices><vertex x='0' y='0' z='0'/><vertex x='1' y='0' z='0'/><vertex x='0' y='1' z='0'/></vertices><triangles><triangle v1='0' v2='1' v3='2'/></triangles></mesh></object>";

fn model(build: &str) -> String {
    format!("<model unit='millimeter' xmlns='http://schemas.microsoft.com/3dmanufacturing/core/2015/02'><resources>{TRIANGLE}</resources><build>{build}</build></model>")
}

fn package(model: &str, settings: &str, method: CompressionMethod) -> Vec<u8> {
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(method);
    for (path, text) in [
        ("3D/3dmodel.model", model),
        ("Metadata/model_settings.config", settings),
    ] {
        archive.start_file(path, options).unwrap();
        archive.write_all(text.as_bytes()).unwrap();
    }
    archive.finish().unwrap().into_inner()
}

fn instance(object: u32, occurrence: u32) -> String {
    format!("<model_instance><metadata key='object_id' value='{object}'/><metadata key='instance_id' value='{occurrence}'/></model_instance>")
}

fn plate(id: u32, name: &str, assignments: &str) -> String {
    format!("<plate><metadata key='plater_id' value='{id}'/><metadata key='plater_name' value='{name}'/>{assignments}</plate>")
}

fn assert_fallback(settings: &str, detail: &str) {
    let root = model("<item objectid='1'/>");
    let expected = format!("Plate metadata is unusable ({detail}); showing the whole build.");
    for method in METHODS {
        let parsed = parse_model_with_plates(&package(&root, settings, method)).unwrap();
        assert_eq!(parsed.model.triangle_count, 1);
        assert_eq!(parsed.model.positions, [0., 0., 0., 1., 0., 0., 0., 1., 0.]);
        assert!(!parsed.plate_metadata);
        assert_eq!(parsed.plate_warning.as_deref(), Some(expected.as_str()));
        assert_eq!(parsed.plates.len(), 1);
        assert_eq!(parsed.plates[0].name, "Whole build");
        assert_eq!(parsed.parts.len(), 1);
        assert!(parsed.part_warning.is_none());
    }
}

#[test]
fn public_derived_maps_streaming_structure_precedes_deferred_duplicate_plate_ids() {
    let settings = format!(
        "<config>{}{}<plate><plate></plate></plate></config>",
        plate(1, "First", &instance(1, 0)),
        plate(1, "Duplicate", ""),
    );
    assert_fallback(&settings, "nested plate metadata");
}

#[test]
fn public_derived_maps_deferred_duplicate_plate_ids_precede_unknown_membership() {
    let settings = format!(
        "<config>{}{}</config>",
        plate(7, "Unknown assignment", &instance(99, 0)),
        plate(7, "Duplicate id", ""),
    );
    assert_fallback(&settings, "duplicated plate plater_id");
}

#[test]
fn public_derived_maps_plate_limit_precedes_missing_id_on_257th_close() {
    let mut settings = String::from("<config>");
    for id in 1..=256 {
        settings.push_str(&plate(id, "Empty", ""));
    }
    settings.push_str("<plate><metadata key='plater_name' value='Missing ID'/></plate></config>");
    assert_fallback(&settings, "more than 256 plates");
}

#[test]
fn public_derived_maps_late_missing_instance_fields_precede_earlier_membership_error() {
    for (incomplete, detail) in [
        (
            "<model_instance><metadata key='object_id' value='1'/></model_instance>",
            "model instance has no instance_id",
        ),
        (
            "<model_instance><metadata key='instance_id' value='0'/></model_instance>",
            "model instance has no object_id",
        ),
    ] {
        let settings = format!(
            "<config>{}{}</config>",
            plate(1, "Unknown assignment", &instance(99, 0)),
            plate(2, "Incomplete", incomplete),
        );
        assert_fallback(&settings, detail);
    }
}

#[test]
fn public_derived_maps_disabled_occurrences_preserve_empty_and_authored_plate_order() {
    let root = model("<item objectid='1' printable='0'/><item objectid='1' transform='1 0 0 0 1 0 0 0 1 10 0 0'/><item objectid='1' transform='1 0 0 0 1 0 0 0 1 20 0 0'/>");
    for claim_disabled in [false, true] {
        let empty_assignment = if claim_disabled {
            instance(1, 0)
        } else {
            String::new()
        };
        let settings = format!(
            "<config>{}{}{}</config>",
            plate(30, "Empty", &empty_assignment),
            plate(10, "Second occurrence", &instance(1, 2)),
            plate(20, "First occurrence", &instance(1, 1)),
        );
        for method in METHODS {
            let parsed = parse_model_with_plates(&package(&root, &settings, method)).unwrap();
            assert!(parsed.plate_metadata);
            assert!(parsed.plate_warning.is_none());
            assert!(parsed.part_warning.is_none());
            assert_eq!(parsed.model.triangle_count, 2);
            assert_eq!(
                parsed
                    .plates
                    .iter()
                    .map(|plate| (
                        plate.name.as_str(),
                        plate.triangle_start,
                        plate.triangle_count
                    ))
                    .collect::<Vec<_>>(),
                vec![
                    ("Empty", 0, 0),
                    ("Second occurrence", 0, 1),
                    ("First occurrence", 1, 1)
                ],
            );
            assert_eq!(parsed.model.positions[0], 20.0);
            assert_eq!(parsed.model.positions[9], 10.0);
            assert_eq!(parsed.parts.len(), 2);
            assert_eq!(parsed.parts[0].id, "object:1:instance:2");
            assert_eq!(parsed.parts[1].id, "object:1:instance:1");
            assert_eq!(parsed.parts[0].triangle_start, 0);
            assert_eq!(parsed.parts[1].triangle_start, 1);
        }
    }
}
