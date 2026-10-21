//! Read-only validation of an existing catalog selection. No filesystem access,
//! source admission, production wiring, or generated-file ownership is provided.

use library_core::{normalize_thumbnail_catalog_path, ThumbnailReadSnapshot};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogDisplayKind {
    Custom,
    LegacyUnbound,
    CurrentRecipe,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogPngError {
    UnsafePath,
    MissingStoredDigest,
    InvalidPng,
    DigestMismatch,
    StaleSource,
    StaleRecipe,
    UnsupportedRecipe,
}
impl std::fmt::Display for CatalogPngError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::UnsafePath => "Catalog preview path is unsafe",
            Self::MissingStoredDigest => "Catalog preview has no valid stored digest",
            Self::InvalidPng => "Catalog preview is not a bounded valid PNG",
            Self::DigestMismatch => "Catalog preview differs from its stored digest",
            Self::StaleSource => "Catalog preview source binding is stale",
            Self::StaleRecipe => "Catalog preview recipe is stale",
            Self::UnsupportedRecipe => "Catalog preview recipe is unsupported",
        })
    }
}
impl std::error::Error for CatalogPngError {}

/// A borrowed, digest-checked display value only. It cannot become renderer proof,
/// a producer payload, a file receipt, or filesystem mutation authority.
#[derive(Debug)]
pub struct ValidatedCatalogPng<'a> {
    bytes: &'a [u8],
    kind: CatalogDisplayKind,
}
impl<'a> ValidatedCatalogPng<'a> {
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }
    pub fn display_kind(&self) -> CatalogDisplayKind {
        self.kind
    }
    pub fn reusable_current_recipe(&self) -> bool {
        self.kind == CatalogDisplayKind::CurrentRecipe
    }
}

pub fn validate_catalog_png<'a>(
    row: &ThumbnailReadSnapshot,
    bytes: &'a [u8],
) -> Result<ValidatedCatalogPng<'a>, CatalogPngError> {
    validate_catalog_png_with_path_mode(row, bytes, cfg!(windows))
}

fn validate_catalog_png_with_path_mode<'a>(
    row: &ThumbnailReadSnapshot,
    bytes: &'a [u8],
    windows_separators: bool,
) -> Result<ValidatedCatalogPng<'a>, CatalogPngError> {
    let text = row
        .relative_path
        .to_str()
        .ok_or(CatalogPngError::UnsafePath)?;
    let normalized = normalize_thumbnail_catalog_path(text, windows_separators)
        .ok_or(CatalogPngError::UnsafePath)?;
    let parts = normalized.split('/').collect::<Vec<_>>();
    if row.png_hash.len() != 64 || !row.png_hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(CatalogPngError::MissingStoredDigest);
    }
    if bytes.len() as u64 > crate::thumbnails::MAX_CACHED_PNG_BYTES {
        return Err(CatalogPngError::InvalidPng);
    }
    if !format!("{:x}", Sha256::digest(bytes)).eq_ignore_ascii_case(&row.png_hash) {
        return Err(CatalogPngError::DigestMismatch);
    }
    crate::thumbnails::validate_png_bytes(bytes).map_err(|_| CatalogPngError::InvalidPng)?;
    let kind = match (row.is_custom, row.source_content_hash.as_deref()) {
        (true, _) => CatalogDisplayKind::Custom,
        (false, None) => CatalogDisplayKind::LegacyUnbound,
        (false, Some(source)) => {
            if !row
                .current_content_hash
                .as_deref()
                .is_some_and(|hash| hash.eq_ignore_ascii_case(source))
            {
                return Err(CatalogPngError::StaleSource);
            }
            if parts.len() != 2 {
                return Err(CatalogPngError::UnsupportedRecipe);
            }
            let recipe =
                crate::thumbnails::ThumbnailRecipe::parse_filename(std::ffi::OsStr::new(parts[1]))
                    .ok_or(CatalogPngError::UnsupportedRecipe)?;
            if !recipe.matches_catalog(source, &row.settings, &row.png_hash) {
                return Err(CatalogPngError::StaleRecipe);
            }
            CatalogDisplayKind::CurrentRecipe
        }
    };
    Ok(ValidatedCatalogPng { bytes, kind })
}

#[cfg(test)]
mod tests {
    use super::*;
    use library_core::{CatalogSettings, ThumbnailReadSnapshot};
    use sha2::{Digest, Sha256};
    use std::path::PathBuf;

