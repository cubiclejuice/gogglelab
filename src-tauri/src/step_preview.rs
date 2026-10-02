//! Read bounded STEP bytes for the isolated webview preview worker.

use std::path::Path;
use std::{fs::File, io::Read};

use tauri::ipc::Response;

use crate::error::{map_io_error, AppError};
use crate::state::AppState;

const MAX_STEP_BYTES: u64 = 25 * 1024 * 1024;

#[tauri::command]
pub fn read_step_file(
    state: tauri::State<'_, AppState>,
    path: String,
    generation: u64,
) -> Result<Response, AppError> {
    state.register_generation(generation);
    Ok(Response::new(read_step_bytes(Path::new(&path))?))
}

pub(crate) fn read_step_bytes(path: &Path) -> Result<Vec<u8>, AppError> {
    let path = path.canonicalize().map_err(|e| map_io_error(&e))?;
    let is_step = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("step") || extension.eq_ignore_ascii_case("stp")
        });
    if !is_step {
        return Err(AppError::MalformedStep {
            detail: "Choose a .step or .stp file.".into(),
        });
    }
    let mut file = File::open(&path).map_err(|e| map_io_error(&e))?;
    let metadata = file.metadata().map_err(|e| map_io_error(&e))?;
    if !metadata.is_file() {
        return Err(AppError::NotReadable);
    }
    if metadata.len() > MAX_STEP_BYTES {
        return Err(AppError::FileTooLarge {
            size: metadata.len(),
            limit: MAX_STEP_BYTES,
        });
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.by_ref()
        .take(MAX_STEP_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| map_io_error(&e))?;
    if bytes.len() as u64 > MAX_STEP_BYTES {
        return Err(AppError::FileTooLarge {
            size: bytes.len() as u64,
            limit: MAX_STEP_BYTES,
        });
    }
    let body = bytes
        .strip_prefix(b"\xef\xbb\xbf")
        .unwrap_or(&bytes)
        .trim_ascii_start();
    if !body.starts_with(b"ISO-10303-21;") {
        return Err(AppError::MalformedStep {
            detail: "The file has no STEP Part 21 header.".into(),
        });
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_step_header_and_rejects_non_step_content() {
        let path =
            std::env::temp_dir().join(format!("gogglelab-step-reader-{}.step", std::process::id()));
        std::fs::write(&path, b"\xef\xbb\xbf\nISO-10303-21;\n").unwrap();
        assert!(read_step_bytes(&path).is_ok());
        std::fs::write(&path, b"not a STEP file").unwrap();
        assert!(matches!(
            read_step_bytes(&path),
            Err(AppError::MalformedStep { .. })
        ));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_oversized_step_before_reading_payload() {
        let path = std::env::temp_dir().join(format!(
            "gogglelab-step-oversize-{}.stp",
            std::process::id()
        ));
        let file = File::create(&path).unwrap();
        file.set_len(MAX_STEP_BYTES + 1).unwrap();
        assert!(matches!(
            read_step_bytes(&path),
            Err(AppError::FileTooLarge { .. })
        ));
        std::fs::remove_file(path).unwrap();
    }
}
