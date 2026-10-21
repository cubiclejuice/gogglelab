//! Public controls for settings selection and permissive scanner behavior.
use std::io::{Cursor, Write};

use stl_core::{parse_model_with_plates, ParsedModel};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

const METHODS: [CompressionMethod; 2] = [CompressionMethod::Stored, CompressionMethod::Deflated];
const ROOT: &str = "3D/3dmodel.model";
const SETTINGS: &str = "Metadata/model_settings.config";
const SOURCE: &str = "<model unit='millimeter' xmlns='http://schemas.microsoft.com/3dmanufacturing/core/2015/02'><resources><object id='2' type='model' name='Child source'><mesh><vertices><vertex x='0' y='0' z='0'/><vertex x='1' y='0' z='0'/><vertex x='0' y='1' z='0'/></vertices><triangles><triangle v1='0' v2='1' v3='2'/></triangles></mesh></object><object id='1' name='Root source'><components><component objectid='2'/></components></object></resources><build><item objectid='1'/></build></model>";

fn parse(settings: &str, method: CompressionMethod) -> ParsedModel {
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(method);
    for (name, text) in [(ROOT, SOURCE), (SETTINGS, settings)] {
        archive.start_file(name, options).unwrap();
        archive.write_all(text.as_bytes()).unwrap();
    }
    let bytes = archive.finish().unwrap().into_inner();
    parse_model_with_plates(&bytes).unwrap()
}

fn assert_boundary(settings: &str, name: &str, slot: Option<u32>) {
    for method in METHODS {
        let parsed = parse(settings, method);
        assert_eq!(parsed.model.triangle_count, 1);
        assert_eq!(
            parsed.model.positions,
            vec![0., 0., 0., 1., 0., 0., 0., 1., 0.]
        );
        assert_eq!(parsed.parts.len(), 1);
        let part = &parsed.parts[0];
        assert_eq!(part.id, "object:1:instance:0/component:0:object:2");
        assert_eq!(part.name, name);
        assert_eq!(part.filament_slot, slot);
        assert_eq!((part.triangle_start, part.triangle_count), (0, 1));
        assert!(parsed.part_warning.is_none());
        assert!(!parsed.plate_metadata);
        assert!(parsed.plate_warning.is_some());
    }
}

#[test]
fn settings_relevance_part_under_irrelevant_object_remains_consumed() {
    let settings = "<config><object id='1'><metadata key='name' value='Root label'/><metadata key='extruder' value='2'/></object><object id='999'><part id='2'><metadata key='name' value='  Useful part  '/><metadata key='extruder' value=' 3 '/></part></object></config>";
    assert_boundary(settings, "Root label · Useful part", Some(3));
}

#[test]
fn settings_relevance_irrelevant_open_part_still_suppresses_nested_relevant_part() {
    let settings = "<config><object id='1'><metadata key='name' value='Root label'/><metadata key='extruder' value='2'/><part id='999'><metadata key='name' value='Ignored outer'/><part id='2'><metadata key='name' value='Must stay ignored'/><metadata key='extruder' value='3'/></part></part></object></config>";
    assert_boundary(settings, "Child source", Some(2));
}

#[test]
fn settings_relevance_duplicate_ids_merge_complementary_fields() {
    let settings = "<config><object id='1'><metadata key='name' value='Root label'/></object><object id='1'><metadata key='extruder' value='2'/></object><object id='999'><part id='2'><metadata key='name' value='Part label'/></part></object><object id='999'><part id='2'><metadata key='extruder' value='3'/></part></object></config>";
    assert_boundary(settings, "Root label · Part label", Some(3));
}

#[test]
fn settings_relevance_equal_duplicate_ids_permanently_invalidate_only_repeated_fields() {
    let settings = "<config><object id='1'><metadata key='name' value='Root label'/><metadata key='extruder' value='2'/></object><object id='1'><metadata key='name' value='Root label'/></object><object id='1'><metadata key='name' value='Cannot recover'/></object><object id='999'><part id='2'><metadata key='name' value='Part label'/><metadata key='extruder' value='3'/></part></object><object id='999'><part id='2'><metadata key='name' value='Part label'/><metadata key='extruder' value='3'/></part></object><object id='999'><part id='2'><metadata key='name' value='Cannot recover'/><metadata key='extruder' value='4'/></part></object></config>";
    assert_boundary(settings, "Child source", Some(2));
}

#[test]
fn settings_relevance_equal_duplicate_fields_and_invalid_values_do_not_recover() {
    for settings in [
        "<config><object id='1'><metadata key='name' value='Root label'/><metadata key='name' value='Root label'/><metadata key='extruder' value='2'/><metadata key='extruder' value='2'/></object></config>",
        "<config><object id='1'><metadata key='name' value='   '/><metadata key='name' value='Cannot recover'/><metadata key='extruder' value='0'/><metadata key='extruder' value='2'/></object></config>",
    ] {
        assert_boundary(settings, "Child source", None);
    }
}

