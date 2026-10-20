//! Actual package-entry compatibility controls for optional project palettes.
use std::io::{Cursor, Write};

use stl_core::package_bounds::PackageProcessingContext;
use stl_core::zip_bounds::ZipResourceObserver;
use stl_core::{parse_model_with_plates, parse_model_with_plates_in_context, ParsedFilament};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const ROOT: &str = r#"<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources><object id="1" type="model"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/></vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object></resources><build><item objectid="1"/></build></model>"#;

fn package(project: &[u8], method: CompressionMethod) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(method);
    zip.start_file("3D/3dmodel.model", options).unwrap();
    zip.write_all(ROOT.as_bytes()).unwrap();
    zip.start_file("Metadata/project_settings.config", options)
        .unwrap();
    zip.write_all(project).unwrap();
    zip.finish().unwrap().into_inner()
}

fn palette(project: &[u8], method: CompressionMethod) -> Vec<ParsedFilament> {
    let model = parse_model_with_plates(&package(project, method)).unwrap();
    assert_eq!(model.model.triangle_count, 1);
    assert_eq!(model.model.positions, [0., 0., 0., 1., 0., 0., 0., 1., 0.]);
    assert_eq!(model.plates.len(), 1);
    assert_eq!(model.plates[0].triangle_count, 1);
    model.embedded_filaments
}

#[test]
fn real_project_palette_preserves_slots_decoded_duplicates_and_labels() {
    let json = r##"{
      "filament_colour":["#000000"],
      "filament_\u0063olour":[null," #a1b2c3ff ",false,"#123456"],
      "filament_settings_id":["unused"," Basic ",null,"Acme\u2003Special"],
      "filament_type":[null," PLA ",{},"PETG"],
      "filament_vendor":["unused"," Acme ",false,"Acme"],
      "unknown":{"filament_colour":["#FFFFFF"],"nested":[true,null,1e-400]}
    }"##;
    let expected = vec![
        ParsedFilament {
            slot: 2,
            name: Some("Acme Basic".into()),
            material: Some("PLA".into()),
            color: "#A1B2C3".into(),
        },
        ParsedFilament {
            slot: 4,
            name: Some("Acme\u{2003}Special".into()),
            material: Some("PETG".into()),
            color: "#123456".into(),
        },
    ];
    for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
        assert_eq!(palette(json.as_bytes(), method), expected);
        for bad in [
            r##"{"filament_colour":["#123456"],"filament_\u0063olour":false}"##,
            r##"{"filament_colour":"#123456"}"##,
            r##"{"filament_color":["#123456"]}"##,
        ] {
            assert!(palette(bad.as_bytes(), method).is_empty());
        }
    }
}

#[test]
fn real_project_invalid_ignored_data_falls_back_without_losing_geometry() {
    let mut invalid = vec![
        br##"{"filament_colour":["#123456"],"ignored":1e400}"##.to_vec(),
        br##"{"filament_colour":["#123456"],"ignored":-1e400}"##.to_vec(),
        br##"{"filament_colour":["#123456"],"ignored":"\uD800"}"##.to_vec(),
        br##"{"filament_colour":["#123456"]} true"##.to_vec(),
        br##"{"filament_colour":["#123456"],"ignored":[0,]}"##.to_vec(),
    ];
    let mut invalid_utf8 = br##"{"filament_colour":["#123456"],"ignored":""##.to_vec();
    invalid_utf8.extend_from_slice(&[0xff, b'"', b'}']);
    invalid.push(invalid_utf8);
    let colors = std::iter::repeat_n("null", 256)
        .collect::<Vec<_>>()
        .join(",");
    invalid.push(format!(r#"{{"filament_colour":[{colors},1e400]}}"#).into_bytes());
    invalid.push(format!(r#"{{"filament_colour":[{colors},"\uD800"]}}"#).into_bytes());
    for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
        for project in &invalid {
            assert!(palette(project, method).is_empty());
        }
    }
}

#[test]
fn real_project_position_256_is_selected_and_257_is_only_validated() {
    let mut entries = vec!["null"; 257];
    entries[255] = "\"#abcdef\"";
    entries[256] = "\"#123456\"";
    let labels = std::iter::repeat_n("\" PLA \"", 257)
        .collect::<Vec<_>>()
        .join(",");
    let json = format!(
        r#"{{"filament_colour":[{}],"filament_type":[{labels}]}}"#,
        entries.join(",")
    );
    for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
        assert_eq!(
            palette(json.as_bytes(), method),
            [ParsedFilament {
                slot: 256,
                name: None,
                material: Some("PLA".into()),
                color: "#ABCDEF".into(),
            }]
        );
    }
}

#[test]
fn real_project_recursion_boundary_preserves_optional_fallback() {
    for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
        for (depth, accepted) in [(126, true), (127, false)] {
            let json = format!(
                r##"{{"filament_colour":["#123456"],"ignored":{}0{}}}"##,
                "[".repeat(depth),
                "]".repeat(depth)
            );
            assert_eq!(
                palette(json.as_bytes(), method).len(),
                usize::from(accepted)
            );
        }
    }
}

#[test]
fn real_project_retained_parse_does_not_recharge_input_or_inflation() {
    let json = br##"{"filament_colour":["#123456"],"filament_type":["PLA"]}"##;
    for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
        let bytes = package(json, method);
        let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
        for passes in 1..=2 {
            let parsed = parse_model_with_plates_in_context(&bytes, &mut context).unwrap();
            assert_eq!(parsed.embedded_filaments.len(), 1);
            let usage = context.usage();
            assert_eq!(usage.retained_input_bytes, json.len() * passes);
            assert_eq!(
                usage.inflation_bytes,
                ((ROOT.len() + json.len()) * passes) as u64
            );
        }
    }
}
