use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::settings::{Settings, SettingsState};
use crate::slicer_profiles::{
    capture_profile_identity, profile_identity_is_current, SlicerProfileIdentity,
};
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
    #[serde(skip)]
    pub(crate) custom: bool,
    #[serde(skip)]
    pub(crate) profile_identity: Option<SlicerProfileIdentity>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SlicerProfileView {
    pub id: String,
    pub bundle_id: String,
    pub display_name: String,
    pub app_path: String,
    pub is_custom: bool,
    pub available: bool,
}

#[derive(Clone, Copy)]
struct CandidateRule {
    name: &'static str,
    stems: &'static [&'static str],
    path_include: Option<&'static str>,
    path_exclude: Option<&'static str>,
    path_include_any: &'static [&'static str],
}

const CANDIDATE_RULES: &[CandidateRule] = &[
    CandidateRule {
        name: "Bambu Studio",
        stems: &["bambu-studio", "bambustudio", "bambu studio"],
        path_include: None,
        path_exclude: None,
        path_include_any: &[],
    },
    CandidateRule {
        name: "Snapmaker Orca",
        stems: &[
            "snapmaker-orca",
            "snapmaker orca",
            "snapmakerorca",
            "orca-slicer",
            "orcaslicer",
        ],
        path_include: Some("snapmaker"),
        path_exclude: None,
        path_include_any: &[],
    },
    CandidateRule {
        name: "OrcaSlicer",
        stems: &[
            "orca-slicer",
            "orcaslicer",
            "com.softfever3d.orcaslicer",
            "io.github.softfever.orcaslicer",
        ],
        path_include: None,
        path_exclude: Some("snapmaker"),
        path_include_any: &[],
    },
    CandidateRule {
        name: "PrusaSlicer",
        stems: &["prusa-slicer", "prusaslicer", "com.prusa3d.prusaslicer"],
        path_include: None,
        path_exclude: None,
        path_include_any: &[],
    },
    CandidateRule {
        name: "SuperSlicer",
        stems: &["superslicer", "super-slicer"],
        path_include: None,
        path_exclude: None,
        path_include_any: &[],
    },
    CandidateRule {
        name: "Slic3r",
        stems: &["slic3r"],
        path_include: None,
        path_exclude: None,
        path_include_any: &[],
    },
    CandidateRule {
        name: "UltiMaker Cura",
        stems: &["ultimaker-cura", "ultimaker cura", "cura"],
        path_include: None,
        path_exclude: None,
        path_include_any: &["ultimaker", "cura"],
    },
    CandidateRule {
        name: "Creality Print",
        stems: &[
            "crealityprint",
            "creality-print",
            "creality print",
            "crealityprint5",
        ],
        path_include: None,
        path_exclude: None,
        path_include_any: &[],
    },
    CandidateRule {
        name: "ElegooSlicer",
        stems: &["elegooslicer", "elegoo-slicer", "elegoo slicer"],
        path_include: None,
        path_exclude: None,
        path_include_any: &[],
    },
    CandidateRule {
        name: "Anycubic Slicer Next",
        stems: &["anycubicslicernext", "anycubic-slicer-next"],
        path_include: None,
        path_exclude: None,
        path_include_any: &[],
    },
    CandidateRule {
        name: "Anycubic Slicer",
        stems: &["anycubicslicer", "anycubic-slicer"],
        path_include: None,
        path_exclude: None,
        path_include_any: &[],
    },
    CandidateRule {
        name: "ideaMaker",
        stems: &["ideamaker"],
        path_include: None,
        path_exclude: None,
        path_include_any: &[],
    },
    CandidateRule {
        name: "Lychee Slicer",
        stems: &["lycheeslicer", "lychee slicer"],
        path_include: None,
        path_exclude: None,
        path_include_any: &[],
    },
    CandidateRule {
        name: "CHITUBOX",
        stems: &["chitubox", "chituboxpro", "chitubox basic"],
        path_include: None,
        path_exclude: None,
        path_include_any: &[],
    },
    CandidateRule {
        name: "FlashPrint",
        stems: &["flashprint"],
        path_include: None,
        path_exclude: None,
        path_include_any: &[],
    },
    CandidateRule {
        name: "QIDI Studio",
        stems: &["qidistudio", "qidi-studio", "qidi studio", "qidislicer"],
        path_include: None,
        path_exclude: None,
        path_include_any: &[],
    },
    CandidateRule {
        name: "Simplify3D",
        stems: &["simplify3d"],
        path_include: None,
        path_exclude: None,
        path_include_any: &[],
    },
];

