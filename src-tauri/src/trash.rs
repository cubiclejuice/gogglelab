//! Move a supported file or visible folder from the native file tree into an
//! app-managed, same-directory quarantine. The library-runtime hold journal
//! makes the move recoverable without handing a pathname to the OS Trash API.

use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::folder_watch::FolderWatchState;

const TRASHABLE_EXTENSIONS: [&str; 6] = ["stl", "3mf", "step", "stp", "f3d", "zip"];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTreeTrashReceipt {
    pub original_path: String,
    pub recovery_path: String,
    pub journal_path: String,
    pub bytes: Option<u64>,
    pub sha256: Option<String>,
}

#[tauri::command]
pub fn move_tree_file_to_trash(
    path: String,
    state: tauri::State<'_, FolderWatchState>,
) -> Result<FileTreeTrashReceipt, String> {
    let root = state.selected_root().ok_or_else(|| {
        "No folder is currently selected. Select a folder before moving an item to Trash."
            .to_string()
    })?;
    state.with_selected_root(&root, |selected| {
        move_to_quarantine_under_root(selected, Path::new(&path), false)
    })
}

/// ZIP-only primitive for extension cleanup; the caller supplies the native
/// selected-root handle. The resulting held source and recovery journal stay
/// beside the archive so the move never crosses filesystems.
pub(crate) fn move_zip_to_trash(
    selected: &crate::SelectedRootHandle,
    requested: &Path,
) -> Result<(), String> {
    move_to_quarantine_under_root(selected, requested, true).map(|_| ())
}

