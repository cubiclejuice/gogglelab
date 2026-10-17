use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
#[cfg(windows)]
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

pub(crate) const MAX_SLICER_PROFILES: usize = 64;
const MAX_PROFILE_NAME_CHARS: usize = 128;
const MAX_PROFILE_PATH_CHARS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SlicerProfileDraft {
    #[serde(default)]
    pub(crate) id: Option<String>,
    pub(crate) name: String,
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) trust: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SlicerProfile {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) identity: Option<SlicerProfileIdentity>,
}

/// File identity and metadata detect ordinary replacements and updates. A
/// hostile same-user process could rewrite the same inode and restore its
/// modification time; that stronger threat requires a privileged broker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SlicerProfileIdentity {
    pub(crate) canonical_path: String,
    pub(crate) executable_path: String,
    pub(crate) platform: String,
    pub(crate) file_id: String,
    pub(crate) file_size: u64,
    pub(crate) modified_ns: u128,
    #[serde(default)]
    pub(crate) bundle_id: Option<String>,
}

pub(crate) fn validate_profile_drafts(drafts: &[SlicerProfileDraft]) -> Result<(), String> {
    if drafts.len() > MAX_SLICER_PROFILES {
        return Err(format!(
            "At most {MAX_SLICER_PROFILES} slicer profiles can be saved"
        ));
    }

    let mut names = HashSet::with_capacity(drafts.len());
    let mut paths = HashSet::with_capacity(drafts.len());
    for draft in drafts {
        let name = draft.name.trim();
        if name.is_empty()
            || name.chars().count() > MAX_PROFILE_NAME_CHARS
            || name.chars().any(char::is_control)
        {
            return Err("Slicer profile names must be non-empty, at most 128 characters, and contain no control characters".into());
        }

        let path = draft.path.trim();
        if path.is_empty() || path.chars().count() > MAX_PROFILE_PATH_CHARS {
            return Err(
                "Slicer profile paths must be non-empty and at most 4096 characters".into(),
            );
        }

        if !names.insert(name.to_lowercase()) {
            return Err(format!("Slicer profile name '{}' is already used", name));
        }
        if !paths.insert(normalize_profile_path(path)) {
            return Err(format!("Slicer profile path '{}' is already used", path));
        }
    }
    Ok(())
}

pub(crate) fn build_profile_collection(
    drafts: &[SlicerProfileDraft],
    existing: &[SlicerProfile],
) -> Result<Vec<SlicerProfile>, String> {
    validate_profile_drafts(drafts)?;
    let mut seen_ids = HashSet::with_capacity(drafts.len());
    let mut seen_canonical_paths = HashSet::with_capacity(drafts.len());
    let mut profiles = Vec::with_capacity(drafts.len());

    for draft in drafts {
        let id = match draft.id.as_deref() {
            Some(id) if existing.iter().any(|profile| profile.id == id) => id.to_string(),
            Some(_) => return Err("Unknown slicer profile ID".into()),
            None => loop {
                let candidate = uuid::Uuid::new_v4().to_string();
                if !existing.iter().any(|profile| profile.id == candidate)
                    && !seen_ids.contains(&candidate)
                {
                    break candidate;
                }
            },
        };
        if !seen_ids.insert(id.clone()) {
            return Err("Slicer profile IDs must be unique".into());
        }

        let identity = capture_profile_identity(Path::new(draft.path.trim()))?;
        let saved = draft
            .id
            .as_deref()
            .and_then(|id| existing.iter().find(|profile| profile.id == id));
        let identity = match saved.and_then(|profile| profile.identity.as_ref()) {
            Some(previous) if previous == &identity => previous.clone(),
            _ if draft.trust => identity,
            _ => {
                return Err(
                    "A new or changed slicer executable requires explicit trust confirmation"
                        .into(),
                )
            }
        };
        if !seen_canonical_paths.insert(normalize_profile_path(&identity.canonical_path)) {
            return Err("Slicer profile paths must resolve to distinct applications".into());
        }
        profiles.push(SlicerProfile {
            id,
            name: draft.name.trim().to_string(),
            path: identity.canonical_path.clone(),
            identity: Some(identity),
        });
    }
    Ok(profiles)
}

