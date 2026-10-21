use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::AppError;

/// The file types the sidebar lists and the app can open. Matched on
/// extension only; the parser sniffs the real format when the file is opened.
pub const MODEL_EXTENSIONS: [&str; 5] = ["stl", "3mf", "step", "stp", "f3d"];

fn is_model_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| MODEL_EXTENSIONS.iter().any(|m| e.eq_ignore_ascii_case(m)))
        .unwrap_or(false)
}

fn is_tree_file(path: &Path) -> bool {
    is_model_file(path)
        || path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
}

#[derive(Debug, Clone, Serialize)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

/// Lists `dir`, restricted to descendants of `root`. Both come from the
/// frontend, but `root` is only ever the parent of a file the OS handed us
/// (dialog / drop / Finder / argv) or a folder chosen via the native dialog --
/// so the webview can navigate *within* an OS-blessed root, never outside it.
/// Symlinks are resolved before the containment check so a link out of the
/// root can't escape it.
#[tauri::command]
pub fn list_dir(root: String, dir: String) -> Result<Vec<DirEntry>, AppError> {
    let root = canonical(&root)?;
    let dir = canonical(&dir)?;
    if !dir.starts_with(&root) {
        return Err(AppError::OutsideRoot);
    }

    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .map_err(|_| AppError::NotReadable)?
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let is_dir = path.is_dir();
        if !is_dir && !is_tree_file(&path) {
            continue;
        }
        entries.push(DirEntry {
            name,
            path: path.to_string_lossy().into_owned(),
            is_dir,
        });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

const SEARCH_CAP: usize = 500;

/// Recursive, case-insensitive substring search under `root`, matching:
///   - a directory whose name matches (returned as a dir entry), and
///   - an .stl/.3mf/.step/.stp/.f3d/.zip file whose name matches OR that lives anywhere under a
///     matching directory (so "whistle" finds every file in a whistle/ folder).
/// Symlinked directories are not followed (an escape route and a loop risk);
/// dotfiles are skipped. Sorted by path, capped so a huge tree can't hand the
/// webview an unbounded list.
#[tauri::command]
pub fn search_dir(root: String, query: String) -> Result<Vec<DirEntry>, AppError> {
    let root = canonical(&root)?;
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Ok(vec![]);
    }
    let mut out = Vec::new();
    walk_search(&root, &needle, false, &mut out);
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.path.to_lowercase().cmp(&b.path.to_lowercase()))
    });
    out.truncate(SEARCH_CAP);
    Ok(out)
}

const ZIP_SCAN_CAP: usize = 10_000;
const MODEL_SCAN_CAP: usize = 10_000;

#[derive(Debug, Serialize)]
pub struct ZipArchiveList {
    pub archives: Vec<DirEntry>,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct ModelFileList {
    pub models: Vec<DirEntry>,
    pub truncated: bool,
}

#[tauri::command]
pub async fn list_model_files(root: String) -> Result<ModelFileList, AppError> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = canonical(&root)?;
        let mut models = Vec::new();
        let mut truncated = false;
        walk_model_files(&root, &mut models, &mut truncated);
        models.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()));
        Ok(ModelFileList { models, truncated })
    })
    .await
    .map_err(|_| AppError::NotReadable)?
}

fn walk_model_files(dir: &Path, models: &mut Vec<DirEntry>, truncated: &mut bool) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if *truncated {
            return;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            walk_model_files(&path, models, truncated);
        } else if kind.is_file() && is_model_file(&path) {
            if models.len() == MODEL_SCAN_CAP {
                *truncated = true;
                return;
            }
            models.push(DirEntry {
                name,
                path: path.to_string_lossy().into_owned(),
                is_dir: false,
            });
        }
    }
}

/// Keep the ZIP inventory separate from search results. Run filesystem traversal
/// off the command/UI thread, and report when the bounded inventory is incomplete.
#[tauri::command]
pub async fn list_zip_files(root: String) -> Result<ZipArchiveList, AppError> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = canonical(&root)?;
        let mut archives = Vec::new();
        let mut truncated = false;
        walk_zip_files(&root, &mut archives, &mut truncated);
        archives.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()));
        Ok(ZipArchiveList {
            archives,
            truncated,
        })
    })
    .await
    .map_err(|_| AppError::NotReadable)?
}