    fn png() -> Vec<u8> {
        let mut output = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut output, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[1, 2, 3, 255])
                .unwrap();
        }
        output
    }
    fn snapshot(bytes: &[u8]) -> ThumbnailReadSnapshot {
        ThumbnailReadSnapshot {
            relative_path: PathBuf::from(format!(
                "thumbnails/{}-isometric-v1-128-12171f.png",
                "a".repeat(64)
            )),
            png_hash: format!("{:x}", Sha256::digest(bytes)),
            source_content_hash: Some("a".repeat(64)),
            is_custom: false,
            current_content_hash: Some("a".repeat(64)),
            settings: CatalogSettings {
                thumbnail_size: 128,
                thumbnail_background: "#12171F".into(),
                thumbnail_concurrency: 1,
                scan_batch_size: 128,
            },
        }
    }
    #[test]
    fn catalog_thumbnail_valid_bound_legacy_and_ca_are_read_reusable_only() {
        let bytes = png();
        let mut row = snapshot(&bytes);
        let read = validate_catalog_png(&row, &bytes).unwrap();
        assert_eq!(read.display_kind(), CatalogDisplayKind::CurrentRecipe);
        assert!(read.reusable_current_recipe());
        assert_eq!(read.bytes(), bytes);
        row.relative_path = PathBuf::from(format!(
            "thumbnails/{}-isometric-v1-128-12171f-{}.png",
            "a".repeat(64),
            row.png_hash
        ));
        assert!(validate_catalog_png(&row, &bytes)
            .unwrap()
            .reusable_current_recipe());
        row.relative_path = PathBuf::from(format!(
            "thumbnails/{}-isometric-v1-128-12171f-{}.png",
            "a".repeat(64),
            "b".repeat(64)
        ));
        assert_eq!(
            validate_catalog_png(&row, &bytes).unwrap_err(),
            CatalogPngError::StaleRecipe
        );
    }
    #[test]
    fn catalog_thumbnail_offline_custom_and_unbound_legacy_never_gain_producer_reuse() {
        let bytes = png();
        let mut row = snapshot(&bytes);
        row.current_content_hash = None;
        row.source_content_hash = None;
        row.relative_path = PathBuf::from("thumbnails/nested/legacy.png");
        let before = row.clone();
        let read = validate_catalog_png(&row, &bytes).unwrap();
        assert_eq!(read.display_kind(), CatalogDisplayKind::LegacyUnbound);
        assert!(!read.reusable_current_recipe());
        assert_eq!(row, before);
        row.is_custom = true;
        row.source_content_hash = Some("b".repeat(64));
        let read = validate_catalog_png(&row, &bytes).unwrap();
        assert_eq!(read.display_kind(), CatalogDisplayKind::Custom);
        assert!(!read.reusable_current_recipe());
        row.is_custom = false;
        row.source_content_hash = None;
        row.current_content_hash = Some("a".repeat(64));
        row.relative_path = snapshot(&bytes).relative_path;
        assert!(!validate_catalog_png(&row, &bytes)
            .unwrap()
            .reusable_current_recipe());
    }
    #[test]
    fn catalog_thumbnail_rejects_stale_source_settings_and_unsupported_embedded_key() {
        let bytes = png();
        let original = snapshot(&bytes);
        for field in ["source", "size", "background", "embedded", "nested"] {
            let mut row = original.clone();
            match field {
                "source" => row.current_content_hash = Some("b".repeat(64)),
                "size" => row.settings.thumbnail_size = 256,
                "background" => row.settings.thumbnail_background = "#000000".into(),
                "embedded" => {
                    row.relative_path = PathBuf::from(format!(
                        "thumbnails/{}-embedded-copy-v1-{}.png",
                        "a".repeat(64),
                        row.png_hash
                    ))
                }
                _ => row.relative_path = PathBuf::from("thumbnails/nested/generated.png"),
            }
            assert!(validate_catalog_png(&row, &bytes).is_err(), "{field}");
        }
    }
    #[test]
    fn catalog_thumbnail_requires_stored_digest_even_for_custom_and_legacy() {
        let bytes = png();
        for custom in [false, true] {
            for digest in ["".to_owned(), "z".repeat(64), "b".repeat(64)] {
                let mut row = snapshot(&bytes);
                row.is_custom = custom;
                row.source_content_hash = None;
                row.png_hash = digest;
                assert!(validate_catalog_png(&row, &bytes).is_err());
            }
        }
        let mut row = snapshot(&bytes);
        row.png_hash.make_ascii_uppercase();
        assert!(validate_catalog_png(&row, &bytes).is_ok());
    }
    #[test]
    fn catalog_thumbnail_rejects_escape_and_noncanonical_paths_without_filesystem_access() {
        let bytes = png();
        for path in [
            "/thumbnails/a.png",
            "../thumbnails/a.png",
            "thumbnails/../a.png",
            "other/a.png",
            "thumbnails",
            "thumbnails/./a.png",
            "thumbnails//a.png",
            "thumbnails/a.png/",
            "thumbnails/a:stream",
            "thumbnails/..\\a.png",
        ] {
            let mut row = snapshot(&bytes);
            row.is_custom = true;
            row.relative_path = PathBuf::from(path);
            assert_eq!(
                validate_catalog_png(&row, &bytes).unwrap_err(),
                CatalogPngError::UnsafePath,
                "{path}"
            );
        }
    }
    #[test]
    fn catalog_thumbnail_rejects_invalid_or_oversized_png_and_mutates_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("untouched");
        std::fs::write(&path, b"keep").unwrap();
        for bytes in [b"not png".to_vec(), vec![0u8; 4 * 1024 * 1024 + 1]] {
            let row = snapshot(&bytes);
            assert_eq!(
                validate_catalog_png(&row, &bytes).unwrap_err(),
                CatalogPngError::InvalidPng
            );
        }
        assert_eq!(std::fs::read(&path).unwrap(), b"keep");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }
    #[test]
    fn catalog_thumbnail_rejects_decoded_dimension_overflow_with_a_matching_digest() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1025, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&vec![0; 1025 * 4])
                .unwrap();
        }
        let row = snapshot(&bytes);
        assert_eq!(
            validate_catalog_png(&row, &bytes).unwrap_err(),
            CatalogPngError::InvalidPng
        );
    }
    #[test]
    fn catalog_thumbnail_windows_mode_validates_whole_native_recipe_and_legacy_snapshot() {
        let bytes = png();
        let mut row = snapshot(&bytes);
        row.relative_path = PathBuf::from(format!(
            r"thumbnails\{}-isometric-v1-128-12171f-{}.png",
            "a".repeat(64),
            row.png_hash
        ));
        assert!(validate_catalog_png_with_path_mode(&row, &bytes, true)
            .unwrap()
            .reusable_current_recipe());
        assert_eq!(
            validate_catalog_png_with_path_mode(&row, &bytes, false).unwrap_err(),
            CatalogPngError::UnsafePath
        );
        row.source_content_hash = None;
        row.current_content_hash = None;
        row.relative_path = PathBuf::from("thumbnails/模型/café.png");
        assert_eq!(
            validate_catalog_png_with_path_mode(&row, &bytes, true)
                .unwrap()
                .display_kind(),
            CatalogDisplayKind::LegacyUnbound
        );
        row.relative_path = PathBuf::from(r"thumbnails\nested/legacy.png");
        assert_eq!(
            validate_catalog_png_with_path_mode(&row, &bytes, true)
                .unwrap()
                .display_kind(),
            CatalogDisplayKind::LegacyUnbound
        );
        for path in [
            r"thumbnails\..\outside.png",
            r"thumbnails/..\outside.png",
            r"C:\thumbnails\a.png",
            r"\\server\thumbnails\a.png",
            r"thumbnails\a.png:stream",
            r"thumbnails\.. \outside.png",
            r"thumbnails\CON.png",
            r"thumbnails\NUL\outside.png",
            r"thumbnails\COM1.png",
        ] {
            row.relative_path = PathBuf::from(path);
            assert_eq!(
                validate_catalog_png_with_path_mode(&row, &bytes, true).unwrap_err(),
                CatalogPngError::UnsafePath
            );
        }
    }
    #[cfg(windows)]
    #[test]
    fn catalog_thumbnail_windows_database_snapshot_passes_full_pure_validator() {
        let temp = tempfile::tempdir().unwrap();
        let db = library_core::LibraryDb::open(&temp.path().join("library")).unwrap();
        let model = db.create_model("Offline", None).unwrap();
        let bytes = png();
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let native = PathBuf::from("thumbnails")
            .join("nested")
            .join("custom.png");
        std::fs::create_dir_all(db.root().join("thumbnails/nested")).unwrap();
        std::fs::write(db.root().join(&native), &bytes).unwrap();
        db.set_custom_thumbnail(&model.id, model.metadata_version, &digest, &native)
            .unwrap();
        let row = db.thumbnail_read_snapshot(&model.id).unwrap().unwrap();
        assert_eq!(
            row.relative_path.to_str(),
            Some("thumbnails/nested/custom.png")
        );
        let read = validate_catalog_png(&row, &bytes).unwrap();
        assert_eq!(read.display_kind(), CatalogDisplayKind::Custom);
        assert!(!read.reusable_current_recipe());
    }
}
