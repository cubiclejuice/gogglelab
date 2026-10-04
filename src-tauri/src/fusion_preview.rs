//! User-local Fusion add-in bridge. No cloud upload or source-file modification.
use crate::error::{map_io_error, AppError};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{ipc::Response, State};

const MAX_F3D_BYTES: u64 = 50 * 1024 * 1024;
const CONVERSION_TIMEOUT: Duration = Duration::from_secs(120);
const BRIDGE_VERSION: u32 = 1;
const ADDIN_NAME: &str = "GoggleLabFusionBridge";
const ADDIN_SCRIPT: &str =
    include_str!("../../integrations/fusion/GoggleLabFusionBridge/GoggleLabFusionBridge.py");
const ADDIN_MANIFEST: &str =
    include_str!("../../integrations/fusion/GoggleLabFusionBridge/GoggleLabFusionBridge.manifest");

#[derive(Default)]
pub struct FusionBridgeState(Mutex<HashMap<String, Arc<AtomicBool>>>);

fn fusion_error(message: impl Into<String>, setup_required: bool) -> AppError {
    AppError::Fusion {
        message: message.into(),
        setup_required,
    }
}
fn valid_request_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn ensure_supported_platform() -> Result<(), AppError> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err(fusion_error("F3D conversion through Autodesk Fusion is available only on macOS. Export STEP from Fusion to view this model on Windows or Linux.", false))
    }
}
fn home() -> Result<PathBuf, AppError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| fusion_error("Could not locate your home folder.", false))
}
fn bridge_root(home: &Path) -> PathBuf {
    home.join("Library/Application Support/GoggleLab/FusionBridge")
}
fn addin_root(home: &Path) -> PathBuf {
    home.join("Library/Application Support/Autodesk/Autodesk Fusion 360/API/AddIns")
        .join(ADDIN_NAME)
}
fn fusion_app(home: &Path) -> Option<PathBuf> {
    [
        home.join("Applications/Autodesk Fusion.app"),
        PathBuf::from("/Applications/Autodesk Fusion.app"),
        home.join("Library/Application Support/Autodesk/webdeploy/production/Autodesk Fusion.app"),
    ]
    .into_iter()
    .find(|p| p.is_dir())
}

fn register_job(state: &FusionBridgeState, id: &str) -> Result<Arc<AtomicBool>, AppError> {
    let mut jobs = state
        .0
        .lock()
        .map_err(|_| fusion_error("The Fusion bridge is busy.", false))?;
    if let Some(existing) = jobs.get(id) {
        if existing.load(Ordering::Acquire) {
            jobs.remove(id);
            return Err(AppError::Superseded);
        }
        return Err(fusion_error(
            "This Fusion request is already running.",
            false,
        ));
    }
    if jobs.len() >= 256 {
        jobs.retain(|_, flag| !flag.load(Ordering::Acquire));
    }
    if jobs.len() >= 256 {
        return Err(fusion_error("Too many Fusion requests are pending.", false));
    }
    for previous in jobs.values() {
        previous.store(true, Ordering::Release);
    }
    let canceled = Arc::new(AtomicBool::new(false));
    jobs.insert(id.to_owned(), canceled.clone());
    Ok(canceled)
}
fn mark_canceled(state: &FusionBridgeState, id: &str) -> Result<(), AppError> {
    let mut jobs = state
        .0
        .lock()
        .map_err(|_| fusion_error("The Fusion bridge is busy.", false))?;
    if jobs.len() >= 256 && !jobs.contains_key(id) {
        jobs.retain(|_, flag| !flag.load(Ordering::Acquire));
    }
    // Remember cancellation even if this command arrives before the async reader.
    jobs.entry(id.to_owned())
        .or_insert_with(|| Arc::new(AtomicBool::new(true)))
        .store(true, Ordering::Release);
    Ok(())
}
fn launch_fusion(app: &Path) -> Result<(), AppError> {
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("/usr/bin/open")
            .args(["-g", "-a"])
            .arg(app)
            .status()
            .map_err(|e| map_io_error(&e))?;
        if !status.success() {
            return Err(fusion_error(
                "Open Autodesk Fusion and sign in, then try again.",
                true,
            ));
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err(fusion_error(
            "The local Fusion bridge is currently available on macOS.",
            false,
        ))
    }
}

