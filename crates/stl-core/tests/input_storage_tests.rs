//! Production-default compatibility for physical input storage admission.
//! Tiny public-entry fixtures; no private quota hooks or large allocations.
use std::io::{Cursor, Write};

use stl_core::package_bounds::{PackageProcessingContext, PackageUsage};
use stl_core::zip_bounds::ZipResourceObserver;
use stl_core::{parse_model_with_plates, parse_model_with_plates_in_context, StlError};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

const METHODS: [CompressionMethod; 2] = [CompressionMethod::Stored, CompressionMethod::Deflated];
const IDENTITY: &str = "1 0 0 0 1 0 0 0 1 0 0 0";

fn mesh(x: f32) -> String {
    format!("<mesh><vertices><vertex x='{x}' y='0' z='0'/><vertex x='{}' y='0' z='0'/><vertex x='{x}' y='1' z='0'/></vertices><triangles><triangle v1='0' v2='1' v3='2'/></triangles></mesh>", x + 1.0)
}

fn object(id: u32, x: f32, kind: &str) -> String {
    format!("<object id='{id}' type='{kind}'>{}</object>", mesh(x))
}

fn model(resources: &str, build: &str) -> String {
    format!("<model unit='millimeter' xmlns='http://schemas.microsoft.com/3dmanufacturing/core/2015/02' xmlns:p='http://schemas.microsoft.com/3dmanufacturing/production/2015/06'><resources>{resources}</resources><build>{build}</build></model>")
}

fn package(parts: &[(&str, &str)], method: CompressionMethod) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(method);
    for (path, xml) in parts {
        zip.start_file(*path, options).unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[test]
fn public_input_storage_cached_part_aliases_preserve_transforms_and_repeated_parse_work() {
    let root = model(
        "<object id='1' type='model'><components><component objectid='7' p:path='/3D/child.model' transform='1 0 0 0 1 0 0 0 1 10 0 0'/><component objectid='7' p:path='3D/child.model' transform='1 0 0 0 1 0 0 0 1 20 0 0'/></components></object>",
        "<item objectid='1'/>",
    );
    let child = model(&object(7, 0.0, "model"), "");
    for method in METHODS {
        let bytes = package(
            &[("3D/3dmodel.model", &root), ("3D/child.model", &child)],
            method,
        );
        let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
        for passes in 1..=2 {
            let parsed = parse_model_with_plates_in_context(&bytes, &mut context).unwrap();
            assert_eq!(parsed.model.triangle_count, 2);
            assert_eq!(
                parsed.model.positions,
                [10., 0., 0., 11., 0., 0., 10., 1., 0., 20., 0., 0., 21., 0., 0., 20., 1., 0.]
            );
            assert_eq!(context.usage(), PackageUsage {
                inflation_bytes: ((root.len() + child.len()) * passes) as u64,
                retained_input_bytes: 0,
            }, "leading-slash cache reuse reads the child once per independent parse; later parses still spend work");
        }
    }
}

#[test]
fn public_input_storage_reused_ids_in_distinct_parts_keep_independent_geometry() {
    let root = model(
        &object(1, 0.0, "model"),
        "<item objectid='1'/><item objectid='1' p:path='3D/child.model'/>",
    );
    let child = model(&object(1, 10.0, "model"), "");
    for method in METHODS {
        let bytes = package(
            &[("3D/3dmodel.model", &root), ("3D/child.model", &child)],
            method,
        );
        let parsed = parse_model_with_plates(&bytes).unwrap();
        assert_eq!(parsed.model.triangle_count, 2);
        assert_eq!(parsed.model.positions[0], 0.0);
        assert_eq!(parsed.model.positions[9], 10.0);
    }
}

#[test]
fn public_input_storage_unused_and_unprinted_nonfinite_vertices_stay_accepted() {
    let resources = format!(
        "{}{}{}",
        object(1, 0.0, "model"),
        object(2, f32::NAN, "other"),
        object(3, f32::NAN, "model")
    );
    let unused = model(&resources, "<item objectid='1'/><item objectid='2'/>");
    let emitted = model(&object(1, f32::NAN, "model"), "<item objectid='1'/>");
    for method in METHODS {
        let parsed =
            parse_model_with_plates(&package(&[("3D/3dmodel.model", &unused)], method)).unwrap();
        assert_eq!(parsed.model.triangle_count, 1);
        assert!(parsed.model.positions.iter().all(|value| value.is_finite()));
        assert_eq!(
            parse_model_with_plates(&package(&[("3D/3dmodel.model", &emitted)], method)),
            Err(StlError::NonFiniteCoordinate { triangle_index: 0 })
        );
    }
}

#[test]
fn public_input_storage_transform_tail_validation_preserves_error_priority() {
    for (transform, expected) in [
        (format!("{IDENTITY} 0"), "transform must have 12 entries"),
        (
            format!("{IDENTITY} 0 invalid"),
            "transform has a non-numeric entry",
        ),
        (
            "1 0 0 0 1 0 0 0 1 NaN 0 0".into(),
            "transform has a non-finite entry",
        ),
    ] {
        let xml = model(
            &object(1, 0.0, "model"),
            &format!("<item objectid='1' transform='{transform}'/>"),
        );
        for method in METHODS {
            assert_eq!(
                parse_model_with_plates(&package(&[("3D/3dmodel.model", &xml)], method)),
                Err(StlError::Malformed3mf {
                    detail: expected.into()
                })
            );
        }
    }
}

#[test]
fn public_input_storage_components_keep_discarding_mixed_mesh_indices() {
    let mixed = "<object id='1' type='model'><mesh><vertices><vertex x='0' y='0' z='0'/></vertices><triangles><triangle v1='999' v2='998' v3='997'/></triangles></mesh><components><component objectid='7' p:path='3D/child.model'/></components></object>";
    let root = model(mixed, "<item objectid='1'/>");
    let child = model(&object(7, 10.0, "model"), "");
    for method in METHODS {
        let parsed = parse_model_with_plates(&package(
            &[("3D/3dmodel.model", &root), ("3D/child.model", &child)],
            method,
        ))
        .unwrap();
        assert_eq!(parsed.model.triangle_count, 1);
        assert_eq!(parsed.model.positions[0], 10.0);
    }
}

#[test]
fn public_input_storage_duplicate_object_id_keeps_last_physical_object() {
    let root = model(
        &format!("{}{}", object(1, 0.0, "model"), object(1, 10.0, "model")),
        "<item objectid='1'/>",
    );
    for method in METHODS {
        let parsed =
            parse_model_with_plates(&package(&[("3D/3dmodel.model", &root)], method)).unwrap();
        assert_eq!(parsed.model.triangle_count, 1);
        assert_eq!(parsed.model.positions[0], 10.0);
    }
}