fn move_to_quarantine_under_root(
    selected: &crate::SelectedRootHandle,
    requested: &Path,
    zip_only: bool,
) -> Result<FileTreeTrashReceipt, String> {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (selected, requested, zip_only);
        return Err("File-tree Trash is unavailable because handle-relative quarantine is unsupported on this platform".into());
    }

    selected.verify_current()?;
    let root = selected.path();
    let file_name = requested
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "The requested item path is invalid.".to_string())?;
    if matches!(file_name.to_str(), Some("." | "..")) {
        return Err("The selected folder itself cannot be moved to Trash.".into());
    }

    // Canonicalize the parent, never the final component. The guarded parent
    // opens each path component without following symlinks and remains pinned
    // while the entry is moved into quarantine.
    let parent_path = requested
        .parent()
        .ok_or_else(|| "The requested item path is invalid.".to_string())?
        .canonicalize()
        .map_err(|_| "The item's parent folder no longer exists or cannot be read.".to_string())?;
    if !parent_path.starts_with(root) {
        return Err("The item is outside the currently selected folder.".into());
    }
    let target = parent_path.join(file_name);
    let relative = target
        .strip_prefix(root)
        .map_err(|_| "The item is outside the currently selected folder.".to_string())?;
    if target == root {
        return Err("The selected folder itself cannot be moved to Trash.".into());
    }
    if relative
        .components()
        .any(|component| component.as_os_str().to_string_lossy().starts_with('.'))
    {
        return Err("Hidden items cannot be moved from the file tree.".into());
    }

    if zip_only {
        validate_zip_source(&target)?;
    } else {
        validate_trashable_entry(&target)?;
    }

    selected.verify_current()?;
    let parent = library_runtime::GuardedDirectoryParent::open(&parent_path)
        .map_err(|error| format!("Could not open the item's parent safely: {error}"))?;
    if !parent
        .matches_path(&parent_path)
        .map_err(|error| error.to_string())?
    {
        return Err("The item's parent folder changed before it could be quarantined.".into());
    }

    let target_metadata = fs::symlink_metadata(&target)
        .map_err(|_| "The item no longer exists or cannot be read.".to_string())?;
    if target_metadata.file_type().is_symlink() {
        return Err("Symbolic links cannot be moved from the file tree.".into());
    }
    if target_metadata.is_file() {
        let (bytes, sha256) = parent
            .hash_file(file_name, u64::MAX)
            .map_err(|error| format!("Could not verify the file before quarantine: {error}"))?;
        let held = parent
            .hold_verified_file(file_name, bytes, &sha256, u64::MAX)
            .map_err(|error| {
                format!(
                    "Could not quarantine the file safely. No source bytes were automatically removed: {error}"
                )
            })?;
        let recovery_path = parent_path.join(&held.hold_name);
        let journal_path = parent_path.join(&held.journal_name);
        if !parent
            .matches_path(&parent_path)
            .map_err(|error| error.to_string())?
            || selected.verify_current().is_err()
        {
            return Err(format!(
                "The file was quarantined, but its parent changed during the operation. Recovery data remains at {} with journal {}.",
                recovery_path.display(),
                journal_path.display()
            ));
        }
        return Ok(FileTreeTrashReceipt {
            original_path: target.to_string_lossy().into_owned(),
            recovery_path: recovery_path.to_string_lossy().into_owned(),
            journal_path: journal_path.to_string_lossy().into_owned(),
            bytes: Some(bytes),
            sha256: Some(sha256),
        });
    } else if target_metadata.is_dir() {
        if zip_only {
            return Err("Automatic archive cleanup requires a regular ZIP file.".into());
        }
        let held = parent.hold_entry(file_name).map_err(|error| {
            format!(
                "Could not quarantine the folder safely. No source bytes were automatically removed: {error}"
            )
        })?;
        let recovery_path = parent_path.join(&held.hold_name);
        let journal_path = parent_path.join(&held.journal_name);
        if !parent
            .matches_path(&parent_path)
            .map_err(|error| error.to_string())?
            || selected.verify_current().is_err()
        {
            return Err(format!(
                "The folder was quarantined, but its parent changed during the operation. Recovery data remains at {} with journal {}.",
                recovery_path.display(),
                journal_path.display()
            ));
        }
        return Ok(FileTreeTrashReceipt {
            original_path: target.to_string_lossy().into_owned(),
            recovery_path: recovery_path.to_string_lossy().into_owned(),
            journal_path: journal_path.to_string_lossy().into_owned(),
            bytes: None,
            sha256: None,
        });
    } else {
        return Err("Only regular files and folders can be moved to Trash.".into());
    }
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
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root.canonicalize().unwrap()
    }

    fn with_selected_root<T>(
        root: &Path,
        action: impl FnOnce(&crate::SelectedRootHandle) -> T,
    ) -> T {
        let state = FolderWatchState::default();
        state.select_root_for_test(root);
        state
            .with_selected_root(root, |selected| Ok(action(selected)))
            .unwrap()
    }

    #[test]
    fn automatic_cleanup_rejects_non_zip_and_outside_selection() {
        let root = temp("zip-cleanup-root");
        let outside = temp("zip-cleanup-outside");
        let non_zip = root.join("model.stl");
        fs::write(&non_zip, b"source").unwrap();
        let wrong_root_zip = outside.join("archive.zip");
        fs::write(&wrong_root_zip, b"source").unwrap();
        let directory = root.join("folder.zip");
        fs::create_dir(&directory).unwrap();
        with_selected_root(&root, |selected| {
            for path in [&non_zip, &wrong_root_zip, &directory] {
                assert!(move_zip_to_trash(selected, path).is_err());
                assert!(path.exists());
            }
        });
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    #[test]
    fn file_tree_trash_fails_closed_without_handle_relative_support() {
        let root = temp("unsupported-platform");
        let path = root.join("part.stl");
        fs::write(&path, b"synthetic source").unwrap();
        with_selected_root(&root, |selected| {
            let error = move_to_quarantine_under_root(selected, &path, false).unwrap_err();
            assert!(error.contains("handle-relative quarantine"));
            assert_eq!(fs::read(path).unwrap(), b"synthetic source");
        });
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn supported_files_are_moved_to_a_verified_app_managed_hold() {
        let root = temp("supported-files");
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
            let receipt = with_selected_root(&root, |selected| {
                move_to_quarantine_under_root(selected, &path, false).unwrap()
            });
            assert!(!path.exists());
            assert_eq!(fs::read(&receipt.recovery_path).unwrap(), b"test");
            assert!(Path::new(&receipt.journal_path).is_file());
            assert_eq!(receipt.bytes, Some(4));
            assert!(receipt.sha256.is_some());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn visible_directories_are_held_and_unsupported_files_are_rejected() {
        let root = temp("types");
        let directory = root.join("folder.stl");
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("model.stl"), b"folder payload").unwrap();
        let receipt = with_selected_root(&root, |selected| {
            move_to_quarantine_under_root(selected, &directory, false).unwrap()
        });
        assert!(!directory.exists());
        assert_eq!(
            fs::read(Path::new(&receipt.recovery_path).join("model.stl")).unwrap(),
            b"folder payload"
        );
        assert!(Path::new(&receipt.journal_path).is_file());
        assert_eq!(receipt.bytes, None);

        let text = root.join("notes.txt");
        fs::write(&text, b"test").unwrap();
        let error = with_selected_root(&root, |selected| {
            move_to_quarantine_under_root(selected, &text, false).unwrap_err()
        });
        assert!(error.contains("Only STL"));
        assert_eq!(fs::read(text).unwrap(), b"test");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_final_symlinks_even_when_the_target_is_inside_the_root() {
        let root = temp("symlink");
        let target = root.join("real.stl");
        let link = root.join("link.stl");
        fs::write(&target, b"test").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let error = with_selected_root(&root, |selected| {
            move_to_quarantine_under_root(selected, &link, false).unwrap_err()
        });
        assert!(error.contains("Symbolic links"));
        assert!(link.symlink_metadata().is_ok());
        assert_eq!(fs::read(target).unwrap(), b"test");
        fs::remove_dir_all(root).unwrap();
    }
}