/// Never follow a symlink when creating our private IPC/add-in directories.
fn private_dir(path: &Path) -> Result<(), AppError> {
    for ancestor in path.ancestors() {
        if fs::symlink_metadata(ancestor).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(fusion_error(
                "The Fusion bridge folder contains a symbolic link.",
                false,
            ));
        }
    }
    let existed = fs::symlink_metadata(path).is_ok();
    if let Ok(meta) = fs::symlink_metadata(path) {
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(fusion_error(
                "The Fusion bridge folder is not a regular directory.",
                false,
            ));
        }
    } else {
        if let Some(parent) = path.parent() {
            // Create each missing component separately; validate existing ancestors first.
            if !parent.as_os_str().is_empty() {
                private_dir(parent)?;
            }
        }
        fs::create_dir(path).map_err(|e| map_io_error(&e))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if !existed
            || path
                .file_name()
                .is_some_and(|n| n == "FusionBridge" || n == "jobs" || n == ADDIN_NAME)
        {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                .map_err(|e| map_io_error(&e))?;
        }
    }
    Ok(())
}
fn write_owned_file(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(fusion_error(
            "A Fusion bridge file points outside its folder.",
            false,
        ));
    }
    fs::write(path, bytes).map_err(|e| map_io_error(&e))
}

#[tauri::command]
pub fn install_fusion_bridge() -> Result<(), AppError> {
    ensure_supported_platform()?;
    let home = home()?;
    let app = fusion_app(&home).ok_or_else(|| {
        fusion_error(
            "Install Autodesk Fusion and sign in before connecting it to GoggleLab.",
            false,
        )
    })?;
    let root = bridge_root(&home);
    private_dir(&root)?;
    private_dir(&root.join("jobs"))?;
    let addin = addin_root(&home);
    private_dir(&addin)?;
    write_owned_file(
        &addin.join(format!("{ADDIN_NAME}.py")),
        ADDIN_SCRIPT.as_bytes(),
    )?;
    write_owned_file(
        &addin.join(format!("{ADDIN_NAME}.manifest")),
        ADDIN_MANIFEST.as_bytes(),
    )?;
    launch_fusion(&app)?;
    Ok(())
}

#[derive(Deserialize)]
struct Ready {
    version: u32,
    updated_at: f64,
}
fn is_ready(root: &Path) -> bool {
    let Some(ready) = fs::read(root.join("ready.json"))
        .ok()
        .filter(|b| b.len() <= 1024)
        .and_then(|b| serde_json::from_slice::<Ready>(&b).ok())
    else {
        return false;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    ready.version == BRIDGE_VERSION
        && ready.updated_at.is_finite()
        && (0.0..=4.0).contains(&(now - ready.updated_at))
}

fn read_source(path: &Path) -> Result<Vec<u8>, AppError> {
    if !path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("f3d"))
    {
        return Err(fusion_error("Choose a .f3d Fusion archive.", false));
    }
    let resolved = path.canonicalize().map_err(|e| map_io_error(&e))?;
    if !fs::metadata(&resolved)
        .map_err(|e| map_io_error(&e))?
        .is_file()
    {
        return Err(AppError::NotReadable);
    }
    let mut file = File::open(&resolved).map_err(|e| map_io_error(&e))?;
    let meta = file.metadata().map_err(|e| map_io_error(&e))?;
    if !meta.is_file() {
        return Err(AppError::NotReadable);
    }
    if meta.len() > MAX_F3D_BYTES {
        return Err(AppError::FileTooLarge {
            size: meta.len(),
            limit: MAX_F3D_BYTES,
        });
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_F3D_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| map_io_error(&e))?;
    if bytes.len() as u64 > MAX_F3D_BYTES {
        return Err(AppError::FileTooLarge {
            size: bytes.len() as u64,
            limit: MAX_F3D_BYTES,
        });
    }
    if bytes.is_empty() {
        return Err(AppError::EmptyFile);
    }
    // F3D is a ZIP archive. Fusion remains the authoritative format reader.
    if !bytes.starts_with(b"PK\x03\x04") {
        return Err(fusion_error("This file is not a Fusion archive.", false));
    }
    Ok(bytes)
}

