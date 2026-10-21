use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use library_protocol::{
    EmbeddedFilament, GeometryMaterial, GeometryPart, GeometryPlate, RuntimeError, RuntimeErrorCode,
};

use crate::secure_fs::{Directory, SealedGeometryFile};
use crate::{LibraryRuntime, ProcessingError};
use stl_core::package_bounds::PackageProcessingContext;
#[cfg(test)]
use stl_core::zip_bounds::ZipResourceObserver;

pub(crate) const MAX_GEOMETRY_PACKET_BYTES: u64 = crate::secure_fs::MAX_GEOMETRY_PACKET_BYTES;

pub(crate) struct GeneratedGeometryPacket {
    // Rust drops fields in declaration order: close the anonymous descriptor
    // before releasing its byte charge, including on the last reader's drop.
    file: SealedGeometryFile,
    _charge: crate::GeometryByteCharge,
}
impl GeneratedGeometryPacket {
    pub(crate) fn bytes(&self) -> u64 {
        self.file.bytes()
    }
    pub(crate) fn sha256(&self) -> &str {
        self.file.sha256()
    }
    pub(crate) fn verify(&self) -> io::Result<()> {
        self.file.verify()
    }
    pub(crate) fn stream_to(&self, sink: &mut dyn Write) -> io::Result<()> {
        self.file.stream_to(sink)
    }
}

pub(crate) struct PreparedGeometry {
    pub(crate) packet: Arc<GeneratedGeometryPacket>,
    pub(crate) model_format: String,
    pub(crate) triangle_count: u32,
    pub(crate) plates: Vec<GeometryPlate>,
    pub(crate) parts: Vec<GeometryPart>,
    pub(crate) materials: Vec<GeometryMaterial>,
    pub(crate) embedded_filaments: Vec<EmbeddedFilament>,
    pub(crate) part_warning: Option<String>,
    pub(crate) plate_warning: Option<String>,
}

pub(crate) fn prepare_model_geometry(
    runtime: &LibraryRuntime,
    model: &library_core::Model,
    plate_id: Option<u32>,
) -> Result<PreparedGeometry, RuntimeError> {
    prepare_model_geometry_with_hook(runtime, model, plate_id, || {})
}
fn prepare_model_geometry_with_hook(
    runtime: &LibraryRuntime,
    model: &library_core::Model,
    plate_id: Option<u32>,
    before_derivative: impl FnOnce(),
) -> Result<PreparedGeometry, RuntimeError> {
    prepare_model_geometry_with_parser(
        runtime,
        model,
        plate_id,
        before_derivative,
        stl_core::parse_model_with_plates_in_context,
    )
}

fn prepare_model_geometry_with_parser(
    runtime: &LibraryRuntime,
    model: &library_core::Model,
    plate_id: Option<u32>,
    before_derivative: impl FnOnce(),
    parse: impl FnOnce(
        &[u8],
        &mut PackageProcessingContext,
    ) -> Result<stl_core::ParsedModel, stl_core::StlError>,
) -> Result<PreparedGeometry, RuntimeError> {
    let mut context = runtime.model_processing_context();
    prepare_model_geometry_with_parser_in_context(
        runtime,
        model,
        plate_id,
        before_derivative,
        parse,
        &mut context,
    )
}

