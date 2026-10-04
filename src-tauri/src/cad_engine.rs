use crate::error::AppError;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use tauri::{AppHandle, Manager};
const LIMIT: u64 = 128 * 1024 * 1024;
fn read_kernel(resource: &Path, replacement: &Path) -> Result<Vec<u8>, AppError> {
    let path = if replacement
        .try_exists()
        .map_err(|_| AppError::NotReadable)?
    {
        replacement
    } else {
        resource
    };
    let file = File::open(path).map_err(|_| AppError::NotReadable)?;
    let metadata = file.metadata().map_err(|_| AppError::NotReadable)?;
    if !metadata.is_file() {
        return Err(AppError::NotReadable);
    }
    if metadata.len() > LIMIT {
        return Err(AppError::FileTooLarge {
            size: metadata.len(),
            limit: LIMIT,
        });
    }
    let mut bytes = Vec::new();
    file.take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| AppError::NotReadable)?;
    if bytes.len() as u64 > LIMIT {
        return Err(AppError::FileTooLarge {
            size: bytes.len() as u64,
            limit: LIMIT,
        });
    }
    if !bytes.starts_with(b"\0asm\x01\0\0\0") {
        return Err(AppError::MalformedStep { detail: "The replacement CAD engine is not a WebAssembly module. Remove or rebuild the replacement file.".into() });
    }
    Ok(bytes)
}
/// Fixed resource and user-owned override; no caller-supplied filesystem path.
#[tauri::command]
pub fn read_cad_engine(app: AppHandle) -> Result<tauri::ipc::Response, AppError> {
    let resource = app
        .path()
        .resource_dir()
        .map_err(|_| AppError::NotReadable)?
        .join("cad/occt-wasm.wasm");
    let replacement = app
        .path()
        .app_data_dir()
        .map_err(|_| AppError::NotReadable)?
        .join("cad/occt-wasm.wasm");
    read_kernel(&resource, &replacement).map(tauri::ipc::Response::new)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn user_modified_kernel_takes_precedence_and_invalid_replacement_is_not_hidden() {
        let dir = std::env::temp_dir().join(format!("gogglelab-cad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let original = dir.join("original.wasm");
        let modified = dir.join("modified.wasm");
        std::fs::write(&original, b"\0asm\x01\0\0\0original").unwrap();
        assert!(read_kernel(&original, &modified)
            .unwrap()
            .ends_with(b"original"));
        std::fs::write(&modified, b"\0asm\x01\0\0\0modified").unwrap();
        assert!(read_kernel(&original, &modified)
            .unwrap()
            .ends_with(b"modified"));
        std::fs::write(&modified, b"not a wasm module").unwrap();
        assert!(read_kernel(&original, &modified).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
