use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::types::SlicerPaletteLayout;

/// Definition for a supported slicer. Matched by bundle identifier, never by
/// app or file name -- the machine this was developed on has
/// `/Applications/Orca.app`, which is Stably's unrelated agent app, not
/// OrcaSlicer. Extend this table, not the matching logic, to support more
/// slicers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SlicerDefinition {
    pub(crate) bundle_id: &'static str,
    pub(crate) display_name: &'static str,
    pub(crate) support_dir: &'static str,
    pub(crate) config_name: &'static str,
    pub(crate) palette_layout: SlicerPaletteLayout,
}

const SLICER_DEFINITIONS: &[SlicerDefinition] = &[
    SlicerDefinition {
        bundle_id: "com.bambulab.bambu-studio",
        display_name: "Bambu Studio",
        support_dir: "BambuStudio",
        config_name: "BambuStudio.conf",
        palette_layout: SlicerPaletteLayout::Bambu,
    },
    SlicerDefinition {
        bundle_id: "com.elegoo3d.elegoo-slicer",
        display_name: "ElegooSlicer",
        support_dir: "ElegooSlicer",
        config_name: "ElegooSlicer.conf",
        palette_layout: SlicerPaletteLayout::Elegoo,
    },
];

pub(crate) fn slicer_definitions() -> &'static [SlicerDefinition] {
    SLICER_DEFINITIONS
}

pub(crate) fn slicer_definition(bundle_id: &str) -> Option<&'static SlicerDefinition> {
    SLICER_DEFINITIONS
        .iter()
        .find(|definition| definition.bundle_id == bundle_id)
}

#[derive(Debug, Clone, Serialize)]
pub struct SlicerApp {
    pub id: String, // the bundle identifier itself -- already a stable, unique handle
    pub bundle_id: String,
    pub display_name: String,
    pub app_path: String,
}

/// Scans the standard application directories (non-recursive -- installers
/// place .app bundles directly here, not nested) and returns every installed
/// app whose `CFBundleIdentifier` matches the slicer definition table, in
/// table order. A slicer installed in a non-standard location won't be found;
/// that's an accepted v1 limitation, not a bug (spec's non-goals: no
/// arbitrary-path configuration in v1).
#[cfg(target_os = "macos")]
#[tauri::command]
pub fn detect_slicers() -> Vec<SlicerApp> {
    let mut scan_dirs = vec![PathBuf::from("/Applications")];
    if let Some(home) = std::env::var_os("HOME") {
        scan_dirs.push(PathBuf::from(home).join("Applications"));
    }

    let mut found = Vec::new();
    for dir in scan_dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("app") {
                continue;
            }
            if let Some(bundle_id) = read_bundle_identifier(&path) {
                if let Some(definition) = slicer_definition(&bundle_id) {
                    found.push(SlicerApp {
                        id: bundle_id.clone(),
                        bundle_id,
                        display_name: definition.display_name.to_string(),
                        app_path: path.to_string_lossy().into_owned(),
                    });
                }
            }
        }
    }

    // Stable, predictable order for the UI regardless of filesystem iteration
    // order, which is unspecified by read_dir.
    found.sort_by(|a, b| {
        let ai = slicer_definitions()
            .iter()
            .position(|definition| definition.bundle_id == a.bundle_id);
        let bi = slicer_definitions()
            .iter()
            .position(|definition| definition.bundle_id == b.bundle_id);
        ai.cmp(&bi)
    });
    found
}

/// Discover only explicit installation candidates; never return a .desktop
/// launcher, arbitrary PATH entry, or directory as an executable.
#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub fn detect_slicers() -> Vec<SlicerApp> {
    slicer_definitions()
        .iter()
        .filter_map(|definition| {
            platform_candidates(definition)
                .into_iter()
                .find_map(|(candidate, app_image)| {
                    let path = validated_executable(&candidate, app_image)?;
                    Some(SlicerApp {
                        id: definition.bundle_id.to_owned(),
                        bundle_id: definition.bundle_id.to_owned(),
                        display_name: definition.display_name.to_owned(),
                        app_path: path.to_string_lossy().into_owned(),
                    })
                })
        })
        .collect()
}

#[cfg(windows)]
fn platform_candidates(definition: &SlicerDefinition) -> Vec<(PathBuf, bool)> {
    let (folder, executable) = match definition.bundle_id {
        "com.bambulab.bambu-studio" => ("Bambu Studio", "bambu-studio.exe"),
        "com.elegoo3d.elegoo-slicer" => ("ElegooSlicer", "elegoo-slicer.exe"),
        _ => return vec![],
    };
    let mut candidates = Vec::new();
    for key in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(root) = std::env::var_os(key) {
            candidates.push((PathBuf::from(root).join(folder).join(executable), false));
        }
    }
    if let Some(root) = std::env::var_os("LOCALAPPDATA") {
        candidates.push((
            PathBuf::from(root)
                .join("Programs")
                .join(folder)
                .join(executable),
            false,
        ));
    }
    candidates
}

