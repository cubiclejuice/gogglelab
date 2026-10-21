//! Production-default material/index compatibility for the acquisition migration.
//! Public parsers only: no private quota/fault hooks or new supported-input cap.
use std::io::{Cursor, Write};

use stl_core::{parse_model_with_plates, MaterialSource, ParsedModel};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

const METHODS: [CompressionMethod; 2] = [CompressionMethod::Stored, CompressionMethod::Deflated];
const ROOT: &str = "3D/3dmodel.model";
const CHILD: &str = "3D/child.model";
const TRIANGLE: &str = "<mesh><vertices><vertex x='0' y='0' z='0'/><vertex x='1' y='0' z='0'/><vertex x='0' y='1' z='0'/></vertices><triangles><triangle v1='0' v2='1' v3='2'/></triangles></mesh>";

fn object(id: u32, group: u32, index: u32) -> String {
    format!("<object id='{id}' type='model' pid='{group}' pindex='{index}'>{TRIANGLE}</object>")
}
fn model(resources: &str, build: &str) -> String {
    format!("<model unit='millimeter' xmlns='http://schemas.microsoft.com/3dmanufacturing/core/2015/02' xmlns:a='urn:fixture:a' xmlns:b='urn:fixture:b'><resources>{resources}</resources><build>{build}</build></model>")
}
fn package(parts: &[(&str, &str)], method: CompressionMethod) -> Vec<u8> {
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(method);
    for (name, text) in parts {
        archive.start_file(*name, options).unwrap();
        archive.write_all(text.as_bytes()).unwrap();
    }
    archive.finish().unwrap().into_inner()
}
fn parse(root: &str, method: CompressionMethod) -> ParsedModel {
    parse_model_with_plates(&package(&[(ROOT, root)], method)).unwrap()
}
fn assert_no_dangling_material_ids(parsed: &ParsedModel) {
    for part in &parsed.parts {
        if let Some(id) = &part.material_id {
            assert!(
                parsed.materials.iter().any(|material| &material.id == id),
                "dangling {id}"
            );
        }
    }
}