fn walk_zip_files(dir: &Path, archives: &mut Vec<DirEntry>, truncated: &mut bool) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if *truncated {
            return;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            walk_zip_files(&path, archives, truncated);
        } else if kind.is_file() && name.to_ascii_lowercase().ends_with(".zip") {
            if archives.len() == ZIP_SCAN_CAP {
                *truncated = true;
                return;
            }
            archives.push(DirEntry {
                name,
                path: path.to_string_lossy().into_owned(),
                is_dir: false,
            });
        }
    }
}

#[tauri::command]
pub fn move_tree_entry(
    root: String,
    path: String,
    destination_directory: String,
    state: tauri::State<'_, crate::folder_watch::FolderWatchState>,
) -> Result<(), String> {
    state.with_selected_root(Path::new(&root), |selected| {
        move_tree_entry_under_root(
            selected,
            Path::new(&root),
            Path::new(&path),
            Path::new(&destination_directory),
        )
    })
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn move_tree_entry_under_root(
    selected: &crate::folder_watch::SelectedRootHandle,
    requested_root: &Path,
    requested_source: &Path,
    requested_destination_directory: &Path,
) -> Result<(), String> {
    move_tree_entry_under_root_with_hook(
        selected,
        requested_root,
        requested_source,
        requested_destination_directory,
        || {},
    )
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn move_tree_entry_under_root(
    _selected: &crate::folder_watch::SelectedRootHandle,
    _requested_root: &Path,
    _requested_source: &Path,
    _requested_destination_directory: &Path,
) -> Result<(), String> {
    Err("Safe file-tree moves are unavailable because handle-relative verified rename is unsupported on this platform".into())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn move_tree_entry_under_root_with_hook<F>(
    selected: &crate::folder_watch::SelectedRootHandle,
    requested_root: &Path,
    requested_source: &Path,
    requested_destination_directory: &Path,
    before_mutation: F,
) -> Result<(), String>
where
    F: FnOnce(),
{
    selected.verify_current()?;
    let root = canonical_mutation_root(selected.path())?;
    let requested_root = requested_root
        .canonicalize()
        .map_err(|_| "The selected folder changed before the move.".to_string())?;
    if requested_root != root {
        return Err("The selected folder changed before the move.".into());
    }

    let source_name = requested_source
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "The source path is invalid.".to_string())?;
    let source_parent = requested_source
        .parent()
        .ok_or_else(|| "The source path is invalid.".to_string())?
        .to_path_buf();
    let source_parent_handle = library_runtime::GuardedDirectoryParent::open(&source_parent)
        .map_err(|_| "Symbolic links cannot be used as source parent folders.".to_string())?;
    let source_parent = source_parent
        .canonicalize()
        .map_err(|_| "The source folder no longer exists or cannot be read.".to_string())?;
    if !source_parent.starts_with(&root) {
        return Err("The source is outside the currently selected folder.".into());
    }
    let source = source_parent.join(source_name);
    let source_metadata = std::fs::symlink_metadata(&source)
        .map_err(|_| "The source no longer exists or cannot be read.".to_string())?;
    let source_type = source_metadata.file_type();
    if source_type.is_symlink() {
        return Err("Symbolic links cannot be moved from the file tree.".into());
    }
    if !source_type.is_dir() && !source_type.is_file() {
        return Err("Only regular files and folders can be moved.".into());
    }
    if source_type.is_file() && !is_tree_file(&source) {
        return Err("Only model and ZIP files shown in the tree can be moved.".into());
    }
    if contains_hidden_component(&root, &source) {
        return Err("Hidden items cannot be moved from the file tree.".into());
    }
    if source == root || !source.starts_with(&root) {
        return Err("The source must be inside the selected folder.".into());
    }

    let destination_link_metadata = std::fs::symlink_metadata(requested_destination_directory)
        .map_err(|_| "The destination folder no longer exists.".to_string())?;
    if destination_link_metadata.file_type().is_symlink() {
        return Err("Symbolic links cannot be used as destination folders.".into());
    }
    if !destination_link_metadata.is_dir() {
        return Err("Choose a folder as the move destination.".into());
    }
    let destination_parent_handle =
        library_runtime::GuardedDirectoryParent::open(requested_destination_directory)
            .map_err(|_| "Symbolic links cannot be used as destination folders.".to_string())?;
    let destination_directory = requested_destination_directory
        .canonicalize()
        .map_err(|_| "The destination folder no longer exists.".to_string())?;
    if !destination_directory.starts_with(&root) {
        return Err("The destination is outside the currently selected folder.".into());
    }
    if contains_hidden_component(&root, &destination_directory) {
        return Err("Hidden folders cannot be used as move destinations.".into());
    }
    if source == destination_directory {
        return Err("A folder cannot be moved into itself.".into());
    }
    if source_type.is_dir() && destination_directory.starts_with(&source) {
        return Err("A folder cannot be moved into one of its own subfolders.".into());
    }
    if source_parent == destination_directory {
        return Ok(());
    }

    let destination_name = source
        .file_name()
        .ok_or_else(|| "The source path is invalid.".to_string())?;
    let destination = destination_directory.join(destination_name);
    match std::fs::symlink_metadata(&destination) {
        Ok(_) => {
            return Err(
                "A file or folder with that name already exists in the destination.".into(),
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("The destination cannot be checked for name conflicts.".into()),
    }

    // Keep the opened parents pinned and check that neither path was replaced
    // before the handle-relative transaction starts. A later path swap cannot
    // redirect the rename through a symlink because the runtime uses the
    // already-open directory handles.
    if !source_parent_handle
        .matches_path(&source_parent)
        .map_err(|_| "The source folder changed before the move.".to_string())?
        || !destination_parent_handle
            .matches_path(&destination_directory)
            .map_err(|_| "The destination folder changed before the move.".to_string())?
    {
        return Err("A folder changed before the move could complete.".into());
    }
    before_mutation();
    selected.verify_current()?;
    if !source_parent_handle
        .matches_path(&source_parent)
        .map_err(|_| "The source folder changed before the move.".to_string())?
        || !destination_parent_handle
            .matches_path(&destination_directory)
            .map_err(|_| "The destination folder changed before the move.".to_string())?
    {
        return Err("A folder changed before the move could complete.".into());
    }
    let result = if source_type.is_file() {
        let (bytes, sha256) = source_parent_handle
            .hash_file(source.file_name().unwrap(), u64::MAX)
            .map_err(|error| format!("Could not verify the source before moving it: {error}"))?;
        source_parent_handle.move_verified_file_to(
            source.file_name().unwrap(),
            &destination_parent_handle,
            destination_name,
            bytes,
            &sha256,
            u64::MAX,
        )
    } else {
        source_parent_handle.move_entry_to(
            source.file_name().unwrap(),
            &destination_parent_handle,
            destination_name,
        )
    };
    match result {
        Ok(moved) => {
            if !source_parent_handle
                .matches_path(&source_parent)
                .map_err(|error| error.to_string())?
                || !destination_parent_handle
                    .matches_path(&destination_directory)
                    .map_err(|error| error.to_string())?
                || selected.verify_current().is_err()
            {
                return Err(format!(
                    "The item moved, but a selected folder changed during the operation. The move journal {} remains beside the original location.",
                    source_parent.join(moved.journal_name).display()
                ));
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err("A file or folder with that name already exists in the destination.".into())
        }
        Err(error) => Err(format!(
            "Could not move the item safely. Any retained bytes remain recoverable through the source-directory journal: {error}"
        )),
    }
}

fn contains_hidden_component(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root).ok().is_some_and(|relative| {
        relative
            .components()
            .any(|component| component.as_os_str().to_string_lossy().starts_with('.'))
    })
}

fn canonical_mutation_root(root: &Path) -> Result<PathBuf, String> {
    let selected_path = root;
    let root = root
        .canonicalize()
        .map_err(|_| "The selected folder no longer exists or cannot be read.".to_string())?;
    if root.as_path() != selected_path {
        return Err("The selected folder changed before the operation.".into());
    }
    if !root.is_dir() {
        return Err("The selected path is no longer a folder.".into());
    }
    if is_filesystem_root(&root) {
        return Err("Choose a folder below a filesystem root before changing files.".into());
    }
    Ok(root)
}

#[cfg(unix)]
fn is_filesystem_root(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    let Some(parent) = path.parent().filter(|parent| *parent != path) else {
        return true;
    };
    match (std::fs::metadata(path), std::fs::metadata(parent)) {
        (Ok(path), Ok(parent)) => path.dev() != parent.dev(),
        _ => true,
    }
}

#[cfg(not(unix))]
fn is_filesystem_root(path: &Path) -> bool {
    path.parent().is_none()
}

/// `ancestor_matched`: some directory above `dir` already matched, so every
/// model file below is a hit regardless of its own name.
fn walk_search(dir: &Path, needle: &str, ancestor_matched: bool, out: &mut Vec<DirEntry>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = entry.file_type() else {
            continue;
        };
        let name_matches = name.to_lowercase().contains(needle);
        if meta.is_dir() {
            if name_matches {
                out.push(DirEntry {
                    name: name.clone(),
                    path: path.to_string_lossy().into_owned(),
                    is_dir: true,
                });
            }
            // file_type() doesn't follow symlinks, so a symlinked dir is skipped here
            walk_search(&path, needle, ancestor_matched || name_matches, out);
        } else if meta.is_file() && is_tree_file(&path) && (name_matches || ancestor_matched) {
            out.push(DirEntry {
                name,
                path: path.to_string_lossy().into_owned(),
                is_dir: false,
            });
        }
    }
}

fn canonical(p: &str) -> Result<PathBuf, AppError> {
    let path = Path::new(p);
    let c = path.canonicalize().map_err(|_| AppError::NotFound)?;
    if !c.is_dir() {
        return Err(AppError::NotReadable);
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gogglelab-tree-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    fn move_for_test(root: &Path, source: &Path, destination: &Path) -> Result<(), String> {
        let state = crate::folder_watch::FolderWatchState::default();
        state.select_root_for_test(root);
        state.with_selected_root(root, |selected| {
            move_tree_entry_under_root(selected, root, source, destination)
        })
    }

    #[test]
    fn move_rejects_replaced_selected_root_before_mutation() {
        let root = tmp("move-replaced-root");
        let state = crate::folder_watch::FolderWatchState::default();
        state.select_root_for_test(&root);
        let retired = root.with_extension("retired");
        let _ = std::fs::remove_dir_all(&retired);
        std::fs::rename(&root, &retired).unwrap();
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("part.stl"), b"replacement").unwrap();
        std::fs::create_dir(root.join("destination")).unwrap();
        let result = state.with_selected_root(&root, |selected| {
            move_tree_entry_under_root(
                selected,
                &root,
                &root.join("part.stl"),
                &root.join("destination"),
            )
        });
        assert!(result.unwrap_err().contains("replaced"));
        assert_eq!(
            std::fs::read(root.join("part.stl")).unwrap(),
            b"replacement"
        );
    }

    #[test]
    fn move_rejects_collision_subtree_and_outside_destination() {
        let root = tmp("move-rejections");
        let destination = root.join("destination");
        let source = root.join("part.stl");
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(&source, b"original").unwrap();
        std::fs::write(destination.join("part.stl"), b"existing").unwrap();
        assert!(move_for_test(&root, &source, &destination)
            .unwrap_err()
            .contains("already exists"));
        assert_eq!(std::fs::read(&source).unwrap(), b"original");
        assert_eq!(
            std::fs::read(destination.join("part.stl")).unwrap(),
            b"existing"
        );
        let nested = destination.join("nested");
        std::fs::create_dir(&nested).unwrap();
        assert!(move_for_test(&root, &destination, &nested)
            .unwrap_err()
            .contains("subfolders"));
        let outside = tmp("move-outside");
        assert!(move_for_test(&root, &source, &outside)
            .unwrap_err()
            .contains("outside"));
    }

    #[cfg(unix)]
    #[test]
    fn move_rejects_symlink_source_and_destination() {
        let root = tmp("move-symlinks");
        let destination = root.join("destination");
        std::fs::create_dir(&destination).unwrap();
        let source = root.join("part.stl");
        std::fs::write(&source, b"original").unwrap();
        let link = root.join("link.stl");
        std::os::unix::fs::symlink(&source, &link).unwrap();
        assert!(move_for_test(&root, &link, &destination)
            .unwrap_err()
            .contains("Symbolic links"));
        let linked_dir = root.join("linked-dir");
        std::os::unix::fs::symlink(&destination, &linked_dir).unwrap();
        assert!(move_for_test(&root, &source, &linked_dir)
            .unwrap_err()
            .contains("Symbolic links"));
    }

    #[cfg(unix)]
    #[test]
    fn move_rejects_destination_parent_swapped_to_outside_symlink_before_commit() {
        let root = tmp("move-parent-race-root");
        let outside = tmp("move-parent-race-outside");
        let destination = root.join("destination");
        let displaced_destination = root.join("destination-held");
        let source = root.join("part.stl");
        let outside_part = outside.join("part.stl");
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(&source, b"source bytes").unwrap();
        std::fs::write(&outside_part, b"outside bytes").unwrap();

        let state = crate::folder_watch::FolderWatchState::default();
        state.select_root_for_test(&root);
        let result = state.with_selected_root(&root, |selected| {
            move_tree_entry_under_root_with_hook(selected, &root, &source, &destination, || {
                std::fs::rename(&destination, &displaced_destination).unwrap();
                std::os::unix::fs::symlink(&outside, &destination).unwrap();
            })
        });

        assert!(result.unwrap_err().contains("destination folder changed"));
        assert_eq!(std::fs::read(&source).unwrap(), b"source bytes");
        assert_eq!(std::fs::read(&outside_part).unwrap(), b"outside bytes");
        assert!(!displaced_destination.join("part.stl").exists());
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(outside).unwrap();
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn moves_unicode_file_and_folder_without_overwriting() {
        let root = tmp("move-success");
        let destination = root.join("destination space");
        let source = root.join("pièce.stl");
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(&source, b"mesh").unwrap();
        move_for_test(&root, &source, &destination).unwrap();
        assert!(!source.exists());
        assert_eq!(
            std::fs::read(destination.join("pièce.stl")).unwrap(),
            b"mesh"
        );
        let folder = root.join("folder");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("model.stl"), b"nested").unwrap();
        move_for_test(&root, &folder, &destination).unwrap();
        assert_eq!(
            std::fs::read(destination.join("folder/model.stl")).unwrap(),
            b"nested"
        );
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    #[test]
    fn moves_fail_closed_without_handle_relative_support() {
        let root = tmp("unsupported-platform-move");
        let destination = root.join("destination");
        let source = root.join("part.stl");
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(&source, b"original").unwrap();

        let error = move_for_test(&root, &source, &destination).unwrap_err();
        assert!(error.contains("handle-relative verified rename"));
        assert_eq!(std::fs::read(&source).unwrap(), b"original");
        assert!(!destination.join("part.stl").exists());
    }

    #[test]
    fn lists_dirs_first_then_supported_files_case_insensitively_sorted() {
        let root = tmp("sort");
        std::fs::create_dir(root.join("zeta")).unwrap();
        std::fs::write(root.join("B.stl"), b"").unwrap();
        std::fs::write(root.join("a.STL"), b"").unwrap();
        std::fs::write(root.join("Cover.3mf"), b"").unwrap();
        std::fs::write(root.join("gear.step"), b"").unwrap();
        std::fs::write(root.join("housing.STP"), b"").unwrap();
        std::fs::write(root.join("prototype.F3D"), b"").unwrap();
        std::fs::write(root.join("Kit.ZIP"), b"").unwrap();
        std::fs::write(root.join("notes.txt"), b"").unwrap();
        std::fs::write(root.join("model.obj"), b"").unwrap();
        std::fs::write(root.join(".hidden.stl"), b"").unwrap();

        let e = list_dir(root.to_string_lossy().into(), root.to_string_lossy().into()).unwrap();
        let names: Vec<_> = e.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "zeta",
                "a.STL",
                "B.stl",
                "Cover.3mf",
                "gear.step",
                "housing.STP",
                "Kit.ZIP",
                "prototype.F3D"
            ]
        );
    }

    #[test]
    fn zip_inventory_finds_archives_inside_collapsed_directories() {
        let root = tmp("zips");
        let nested = root.join("parts");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(root.join("kit.zip"), b"").unwrap();
        std::fs::write(nested.join("nested.ZIP"), b"").unwrap();
        std::fs::write(nested.join("not-zip.stl"), b"").unwrap();
        let mut archives = Vec::new();
        let mut truncated = false;
        walk_zip_files(&root, &mut archives, &mut truncated);
        assert!(!truncated);
        let mut names: Vec<_> = archives.into_iter().map(|entry| entry.name).collect();
        names.sort();
        assert_eq!(names, ["kit.zip", "nested.ZIP"]);
    }

    #[test]
    fn search_is_recursive_case_insensitive_and_skips_symlinked_dirs() {
        let root = tmp("search");
        let outside = tmp("search-out");
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/b/Bracket_v2.STL"), b"").unwrap();
        std::fs::write(root.join("bracket-old.stl"), b"").unwrap();
        std::fs::write(root.join("bracket-set.3mf"), b"").unwrap();
        std::fs::write(root.join("bracket-source.F3D"), b"").unwrap();
        std::fs::write(root.join("bracket-source.zip"), b"").unwrap();
        std::fs::write(root.join("cup.stl"), b"").unwrap();
        std::fs::write(outside.join("bracket-escaped.stl"), b"").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();

        let r = search_dir(root.to_string_lossy().into(), "BRACKET".into()).unwrap();
        let names: Vec<_> = r.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "Bracket_v2.STL",
                "bracket-old.stl",
                "bracket-set.3mf",
                "bracket-source.F3D",
                "bracket-source.zip"
            ]
        );
    }

    #[test]
    fn search_matches_folder_names_and_files_under_matching_folders() {
        let root = tmp("search-dirs");
        std::fs::create_dir_all(root.join("Whistle Kit/parts")).unwrap();
        std::fs::write(root.join("Whistle Kit/parts/body.stl"), b"").unwrap();
        std::fs::write(root.join("Whistle Kit/cap.stl"), b"").unwrap();
        std::fs::write(root.join("unrelated.stl"), b"").unwrap();

        let r = search_dir(root.to_string_lossy().into(), "whistle".into()).unwrap();
        let hits: Vec<(bool, &str)> = r.iter().map(|e| (e.is_dir, e.name.as_str())).collect();
        // The folder itself first, then every .stl under it -- not the unrelated file.
        assert_eq!(
            hits,
            vec![
                (true, "Whistle Kit"),
                (false, "cap.stl"),
                (false, "body.stl")
            ]
        );
    }

    #[test]
    fn refuses_to_list_outside_the_root() {
        let root = tmp("root");
        let other = tmp("other");
        let r = list_dir(
            root.to_string_lossy().into(),
            other.to_string_lossy().into(),
        );
        assert!(matches!(r, Err(AppError::OutsideRoot)));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_out_of_root_cannot_escape() {
        let root = tmp("symroot");
        let outside = tmp("symout");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        let r = list_dir(
            root.to_string_lossy().into(),
            root.join("link").to_string_lossy().into(),
        );
        assert!(matches!(r, Err(AppError::OutsideRoot)));
    }
}