#[cfg(target_os = "linux")]
fn platform_candidates(definition: &SlicerDefinition) -> Vec<(PathBuf, bool)> {
    let (folder, executable, app_images) = match definition.bundle_id {
        "com.bambulab.bambu-studio" => (
            "BambuStudio",
            "bambu-studio",
            &["Bambu_Studio.AppImage", "BambuStudio.AppImage"][..],
        ),
        "com.elegoo3d.elegoo-slicer" => (
            "ElegooSlicer",
            "elegoo-slicer",
            &["ElegooSlicer.AppImage", "ELEGOOSlicer.AppImage"][..],
        ),
        _ => return vec![],
    };
    let mut candidates = vec![
        (PathBuf::from("/usr/bin").join(executable), false),
        (PathBuf::from("/usr/local/bin").join(executable), false),
        (PathBuf::from("/opt").join(folder).join(executable), false),
        (
            PathBuf::from("/opt")
                .join(folder)
                .join("bin")
                .join(executable),
            false,
        ),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        let directory = PathBuf::from(home).join("Applications");
        candidates.extend(app_images.iter().map(|name| (directory.join(name), true)));
    }
    candidates
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn platform_candidates(_: &SlicerDefinition) -> Vec<(PathBuf, bool)> {
    vec![]
}

#[cfg(any(not(target_os = "macos"), test))]
fn validated_executable(candidate: &Path, app_image: bool) -> Option<PathBuf> {
    use std::io::Read;

    let metadata = std::fs::symlink_metadata(candidate).ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return None;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return None;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return None;
        }
    }
    if app_image {
        let mut magic = [0u8; 11];
        std::fs::File::open(candidate)
            .ok()?
            .read_exact(&mut magic)
            .ok()?;
        if magic[..4] != *b"\x7fELF" || (magic[8..11] != *b"AI\x01" && magic[8..11] != *b"AI\x02") {
            return None;
        }
    }
    candidate.canonicalize().ok()
}

pub(crate) fn slicer_support_directory(definition: &SlicerDefinition) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let root = PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support");
    #[cfg(windows)]
    let root = PathBuf::from(std::env::var_os("APPDATA")?);
    #[cfg(target_os = "linux")]
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".config")))?;
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    return None;
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    Some(root.join(definition.support_dir))
}

#[cfg(any(target_os = "macos", test))]
fn read_bundle_identifier(app_path: &Path) -> Option<String> {
    let plist_path = app_path.join("Contents/Info.plist");
    let value = plist::Value::from_file(plist_path).ok()?;
    value
        .as_dictionary()?
        .get("CFBundleIdentifier")?
        .as_string()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Builds a fake .app bundle at `dir/<name>.app` with the given
    /// CFBundleIdentifier, for testing detection without touching the real
    /// /Applications.
    fn fake_app(dir: &Path, name: &str, bundle_id: &str) {
        let contents = dir.join(format!("{name}.app")).join("Contents");
        std::fs::create_dir_all(&contents).unwrap();
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>{bundle_id}</string>
</dict>
</plist>"#
        );
        let mut f = std::fs::File::create(contents.join("Info.plist")).unwrap();
        f.write_all(plist.as_bytes()).unwrap();
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "gogglelab-slicer-test-{}-{}",
            std::process::id(),
            name
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn read_bundle_identifier_reads_a_real_plist() {
        let dir = temp_dir("read-id");
        fake_app(&dir, "Bambu Studio", "com.bambulab.bambu-studio");
        let id = read_bundle_identifier(&dir.join("Bambu Studio.app"));
        assert_eq!(id.as_deref(), Some("com.bambulab.bambu-studio"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_bundle_identifier_returns_none_for_missing_plist() {
        let dir = temp_dir("missing-plist");
        std::fs::create_dir_all(dir.join("NotReally.app/Contents")).unwrap();
        assert_eq!(read_bundle_identifier(&dir.join("NotReally.app")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn known_slicers_never_matched_by_app_or_file_name() {
        // The exact trap from this project's own dev machine: an app named
        // "Orca.app" that is unrelated to OrcaSlicer. Detection must go by
        // bundle id alone -- a differently-named app with a matching id
        // should match, and a matching-named app with a different id must not.
        let dir = temp_dir("name-trap");
        fake_app(&dir, "Orca", "com.stablyai.orca"); // real machine's actual Orca.app
        fake_app(&dir, "TotallyDifferentName", "com.bambulab.bambu-studio");

        let mut found = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("app") {
                if let Some(id) = read_bundle_identifier(&path) {
                    if slicer_definition(&id).is_some() {
                        found.push(id);
                    }
                }
            }
        }
        assert_eq!(found, vec!["com.bambulab.bambu-studio".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn definitions_cover_each_supported_slicer_configuration() {
        let bambu = slicer_definition("com.bambulab.bambu-studio").unwrap();
        assert_eq!(bambu.display_name, "Bambu Studio");
        assert_eq!(bambu.support_dir, "BambuStudio");
        assert_eq!(bambu.config_name, "BambuStudio.conf");
        assert_eq!(bambu.palette_layout, SlicerPaletteLayout::Bambu);

        let elegoo = slicer_definition("com.elegoo3d.elegoo-slicer").unwrap();
        assert_eq!(elegoo.display_name, "ElegooSlicer");
        assert_eq!(elegoo.support_dir, "ElegooSlicer");
        assert_eq!(elegoo.config_name, "ElegooSlicer.conf");
        assert_eq!(elegoo.palette_layout, SlicerPaletteLayout::Elegoo);
    }

    #[cfg(unix)]
    #[test]
    fn executable_candidate_rejects_symlinks_before_canonicalizing() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let dir = temp_dir("linked-executable");
        let executable = dir.join("BambuStudio.AppImage");
        std::fs::write(&executable, b"\x7fELFxxxxAI\x02payload").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let linked = dir.join("linked.AppImage");
        symlink(&executable, &linked).unwrap();

        assert!(validated_executable(&executable, true).is_some());
        assert!(validated_executable(&linked, true).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
