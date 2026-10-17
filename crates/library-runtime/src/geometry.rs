use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use library_protocol::{
    EmbeddedFilament, GeometryMaterial, GeometryPart, GeometryPlate, RuntimeError, RuntimeErrorCode,
};
use sha2::{Digest, Sha256};

use crate::secure_fs::Directory;
use crate::LibraryRuntime;

pub(crate) struct PreparedGeometry {
    pub(crate) path: PathBuf,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
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
    let source = runtime.verified_model_source_bytes(model)?;
    let parsed = stl_core::parse_model_with_plates(&source)
        .map_err(|_| invalid("Model geometry could not be parsed"))?;
    let (mesh, plates, parts) = if let Some(plate_id) = plate_id {
        let mesh = parsed
            .plate_mesh(plate_id)
            .ok_or_else(|| invalid("Requested plate was not found"))?;
        let plate = parsed
            .plates
            .iter()
            .find(|plate| plate.id == plate_id)
            .ok_or_else(|| invalid("Requested plate was not found"))?;
        let parts = parsed
            .plate_parts(plate_id)
            .ok_or_else(|| invalid("Requested plate has overlapping part ranges"))?;
        (
            mesh,
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
            parsed.model.clone(),
            map_plates(&parsed.plates),
            parsed.parts.clone(),
        )
    };
    if mesh.positions.len() != mesh.triangle_count as usize * 9
        || mesh.positions.iter().any(|value| !value.is_finite())
    {
        return Err(invalid("Model geometry is not a finite triangle packet"));
    }
    let packet = build_packet(mesh.triangle_count, &mesh.positions);
    let namespace = owned_geometry_namespace(runtime.host_library_root())?;
    let path = namespace.join(format!("geometry-{}.stl2", uuid::Uuid::new_v4().simple()));
    let directory = Directory::open(&namespace).map_err(|_| io_error())?;
    let name = path.file_name().ok_or_else(|| io_error())?;
    let mut created = false;
    let result = (|| {
        let mut file = directory
            .create_new_file_read_write(name)
            .map_err(|_| io_error())?;
        created = true;
        file.write_all(&packet).map_err(|_| io_error())?;
        file.sync_all().map_err(|_| io_error())?;
        drop(file);
        Ok(PreparedGeometry {
            path: path.clone(),
            bytes: packet.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&packet)),
            model_format: match mesh.format {
                stl_core::StlFormat::Binary => "stl-binary",
                stl_core::StlFormat::Ascii => "stl-ascii",
                stl_core::StlFormat::ThreeMf => "3mf",
            }
            .into(),
            triangle_count: mesh.triangle_count,
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
    })();
    match result {
        Ok(prepared) => {
            let file = directory.open_file(name).map_err(|_| io_error())?;
            let mut bytes = Vec::new();
            file.take(packet.len() as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| io_error())?;
            if bytes.len() != packet.len()
                || format!("{:x}", Sha256::digest(&bytes)) != prepared.sha256
            {
                let _ = directory.hold_entry(name);
                return Err(invalid("Published geometry packet failed its hash check"));
            }
            Ok(prepared)
        }
        Err(error) => {
            if created {
                let _ = directory.hold_entry(name);
            }
            Err(error)
        }
    }
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
