use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::{fs, io::Write, path::Path};

use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::slicer_profiles::{build_profile_collection, SlicerProfile, SlicerProfileDraft};

static SETTINGS_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Persisted app settings (spec §4). Bed values are placeholders pending
/// confirmation of the actual machines (assumption #5, docs/PLAN.md) --
/// data, not code, specifically so correcting them never touches a rebuild.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Printer {
    pub id: String,
    pub name: String,
    pub bed_mm: [f32; 3],
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppearanceMode {
    #[default]
    System,
    Light,
    Dark,
}

impl AppearanceMode {
    pub fn native_theme(self) -> Option<tauri::Theme> {
        match self {
            Self::System => None,
            Self::Light => Some(tauri::Theme::Light),
            Self::Dark => Some(tauri::Theme::Dark),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub appearance_mode: AppearanceMode,
    pub default_slicer: Option<String>,
    #[serde(default)]
    pub(crate) slicer_profiles: Vec<SlicerProfile>,
    pub printers: Vec<Printer>,
    pub active_printer: String,
    pub display_units: String, // "mm" | "in" -- display conversion only (R-12)
    /// Startup folder for the sidebar. Set only by an explicit choice (the
    /// folder picker or the settings page), never by merely opening a file
    /// elsewhere. Always an OS-supplied path (see list_dir).
    #[serde(default, alias = "last_folder")]
    pub default_folder: Option<String>,
    #[serde(default)]
    pub accent_color: Option<String>,
    #[serde(default = "default_color_preview")]
    pub default_color_preview: bool,
}

fn default_color_preview() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            appearance_mode: AppearanceMode::System,
            default_slicer: None,
            slicer_profiles: vec![],
            printers: vec![],
            active_printer: String::new(),
            display_units: "mm".into(),
            default_folder: None,
            accent_color: None,
            default_color_preview: true,
        }
    }
}

#[derive(Clone, Default)]
pub struct SettingsState(pub Arc<Mutex<Settings>>);

fn settings_path(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|d| d.join("settings.json"))
}

pub fn load(app: &tauri::AppHandle) -> Settings {
    settings_path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub(crate) fn save(app: &tauri::AppHandle, settings: &Settings) -> Result<(), String> {
    let path = settings_path(app).ok_or("Could not locate the settings directory")?;
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("Could not serialize settings: {e}"))?;
    write_settings_atomically(&path, json.as_bytes())
}

fn write_settings_atomically(path: &Path, contents: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Could not locate the settings directory".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create settings directory: {error}"))?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "Settings filename is invalid".to_string())?;
    for _ in 0..8 {
        let sequence = SETTINGS_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            sequence
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(mut file) => {
                let result = (|| {
                    file.write_all(contents)
                        .map_err(|error| format!("Could not write settings: {error}"))?;
                    file.sync_all()
                        .map_err(|error| format!("Could not sync settings: {error}"))?;
                    drop(file);
                    replace_settings_file(&candidate, path)?;
                    #[cfg(unix)]
                    let _ = fs::File::open(parent).and_then(|directory| directory.sync_all());
                    Ok(())
                })();
                if result.is_err() {
                    let _ = fs::remove_file(&candidate);
                }
                return result;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Could not create settings file: {error}")),
        }
    }
    Err("Could not allocate a temporary settings file".into())
}