const CANDIDATE_SCAN_DEPTH: usize = 4;
const MAX_CANDIDATE_SCAN_ENTRIES: usize = 50_000;
const CANDIDATE_SKIP_DIRS: &[&str] = &[
    "resources",
    "locales",
    "node_modules",
    "swiftshader",
    "uninstall",
    ".git",
    "windows",
    "microsoft",
    "common files",
    "windowsapps",
    "internet explorer",
    "windows defender",
    "windows mail",
    "windows nt",
    "windows photo viewer",
    "reference assemblies",
    "msbuild",
];

fn identify_slicer_candidate(path: &str) -> Option<&'static str> {
    let normalized = path.replace('\\', "/").to_lowercase();
    let base = normalized.rsplit('/').next()?;
    if ["console", "curaengine", "gcodeviewer"]
        .iter()
        .any(|skip| base.contains(skip))
    {
        return None;
    }
    let stem = base
        .strip_suffix(".app")
        .or_else(|| base.strip_suffix(".exe"))
        .or_else(|| base.strip_suffix(".appimage"))
        .unwrap_or(base);

    CANDIDATE_RULES.iter().find_map(|rule| {
        if rule
            .path_include
            .is_some_and(|part| !normalized.contains(part))
            || rule
                .path_exclude
                .is_some_and(|part| normalized.contains(part))
            || (!rule.path_include_any.is_empty()
                && !rule
                    .path_include_any
                    .iter()
                    .any(|part| normalized.contains(part)))
        {
            return None;
        }
        rule.stems
            .iter()
            .any(|expected| {
                stem == *expected
                    || stem.starts_with(&format!("{expected}_"))
                    || stem.starts_with(&format!("{expected}-"))
            })
            .then_some(rule.name)
    })
}

#[tauri::command]
pub fn detect_slicer_profile_candidates(
    state: tauri::State<'_, SettingsState>,
) -> Vec<crate::slicer_profiles::SlicerProfileDraft> {
    if crate::visual_qa::active() {
        return Vec::new();
    }
    let settings = state.0.lock().unwrap();
    let mut existing_paths = detect_slicers()
        .into_iter()
        .map(|slicer| normalize_path_key(&slicer.app_path))
        .collect::<HashSet<_>>();
    existing_paths.extend(
        settings
            .slicer_profiles
            .iter()
            .map(|profile| normalize_path_key(&profile.path)),
    );
    scan_profile_candidates(&candidate_scan_roots(), &candidate_paths())
        .into_iter()
        .filter(|candidate| !existing_paths.contains(&normalize_path_key(&candidate.path)))
        .collect()
}

fn scan_profile_candidates(
    roots: &[PathBuf],
    explicit_candidates: &[PathBuf],
) -> Vec<crate::slicer_profiles::SlicerProfileDraft> {
    let mut candidates = Vec::new();
    let mut seen_paths = HashSet::new();
    let mut visited = 0usize;
    for path in explicit_candidates {
        consider_profile_candidate(path, &mut seen_paths, &mut candidates);
    }
    for root in roots {
        if visited >= MAX_CANDIDATE_SCAN_ENTRIES {
            break;
        }
        scan_candidate_root(
            root,
            CANDIDATE_SCAN_DEPTH,
            &mut visited,
            &mut seen_paths,
            &mut candidates,
        );
    }
    disambiguate_candidate_names(&mut candidates);
    candidates.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.path.cmp(&right.path))
    });
    candidates
}

