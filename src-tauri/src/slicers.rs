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
}
