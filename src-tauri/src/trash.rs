//! Move a supported file or visible folder from the native file tree to Trash.
//!
//! The latest canonical folder selection supplies the authorization root.
//! The webview only supplies the target path, and cannot widen the root for
//! an individual request. Selection remains valid if live watching fails.

use std::fs;
use std::path::Path;

use crate::folder_watch::FolderWatchState;

const TRASHABLE_EXTENSIONS: [&str; 6] = ["stl", "3mf", "step", "stp", "f3d", "zip"];

#[tauri::command]
pub fn move_tree_file_to_trash(
    path: String,
    state: tauri::State<'_, FolderWatchState>,
) -> Result<(), String> {
    let root = state.selected_root().ok_or_else(|| {
        "No folder is currently selected. Select a folder before moving an item to Trash."
            .to_string()
    })?;
    state.with_selected_root(&root, |selected| {
        selected.verify_current()?;
        move_to_trash_under_root(selected.path(), Path::new(&path), |target| {
            selected.verify_current()?;
            trash_file(target)
        })
    })
}

/// ZIP-only primitive for extensions; the caller supplies the native selected-root handle.
pub(crate) fn move_zip_to_trash(
    selected: &crate::SelectedRootHandle,
    requested: &Path,
) -> Result<(), String> {
    validate_zip_source(requested)?;
    selected.verify_current()?;
    move_to_trash_under_root(selected.path(), requested, |target| {
        selected.verify_current()?;
        validate_zip_source(target)?;
        trash_file(target)
    })
}

fn validate_zip_source(path: &Path) -> Result<(), String> {
    if !path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
    {
        return Err("Automatic archive cleanup only accepts ZIP files.".into());
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| "The ZIP archive is no longer available.".to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Automatic archive cleanup requires a regular ZIP file.".into());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn trash_file(path: &Path) -> Result<(), String> {
    use trash::macos::{DeleteMethod, TrashContextExtMacos};

    // NSFileManager uses the native Trash API directly and does not require
    // Finder automation permission.
    let mut context = trash::TrashContext::new();
    context.set_delete_method(DeleteMethod::NsFileManager);
    context
        .delete(path)
        .map_err(|error| format!("Could not move the item to Trash: {error}"))
}

#[cfg(not(target_os = "macos"))]
fn trash_file(path: &Path) -> Result<(), String> {
    trash::delete(path).map_err(|error| format!("Could not move the item to Trash: {error}"))
}

fn move_to_trash_under_root<F>(root: &Path, requested: &Path, trash_file: F) -> Result<(), String>
where
    F: FnOnce(&Path) -> Result<(), String>,
{
    let root = root
        .canonicalize()
        .map_err(|_| "The selected folder no longer exists or cannot be read.".to_string())?;
    if !root.is_dir() {
        return Err("The selected path is no longer a folder.".into());
    }

    let file_name = requested
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "The requested item path is invalid.".to_string())?;
    if matches!(file_name.to_str(), Some("." | "..")) {
        return Err("The selected folder itself cannot be moved to Trash.".into());
    }

    // Canonicalize the parent, not the target. Canonicalizing the target first
    // would follow a final symlink before we had a chance to reject it.
    let parent = requested
        .parent()
        .ok_or_else(|| "The requested item path is invalid.".to_string())?
        .canonicalize()
        .map_err(|_| "The item's parent folder no longer exists or cannot be read.".to_string())?;
    if !parent.starts_with(&root) {
        return Err("The item is outside the currently selected folder.".into());
    }
    let target = parent.join(file_name);
    if target == root {
        return Err("The selected folder itself cannot be moved to Trash.".into());
    }
    let relative = target
        .strip_prefix(&root)
        .map_err(|_| "The item is outside the currently selected folder.".to_string())?;
    if relative
        .components()
        .any(|component| component.as_os_str().to_string_lossy().starts_with('.'))
    {
        return Err("Hidden items cannot be moved from the file tree.".into());
    }
    validate_trashable_entry(&target)?;

    // Re-resolve the parent and repeat the lstat immediately before crossing
    // into the OS Trash API. NSFileManager only accepts a pathname, so a local
    // process with write access to this directory could still swap that final
    // pathname after this check. Renaming to a hidden staging name does not
    // close that last race and would break the original Trash name and Finder
    // recovery context, so preserving the user's filename is preferred here.
    let rechecked_parent = target
        .parent()
        .expect("target was constructed from a parent and file name")
        .canonicalize()
        .map_err(|_| "The item's parent folder changed before it could be moved.".to_string())?;
    if rechecked_parent != parent || !rechecked_parent.starts_with(&root) {
        return Err("The item's parent folder changed before it could be moved.".into());
    }
    let rechecked_target = rechecked_parent.join(file_name);
    validate_trashable_entry(&rechecked_target)?;

    trash_file(&rechecked_target)
}