#[test]
fn settings_relevance_invalid_outer_object_id_does_not_hide_inner_valid_object() {
    let settings = "<config><object id='bad'><object id='1'><metadata key='name' value='Root label'/><metadata key='extruder' value='2'/><part id='2'><metadata key='name' value='Inner part'/><metadata key='extruder' value='3'/></part></object></object></config>";
    assert_boundary(settings, "Root label · Inner part", Some(3));
}

#[test]
fn settings_relevance_empty_object_and_part_tags_do_not_change_open_state() {
    let settings = "<config><object id='1'><metadata key='name' value='Root label'/><object id='999'/><metadata key='extruder' value='2'/><part id='999'/><part id='2'><metadata key='name' value='Real part'/><metadata key='extruder' value='3'/></part></object></config>";
    assert_boundary(settings, "Root label · Real part", Some(3));
}

#[test]
fn settings_relevance_nested_object_end_closes_the_old_open_object() {
    let settings = "<config><object id='1'><metadata key='name' value='Root label'/><object id='999'><metadata key='extruder' value='2'/></object><part id='2'><metadata key='name' value='Must stay ignored'/><metadata key='extruder' value='3'/></part></object></config>";
    assert_boundary(settings, "Child source", Some(2));
}

#[test]
fn settings_relevance_invalid_open_part_id_still_suppresses_nested_part() {
    let settings = "<config><object id='1'><metadata key='extruder' value='2'/><part id='bad'><part id='2'><metadata key='name' value='Must stay ignored'/><metadata key='extruder' value='3'/></part></part></object></config>";
    assert_boundary(settings, "Child source", Some(2));
}

#[test]
fn settings_relevance_malformed_irrelevant_tail_discards_earlier_optional_settings() {
    let settings = "<config><object id='1'><metadata key='name' value='Useful root'/><metadata key='extruder' value='2'/><part id='2'><metadata key='name' value='Useful part'/><metadata key='extruder' value='3'/></part></object><object id='999'><part id='999'></object></config>";
    assert_boundary(settings, "Child source", None);
}

#[test]
fn settings_relevance_completed_records_survive_later_unclosed_irrelevant_record() {
    let settings = "<config><object id='1'><metadata key='name' value='Root label'/><metadata key='extruder' value='2'/></object><object id='999'><part id='2'><metadata key='name' value='Useful part'/><metadata key='extruder' value='3'/></part></object><object id='999'><part id='999'><metadata key='name' value='Unfinished irrelevant'/><metadata key='extruder' value='9'/>";
    assert_boundary(settings, "Root label · Useful part", Some(3));
}

#[test]
fn settings_relevance_unfinished_relevant_part_does_not_flush_at_eof() {
    let settings = "<config><object id='1'><metadata key='name' value='Root label'/><metadata key='extruder' value='2'/></object><object id='999'><part id='2'><metadata key='name' value='Must not flush'/><metadata key='extruder' value='3'/>";
    assert_boundary(settings, "Child source", Some(2));
}

#[test]
fn settings_attributes_unicode_and_first_local_values_are_preserved() {
    let label = format!("\u{a0}{}\u{2003}", "&#x1F9EA;".repeat(257));
    let settings = format!("<config xmlns:a='urn:fixture:a' xmlns:b='urn:fixture:b'><object id='1'><metadata key='extruder' value='2'/><part id='2'><metadata a:key='name' b:key='extruder' a:value='{label}' b:value='999'/><metadata key='extruder' value='3'/></part></object></config>");
    assert_boundary(&settings, &"🧪".repeat(256), Some(3));
}

#[test]
fn settings_attributes_metadata_keys_remain_case_sensitive() {
    let settings = "<config><object id='1'><metadata key='extruder' value='2'/><part id='2'><metadata key='Name' value='Must remain ignored'/><metadata key='Extruder' value='3'/></part></object></config>";
    assert_boundary(settings, "Child source", Some(2));
}

#[test]
fn settings_attributes_unknown_key_in_irrelevant_record_still_reads_value_errors() {
    let settings = "<config><object id='1'><metadata key='extruder' value='2'/></object><object id='999'><metadata key='unknown' broken=bad value='Ignored'/></object></config>";
    assert_boundary(settings, "Child source", None);
}

#[test]
fn settings_attributes_error_after_selected_values_is_not_retroactive() {
    let settings = "<config><object id='1'><metadata key='extruder' value='2'/></object><object id='999'><metadata key='unknown' value='Ignored' broken=bad/></object></config>";
    assert_boundary(settings, "Child source", Some(2));
}

#[test]
fn settings_attributes_ignored_nested_part_does_not_decode_attributes() {
    let settings = "<config><object id='1'><metadata key='extruder' value='2'/><part id='999'><part id='2'><metadata broken=bad key='name' value='Must stay ignored'/></part></part></object></config>";
    assert_boundary(settings, "Child source", Some(2));
}
