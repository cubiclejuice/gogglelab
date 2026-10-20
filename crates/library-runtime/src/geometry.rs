use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use library_protocol::{
    EmbeddedFilament, GeometryMaterial, GeometryPart, GeometryPlate, RuntimeError, RuntimeErrorCode,
};

use crate::secure_fs::{Directory, SealedGeometryFile};
use crate::LibraryRuntime;

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
    let source = runtime.verified_model_source_bytes(model)?;
    let parsed = stl_core::parse_model_with_plates(&source)
        .map_err(|_| invalid("Model geometry could not be parsed"))?;
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
    let (positions, plates, parts) = if let Some(plate) = selected {
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
        let parts = parsed
            .plate_parts(plate.id)
            .ok_or_else(|| invalid("Requested plate has overlapping part ranges"))?;
        (
            positions,
            vec![GeometryPlate {
                id: plate.id,
                name: plate.name.clone(),
                triangle_start: 0,
                triangle_count: plate.triangle_count,
            }],
            parts,
        )
    } else {
        (
            parsed.model.positions.as_slice(),
            map_plates(&parsed.plates),
            parsed.parts.clone(),
        )
    };
    checked_packet_bytes(triangles, positions.len())?;
    if positions.iter().any(|value| !value.is_finite()) {
        return Err(invalid("Model geometry is not a finite triangle packet"));
    }
    let packet = build_packet(triangles, positions);
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
        parts: parts
            .into_iter()
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

fn map_plates(plates: &[stl_core::Plate]) -> Vec<GeometryPlate> {
    plates
        .iter()
        .map(|plate| GeometryPlate {
            id: plate.id,
            name: plate.name.clone(),
            triangle_start: plate.triangle_start,
            triangle_count: plate.triangle_count,
        })
        .collect()
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
}