fn has_allowed_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            TRASHABLE_EXTENSIONS
                .iter()
                .any(|allowed| extension.eq_ignore_ascii_case(allowed))
        })
}

fn validate_trashable_entry(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| "The item no longer exists or cannot be read.".to_string())?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return Err("Symbolic links cannot be moved from the file tree.".into());
    }
    if file_type.is_dir() {
        return Ok(());
    }
    if file_type.is_file() && has_allowed_extension(path) {
        return Ok(());
    }
    if file_type.is_file() {
        return Err(
            "Only STL, 3MF, STEP, STP, F3D, ZIP files, and folders can be moved to Trash.".into(),
        );
    }
    Err("Only regular files and folders can be moved to Trash.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_ID: AtomicU64 = AtomicU64::new(0);

    fn temp(name: &str) -> PathBuf {
        let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "gogglelab-trash-{}-{id}-{name}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        root.canonicalize().unwrap()
    }

    fn should_not_trash(_: &Path) -> Result<(), String> {
        panic!("rejected input must not reach the Trash API")
    }

    #[test]
    fn automatic_cleanup_rejects_non_zip_and_outside_selection_without_trashing() {
        let root = temp("zip-cleanup-root");
        let outside = temp("zip-cleanup-outside");
        let watch = FolderWatchState::default();
        watch.select_root_for_test(&root);
        let non_zip = root.join("model.stl");
        fs::write(&non_zip, b"source").unwrap();
        let wrong_root_zip = outside.join("archive.zip");
        fs::write(&wrong_root_zip, b"source").unwrap();
        let directory = root.join("folder.zip");
        fs::create_dir(&directory).unwrap();
        for path in [&non_zip, &wrong_root_zip, &directory] {
            assert!(watch
                .with_selected_root(&root, |selected| selected.move_zip_to_trash(path))
                .is_err());
            assert!(path.exists());
        }
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn allows_supported_regular_files_inside_the_active_root() {
        let root = temp("allowed");
        let nested = root.join("parts");
        fs::create_dir(&nested).unwrap();
        for name in [
            "part.stl",
            "plate.3MF",
            "source.step",
            "source.STP",
            "design.F3D",
            "kit.zip",
        ] {
            let path = nested.join(name);
            fs::write(&path, b"test").unwrap();
            let mut observed = None;
            move_to_trash_under_root(&root, &path, |validated| {
                observed = Some(validated.to_path_buf());
                Ok(())
            })
            .unwrap();
            assert_eq!(observed.as_deref(), Some(path.as_path()));
        }
    }

    #[test]
    fn rejects_files_outside_the_active_root() {
        let root = temp("inside");
        let outside = temp("outside");
        let path = outside.join("part.stl");
        fs::write(&path, b"test").unwrap();
        let error = move_to_trash_under_root(&root, &path, should_not_trash).unwrap_err();
        assert!(error.contains("outside"));
        assert!(path.exists());
    }

    #[test]
    fn allows_visible_directories_and_rejects_unsupported_files() {
        let root = temp("types");
        let directory = root.join("folder.stl");
        fs::create_dir(&directory).unwrap();
        let mut seen = None;
        move_to_trash_under_root(&root, &directory, |path| {
            seen = Some(path.to_path_buf());
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, Some(directory));

        let text = root.join("notes.txt");
        fs::write(&text, b"test").unwrap();
        let error = move_to_trash_under_root(&root, &text, should_not_trash).unwrap_err();
        assert!(error.contains("Only STL"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_final_symlinks_even_when_the_target_is_inside_the_root() {
        let root = temp("symlink");
        let target = root.join("real.stl");
        let link = root.join("link.stl");
        fs::write(&target, b"test").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let error = move_to_trash_under_root(&root, &link, should_not_trash).unwrap_err();
        assert!(error.contains("Symbolic links"));
        assert!(link.symlink_metadata().is_ok());
        assert!(target.exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "moves a real disposable file to macOS Trash; run explicitly when validating OS integration"]
    fn moves_a_benign_temp_file_to_the_macos_trash() {
        let root = temp("real-trash");
        let path = root.join(format!(
            "gogglelab-trash-test-{}.stl",
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, b"safe disposable test file").unwrap();

        eprintln!(
            "moving disposable test file to macOS Trash from {}; remove it from Trash manually after this opt-in smoke test",
            path.display(),
        );
        move_to_trash_under_root(&root, &path, trash_file).unwrap();

        assert!(!path.exists());
    }
}