#[cfg(windows)]
fn replace_settings_file(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let flags = MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH;
    if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), flags) } == 0 {
        Err(format!(
            "Could not replace settings file: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_settings_file(source: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(source, destination)
        .map_err(|error| format!("Could not replace settings file: {error}"))
}

#[tauri::command]
pub fn get_settings(state: tauri::State<'_, SettingsState>) -> Settings {
    state.0.lock().unwrap().clone()
}

#[tauri::command]
pub fn get_default_slicer(state: tauri::State<'_, SettingsState>) -> Option<String> {
    state.0.lock().unwrap().default_slicer.clone()
}

#[tauri::command]
pub fn set_default_slicer(
    app: tauri::AppHandle,
    state: tauri::State<'_, SettingsState>,
    slicer_id: Option<String>,
) -> Result<(), String> {
    let mut settings = state.0.lock().unwrap();
    let mut updated = settings.clone();
    set_default_slicer_value(&mut updated, slicer_id)?;
    save(&app, &updated)?;
    *settings = updated;
    Ok(())
}

#[tauri::command]
pub fn save_slicer_profiles(
    app: tauri::AppHandle,
    state: tauri::State<'_, SettingsState>,
    profiles: Vec<SlicerProfileDraft>,
) -> Result<(), String> {
    let mut settings = state.0.lock().unwrap();
    let mut updated = settings.clone();
    let built_in_paths = crate::slicers::detect_slicers()
        .into_iter()
        .map(|slicer| normalize_profile_path(&slicer.app_path))
        .collect::<std::collections::HashSet<_>>();
    let profiles = build_profile_collection(&profiles, &updated.slicer_profiles)?;
    if profiles
        .iter()
        .any(|profile| built_in_paths.contains(&normalize_profile_path(&profile.path)))
    {
        return Err(
            "A detected slicer is already available and cannot be added as a custom profile".into(),
        );
    }
    replace_slicer_profiles(&mut updated, profiles);
    save(&app, &updated)?;
    *settings = updated;
    Ok(())
}

/// Appends only user-selected, explicitly trusted local executables. Imported
/// JSON metadata cannot replace a saved profile or carry executable trust.
#[tauri::command]
pub fn import_slicer_profiles(
    app: tauri::AppHandle,
    state: tauri::State<'_, SettingsState>,
    profiles: Vec<SlicerProfileDraft>,
) -> Result<Vec<String>, String> {
    let mut settings = state.0.lock().unwrap();
    let updated = append_slicer_profile_drafts(&settings, &profiles)?;
    let built_in_paths = crate::slicers::detect_slicers()
        .into_iter()
        .map(|slicer| normalize_profile_path(&slicer.app_path))
        .collect::<std::collections::HashSet<_>>();
    let added = &updated.slicer_profiles[settings.slicer_profiles.len()..];
    if added
        .iter()
        .any(|profile| built_in_paths.contains(&normalize_profile_path(&profile.path)))
    {
        return Err(
            "A detected slicer is already available and cannot be added as a custom profile".into(),
        );
    }
    let ids = added.iter().map(|profile| profile.id.clone()).collect();
    save(&app, &updated)?;
    *settings = updated;
    Ok(ids)
}

fn append_slicer_profile_drafts(
    settings: &Settings,
    additions: &[SlicerProfileDraft],
) -> Result<Settings, String> {
    if additions.iter().any(|draft| draft.id.is_some()) {
        return Err("Imported slicers must be new profiles without saved IDs".into());
    }
    if additions.iter().any(|draft| !draft.trust) {
        return Err("Imported slicers require explicit trust confirmation for each selected local executable".into());
    }
    if settings
        .slicer_profiles
        .len()
        .saturating_add(additions.len())
        > crate::slicer_profiles::MAX_SLICER_PROFILES
    {
        return Err("At most 64 slicer profiles can be saved".into());
    }
    let mut drafts = settings
        .slicer_profiles
        .iter()
        .map(|profile| SlicerProfileDraft {
            id: Some(profile.id.clone()),
            name: profile.name.clone(),
            path: profile.path.clone(),
            trust: false,
        })
        .collect::<Vec<_>>();
    drafts.extend_from_slice(additions);
    let profiles = build_profile_collection(&drafts, &settings.slicer_profiles)?;
    let mut updated = settings.clone();
    // Every existing profile remains in order, so its selected default remains.
    updated.slicer_profiles = profiles;
    Ok(updated)
}

fn set_default_slicer_value(
    settings: &mut Settings,
    slicer_id: Option<String>,
) -> Result<(), String> {
    if let Some(id) = slicer_id.as_deref() {
        let is_builtin = crate::slicers::slicer_definition(id).is_some();
        let is_custom = settings
            .slicer_profiles
            .iter()
            .any(|profile| profile.id == id);
        if id.trim().is_empty() || (!is_builtin && !is_custom) {
            return Err("Select a detected or saved slicer profile".into());
        }
    }
    settings.default_slicer = slicer_id;
    Ok(())
}

fn replace_slicer_profiles(settings: &mut Settings, profiles: Vec<SlicerProfile>) {
    if settings.default_slicer.as_ref().is_some_and(|selected| {
        settings
            .slicer_profiles
            .iter()
            .any(|old| old.id == *selected)
            && !profiles.iter().any(|profile| profile.id == *selected)
    }) {
        settings.default_slicer = None;
    }
    settings.slicer_profiles = profiles;
}

fn normalize_profile_path(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

#[tauri::command]
pub fn set_active_printer(
    app: tauri::AppHandle,
    state: tauri::State<'_, SettingsState>,
    library: tauri::State<'_, crate::library_commands::LibraryState>,
    printer_id: String,
) -> Result<(), String> {
    let is_generic_fallback = printer_id == "generic-256";
    let is_detected = crate::printers::detect_printers()
        .iter()
        .any(|printer| printer.id == printer_id);
    let is_managed = library
        .with_db(|db| {
            db.get_printer(&printer_id)
                .map(|printer| printer.is_some())
                .map_err(crate::library_commands::library_error)
        })
        .map_err(|error| match error {
            crate::extension_api::AppError::Library { message } => message,
            _ => "Could not validate the selected printer".into(),
        })?;
    if !is_generic_fallback && !is_detected && !is_managed {
        return Err("Select an available printer".into());
    }

    let mut settings = state.0.lock().unwrap();
    let mut updated = settings.clone();
    updated.active_printer = printer_id;
    save(&app, &updated)?;
    *settings = updated;
    Ok(())
}

#[tauri::command]
pub fn set_appearance_mode(
    app: tauri::AppHandle,
    state: tauri::State<'_, SettingsState>,
    mode: AppearanceMode,
) -> Result<(), String> {
    let mut settings = state.0.lock().unwrap();
    let mut updated = settings.clone();
    updated.appearance_mode = mode;
    save(&app, &updated)?;
    *settings = updated;
    app.set_theme(mode.native_theme());
    Ok(())
}

#[tauri::command]
pub fn set_display_units(
    app: tauri::AppHandle,
    state: tauri::State<'_, SettingsState>,
    units: String,
) {
    if units != "mm" && units != "in" {
        return;
    }
    let mut settings = state.0.lock().unwrap();
    settings.display_units = units;
    let _ = save(&app, &settings);
}

#[tauri::command]
pub fn set_default_folder(
    app: tauri::AppHandle,
    state: tauri::State<'_, SettingsState>,
    folder: Option<String>,
) {
    let mut settings = state.0.lock().unwrap();
    settings.default_folder = folder;
    let _ = save(&app, &settings);
}

fn validate_accent_color(color: Option<&str>) -> Result<(), String> {
    if let Some(color) = color {
        if color.len() != 7
            || !color.starts_with('#')
            || !color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
        {
            return Err("Accent color must use #RRGGBB format".into());
        }
    }
    Ok(())
}

#[tauri::command]
pub fn set_accent_color(
    app: tauri::AppHandle,
    state: tauri::State<'_, SettingsState>,
    color: Option<String>,
) -> Result<(), String> {
    validate_accent_color(color.as_deref())?;
    let mut settings = state.0.lock().unwrap();
    let mut updated = settings.clone();
    updated.accent_color = color;
    save(&app, &updated)?;
    *settings = updated;
    Ok(())
}

#[tauri::command]
pub fn set_default_color_preview(
    app: tauri::AppHandle,
    state: tauri::State<'_, SettingsState>,
    enabled: bool,
) -> Result<(), String> {
    let mut settings = state.0.lock().unwrap();
    let mut updated = settings.clone();
    updated.default_color_preview = enabled;
    save(&app, &updated)?;
    *settings = updated;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_settings_default_to_theme_accent() {
        let settings: Settings = serde_json::from_str(
            r#"{
            "default_slicer": null,
            "printers": [],
            "active_printer": "",
            "display_units": "mm",
            "last_folder": "/models"
        }"#,
        )
        .unwrap();
        assert_eq!(settings.accent_color, None);
        assert_eq!(settings.default_folder.as_deref(), Some("/models"));
        assert!(settings.default_color_preview);
    }

    #[test]
    fn legacy_settings_migrate_with_no_custom_slicer_profiles() {
        let settings: Settings = serde_json::from_str(
            r#"{
            "default_slicer": "com.bambulab.bambu-studio",
            "printers": [],
            "active_printer": "",
            "display_units": "mm"
        }"#,
        )
        .unwrap();

        assert!(settings.slicer_profiles.is_empty());
        assert_eq!(
            settings.default_slicer.as_deref(),
            Some("com.bambulab.bambu-studio")
        );
    }

    #[test]
    fn default_slicer_accepts_a_saved_profile_or_builtin_and_can_be_cleared() {
        let mut settings = Settings::default();
        let custom = SlicerProfile {
            id: "custom-profile-id".into(),
            name: "Custom Slicer".into(),
            path: "/Applications/CustomSlicer.app".into(),
            identity: None,
        };
        settings.slicer_profiles.push(custom);

        assert!(
            set_default_slicer_value(&mut settings, Some("com.bambulab.bambu-studio".into()))
                .is_ok()
        );
        assert!(set_default_slicer_value(&mut settings, Some("custom-profile-id".into())).is_ok());
        assert_eq!(
            settings.default_slicer.as_deref(),
            Some("custom-profile-id")
        );
        assert!(set_default_slicer_value(&mut settings, None).is_ok());
        assert_eq!(settings.default_slicer, None);
    }

    #[test]
    fn unknown_default_slicer_is_rejected_without_changing_the_saved_value() {
        let mut settings = Settings {
            default_slicer: Some("com.bambulab.bambu-studio".into()),
            ..Settings::default()
        };

        assert!(
            set_default_slicer_value(&mut settings, Some("/tmp/untrusted-app".into())).is_err()
        );
        assert_eq!(
            settings.default_slicer.as_deref(),
            Some("com.bambulab.bambu-studio")
        );
    }

    #[test]
    fn removing_a_profile_clears_it_as_the_default() {
        let mut settings = Settings::default();
        settings.slicer_profiles.push(SlicerProfile {
            id: "custom-profile-id".into(),
            name: "Custom Slicer".into(),
            path: "/Applications/CustomSlicer.app".into(),
            identity: None,
        });
        settings.default_slicer = Some("custom-profile-id".into());

        replace_slicer_profiles(&mut settings, vec![]);

        assert_eq!(settings.default_slicer, None);
        assert!(settings.slicer_profiles.is_empty());
    }

    #[test]
    fn imported_slicer_drafts_require_new_ids_and_explicit_trust() {
        let settings = Settings::default();
        let mut draft = SlicerProfileDraft {
            id: Some("injected-id".into()),
            name: "Imported".into(),
            path: "/not-selected".into(),
            trust: true,
        };
        assert!(append_slicer_profile_drafts(&settings, &[draft.clone()])
            .unwrap_err()
            .contains("new profiles"));
        draft.id = None;
        draft.trust = false;
        assert!(append_slicer_profile_drafts(&settings, &[draft])
            .unwrap_err()
            .contains("explicit trust"));
        assert!(settings.slicer_profiles.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn imported_slicer_batch_preserves_existing_and_default_and_is_all_or_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("existing-slicer");
        let second = temp.path().join("imported-slicer");
        for path in [&first, &second] {
            std::fs::write(path, b"#!/bin/sh\nexit 0\n").unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let profiles = build_profile_collection(
            &[SlicerProfileDraft {
                id: None,
                name: "Existing".into(),
                path: first.to_string_lossy().into_owned(),
                trust: true,
            }],
            &[],
        )
        .unwrap();
        let settings = Settings {
            default_slicer: Some(profiles[0].id.clone()),
            slicer_profiles: profiles.clone(),
            ..Settings::default()
        };
        let draft = SlicerProfileDraft {
            id: None,
            name: "Imported".into(),
            path: second.to_string_lossy().into_owned(),
            trust: true,
        };
        let updated =
            append_slicer_profile_drafts(&settings, std::slice::from_ref(&draft)).unwrap();
        assert_eq!(updated.slicer_profiles.len(), 2);
        assert_eq!(updated.slicer_profiles[0], profiles[0]);
        assert_eq!(updated.default_slicer, settings.default_slicer);
        assert_ne!(updated.slicer_profiles[1].id, profiles[0].id);
        let mut duplicate = draft.clone();
        duplicate.name = "existing".into();
        assert!(append_slicer_profile_drafts(&settings, &[duplicate]).is_err());
        assert!(append_slicer_profile_drafts(&settings, &[draft.clone(), draft]).is_err());
        assert_eq!(settings.slicer_profiles, profiles);
        std::fs::write(first, b"changed executable").unwrap();
        assert!(append_slicer_profile_drafts(
            &settings,
            &[SlicerProfileDraft {
                id: None,
                name: "Imported".into(),
                path: second.to_string_lossy().into_owned(),
                trust: true,
            }]
        )
        .unwrap_err()
        .contains("explicit trust"));
        assert_eq!(settings.slicer_profiles, profiles);
    }

    #[test]
    fn settings_write_atomically_replaces_the_previous_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        std::fs::write(&path, b"previous settings").unwrap();

        write_settings_atomically(&path, b"updated settings").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"updated settings");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn color_preview_default_can_be_disabled_in_saved_settings() {
        let settings = Settings {
            default_color_preview: false,
            ..Settings::default()
        };
        let restored: Settings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert!(!restored.default_color_preview);
    }

    #[test]
    fn accent_color_accepts_hex_and_reset() {
        for color in [None, Some("#12aBcF"), Some("#000000"), Some("#FFFFFF")] {
            assert!(validate_accent_color(color).is_ok());
        }
    }

    #[test]
    fn accent_color_rejects_other_formats() {
        for color in [
            "",
            "red",
            "#abc",
            "123456",
            "#12345678",
            "#12GG00",
            "#éabcd",
            " #123456",
        ] {
            assert!(
                validate_accent_color(Some(color)).is_err(),
                "accepted {color}"
            );
        }
    }
}