struct JobFolder(PathBuf);
impl Drop for JobFolder {
    fn drop(&mut self) {
        // A synchronous Fusion API call cannot be interrupted. Its add-in removes
        // canceled jobs after closing the private document; keep its files alive.
        if self.0.join("document-open").exists()
            || (self.0.join("processing").exists() && !self.0.join("result.json").exists())
        {
            let _ = write_owned_file(&self.0.join("cancelled"), b"");
            return;
        }
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[derive(Deserialize)]
struct JobResult {
    ok: bool,
    error: Option<String>,
}

fn wait_for_result(job: &Path, canceled: &AtomicBool, timeout: Duration) -> Result<(), AppError> {
    let start = Instant::now();
    loop {
        if canceled.load(Ordering::Acquire) {
            let _ = write_owned_file(&job.join("cancelled"), b"");
            return Err(AppError::Superseded);
        }
        let result_path = job.join("result.json");
        if result_path.exists() {
            if fs::symlink_metadata(&result_path)
                .map_err(|e| map_io_error(&e))?
                .file_type()
                .is_symlink()
            {
                return Err(fusion_error("Fusion returned an invalid response.", false));
            }
            let mut bytes = Vec::new();
            File::open(result_path)
                .map_err(|e| map_io_error(&e))?
                .take(8193)
                .read_to_end(&mut bytes)
                .map_err(|e| map_io_error(&e))?;
            if bytes.len() > 8192 {
                return Err(fusion_error("Fusion returned an invalid response.", false));
            }
            let result: JobResult = serde_json::from_slice(&bytes)
                .map_err(|_| fusion_error("Fusion returned an invalid response.", false))?;
            return if result.ok {
                Ok(())
            } else {
                Err(fusion_error(
                result.error.unwrap_or_else(|| "Fusion could not export this design. Check its license and export availability.".into()), false))
            };
        }
        if start.elapsed() >= timeout {
            let _ = write_owned_file(&job.join("cancelled"), b"");
            return Err(fusion_error("Fusion conversion timed out after 120 seconds. Finish any open Fusion dialog and try again.", false));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn convert(path: &Path, id: &str, canceled: &AtomicBool) -> Result<Response, AppError> {
    let start = Instant::now();
    let home = home()?;
    let app = fusion_app(&home).ok_or_else(|| {
        fusion_error(
            "F3D preview requires Autodesk Fusion. Install Fusion and sign in, then try again.",
            false,
        )
    })?;
    let source = read_source(path)?;
    let root = bridge_root(&home);
    private_dir(&root)?;
    let jobs = root.join("jobs");
    private_dir(&jobs)?;
    if !is_ready(&root) {
        if !addin_root(&home)
            .join(format!("{ADDIN_NAME}.manifest"))
            .is_file()
        {
            return Err(fusion_error(
                "Connect the Fusion helper to preview this design.",
                true,
            ));
        }
        launch_fusion(&app)?;
        while !is_ready(&root) && start.elapsed() < Duration::from_secs(30) {
            if canceled.load(Ordering::Acquire) {
                return Err(AppError::Superseded);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if !is_ready(&root) {
            return Err(fusion_error("In Fusion’s Scripts and Add-Ins, enable GoggleLabFusionBridge and Run on Startup, then try again.", true));
        }
    }
    if canceled.load(Ordering::Acquire) {
        return Err(AppError::Superseded);
    }
    let job_path = jobs.join(id);
    fs::create_dir(&job_path).map_err(|e| map_io_error(&e))?;
    let job = JobFolder(job_path);
    private_dir(&job.0)?;
    write_owned_file(&job.0.join("source.f3d"), &source)?;
    // Publish the request last, so Fusion never reads a partially copied source.
    write_owned_file(&job.0.join("request.tmp"), br#"{"version":1}"#)?;
    fs::rename(job.0.join("request.tmp"), job.0.join("request.json"))
        .map_err(|e| map_io_error(&e))?;
    wait_for_result(
        &job.0,
        canceled,
        CONVERSION_TIMEOUT.saturating_sub(start.elapsed()),
    )?;
    let output = job.0.join("conversion.step");
    if fs::symlink_metadata(&output)
        .map_err(|e| map_io_error(&e))?
        .file_type()
        .is_symlink()
    {
        return Err(fusion_error(
            "Fusion returned an invalid output file.",
            false,
        ));
    }
    let step = crate::step_preview::read_step_bytes(&output)?;
    if canceled.load(Ordering::Acquire) {
        return Err(AppError::Superseded);
    }
    // F3D1 + original size (LE u64) + original SHA-256 + bounded STEP bytes.
    let mut response = Vec::with_capacity(44 + step.len());
    response.extend_from_slice(b"F3D1");
    response.extend_from_slice(&(source.len() as u64).to_le_bytes());
    response.extend_from_slice(&Sha256::digest(&source));
    response.extend_from_slice(&step);
    Ok(Response::new(response))
}

#[tauri::command]
pub async fn read_fusion_file(
    path: String,
    request_id: String,
    state: State<'_, FusionBridgeState>,
) -> Result<Response, AppError> {
    ensure_supported_platform()?;
    if !valid_request_id(&request_id) {
        return Err(fusion_error("Invalid Fusion preview request.", false));
    }
    let canceled = register_job(&state, &request_id)?;
    let id = request_id.clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || convert(Path::new(&path), &id, &canceled))
            .await
            .map_err(|_| fusion_error("The Fusion conversion worker stopped.", false));
    if let Ok(mut jobs) = state.0.lock() {
        jobs.remove(&request_id);
    }
    result?
}

#[tauri::command]
pub fn cancel_fusion_preview(
    request_id: String,
    state: State<'_, FusionBridgeState>,
) -> Result<(), AppError> {
    if !valid_request_id(&request_id) {
        return Err(fusion_error("Invalid Fusion preview request.", false));
    }
    mark_canceled(&state, &request_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temp_job(name: &str) -> JobFolder {
        let path =
            std::env::temp_dir().join(format!("gogglelab-fusion-{name}-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        JobFolder(path)
    }
    #[test]
    fn cancellation_before_registration_cannot_start_a_conversion() {
        let state = FusionBridgeState::default();
        let id = "0123456789abcdef0123456789abcdef";
        mark_canceled(&state, id).unwrap();
        assert!(matches!(
            register_job(&state, id),
            Err(AppError::Superseded)
        ));
        let first = register_job(&state, id).unwrap();
        let second = register_job(&state, "1123456789abcdef0123456789abcdef").unwrap();
        assert!(first.load(Ordering::Acquire));
        assert!(!second.load(Ordering::Acquire));
    }
    #[test]
    fn request_ids_cannot_escape_private_job_folder() {
        assert!(valid_request_id("0123456789abcdef0123456789abcdef"));
        for id in [
            "../file",
            "",
            "A123456789abcdef0123456789abcdef",
            "/tmp/0123456789abcdef0123456789ab",
        ] {
            assert!(!valid_request_id(id));
        }
    }
    #[test]
    fn reject_wrong_extension_bad_header_and_oversized_source() {
        let job = temp_job("input");
        let path = job.0.join("source.f3d");
        fs::write(&path, b"not an archive").unwrap();
        assert!(matches!(read_source(&path), Err(AppError::Fusion { .. })));
        fs::write(&path, b"PK\x03\x04fixture").unwrap();
        assert!(read_source(&path).is_ok());
        assert!(read_source(&job.0.join("source.step")).is_err());
        File::create(&path)
            .unwrap()
            .set_len(MAX_F3D_BYTES + 1)
            .unwrap();
        assert!(matches!(
            read_source(&path),
            Err(AppError::FileTooLarge { .. })
        ));
    }
    #[test]
    fn cancellation_and_timeout_do_not_leave_jobs() {
        let job = temp_job("cancel");
        assert!(matches!(
            wait_for_result(&job.0, &AtomicBool::new(true), Duration::ZERO),
            Err(AppError::Superseded)
        ));
        assert!(job.0.join("cancelled").exists());
        assert!(matches!(
            wait_for_result(&job.0, &AtomicBool::new(false), Duration::ZERO),
            Err(AppError::Fusion { .. })
        ));
        let path = job.0.clone();
        drop(job);
        assert!(!path.exists());
    }
    #[test]
    fn active_fusion_jobs_live_until_document_cleanup_is_acknowledged() {
        let job = temp_job("active");
        fs::write(job.0.join("processing"), b"").unwrap();
        let path = job.0.clone();
        drop(job);
        assert!(path.join("cancelled").exists());
        fs::write(path.join("result.json"), br#"{"ok":false}"#).unwrap();
        fs::write(path.join("document-open"), b"").unwrap();
        drop(JobFolder(path.clone()));
        assert!(path.exists());
        fs::remove_file(path.join("document-open")).unwrap();
        drop(JobFolder(path.clone()));
        assert!(!path.exists());
    }
    #[test]
    fn cancellation_history_cannot_permanently_fill_the_bridge() {
        let state = FusionBridgeState::default();
        for i in 0..300 {
            mark_canceled(&state, &format!("{i:032x}")).unwrap();
        }
        assert!(register_job(&state, "ffffffffffffffffffffffffffffffff").is_ok());
    }
    #[cfg(unix)]
    #[test]
    fn nonregular_input_is_rejected_without_opening_a_fifo() {
        let job = temp_job("fifo");
        let path = job.0.join("source.f3d");
        assert!(std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap()
            .success());
        assert!(matches!(read_source(&path), Err(AppError::NotReadable)));
    }
    #[test]
    fn results_require_valid_json_and_report_export_failure() {
        let job = temp_job("result");
        fs::write(job.0.join("result.json"), b"bad").unwrap();
        assert!(wait_for_result(&job.0, &AtomicBool::new(false), Duration::ZERO).is_err());
        fs::write(
            job.0.join("result.json"),
            br#"{"ok":false,"error":"No export entitlement"}"#,
        )
        .unwrap();
        assert!(matches!(
            wait_for_result(&job.0, &AtomicBool::new(false), Duration::ZERO),
            Err(AppError::Fusion {
                setup_required: false,
                ..
            })
        ));
        fs::write(job.0.join("result.json"), br#"{"ok":true}"#).unwrap();
        assert!(wait_for_result(&job.0, &AtomicBool::new(false), Duration::ZERO).is_ok());
    }
    #[cfg(unix)]
    #[test]
    fn bridge_directories_and_responses_reject_symlinks() {
        let job = temp_job("symlink");
        std::os::unix::fs::symlink(&job.0, job.0.join("link")).unwrap();
        assert!(private_dir(&job.0.join("link")).is_err());
        std::os::unix::fs::symlink(job.0.join("unused"), job.0.join("result.json")).unwrap();
        assert!(write_owned_file(&job.0.join("result.json"), b"{}").is_err());
    }
}