#[test]
fn public_material_retention_invalid_rows_advance_indices_and_repeated_groups_fill_only_missing_keys(
) {
    let mut resources = String::from("<basematerials id='5'><base name='  ' displaycolor='#000000'/><base name='Invalid color' displaycolor='invalid'/><base name='Original two' displaycolor='#11223344'/></basematerials><basematerials id='5'><base name='Filled zero' displaycolor='#445566'/><base name='Filled one' displaycolor='#778899'/><base name='Replacement two' displaycolor='#AABBCC'/></basematerials><colorgroup id='8'><color color='bad'/><color color='#a0B1c2dd'/></colorgroup><colorgroup id='8'><color color='#334455'/><color color='#ffffff'/></colorgroup>");
    let mut build = String::new();
    for (id, group, index) in [(1, 5, 0), (2, 5, 1), (3, 5, 2), (4, 8, 0), (5, 8, 1)] {
        resources.push_str(&object(id, group, index));
        build.push_str(&format!("<item objectid='{id}'/>"));
    }
    let root = model(&resources, &build);
    for method in METHODS {
        let parsed = parse(&root, method);
        assert_eq!(parsed.model.triangle_count, 5);
        assert_eq!(parsed.materials.len(), 5);
        let rows = parsed
            .materials
            .iter()
            .map(|material| {
                (
                    material.id.as_str(),
                    material.name.as_deref(),
                    material.color.as_deref(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            rows,
            vec![
                (
                    "3mf:3D/3dmodel.model:5:2",
                    Some("Original two"),
                    Some("#112233")
                ),
                (
                    "3mf:3D/3dmodel.model:5:0",
                    Some("Filled zero"),
                    Some("#445566")
                ),
                (
                    "3mf:3D/3dmodel.model:5:1",
                    Some("Filled one"),
                    Some("#778899")
                ),
                ("3mf:3D/3dmodel.model:8:1", None, Some("#A0B1C2")),
                ("3mf:3D/3dmodel.model:8:0", None, Some("#334455")),
            ]
        );
        assert!(parsed.materials[..3]
            .iter()
            .all(|m| m.source == MaterialSource::ThreeMfBaseMaterial));
        assert!(parsed.materials[3..]
            .iter()
            .all(|m| m.source == MaterialSource::ThreeMfColorGroup));
        assert_eq!(
            parsed
                .parts
                .iter()
                .map(|p| p.material_id.as_deref())
                .collect::<Vec<_>>(),
            vec![
                Some("3mf:3D/3dmodel.model:5:0"),
                Some("3mf:3D/3dmodel.model:5:1"),
                Some("3mf:3D/3dmodel.model:5:2"),
                Some("3mf:3D/3dmodel.model:8:0"),
                Some("3mf:3D/3dmodel.model:8:1"),
            ]
        );
        assert_no_dangling_material_ids(&parsed);
    }
}

#[test]
fn public_material_retention_unicode_clipping_and_first_namespaced_local_attributes_stay_exact() {
    let name = format!("\u{a0}{}\u{2003}", "&#x1F9EA;".repeat(257));
    let resources = format!("<basematerials a:id='5' b:id='99'><base a:name='{name}' b:name='Later name' a:displaycolor=' #1a2b3c44 ' b:displaycolor='#DDEEFF'/></basematerials>{}",object(1,5,0));
    let root = model(&resources, "<item objectid='1'/>");
    let expected_name = "🧪".repeat(256);
    for method in METHODS {
        let parsed = parse(&root, method);
        assert_eq!(parsed.model.triangle_count, 1);
        assert_eq!(parsed.materials.len(), 1);
        let material = &parsed.materials[0];
        assert_eq!(material.id, "3mf:3D/3dmodel.model:5:0");
        assert_eq!(material.name.as_deref(), Some(expected_name.as_str()));
        assert_eq!(material.name.as_ref().unwrap().len(), 1024);
        assert_eq!(material.color.as_deref(), Some("#1A2B3C"));
        assert_eq!(
            parsed.parts[0].material_id.as_deref(),
            Some(material.id.as_str())
        );
    }
}

#[test]
fn public_material_retention_local_1024_boundary_does_not_remap_clipped_appearance_indices() {
    let bases = (0..1025)
        .map(|index| format!("<base name='Material {index}' displaycolor='#123456'/>"))
        .collect::<String>();
    let resources = format!(
        "<basematerials id='5'>{bases}</basematerials>{}{}",
        object(1, 5, 1023),
        object(2, 5, 1024)
    );
    let root = model(&resources, "<item objectid='1'/><item objectid='2'/>");
    for method in METHODS {
        let parsed = parse(&root, method);
        assert_eq!(parsed.model.triangle_count, 2);
        assert_eq!(parsed.materials.len(), 1024);
        assert_eq!(parsed.materials[1023].id, "3mf:3D/3dmodel.model:5:1023");
        assert_eq!(
            parsed.materials[1023].name.as_deref(),
            Some("Material 1023")
        );
        assert_eq!(
            parsed.parts[0].material_id.as_deref(),
            Some("3mf:3D/3dmodel.model:5:1023")
        );
        assert_eq!(parsed.parts[1].material_id, None);
        assert_no_dangling_material_ids(&parsed);
    }
}

#[test]
fn public_material_retention_global_load_order_clips_child_display_without_dangling_ids() {
    let child=model(&format!("<basematerials id='5'><base name='Child zero' displaycolor='#112233'/><base name='Child one' displaycolor='#445566'/></basematerials>{}{}",object(7,5,0),object(8,5,1)),"");
    for root_count in [1023, 1024] {
        let bases = (0..root_count)
            .map(|index| format!("<base name='Root {index}' displaycolor='#778899'/>"))
            .collect::<String>();
        let resources=format!("<basematerials id='5'>{bases}</basematerials><object id='1'><components><component objectid='7' path='/3D/child.model' transform='1 0 0 0 1 0 0 0 1 10 0 0'/><component objectid='8' path='3D/child.model' transform='1 0 0 0 1 0 0 0 1 20 0 0'/></components></object>");
        let root = model(&resources, "<item objectid='1'/>");
        for method in METHODS {
            let parsed =
                parse_model_with_plates(&package(&[(ROOT, &root), (CHILD, &child)], method))
                    .unwrap();
            assert_eq!(parsed.model.triangle_count, 2);
            assert_eq!(parsed.model.positions[0], 10.0);
            assert_eq!(parsed.model.positions[9], 20.0);
            assert_eq!(parsed.materials.len(), 1024);
            assert_eq!(parsed.materials[0].id, "3mf:3D/3dmodel.model:5:0");
            if root_count == 1023 {
                assert_eq!(parsed.materials[1023].id, "3mf:3D/child.model:5:0");
                assert_eq!(parsed.materials[1023].name.as_deref(), Some("Child zero"));
                assert_eq!(
                    parsed.parts[0].material_id.as_deref(),
                    Some("3mf:3D/child.model:5:0")
                );
            } else {
                assert_eq!(parsed.materials[1023].id, "3mf:3D/3dmodel.model:5:1023");
                assert_eq!(parsed.parts[0].material_id, None);
            }
            assert_eq!(parsed.parts[1].material_id, None);
            assert_no_dangling_material_ids(&parsed);
        }
    }
}

#[test]
fn public_material_retention_attribute_errors_before_and_after_selected_values_preserve_output() {
    // This is an end-to-end ordinary compatibility control, not a private hook
    // claim. The pinned XML facade may inspect the malformed attributes first;
    // parent execution establishes this actual fixture's route/behavior.
    let resources=format!("<basematerials id='5'><base broken=bad name='Before error' displaycolor='#112233'/><base name='After error' displaycolor='#445566' broken=bad/></basematerials><colorgroup id='8'><color broken=bad color='#AABBCC'/><color color='#DDEEFF' broken=bad/></colorgroup>{}{}",object(1,5,1),object(2,8,1));
    let root = model(&resources, "<item objectid='1'/><item objectid='2'/>");
    for method in METHODS {
        let parsed = parse(&root, method);
        assert_eq!(parsed.model.triangle_count, 2);
        assert_eq!(parsed.materials.len(), 2);
        assert_eq!(parsed.materials[0].id, "3mf:3D/3dmodel.model:5:1");
        assert_eq!(parsed.materials[0].name.as_deref(), Some("After error"));
        assert_eq!(parsed.materials[0].color.as_deref(), Some("#445566"));
        assert_eq!(parsed.materials[1].id, "3mf:3D/3dmodel.model:8:1");
        assert_eq!(parsed.materials[1].name, None);
        assert_eq!(parsed.materials[1].color.as_deref(), Some("#DDEEFF"));
        assert_eq!(
            parsed.parts[0].material_id.as_deref(),
            Some(parsed.materials[0].id.as_str())
        );
        assert_eq!(
            parsed.parts[1].material_id.as_deref(),
            Some(parsed.materials[1].id.as_str())
        );
    }
}