fn prepare_model_geometry_with_parser_in_context(
    runtime: &LibraryRuntime,
    model: &library_core::Model,
    plate_id: Option<u32>,
    before_derivative: impl FnOnce(),
    parse: impl FnOnce(
        &[u8],
        &mut PackageProcessingContext,
    ) -> Result<stl_core::ParsedModel, stl_core::StlError>,
    context: &mut PackageProcessingContext,
) -> Result<PreparedGeometry, RuntimeError> {
    let observer = context.observer();
    let check = || {
        observer.check().map_err(|error| {
            ProcessingError::from(error).into_runtime(invalid("Model geometry could not be parsed"))
        })
    };
    check()?;
    let result = (|| {
        let source = runtime.verified_model_source_bytes_in_context(model, context)?;
        let parsed = parse(&source, context).map_err(|error| {
            ProcessingError::from(error).into_runtime(invalid("Model geometry could not be parsed"))
        })?;
        check()?;
        // Select only metadata before admission. Never clone the source mesh or
        // materialize a selected plate's position/normal vectors.
        let selected = plate_id
            .map(|id| {
                parsed
                    .plates
                    .iter()
                    .find(|plate| plate.id == id)
                    .ok_or_else(|| invalid("Requested plate was not found"))
            })
            .transpose()?;
        let triangles = selected.map_or(parsed.model.triangle_count, |plate| plate.triangle_count);
        let expected_positions = usize::try_from(triangles)
            .ok()
            .and_then(|count| count.checked_mul(9))
            .ok_or_else(|| invalid("Model geometry position count overflowed"))?;
        let expected_bytes = checked_packet_bytes(triangles, expected_positions)?;
        // This exact charge precedes all derivative materialization and packet
        // allocation. Subsequent writers/files drop before the local charge on error.
        let charge = runtime
            .reserve_temp(crate::ReservationKind::Geometry, expected_bytes)?
            .into_geometry_charge(expected_bytes)?;
        before_derivative();
        check()?;
        let positions = if let Some(plate) = selected {
            let start = usize::try_from(plate.triangle_start)
                .ok()
                .and_then(|value| value.checked_mul(9))
                .ok_or_else(|| invalid("Requested plate was not found"))?;
            let end = start
                .checked_add(expected_positions)
                .ok_or_else(|| invalid("Requested plate was not found"))?;
            let positions = parsed
                .model
                .positions
                .get(start..end)
                .ok_or_else(|| invalid("Requested plate was not found"))?;
            let normal_start = usize::try_from(plate.triangle_start)
                .ok()
                .and_then(|value| value.checked_mul(3))
                .ok_or_else(|| invalid("Requested plate was not found"))?;
            let normal_end = normal_start
                .checked_add(
                    usize::try_from(triangles)
                        .ok()
                        .and_then(|value| value.checked_mul(3))
                        .ok_or_else(|| invalid("Requested plate was not found"))?,
                )
                .ok_or_else(|| invalid("Requested plate was not found"))?;
            if parsed
                .model
                .stored_normals
                .get(normal_start..normal_end)
                .is_none()
            {
                return Err(invalid("Requested plate was not found"));
            }
            parsed
                .plate_part_refs(plate.id)
                .ok_or_else(|| invalid("Requested plate has overlapping part ranges"))?;
            positions
        } else {
            parsed.model.positions.as_slice()
        };
        // A selected range and every part end were checked by plate_part_refs.
        let selected_range = selected.map(|plate| {
            (
                plate.triangle_start,
                plate.triangle_start + plate.triangle_count,
            )
        });
        checked_packet_bytes(triangles, positions.len())?;
        if positions.iter().any(|value| !value.is_finite()) {
            return Err(invalid("Model geometry is not a finite triangle packet"));
        }
        let packet = build_packet(triangles, positions);
        check()?;
        let namespace = owned_geometry_namespace(runtime.host_library_root())?;
        let directory = Directory::open(&namespace).map_err(|_| io_error())?;
        let writer = directory
            .create_anonymous_geometry_writer()
            .map_err(|_| io_error())?;
        let file = writer.write_and_seal(&packet).map_err(|_| io_error())?;
        let packet = Arc::new(GeneratedGeometryPacket {
            file,
            _charge: charge,
        });
        let plates = if let Some(id) = plate_id {
            let plate = parsed
                .plates
                .into_iter()
                .find(|plate| plate.id == id)
                .ok_or_else(|| invalid("Requested plate was not found"))?;
            vec![GeometryPlate {
                id: plate.id,
                name: plate.name,
                triangle_start: 0,
                triangle_count: plate.triangle_count,
            }]
        } else {
            parsed
                .plates
                .into_iter()
                .map(|plate| GeometryPlate {
                    id: plate.id,
                    name: plate.name,
                    triangle_start: plate.triangle_start,
                    triangle_count: plate.triangle_count,
                })
                .collect()
        };
        Ok(PreparedGeometry {
            packet,
            model_format: match parsed.model.format {
                stl_core::StlFormat::Binary => "stl-binary",
                stl_core::StlFormat::Ascii => "stl-ascii",
                stl_core::StlFormat::ThreeMf => "3mf",
            }
            .into(),
            triangle_count: triangles,
            plates,
            parts: parsed
                .parts
                .into_iter()
                .filter_map(|mut part| {
                    if let Some((start, end)) = selected_range {
                        if part.triangle_start < start
                            || part.triangle_start + part.triangle_count > end
                        {
                            return None;
                        }
                        part.triangle_start -= start;
                    }
                    Some(part)
                })
                .map(|part| GeometryPart {
                    id: part.id,
                    name: part.name,
                    triangle_start: part.triangle_start,
                    triangle_count: part.triangle_count,
                    material_id: part.material_id,
                    filament_slot: part.filament_slot,
                })
                .collect(),
            materials: parsed
                .materials
                .into_iter()
                .map(|material| GeometryMaterial {
                    id: material.id,
                    name: material.name,
                    color: material.color,
                    source: match material.source {
                        stl_core::MaterialSource::ThreeMfBaseMaterial => "threeMfBaseMaterial",
                        stl_core::MaterialSource::ThreeMfColorGroup => "threeMfColorGroup",
                    }
                    .into(),
                })
                .collect(),
            embedded_filaments: parsed
                .embedded_filaments
                .into_iter()
                .map(|filament| EmbeddedFilament {
                    slot: filament.slot,
                    name: filament.name,
                    material: filament.material,
                    color: filament.color,
                })
                .collect(),
            part_warning: parsed.part_warning,
            plate_warning: parsed.plate_warning,
        })
    })();
    check()?;
    result
}