pub(crate) fn capture_profile_identity(path: &Path) -> Result<SlicerProfileIdentity, String> {
    let selected_metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Could not inspect selected slicer: {error}"))?;
    if selected_metadata.file_type().is_symlink() || is_reparse_point(&selected_metadata) {
        return Err("Slicer profiles cannot point through a symbolic link or reparse point".into());
    }

    let canonical_path = path
        .canonicalize()
        .map_err(|error| format!("Could not resolve selected slicer: {error}"))?;
    if !canonical_path.is_absolute() {
        return Err("Slicer profile path must be absolute".into());
    }

    #[cfg(target_os = "macos")]
    let (executable_path, bundle_id) = macos_application_target(&canonical_path)?;
    #[cfg(not(target_os = "macos"))]
    let (executable_path, bundle_id) = (canonical_path.clone(), None);

    let metadata_before = fs::symlink_metadata(&executable_path)
        .map_err(|error| format!("Could not inspect slicer executable: {error}"))?;
    if metadata_before.file_type().is_symlink()
        || is_reparse_point(&metadata_before)
        || !metadata_before.is_file()
    {
        return Err("Slicer executable must be a regular file, not a link or directory".into());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata_before.permissions().mode() & 0o111 == 0 {
            return Err("Selected slicer is not executable".into());
        }
    }
    #[cfg(windows)]
    {
        if !is_windows_executable(&executable_path)? {
            return Err("Selected file is not a Windows executable".into());
        }
    }

    let executable = open_profile_file(&executable_path)?;
    let metadata = executable
        .metadata()
        .map_err(|error| format!("Could not read slicer metadata: {error}"))?;
    let metadata_after = fs::symlink_metadata(&executable_path)
        .map_err(|error| format!("Slicer executable changed while it was checked: {error}"))?;
    let current_file = open_profile_file(&executable_path)?;
    let current_metadata = current_file
        .metadata()
        .map_err(|error| format!("Could not recheck slicer metadata: {error}"))?;
    if metadata_after.file_type().is_symlink()
        || is_reparse_point(&metadata_after)
        || !same_file_identity(&executable, &current_file)?
        || metadata.len() != current_metadata.len()
        || metadata.modified().ok() != current_metadata.modified().ok()
    {
        return Err("Slicer executable changed while it was checked".into());
    }

    let modified_ns = metadata
        .modified()
        .map_err(|error| format!("Could not read slicer modification time: {error}"))?
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "Slicer modification time is invalid".to_string())?
        .as_nanos();

    Ok(SlicerProfileIdentity {
        canonical_path: canonical_path.to_string_lossy().into_owned(),
        executable_path: executable_path.to_string_lossy().into_owned(),
        platform: std::env::consts::OS.to_string(),
        file_id: profile_file_id(&metadata, &executable)?,
        file_size: metadata.len(),
        modified_ns,
        bundle_id,
    })
}

pub(crate) fn profile_identity_is_current(identity: &SlicerProfileIdentity) -> bool {
    if identity.platform != std::env::consts::OS {
        return false;
    }
    capture_profile_identity(Path::new(&identity.canonical_path))
        .is_ok_and(|current| current == *identity)
}

fn open_profile_file(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options
        .open(path)
        .map_err(|error| format!("Could not open slicer executable: {error}"))
}

fn same_file_identity(left: &File, right: &File) -> Result<bool, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let left = left
            .metadata()
            .map_err(|error| format!("Could not read slicer metadata: {error}"))?;
        let right = right
            .metadata()
            .map_err(|error| format!("Could not recheck slicer metadata: {error}"))?;
        Ok(left.dev() == right.dev() && left.ino() == right.ino())
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };

        Ok(windows_file_id(left.as_raw_handle())? == windows_file_id(right.as_raw_handle())?)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let left = left
            .metadata()
            .map_err(|error| format!("Could not read slicer metadata: {error}"))?;
        let right = right
            .metadata()
            .map_err(|error| format!("Could not recheck slicer metadata: {error}"))?;
        Ok(left.len() == right.len())
    }
}

fn profile_file_id(metadata: &fs::Metadata, file: &File) -> Result<String, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = file;
        Ok(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        let _ = metadata;
        let (volume, index) = windows_file_id(file.as_raw_handle())?;
        Ok(format!("windows:{volume}:{index}"))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (metadata, file);
        Ok("portable-identity-unavailable".into())
    }
}

pub(crate) fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
    }
    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

#[cfg(windows)]
fn windows_file_id(handle: windows_sys::Win32::Foundation::HANDLE) -> Result<(u32, u64), String> {
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT,
    };

    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(handle, &mut information) } == 0 {
        return Err(format!(
            "Could not read slicer identity: {}",
            std::io::Error::last_os_error()
        ));
    }
    if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err("Slicer executable cannot be a reparse point".into());
    }
    let index =
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
    Ok((information.dwVolumeSerialNumber, index))
}