fn scan_candidate_root(
    root: &Path,
    depth_left: usize,
    visited: &mut usize,
    seen_paths: &mut HashSet<String>,
    candidates: &mut Vec<crate::slicer_profiles::SlicerProfileDraft>,
) {
    if depth_left == 0 || *visited >= MAX_CANDIDATE_SCAN_ENTRIES {
        return;
    }
    let Ok(root_metadata) = std::fs::symlink_metadata(root) else {
        return;
    };
    if root_metadata.file_type().is_symlink()
        || crate::slicer_profiles::is_reparse_point(&root_metadata)
        || !root_metadata.is_dir()
    {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut entries = entries.flatten().collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    let mut subdirectories = Vec::new();
    for entry in entries {
        if *visited >= MAX_CANDIDATE_SCAN_ENTRIES {
            break;
        }
        *visited += 1;
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() || crate::slicer_profiles::is_reparse_point(&metadata)
        {
            continue;
        }
        if metadata.is_file() || metadata.is_dir() && is_app_bundle(&path) {
            consider_profile_candidate(&path, seen_paths, candidates);
            continue;
        }
        if metadata.is_dir() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if !CANDIDATE_SKIP_DIRS.contains(&name.as_str()) {
                subdirectories.push(path);
            }
        }
    }
    for directory in subdirectories {
        if *visited >= MAX_CANDIDATE_SCAN_ENTRIES {
            break;
        }
        let name = directory
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_lowercase();
        if should_scan_slicer_directory(&name) {
            scan_candidate_root(&directory, depth_left - 1, visited, seen_paths, candidates);
        }
    }
}

fn consider_profile_candidate(
    path: &Path,
    seen_paths: &mut HashSet<String>,
    candidates: &mut Vec<crate::slicer_profiles::SlicerProfileDraft>,
) -> bool {
    let Some(name) = identify_slicer_candidate(&path.to_string_lossy()) else {
        return false;
    };
    let Ok(identity) = capture_profile_identity(path) else {
        return false;
    };
    let key = normalize_path_key(&identity.canonical_path);
    if !seen_paths.insert(key) {
        return true;
    }
    candidates.push(crate::slicer_profiles::SlicerProfileDraft {
        id: None,
        name: name.to_string(),
        path: identity.canonical_path,
        trust: false,
    });
    true
}

fn disambiguate_candidate_names(candidates: &mut [crate::slicer_profiles::SlicerProfileDraft]) {
    let mut used = HashSet::new();
    for candidate in candidates {
        let base = candidate.name.clone();
        let mut name = base.clone();
        let mut index = 1usize;
        while !used.insert(name.to_lowercase()) {
            let location = Path::new(&candidate.path)
                .parent()
                .and_then(Path::file_name)
                .and_then(|part| part.to_str())
                .unwrap_or("Installed");
            name = if index == 1 {
                format!("{base} ({location})")
            } else {
                format!("{base} ({location} {index})")
            };
            index += 1;
        }
        candidate.name = name;
    }
}

fn is_app_bundle(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
}

fn should_scan_slicer_directory(name: &str) -> bool {
    name == "programs"
        || [
            "bambu",
            "orca",
            "prusa",
            "slic3r",
            "cura",
            "creality",
            "elegoo",
            "anycubic",
            "ideamaker",
            "lychee",
            "chitu",
            "flashprint",
            "qidi",
            "snapmaker",
            "simplify",
            "slicer",
        ]
        .iter()
        .any(|part| name.contains(part))
}

