use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::Manager;

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
            printers: vec![],
            active_printer: String::new(),
            display_units: "mm".into(),
            default_folder: None,
            accent_color: None,
            default_color_preview: true,
        }
    }
}

#[derive(Default)]
pub struct SettingsState(pub Mutex<Settings>);

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

fn save(app: &tauri::AppHandle, settings: &Settings) -> Result<(), String> {
    let path = settings_path(app).ok_or("Could not locate the settings directory")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Could not create settings directory: {e}"))?;
    }
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("Could not serialize settings: {e}"))?;
    std::fs::write(path, json).map_err(|e| format!("Could not save settings: {e}"))
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
    slicer_id: String,
) {
    let mut settings = state.0.lock().unwrap();
    settings.default_slicer = Some(slicer_id);
    let _ = save(&app, &settings);
}

#[tauri::command]
pub fn set_active_printer(
    app: tauri::AppHandle,
    state: tauri::State<'_, SettingsState>,
    printer_id: String,
) {
    // Accepts detected-printer ids too ("<bundle>:<preset>"), which aren't in
    // the manual list; the frontend owns the merged list.
    let mut settings = state.0.lock().unwrap();
    settings.active_printer = printer_id;
    let _ = save(&app, &settings);
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
    fn color_preview_default_can_be_disabled_in_saved_settings() {
        let mut settings = Settings::default();
        settings.default_color_preview = false;
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