#[cfg(windows)]
fn is_windows_executable(path: &Path) -> Result<bool, String> {
    let mut file = File::open(path)
        .map_err(|error| format!("Could not inspect selected executable: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("Could not read executable metadata: {error}"))?;
    let mut dos = [0u8; 64];
    file.read_exact(&mut dos)
        .map_err(|_| "Selected file does not contain a valid executable header".to_string())?;
    if &dos[..2] != b"MZ" {
        return Ok(false);
    }
    let pe_offset = u32::from_le_bytes(dos[0x3c..0x40].try_into().unwrap()) as u64;
    if pe_offset > metadata.len().saturating_sub(4) {
        return Ok(false);
    }
    file.seek(SeekFrom::Start(pe_offset))
        .map_err(|error| format!("Could not inspect executable header: {error}"))?;
    let mut signature = [0u8; 4];
    file.read_exact(&mut signature)
        .map_err(|_| "Selected file does not contain a valid executable header".to_string())?;
    Ok(&signature == b"PE\0\0")
}

#[cfg(target_os = "macos")]
fn macos_application_target(path: &Path) -> Result<(PathBuf, Option<String>), String> {
    if path.extension().and_then(|extension| extension.to_str()) != Some("app") {
        return Ok((path.to_path_buf(), None));
    }
    if !fs::symlink_metadata(path)
        .map_err(|error| format!("Could not inspect selected application: {error}"))?
        .is_dir()
    {
        return Err("Selected application bundle is not a directory".into());
    }

    let plist_path = path.join("Contents/Info.plist");
    let value = plist::Value::from_file(&plist_path)
        .map_err(|error| format!("Could not read selected application's Info.plist: {error}"))?;
    let dictionary = value
        .as_dictionary()
        .ok_or_else(|| "Selected application's Info.plist is invalid".to_string())?;
    let bundle_id = dictionary
        .get("CFBundleIdentifier")
        .and_then(plist::Value::as_string)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "Selected application has no bundle identifier".to_string())?
        .to_string();
    let executable = dictionary
        .get("CFBundleExecutable")
        .and_then(plist::Value::as_string)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "Selected application has no executable".to_string())?;
    let executable_path = Path::new(executable);
    if executable_path.components().count() != 1
        || !matches!(
            executable_path.components().next(),
            Some(Component::Normal(_))
        )
    {
        return Err("Selected application's executable name is invalid".into());
    }
    let target = path.join("Contents/MacOS").join(executable_path);
    Ok((target, Some(bundle_id)))
}

#[cfg(not(target_os = "macos"))]
fn macos_application_target(path: &Path) -> Result<(PathBuf, Option<String>), String> {
    Ok((path.to_path_buf(), None))
}