fn candidate_scan_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    #[cfg(windows)]
    {
        for key in [
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramW6432",
            "LOCALAPPDATA",
        ] {
            if let Some(root) = std::env::var_os(key) {
                roots.push(PathBuf::from(root));
            }
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            roots.push(PathBuf::from(local).join("Programs"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        roots.push(PathBuf::from("/Applications"));
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(PathBuf::from(home).join("Applications"));
        }
        roots.push(PathBuf::from("/opt"));
    }
    #[cfg(target_os = "linux")]
    {
        roots.extend([PathBuf::from("/opt"), PathBuf::from("/usr/local")]);
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(PathBuf::from(home).join("Applications"));
            roots.push(PathBuf::from(home).join(".local"));
        }
    }
    roots
}

fn candidate_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    #[cfg(windows)]
    {
        let mut roots = ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
            .into_iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            roots.push(PathBuf::from(local).join("Programs"));
        }
        let relative: &[&[&str]] = &[
            &["Bambu Studio", "bambu-studio.exe"],
            &["OrcaSlicer", "orca-slicer.exe"],
            &["Snapmaker Orca", "orca-slicer.exe"],
            &["Prusa3D", "PrusaSlicer", "prusa-slicer.exe"],
            &["PrusaSlicer", "prusa-slicer.exe"],
            &["SuperSlicer", "superslicer.exe"],
            &["ideaMaker", "ideaMaker.exe"],
            &["LycheeSlicer", "LycheeSlicer.exe"],
            &["ChiTuBox", "CHITUBOX.exe"],
            &["ElegooSlicer", "ElegooSlicer.exe"],
            &["FlashPrint", "FlashPrint.exe"],
            &["Simplify3D", "Simplify3D.exe"],
        ];
        for root in roots {
            for parts in relative {
                paths.push(
                    parts
                        .iter()
                        .fold(root.clone(), |path, part| path.join(part)),
                );
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        let directories = [PathBuf::from("/Applications")]
            .into_iter()
            .chain(std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Applications")));
        let app_names = [
            "BambuStudio.app",
            "Bambu Studio.app",
            "OrcaSlicer.app",
            "Snapmaker Orca.app",
            "PrusaSlicer.app",
            "SuperSlicer.app",
            "UltiMaker Cura.app",
            "Creality Print.app",
            "ideaMaker.app",
            "CHITUBOX.app",
            "Lychee Slicer.app",
            "ElegooSlicer.app",
            "FlashPrint.app",
            "QIDI Studio.app",
            "Simplify3D.app",
        ];
        for directory in directories {
            paths.extend(app_names.iter().map(|name| directory.join(name)));
        }
    }
    #[cfg(target_os = "linux")]
    {
        let bins = [
            "bambu-studio",
            "orca-slicer",
            "prusa-slicer",
            "superslicer",
            "slic3r",
            "cura",
            "ultimaker-cura",
            "crealityprint",
            "elegooslicer",
            "ideamaker",
        ];
        let mut directories = vec![
            PathBuf::from("/usr/bin"),
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/snap/bin"),
        ];
        if let Some(home) = std::env::var_os("HOME") {
            directories.push(PathBuf::from(home).join(".local/bin"));
        }
        paths.extend(
            directories
                .iter()
                .flat_map(|directory| bins.iter().map(|bin| directory.join(bin))),
        );
    }
    paths
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
    if crate::visual_qa::active() {
        return Vec::new();
    }
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
                    let Ok(identity) = capture_profile_identity(&path) else {
                        continue;
                    };
                    found.push(SlicerApp {
                        id: bundle_id.clone(),
                        bundle_id,
                        display_name: definition.display_name.to_string(),
                        app_path: identity.canonical_path.clone(),
                        custom: false,
                        profile_identity: Some(identity),
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
    if crate::visual_qa::active() {
        return Vec::new();
    }
    slicer_definitions()
        .iter()
        .filter_map(|definition| {
            platform_candidates(definition)
                .into_iter()
                .find_map(|(candidate, app_image)| {
                    let path = validated_executable(&candidate, app_image)?;
                    let identity = capture_profile_identity(&path).ok()?;
                    Some(SlicerApp {
                        id: definition.bundle_id.to_owned(),
                        bundle_id: definition.bundle_id.to_owned(),
                        display_name: definition.display_name.to_owned(),
                        app_path: identity.canonical_path.clone(),
                        custom: false,
                        profile_identity: Some(identity),
                    })
                })
        })
        .collect()
}

pub(crate) fn available_slicers(settings: &Settings) -> Vec<SlicerApp> {
    let mut slicers = detect_slicers();
    let detected_paths = slicers
        .iter()
        .map(|slicer| normalize_path_key(&slicer.app_path))
        .collect::<std::collections::HashSet<_>>();

    for profile in &settings.slicer_profiles {
        let Some(identity) = profile.identity.as_ref() else {
            continue;
        };
        if profile.path != identity.canonical_path
            || !profile_identity_is_current(identity)
            || detected_paths.contains(&normalize_path_key(&profile.path))
        {
            continue;
        }
        slicers.push(SlicerApp {
            id: profile.id.clone(),
            bundle_id: identity.bundle_id.clone().unwrap_or_default(),
            display_name: profile.name.clone(),
            app_path: identity.canonical_path.clone(),
            custom: true,
            profile_identity: Some(identity.clone()),
        });
    }
    slicers
}

/// Hold the settings lock while a custom profile is authorized and the process
/// is launched. This closes the app-level race where a profile could be
/// removed or edited after detection but before launch.
pub(crate) fn launch_with_current_profile<T>(
    state: &SettingsState,
    slicer: &SlicerApp,
    launch: impl FnOnce() -> Result<T, crate::extension_api::AppError>,
) -> Result<T, crate::extension_api::AppError> {
    if !slicer.custom {
        return launch();
    }

    let settings = state.0.lock().unwrap();
    let still_saved = settings.slicer_profiles.iter().any(|profile| {
        profile.id == slicer.id
            && profile.path == slicer.app_path
            && profile.identity.as_ref().is_some_and(|identity| {
                slicer.profile_identity.as_ref() == Some(identity)
                    && identity.canonical_path == profile.path
                    && profile_identity_is_current(identity)
            })
    });
    if !still_saved {
        return Err(crate::extension_api::AppError::UnknownSlicer);
    }
    launch()
}

#[tauri::command]
pub fn list_slicer_profiles(state: tauri::State<'_, SettingsState>) -> Vec<SlicerProfileView> {
    profile_views(&state.0.lock().unwrap())
}

pub(crate) fn profile_views(settings: &Settings) -> Vec<SlicerProfileView> {
    let detected = detect_slicers();
    let detected_paths = detected
        .iter()
        .map(|slicer| normalize_path_key(&slicer.app_path))
        .collect::<std::collections::HashSet<_>>();
    let mut views = detected
        .into_iter()
        .map(|slicer| SlicerProfileView {
            id: slicer.id,
            bundle_id: slicer.bundle_id,
            display_name: slicer.display_name,
            app_path: slicer.app_path,
            is_custom: false,
            available: true,
        })
        .collect::<Vec<_>>();

    for profile in &settings.slicer_profiles {
        let available = profile.identity.as_ref().is_some_and(|identity| {
            profile.path == identity.canonical_path
                && profile_identity_is_current(identity)
                && !detected_paths.contains(&normalize_path_key(&profile.path))
        });
        views.push(SlicerProfileView {
            id: profile.id.clone(),
            bundle_id: profile
                .identity
                .as_ref()
                .and_then(|identity| identity.bundle_id.clone())
                .unwrap_or_default(),
            display_name: profile.name.clone(),
            app_path: profile.path.clone(),
            is_custom: true,
            available,
        });
    }
    views.sort_by(|left, right| {
        left.display_name
            .to_lowercase()
            .cmp(&right.display_name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    views
}

fn normalize_path_key(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_lowercase()
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
    if crate::visual_qa::active() {
        return None;
    }
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
    use crate::slicer_profiles::SlicerProfile;
    use std::fs;
    use std::io::Write;

    #[cfg(unix)]
    fn executable(path: &Path, bytes: &[u8]) {
        use std::os::unix::fs::PermissionsExt;
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

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
    fn custom_profile_is_launchable_only_while_its_saved_identity_matches() {
        let dir = temp_dir("custom-profile");
        let path = dir.join("CustomSlicer");
        executable(&path, b"custom slicer executable");
        let identity = capture_profile_identity(&path).unwrap();
        let mut settings = Settings::default();
        settings.slicer_profiles.push(SlicerProfile {
            id: "custom-profile-id".into(),
            name: "Custom Slicer".into(),
            path: identity.canonical_path.clone(),
            identity: Some(identity),
        });

        let available = available_slicers(&settings);
        assert!(available
            .iter()
            .any(|slicer| slicer.id == "custom-profile-id" && slicer.custom));
        assert!(profile_views(&settings)
            .iter()
            .any(|profile| profile.id == "custom-profile-id" && profile.available));

        let replacement = dir.join("replacement");
        executable(&replacement, b"replacement executable");
        fs::rename(&replacement, &path).unwrap();
        assert!(!available_slicers(&settings)
            .iter()
            .any(|slicer| slicer.id == "custom-profile-id"));
        assert!(profile_views(&settings)
            .iter()
            .any(|profile| profile.id == "custom-profile-id" && !profile.available));
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn deleted_custom_profile_is_rejected_immediately_before_launch() {
        let dir = temp_dir("deleted-custom-profile");
        let path = dir.join("CustomSlicer");
        executable(&path, b"custom slicer executable");
        let identity = capture_profile_identity(&path).unwrap();
        let state = SettingsState::default();
        state.0.lock().unwrap().slicer_profiles.push(SlicerProfile {
            id: "custom-profile-id".into(),
            name: "Custom Slicer".into(),
            path: identity.canonical_path.clone(),
            identity: Some(identity),
        });
        let slicer = available_slicers(&state.0.lock().unwrap())
            .into_iter()
            .find(|slicer| slicer.id == "custom-profile-id")
            .unwrap();
        state.0.lock().unwrap().slicer_profiles.clear();

        let mut launched = false;
        let result = launch_with_current_profile(&state, &slicer, || {
            launched = true;
            Ok(())
        });

        assert!(result.is_err());
        assert!(!launched);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn slicer_candidate_rules_skip_unrelated_apps_and_console_tools() {
        assert_eq!(
            identify_slicer_candidate("/opt/PrusaSlicer/prusa-slicer.AppImage"),
            Some("PrusaSlicer")
        );
        assert_eq!(identify_slicer_candidate("/Applications/Orca.app"), None);
        assert_eq!(
            identify_slicer_candidate("/opt/CuraEngine/CuraEngine"),
            None
        );
        assert_eq!(
            identify_slicer_candidate("/opt/Snapmaker Orca/orca-slicer"),
            Some("Snapmaker Orca")
        );
    }

    #[cfg(unix)]
    #[test]
    fn auto_detect_returns_untrusted_suggestions_and_never_follows_symlinks() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let root = temp_dir("candidate-scan");
        let install = root.join("PrusaSlicer");
        fs::create_dir_all(&install).unwrap();
        let executable = install.join("prusa-slicer");
        fs::write(&executable, b"slicer executable").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&executable, install.join("prusa-slicer-linked")).unwrap();

        let suggestions = scan_profile_candidates(std::slice::from_ref(&root), &[]);

        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].name, "PrusaSlicer");
        assert_eq!(suggestions[0].id, None);
        assert_eq!(
            suggestions[0].path,
            executable.canonicalize().unwrap().to_string_lossy()
        );
        assert!(!available_slicers(&Settings::default())
            .iter()
            .any(|slicer| slicer.app_path == suggestions[0].path));
        let _ = fs::remove_dir_all(root);
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