fn checked_packet_bytes(triangles: u32, positions: usize) -> Result<u64, RuntimeError> {
    let bytes = (triangles as u64)
        .checked_mul(36)
        .and_then(|value| value.checked_add(16))
        .ok_or_else(|| invalid("Model geometry packet size overflowed"))?;
    if bytes > MAX_GEOMETRY_PACKET_BYTES {
        return Err(RuntimeError {
            code: RuntimeErrorCode::Unsupported,
            message: "Model geometry exceeds the 128 MiB packet limit".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        });
    }
    if (triangles as u64).checked_mul(9) != u64::try_from(positions).ok() {
        return Err(invalid(
            "Model geometry positions do not match their triangle count",
        ));
    }
    Ok(bytes)
}

fn build_packet(triangle_count: u32, positions: &[f32]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(16 + positions.len() * 4);
    packet.extend_from_slice(b"STL2");
    packet.extend_from_slice(&2_u32.to_le_bytes());
    packet.extend_from_slice(&triangle_count.to_le_bytes());
    packet.extend_from_slice(&0_u32.to_le_bytes());
    for value in positions {
        packet.extend_from_slice(&value.to_le_bytes());
    }
    packet
}

fn owned_geometry_namespace(library_root: &Path) -> Result<PathBuf, RuntimeError> {
    let staging = library_root.join("staging");
    let metadata = fs::symlink_metadata(&staging).map_err(|_| io_error())?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || staging.canonicalize().map_err(|_| io_error())? != staging
    {
        return Err(invalid("Geometry staging namespace is unsafe"));
    }
    let namespace = staging.join("geometry");
    match fs::symlink_metadata(&namespace) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(invalid("Geometry namespace is unsafe"));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&namespace).map_err(|_| io_error())?;
        }
        Err(_) => return Err(io_error()),
    }
    if namespace.canonicalize().map_err(|_| io_error())? != namespace {
        return Err(invalid("Geometry namespace identity changed"));
    }
    Ok(namespace)
}