fn normalize_profile_path(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::{
        build_profile_collection, capture_profile_identity, profile_identity_is_current,
        validate_profile_drafts, SlicerProfileDraft,
    };
    use std::fs;
    use std::path::Path;

    fn draft(name: &str, path: &str) -> SlicerProfileDraft {
        SlicerProfileDraft {
            id: None,
            name: name.to_string(),
            path: path.to_string(),
            trust: true,
        }
    }

    #[test]
    fn accepts_distinct_user_selected_profiles() {
        let profiles = [
            draft("Bambu Studio", "/Applications/Bambu Studio.app"),
            draft("PrusaSlicer", "/Applications/PrusaSlicer.app"),
        ];

        assert!(validate_profile_drafts(&profiles).is_ok());
    }

    #[test]
    fn rejects_empty_or_oversized_profile_names_and_paths() {
        for profile in [
            draft("  ", "/Applications/Slicer.app"),
            draft("Slicer\nName", "/Applications/Slicer.app"),
            draft(&"s".repeat(129), "/Applications/Slicer.app"),
            draft("Slicer", "  "),
            draft("Slicer", &format!("/{}", "x".repeat(4096))),
        ] {
            assert!(validate_profile_drafts(&[profile]).is_err());
        }
    }

    #[test]
    fn rejects_duplicate_names_ignoring_case_and_outer_whitespace() {
        let profiles = [
            draft("  Bambu Studio ", "/Applications/Bambu Studio.app"),
            draft("bambu studio", "/Applications/Other Slicer.app"),
        ];

        assert!(validate_profile_drafts(&profiles).is_err());
    }

    #[test]
    fn rejects_duplicate_paths_across_separator_and_case_variants() {
        let profiles = [
            draft("One", "C:\\Program Files\\Slicer\\slicer.exe"),
            draft("Two", "c:/program files/slicer/slicer.exe/"),
        ];

        assert!(validate_profile_drafts(&profiles).is_err());
    }

    #[test]
    fn bounds_profile_count() {
        let profiles = (0..65)
            .map(|index| draft(&format!("Slicer {index}"), &format!("/apps/{index}.app")))
            .collect::<Vec<_>>();

        assert!(validate_profile_drafts(&profiles).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn assigns_an_opaque_id_and_pins_the_selected_executable() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("CustomSlicer");
        executable(&path, b"slicer executable");

        let saved =
            build_profile_collection(&[draft(" Custom Slicer ", &path.to_string_lossy())], &[])
                .unwrap();

        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].name, "Custom Slicer");
        assert!(uuid::Uuid::parse_str(&saved[0].id).is_ok());
        assert!(saved[0]
            .identity
            .as_ref()
            .is_some_and(profile_identity_is_current));
    }

    #[cfg(unix)]
    #[test]
    fn profile_update_preserves_its_id_and_unknown_ids_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("CustomSlicer");
        executable(&path, b"slicer executable");
        let original =
            build_profile_collection(&[draft("Original", &path.to_string_lossy())], &[]).unwrap();
        let update = SlicerProfileDraft {
            id: Some(original[0].id.clone()),
            name: "Renamed".into(),
            path: path.to_string_lossy().into_owned(),
            trust: false,
        };

        let updated = build_profile_collection(&[update], &original).unwrap();
        assert_eq!(updated[0].id, original[0].id);
        assert_eq!(updated[0].name, "Renamed");

        let unknown = SlicerProfileDraft {
            id: Some("not-a-saved-profile".into()),
            name: "Unknown".into(),
            path: path.to_string_lossy().into_owned(),
            trust: false,
        };
        assert!(build_profile_collection(&[unknown], &original)
            .unwrap_err()
            .contains("Unknown slicer profile ID"));
    }

    #[cfg(unix)]
    #[test]
    fn profile_replacement_cannot_become_trusted_by_a_name_edit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("CustomSlicer");
        executable(&path, b"original executable");
        let original =
            build_profile_collection(&[draft("Original", &path.to_string_lossy())], &[]).unwrap();
        let replacement = directory.path().join("replacement");
        executable(&replacement, b"replacement executable");
        fs::rename(&replacement, &path).unwrap();

        let untrusted_edit = SlicerProfileDraft {
            id: Some(original[0].id.clone()),
            name: "Renamed".into(),
            path: path.to_string_lossy().into_owned(),
            trust: false,
        };
        assert!(build_profile_collection(&[untrusted_edit], &original)
            .unwrap_err()
            .contains("requires explicit trust confirmation"));

        let explicitly_trusted = SlicerProfileDraft {
            id: Some(original[0].id.clone()),
            name: "Renamed".into(),
            path: path.to_string_lossy().into_owned(),
            trust: true,
        };
        let updated = build_profile_collection(&[explicitly_trusted], &original).unwrap();
        assert_eq!(updated[0].name, "Renamed");
        assert!(updated[0]
            .identity
            .as_ref()
            .is_some_and(profile_identity_is_current));
    }

    #[cfg(unix)]
    fn executable(path: &Path, bytes: &[u8]) {
        use std::os::unix::fs::PermissionsExt;

        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn captures_user_selected_executable_and_detects_replacement() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let selected = directory.path().join("Slicer");
        executable(&selected, b"first executable");

        let identity = capture_profile_identity(&selected).unwrap();
        assert!(profile_identity_is_current(&identity));

        let replacement = directory.path().join("replacement");
        executable(&replacement, b"second executable");
        fs::rename(&replacement, &selected).unwrap();
        assert!(!profile_identity_is_current(&identity));

        let linked = directory.path().join("linked-slicer");
        symlink(&selected, &linked).unwrap();
        assert!(capture_profile_identity(&linked).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn captures_app_bundle_identity_and_rejects_bundle_replacement() {
        use plist::Value;
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let app = directory.path().join("CustomSlicer.app");
        let contents = app.join("Contents");
        let executable_dir = contents.join("MacOS");
        fs::create_dir_all(&executable_dir).unwrap();
        fs::write(executable_dir.join("CustomSlicer"), b"slicer executable").unwrap();
        fs::set_permissions(
            executable_dir.join("CustomSlicer"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let mut plist = plist::Dictionary::new();
        plist.insert(
            "CFBundleIdentifier".to_string(),
            Value::String("com.example.custom-slicer".to_string()),
        );
        plist.insert(
            "CFBundleExecutable".to_string(),
            Value::String("CustomSlicer".to_string()),
        );
        Value::Dictionary(plist)
            .to_file_xml(contents.join("Info.plist"))
            .unwrap();

        let identity = capture_profile_identity(&app).unwrap();
        assert_eq!(
            identity.bundle_id.as_deref(),
            Some("com.example.custom-slicer")
        );
        assert!(profile_identity_is_current(&identity));

        fs::write(
            executable_dir.join("CustomSlicer"),
            b"replacement executable",
        )
        .unwrap();
        assert!(!profile_identity_is_current(&identity));
    }
}