fn invalid(message: &str) -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::InvalidInput,
        message: message.into(),
        retryable: false,
        current_version: None,
        source_holds: Vec::new(),
    }
}

fn io_error() -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::Io,
        message: "Geometry packet filesystem operation failed".into(),
        retryable: false,
        current_version: None,
        source_holds: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn triangle_file() -> Vec<u8> {
        let mut bytes = vec![0; 80];
        bytes.extend_from_slice(&1u32.to_le_bytes());
        for value in [0f32, 0., 1., 0., 0., 0., 1., 0., 0., 0., 1., 0.] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes
    }
    #[test]
    fn funded_archived_geometry_denial_happens_in_hidden_validation_before_visible_parse() {
        use sha2::Digest;
        for method in [
            zip::CompressionMethod::Stored,
            zip::CompressionMethod::Deflated,
        ] {
            let temp = tempfile::tempdir().unwrap();
            let (runtime, model, inner, _) = crate::tests::nested_package_fixture(&temp, method);
            let outer = temp.path().join("nested-sources/models.zip");
            let original_outer = fs::read(&outer).unwrap();
            assert_eq!(model.source_kind, "indexed");
            assert_eq!(model.source_archive_member.as_deref(), Some("part.3mf"));
            assert_eq!(
                model.content_hash.as_deref(),
                Some(format!("{:x}", sha2::Sha256::digest(&inner)).as_str())
            );
            assert_eq!(
                stl_core::parse_model_with_plates(&inner)
                    .unwrap()
                    .model
                    .triangle_count,
                1
            );
            runtime.set_temp_limits_for_test(52, 52);
            let visible_parser_entered = std::cell::Cell::new(false);
            let derivative_entered = std::cell::Cell::new(false);
            let result = prepare_model_geometry_with_parser(
                &runtime,
                &model,
                None,
                || derivative_entered.set(true),
                |bytes, context| {
                    visible_parser_entered.set(true);
                    stl_core::parse_model_with_plates_in_context(bytes, context)
                },
            );
            let error = match result {
                Err(error) => error,
                Ok(_) => panic!("funding denial must terminate archived hidden validation"),
            };
            assert_eq!(error.code, RuntimeErrorCode::Busy);
            assert!(error.retryable);
            assert_eq!(
                error.message,
                "Model processing funding is temporarily unavailable"
            );
            assert!(!visible_parser_entered.get());
            assert!(!derivative_entered.get());
            assert!(runtime.leases.lock().unwrap().is_empty());
            assert_eq!(fs::read(&outer).unwrap(), original_outer);
            assert_eq!(
                runtime.read_db(|db| db.get_model(&model.id)).unwrap(),
                Some(model)
            );
            let state = runtime.temp_resources.lock().unwrap();
            assert_eq!(state.owned_bytes, 0);
            assert_eq!(state.reserved_owned_bytes, 0);
        }
    }

    #[test]
    fn funded_geometry_preserves_first_cause_before_and_after_real_path_denial() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("first-cause.3mf");
        let xml = r#"<model unit="millimeter"><resources><object id="1"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/></vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object></resources><build><item objectid="1"/></build></model>"#;
        let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        archive
            .start_file("3D/3dmodel.model", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(xml.as_bytes()).unwrap();
        let bytes = archive.finish().unwrap().into_inner();
        fs::write(&source, &bytes).unwrap();
        let runtime = LibraryRuntime::open(crate::RuntimeConfig::desktop(
            temp.path().join("library"),
            temp.path().join("integrations.json"),
        ))
        .unwrap();
        let model = runtime
            .mutate_db(|db| db.import_file(&source, None, None).map(|r| r.model))
            .unwrap();
        runtime.set_temp_limits_for_test(52, 52);
        for prior in [true, false] {
            for stop in [
                Some(stl_core::ResourceStop::InputLimitExceeded),
                Some(stl_core::ResourceStop::ReadWorkExceeded),
                Some(stl_core::ResourceStop::EnvelopeChanged),
                None,
            ] {
                let mut context = runtime.model_processing_context();
                let observer = context.observer();
                let latch = || match stop {
                    Some(stop) => {
                        let _ = observer.stop_io(stop);
                    }
                    None => {
                        let _ = observer.allocation_failed_io();
                    }
                };
                if prior {
                    latch();
                }
                let reached = std::cell::Cell::new(false);
                let result = prepare_model_geometry_with_parser_in_context(
                    &runtime,
                    &model,
                    None,
                    || panic!("packet allocation must not start"),
                    |bytes, context| {
                        let result = stl_core::parse_model_with_plates_in_context(bytes, context);
                        assert!(matches!(result, Err(stl_core::StlError::FundingBusy)));
                        reached.set(true);
                        latch();
                        result
                    },
                    &mut context,
                );
                let error = match result {
                    Err(error) => error,
                    Ok(_) => panic!("terminal cause lost"),
                };
                let expected = if !prior {
                    RuntimeErrorCode::Busy
                } else {
                    match stop {
                        Some(stl_core::ResourceStop::EnvelopeChanged) => RuntimeErrorCode::Stale,
                        Some(_) => RuntimeErrorCode::Unsupported,
                        None => RuntimeErrorCode::Internal,
                    }
                };
                assert_eq!(error.code, expected);
                assert_eq!(reached.get(), !prior);
                assert!(runtime.leases.lock().unwrap().is_empty());
                let state = runtime.temp_resources.lock().unwrap();
                assert_eq!(state.owned_bytes, 0);
                assert_eq!(state.reserved_owned_bytes, 0);
            }
        }
        assert_eq!(fs::read(source).unwrap(), bytes);
    }

    #[test]
    fn funded_geometry_path_denial_precedes_packet_allocation() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("funded.3mf");
        let xml = r#"<model unit="millimeter"><resources><object id="1"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/></vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object></resources><build><item objectid="1"/></build></model>"#;
        let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        archive
            .start_file("3D/3dmodel.model", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(xml.as_bytes()).unwrap();
        let bytes = archive.finish().unwrap().into_inner();
        fs::write(&source, &bytes).unwrap();
        let runtime = LibraryRuntime::open(crate::RuntimeConfig::desktop(
            temp.path().join("library"),
            temp.path().join("integrations.json"),
        ))
        .unwrap();
        let model = runtime
            .mutate_db(|db| {
                db.import_file(&source, None, None)
                    .map(|result| result.model)
            })
            .unwrap();
        runtime.set_temp_limits_for_test(52, 52);
        let derivative = std::cell::Cell::new(false);
        let error = match prepare_model_geometry_with_hook(&runtime, &model, None, || {
            derivative.set(true);
        }) {
            Ok(_) => {
                panic!("Host-funded path denial must precede the otherwise fitting 52-byte packet")
            }
            Err(error) => error,
        };
        assert_eq!(error.code, RuntimeErrorCode::Busy);
        assert!(error.retryable);
        assert!(!derivative.get());
        assert!(runtime.leases.lock().unwrap().is_empty());
        assert_eq!(fs::read(&source).unwrap(), bytes);
        let state = runtime.temp_resources.lock().unwrap();
        assert_eq!(state.owned_bytes, 0);
        assert_eq!(state.reserved_owned_bytes, 0);
    }

    fn assert_part_backing_is_moved(selected: Option<u32>) {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("triangle.stl");
        fs::write(&source, triangle_file()).unwrap();
        let runtime = LibraryRuntime::open(crate::RuntimeConfig::desktop(
            temp.path().join("library"),
            temp.path().join("integrations.json"),
        ))
        .unwrap();
        let model = runtime
            .mutate_db(|db| {
                db.import_file(&source, None, None)
                    .map(|result| result.model)
            })
            .unwrap();
        let original = std::cell::RefCell::new(Vec::new());
        let prepared = prepare_model_geometry_with_parser(
            &runtime,
            &model,
            selected,
            || {},
            |bytes, context| {
                let mut parsed = stl_core::parse_model_with_plates_in_context(bytes, context)?;
                let positions = parsed.model.positions.clone();
                let normals = parsed.model.stored_normals.clone();
                parsed.model.positions.extend_from_slice(&positions);
                parsed.model.stored_normals.extend_from_slice(&normals);
                parsed.model.triangle_count = 2;
                parsed.plates = vec![
                    stl_core::Plate {
                        id: 10,
                        name: "First plate".into(),
                        triangle_start: 0,
                        triangle_count: 1,
                    },
                    stl_core::Plate {
                        id: 20,
                        name: "Second plate".into(),
                        triangle_start: 1,
                        triangle_count: 1,
                    },
                ];
                parsed.parts = (0..2)
                    .map(|index| stl_core::ParsedPart {
                        id: format!("source-part-{index}"),
                        name: format!("Source part {index}"),
                        triangle_start: index,
                        triangle_count: 1,
                        material_id: Some(format!("source-material-{index}")),
                        filament_slot: Some(index + 1),
                    })
                    .collect();
                *original.borrow_mut() = parsed
                    .parts
                    .iter()
                    .map(|part| {
                        (
                            part.id.as_ptr() as usize,
                            part.name.as_ptr() as usize,
                            part.material_id.as_ref().unwrap().as_ptr() as usize,
                        )
                    })
                    .collect();
                Ok(parsed)
            },
        )
        .unwrap();
        let indices: &[usize] = if selected.is_some() { &[1] } else { &[0, 1] };
        assert_eq!(prepared.triangle_count as usize, indices.len());
        assert_eq!(prepared.parts.len(), indices.len());
        assert_eq!(prepared.plates.len(), indices.len());
        for (part, index) in prepared.parts.iter().zip(indices) {
            assert_eq!(part.id, format!("source-part-{index}"));
            assert_eq!(part.name, format!("Source part {index}"));
            assert_eq!(part.material_id, Some(format!("source-material-{index}")));
            assert_eq!(
                part.triangle_start,
                if selected.is_some() { 0 } else { *index as u32 }
            );
            assert_eq!(part.triangle_count, 1);
            assert_eq!(part.filament_slot, Some(*index as u32 + 1));
        }
        let mut packet = Vec::new();
        prepared.packet.stream_to(&mut packet).unwrap();
        assert_eq!(
            packet,
            build_packet(
                indices.len() as u32,
                &[0., 0., 0., 1., 0., 0., 0., 1., 0.].repeat(indices.len())
            )
        );
        assert_eq!(fs::read(&source).unwrap(), triangle_file());
        assert!(
            fs::read_dir(runtime.host_library_root().join("staging/geometry"))
                .unwrap()
                .next()
                .is_none()
        );
        for (part, index) in prepared.parts.iter().zip(indices) {
            assert_eq!(
                (
                    part.id.as_ptr() as usize,
                    part.name.as_ptr() as usize,
                    part.material_id.as_ref().unwrap().as_ptr() as usize
                ),
                original.borrow()[*index],
                "nonempty final part fields must retain their original parsed backing"
            );
        }
    }

    #[test]
    fn whole_geometry_moves_nonempty_part_backing() {
        assert_part_backing_is_moved(None);
    }

    #[test]
    fn selected_geometry_moves_nonempty_part_backing() {
        assert_part_backing_is_moved(Some(20));
    }

    #[test]
    fn whole_geometry_does_not_apply_selected_part_range_validation() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("triangle.stl");
        fs::write(&source, triangle_file()).unwrap();
        let runtime = LibraryRuntime::open(crate::RuntimeConfig::desktop(
            temp.path().join("library"),
            temp.path().join("integrations.json"),
        ))
        .unwrap();
        let model = runtime
            .mutate_db(|db| {
                db.import_file(&source, None, None)
                    .map(|result| result.model)
            })
            .unwrap();
        for selected in [Some(0), None] {
            let result = prepare_model_geometry_with_parser(
                &runtime,
                &model,
                selected,
                || {},
                |bytes, context| {
                    let mut parsed = stl_core::parse_model_with_plates_in_context(bytes, context)?;
                    parsed.parts.push(stl_core::ParsedPart {
                        id: "outside-overflow".into(),
                        name: "Outside overflow".into(),
                        triangle_start: u32::MAX,
                        triangle_count: 1,
                        material_id: None,
                        filament_slot: None,
                    });
                    Ok(parsed)
                },
            );
            match (selected, result) {
                (Some(_), Err(error)) => {
                    assert_eq!(error.code, RuntimeErrorCode::InvalidInput);
                    assert_eq!(error.message, "Requested plate has overlapping part ranges");
                }
                (None, Ok(prepared)) => {
                    assert_eq!(prepared.parts.len(), 1);
                    assert_eq!(prepared.parts[0].triangle_start, u32::MAX);
                }
                _ => panic!("whole and selected part range rules changed"),
            }
            let usage = runtime.temp_resources.lock().unwrap();
            assert_eq!(usage.owned_bytes, 0);
            assert_eq!(usage.reserved_owned_bytes, 0);
        }
        assert_eq!(fs::read(source).unwrap(), triangle_file());
    }

    #[test]
    fn typed_geometry_parser_stops_precede_all_derivative_work() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("triangle.stl");
        fs::write(&source, triangle_file()).unwrap();
        let runtime = LibraryRuntime::open(crate::RuntimeConfig::desktop(
            temp.path().join("library"),
            temp.path().join("integrations.json"),
        ))
        .unwrap();
        let model = runtime
            .mutate_db(|db| {
                db.import_file(&source, None, None)
                    .map(|result| result.model)
            })
            .unwrap();
        for (error, code, retryable) in [
            (
                stl_core::StlError::ResourceStopped(stl_core::ResourceStop::InputLimitExceeded),
                RuntimeErrorCode::Unsupported,
                false,
            ),
            (
                stl_core::StlError::ResourceStopped(stl_core::ResourceStop::ReadWorkExceeded),
                RuntimeErrorCode::Unsupported,
                false,
            ),
            (
                stl_core::StlError::ResourceStopped(stl_core::ResourceStop::EnvelopeChanged),
                RuntimeErrorCode::Stale,
                true,
            ),
            (
                stl_core::StlError::AllocationFailed,
                RuntimeErrorCode::Internal,
                false,
            ),
            (
                stl_core::StlError::MalformedHeader,
                RuntimeErrorCode::InvalidInput,
                false,
            ),
        ] {
            let expected_message = if matches!(error, stl_core::StlError::MalformedHeader) {
                "Model geometry could not be parsed".to_string()
            } else {
                error.to_string()
            };
            let derivative = std::cell::Cell::new(false);
            let result = prepare_model_geometry_with_parser(
                &runtime,
                &model,
                None,
                || derivative.set(true),
                |_, _| Err(error),
            );
            let actual = match result {
                Ok(_) => panic!("injected parser stop must fail"),
                Err(error) => error,
            };
            assert_eq!(actual.code, code);
            assert_eq!(actual.retryable, retryable);
            assert_eq!(actual.message, expected_message);
            assert!(!derivative.get());
            let usage = runtime.temp_resources.lock().unwrap();
            assert_eq!(usage.owned_bytes, 0);
            assert_eq!(usage.reserved_owned_bytes, 0);
        }
    }
    #[test]
    fn prepared_geometry_never_names_packet_bytes_or_retains_derivative_holds() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("triangle.stl");
        fs::write(&source, triangle_file()).unwrap();
        let runtime = LibraryRuntime::open(crate::RuntimeConfig::desktop(
            temp.path().join("library"),
            temp.path().join("integrations.json"),
        ))
        .unwrap();
        let model = runtime
            .mutate_db(|db| {
                db.import_file(&source, None, None)
                    .map(|result| result.model)
            })
            .unwrap();
        let prepared = prepare_model_geometry(&runtime, &model, None).unwrap();
        let namespace = runtime.host_library_root().join("staging/geometry");
        let entries = fs::read_dir(&namespace)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert!(
            entries.is_empty(),
            "prepared geometry must have anonymous backing, found {entries:?}"
        );
        drop(prepared);
        drop(runtime);
        assert!(
            fs::read_dir(&namespace).unwrap().next().is_none(),
            "normal retirement must not create generated holds"
        );
        assert_eq!(fs::read(source).unwrap(), triangle_file());
    }
    #[test]
    fn packet_size_limit_is_checked_before_allocation_or_output_creation() {
        assert_eq!(checked_packet_bytes(1, 9).unwrap(), 52);
        let above_limit = (MAX_GEOMETRY_PACKET_BYTES / 36 + 1) as u32;
        assert_eq!(
            checked_packet_bytes(above_limit, 0).unwrap_err().code,
            RuntimeErrorCode::Unsupported
        );
        assert!(checked_packet_bytes(1, 8).is_err());
    }
    #[test]
    fn shared_budget_rejects_whole_and_plate_derivatives_before_materialization() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("triangle.stl");
        fs::write(&source, triangle_file()).unwrap();
        let runtime = LibraryRuntime::open(crate::RuntimeConfig::desktop(
            temp.path().join("library"),
            temp.path().join("integrations.json"),
        ))
        .unwrap();
        let model = runtime
            .mutate_db(|db| {
                db.import_file(&source, None, None)
                    .map(|result| result.model)
            })
            .unwrap();
        runtime.set_temp_limits_for_test(0, 1024);
        let plate = stl_core::parse_model_with_plates(&triangle_file())
            .unwrap()
            .plates[0]
            .id;
        for selected in [None, Some(plate)] {
            let materialized = std::cell::Cell::new(false);
            let result = prepare_model_geometry_with_hook(&runtime, &model, selected, || {
                materialized.set(true)
            });
            assert!(matches!(
                result,
                Err(RuntimeError {
                    code: RuntimeErrorCode::Busy,
                    ..
                })
            ));
            assert!(
                !materialized.get(),
                "shared quota must reject before whole/plate derivative construction"
            );
        }
        assert_eq!(fs::read(source).unwrap(), triangle_file());
    }
    #[test]
    fn geometry_nested_validation_and_parse_share_actual_package_counter() {
        for method in [
            zip::CompressionMethod::Stored,
            zip::CompressionMethod::Deflated,
        ] {
            let temp = tempfile::tempdir().unwrap();
            let (runtime, model, _, xml_bytes) =
                crate::tests::nested_package_fixture(&temp, method);
            let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
            let prepared = prepare_model_geometry_with_parser_in_context(
                &runtime,
                &model,
                None,
                || {},
                |bytes, context| {
                    assert_eq!(
                        context.usage().inflation_bytes,
                        xml_bytes,
                        "hidden source validation already spent"
                    );
                    stl_core::parse_model_with_plates_in_context(bytes, context)
                },
                &mut context,
            )
            .unwrap();
            assert_eq!(context.usage().inflation_bytes, xml_bytes * 2);
            assert_eq!(context.usage().retained_input_bytes, 0);
            assert_eq!(prepared.triangle_count, 1);
        }
    }
}
