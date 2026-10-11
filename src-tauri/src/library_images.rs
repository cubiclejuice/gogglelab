use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use image::{ImageDecoder, ImageFormat, ImageReader, Limits};
use library_core::Model;
use quick_xml::{events::Event, Reader};
use sha2::{Digest, Sha256};

use crate::error::AppError;
use crate::library_commands::{library_error, LibraryState};

const MAX_INPUT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_DIMENSION: u32 = 4096;
const MAX_PIXELS: u64 = 4 * 1024 * 1024;
const MAX_ALLOCATION: u64 = 64 * 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_OUTPUT_DIMENSION: u32 = 512;
const THUMBNAIL_RELATION: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail";
static IMPORT_LOCK: Mutex<()> = Mutex::new(());
static FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Only the two advertised raster formats are decoded. Check dimensions and
/// pixel count before allocating the full image, then normalize to a small PNG.
pub(crate) fn normalize_image(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err("Preview image must be between 1 byte and 16 MiB".into());
    }
    let format = image::guess_format(bytes).map_err(|_| "Choose a PNG or JPEG image".to_owned())?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg) {
        return Err("Only PNG and JPEG preview images are supported".into());
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_ALLOCATION);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|error| error.to_string())?;
    let (width, height) = decoder.dimensions();
    if width == 0
        || height == 0
        || width > MAX_DIMENSION
        || height > MAX_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_PIXELS
        || decoder.total_bytes() > MAX_ALLOCATION
    {
        return Err("Preview image must be at most 4096 pixels per side and 4 megapixels".into());
    }
    let orientation = decoder.orientation().map_err(|error| error.to_string())?;
    let mut decoded =
        image::DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?;
    decoded.apply_orientation(orientation);
    let decoded =
        if decoded.width() > MAX_OUTPUT_DIMENSION || decoded.height() > MAX_OUTPUT_DIMENSION {
            decoded.resize(
                MAX_OUTPUT_DIMENSION,
                MAX_OUTPUT_DIMENSION,
                image::imageops::FilterType::Triangle,
            )
        } else {
            decoded
        };
    let rgba = decoded.into_rgba8();
    let mut output = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut output, rgba.width(), rgba.height());
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
        writer
            .write_image_data(rgba.as_raw())
            .map_err(|error| error.to_string())?;
    }
    if output.len() > MAX_OUTPUT_BYTES {
        return Err("Normalized preview exceeds its output limit".into());
    }
    Ok(output)
}

#[tauri::command]
pub async fn library_set_custom_thumbnail(
    state: tauri::State<'_, LibraryState>,
    model_id: String,
    expected_version: u64,
    path: String,
) -> Result<Model, AppError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        import_custom_image(&state, &model_id, expected_version, Path::new(&path))
    })
    .await
    .map_err(|error| failure(error.to_string()))?
}

#[tauri::command]
pub async fn library_clear_custom_thumbnail(
    state: tauri::State<'_, LibraryState>,
    model_id: String,
    expected_version: u64,
) -> Result<Model, AppError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_db(|db| {
            db.clear_custom_thumbnail(&model_id, expected_version)
                .map_err(library_error)
        })
    })
    .await
    .map_err(|error| failure(error.to_string()))?
}

fn import_custom_image(
    state: &LibraryState,
    model_id: &str,
    expected_version: u64,
    path: &Path,
) -> Result<Model, AppError> {
    let _import = IMPORT_LOCK
        .lock()
        .map_err(|_| failure("Preview importer is unavailable"))?;
    let metadata = fs::symlink_metadata(path).map_err(|error| failure(error.to_string()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_INPUT_BYTES
    {
        return Err(failure(
            "Choose an ordinary PNG/JPEG file no larger than 16 MiB",
        ));
    }
    let mut input = Vec::new();
    fs::File::open(path)
        .map_err(|error| failure(error.to_string()))?
        .take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut input)
        .map_err(|error| failure(error.to_string()))?;
    let png = normalize_image(&input).map_err(failure)?;
    let png_hash = format!("{:x}", Sha256::digest(&png));
    state.with_db(|db| {
        let model = db
            .get_model(model_id)
            .map_err(library_error)?
            .ok_or_else(|| failure("Model no longer exists"))?;
        if model.metadata_version != expected_version {
            return Err(failure(
                "Metadata version changed; reload the catalog and try again",
            ));
        }
        let relative = PathBuf::from("thumbnails").join(format!("custom-{png_hash}.png"));
        store_png(db.root(), &relative, &png, &png_hash).map_err(failure)?;
        db.set_custom_thumbnail(model_id, expected_version, &png_hash, &relative)
            .map_err(library_error)
    })
}

fn store_png(root: &Path, relative: &Path, png: &[u8], hash: &str) -> Result<(), String> {
    if root.canonicalize().map_err(|error| error.to_string())? != root {
        return Err("Library root identity changed".into());
    }
    let parent = root.join("thumbnails");
    let directory = fs::symlink_metadata(&parent).map_err(|error| error.to_string())?;
    if !directory.is_dir()
        || directory.file_type().is_symlink()
        || parent.canonicalize().map_err(|error| error.to_string())? != parent
    {
        return Err("Preview directory is not an owned ordinary directory".into());
    }
    let output = root.join(relative);
    match fs::symlink_metadata(&output) {
        Ok(metadata) => {
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() > MAX_OUTPUT_BYTES as u64
            {
                return Err("Custom preview cache entry is unsafe".into());
            }
            let mut bytes = Vec::new();
            fs::File::open(&output)
                .map_err(|error| error.to_string())?
                .take(MAX_OUTPUT_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            if bytes.len() > MAX_OUTPUT_BYTES {
                return Err("Custom preview cache grew beyond its limit".into());
            }
            if format!("{:x}", Sha256::digest(&bytes)) != hash {
                return Err("Custom preview cache entry failed its hash check".into());
            }
            return Ok(());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    let staging = parent.join(format!(
        ".custom-{}-{}.part",
        std::process::id(),
        FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
            .map_err(|error| error.to_string())?;
        file.write_all(png).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        drop(file);
        // A hard link publishes without replacing any existing cache entry.
        fs::hard_link(&staging, &output).map_err(|error| error.to_string())?;
        Ok(())
    })();
    let _ = fs::remove_file(&staging);
    result
}

fn failure(message: impl Into<String>) -> AppError {
    AppError::Library {
        message: message.into(),
    }
}

/// Package thumbnails stay inside the ZIP. No external relationships or paths
/// are followed, and failed optional images can fall back to mesh rendering.
pub(crate) fn embedded_3mf_thumbnail(bytes: &[u8]) -> Result<Option<Vec<u8>>, String> {
    if !bytes.starts_with(b"PK") {
        return Ok(None);
    }
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    if archive.len() > 10_000 {
        return Err("3MF preview package has too many entries".into());
    }
    let mut candidates = Vec::new();
    if let Some(relations) = package_entry(&mut archive, "_rels/.rels", 64 * 1024)? {
        let mut reader = Reader::from_reader(relations.as_slice());
        let mut xml_version = quick_xml::XmlVersion::Implicit1_0;
        reader.config_mut().trim_text(true);
        loop {
            match reader.read_event().map_err(|error| error.to_string())? {
                Event::Empty(node) | Event::Start(node)
                    if node.local_name().as_ref() == b"Relationship" =>
                {
                    let mut kind = None;
                    let mut target = None;
                    let mut external = false;
                    for attribute in node.attributes().with_checks(true) {
                        let attribute = attribute.map_err(|error| error.to_string())?;
                        let value = attribute
                            .decoded_and_normalized_value(xml_version, reader.decoder())
                            .map_err(|error| error.to_string())?
                            .into_owned();
                        match attribute.key.local_name().as_ref() {
                            b"Type" => kind = Some(value),
                            b"Target" => target = Some(value),
                            b"TargetMode" => external = value.eq_ignore_ascii_case("External"),
                            _ => {}
                        }
                    }
                    if kind.as_deref() == Some(THUMBNAIL_RELATION) && !external {
                        if let Some(target) = target.and_then(|target| safe_package_image(&target))
                        {
                            candidates.push(target);
                        }
                    }
                }
                Event::Decl(declaration) => {
                    xml_version = match declaration
                        .version()
                        .map_err(|error| error.to_string())?
                        .as_ref()
                    {
                        b"1.0" => quick_xml::XmlVersion::Explicit1_0,
                        b"1.1" => quick_xml::XmlVersion::Explicit1_1,
                        _ => {
                            return Err("Unsupported XML version in thumbnail relationships".into())
                        }
                    };
                }
                Event::DocType(_) => {
                    return Err("3MF thumbnail relationships cannot contain a DTD".into())
                }
                Event::Eof => break,
                _ => {}
            }
        }
    }
    for name in [
        "Metadata/thumbnail.png",
        "Metadata/thumbnail.jpg",
        "Metadata/thumbnail.jpeg",
        "thumbnail.png",
    ] {
        if !candidates.iter().any(|candidate| candidate == name) {
            candidates.push(name.into());
        }
    }
    for name in candidates {
        if let Some(bytes) = package_entry(&mut archive, &name, MAX_INPUT_BYTES)? {
            if let Ok(png) = normalize_image(&bytes) {
                return Ok(Some(png));
            }
        }
    }
    Ok(None)
}

fn safe_package_image(target: &str) -> Option<String> {
    let target = target.strip_prefix('/').unwrap_or(target);
    let path = Path::new(target);
    if target.is_empty()
        || target.len() > 4096
        || target.contains('\\')
        || target.contains(':')
        || target.chars().any(char::is_control)
        || target
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return None;
    }
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    matches!(extension.as_str(), "png" | "jpg" | "jpeg").then(|| target.to_owned())
}

fn package_entry(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
    limit: u64,
) -> Result<Option<Vec<u8>>, String> {
    let mut entry = match archive.by_name(name) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if entry.encrypted()
        || !entry.is_file()
        || entry.size() > limit
        || !matches!(
            entry.compression(),
            zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated
        )
        || entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
    {
        return Err("3MF preview image entry is unsafe or too large".into());
    }
    let mut bytes = Vec::new();
    entry
        .by_ref()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("3MF preview inflated beyond its limit".into());
    }
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&vec![127; (width * height * 4) as usize])
                .unwrap();
        }
        bytes
    }
    fn dimensions(bytes: &[u8]) -> (u32, u32) {
        let reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
        (reader.info().width, reader.info().height)
    }
    #[test]
    fn accepts_real_png_and_preserves_aspect_ratio_with_bounded_output() {
        let normalized = normalize_image(&png(1024, 512)).unwrap();
        assert_eq!(dimensions(&normalized), (512, 256));
        assert!(normalized.len() <= 4 * 1024 * 1024);
        assert!(normalize_image(b"<svg onload='alert(1)'/>").is_err());
        assert!(normalize_image(b"\x89PNG\r\n\x1a\nbroken").is_err());
    }
    #[test]
    fn rejects_input_bytes_dimensions_and_pixel_bombs_before_retaining_output() {
        assert!(normalize_image(&vec![0; 16 * 1024 * 1024 + 1]).is_err());
        assert!(normalize_image(&png(4097, 1)).is_err());
        assert!(normalize_image(&png(2049, 2049)).is_err());
    }

    fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut bytes)
            .encode(
                &vec![70; (width * height * 3) as usize],
                width,
                height,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        bytes
    }
    fn package(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            archive
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(bytes).unwrap();
        }
        archive.finish().unwrap().into_inner()
    }
    #[test]
    fn jpeg_normalizes_to_valid_rgba_png_without_upscaling() {
        let output = normalize_image(&jpeg(48, 24)).unwrap();
        assert_eq!(dimensions(&output), (48, 24));
        let reader = png::Decoder::new(Cursor::new(output)).read_info().unwrap();
        assert_eq!(reader.info().color_type, png::ColorType::Rgba);
    }
    #[test]
    fn follows_only_safe_internal_thumbnail_relationships_and_rejects_dtd() {
        let relations = format!(
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Target="/Metadata/custom.jpg" Id="thumbnail" Type="{THUMBNAIL_RELATION}"/></Relationships>"#
        );
        let bytes = package(&[
            ("_rels/.rels", relations.into_bytes()),
            ("Metadata/custom.jpg", jpeg(48, 24)),
        ]);
        let output = embedded_3mf_thumbnail(&bytes).unwrap().unwrap();
        assert_eq!(dimensions(&output), (48, 24));
        for (target, mode) in [
            ("../escape.png", ""),
            (
                "https://example.test/image.png",
                r#" TargetMode="External""#,
            ),
            ("//example.test/image.png", ""),
        ] {
            let relations = format!(
                r#"<Relationships><Relationship Target="{target}" Type="{THUMBNAIL_RELATION}"{mode}/></Relationships>"#
            );
            let bytes = package(&[("_rels/.rels", relations.into_bytes()), (target, png(8, 8))]);
            assert!(embedded_3mf_thumbnail(&bytes).unwrap().is_none());
        }
        let bytes = package(&[(
            "_rels/.rels",
            b"<!DOCTYPE Relationships [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><Relationships/>"
                .to_vec(),
        )]);
        assert!(embedded_3mf_thumbnail(&bytes).is_err());
        let bytes = package(&[("Metadata/thumbnail.png", vec![0; 16 * 1024 * 1024 + 1])]);
        assert!(embedded_3mf_thumbnail(&bytes).is_err());
    }
    fn fixture(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "gogglelab-images-{label}-{}-{}",
            std::process::id(),
            FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        root
    }
    #[test]
    fn custom_preview_survives_explicit_regeneration_and_stale_edits_without_source_access() {
        let root = fixture("custom");
        let source = root.join("source.stl");
        fs::write(&source, vec![0u8; 84]).unwrap();
        let image = root.join("user-photo.jpg");
        let original_image = jpeg(48, 24);
        fs::write(&image, &original_image).unwrap();
        let state = LibraryState::new(root.join("catalog"));
        let model = state
            .with_db(|db| {
                db.import_file(&source, None, None)
                    .map(|result| result.model)
                    .map_err(library_error)
            })
            .unwrap();
        let custom =
            import_custom_image(&state, &model.id, model.metadata_version, &image).unwrap();
        assert!(import_custom_image(&state, &model.id, model.metadata_version, &image).is_err());
        let original_path = state
            .with_db(|db| db.resolve_model_source(&model.id).map_err(library_error))
            .unwrap();
        fs::remove_file(original_path).unwrap();
        let cached = crate::library_commands::get_thumbnail(&state, &model.id, false)
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::library_commands::get_thumbnail(&state, &model.id, true)
                .unwrap()
                .as_deref(),
            Some(cached.as_str())
        );
        assert_eq!(fs::read(&image).unwrap(), original_image);
        state
            .with_db(|db| {
                db.clear_custom_thumbnail(&model.id, custom.metadata_version)
                    .map_err(library_error)
            })
            .unwrap();
        assert!(crate::library_commands::get_thumbnail(&state, &model.id, true).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn custom_import_refuses_symlinked_input_or_cache_ancestor_without_outside_writes() {
        use std::os::unix::fs::symlink;
        let root = fixture("symlink");
        let source = root.join("source.stl");
        fs::write(&source, vec![0; 84]).unwrap();
        let image = root.join("image.png");
        fs::write(&image, png(8, 8)).unwrap();
        let input_link = root.join("image-link.png");
        symlink(&image, &input_link).unwrap();
        let state = LibraryState::new(root.join("catalog"));
        let model = state
            .with_db(|db| {
                db.import_file(&source, None, None)
                    .map(|result| result.model)
                    .map_err(library_error)
            })
            .unwrap();
        assert!(
            import_custom_image(&state, &model.id, model.metadata_version, &input_link).is_err()
        );
        let outside = root.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::remove_dir(root.join("catalog/thumbnails")).unwrap();
        symlink(&outside, root.join("catalog/thumbnails")).unwrap();
        assert!(import_custom_image(&state, &model.id, model.metadata_version, &image).is_err());
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn real_3mf_embedded_jpeg_becomes_a_cached_png_without_touching_original() {
        let root = fixture("embedded");
        let source = root.join("source.3mf");
        let model=br#"<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" unit="millimeter"><resources><object id="1" type="model"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="10" y="0" z="0"/><vertex x="0" y="10" z="0"/></vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object></resources><build><item objectid="1"/></build></model>"#.to_vec();
        let relations = format!(
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="model" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel" Target="/3D/3dmodel.model"/><Relationship Id="preview" Type="{THUMBNAIL_RELATION}" Target="/Metadata/custom.jpg"/></Relationships>"#
        );
        let bytes = package(&[
            ("_rels/.rels", relations.into_bytes()),
            ("3D/3dmodel.model", model),
            ("Metadata/custom.jpg", jpeg(48, 24)),
        ]);
        assert_eq!(
            stl_core::parse_model(&bytes).unwrap().format,
            stl_core::StlFormat::ThreeMf
        );
        fs::write(&source, &bytes).unwrap();
        let modified = fs::metadata(&source).unwrap().modified().unwrap();
        let source_hash = format!("{:x}", Sha256::digest(&bytes));
        let library = root.join("library");
        fs::create_dir(&library).unwrap();
        fs::create_dir(library.join("thumbnails")).unwrap();
        let settings = library_core::CatalogSettings::default();
        let artifact = crate::library_thumbnail::ensure_thumbnail(
            &library,
            &source,
            &source_hash,
            bytes.len() as u64,
            &settings,
            false,
        )
        .unwrap();
        let output = fs::read(library.join(artifact.relative_path)).unwrap();
        assert_eq!(dimensions(&output), (48, 24));
        assert_eq!(format!("{:x}", Sha256::digest(&output)), artifact.png_hash);
        assert_eq!(fs::read(&source).unwrap(), bytes);
        assert_eq!(fs::metadata(&source).unwrap().modified().unwrap(), modified);
        fs::remove_dir_all(root).unwrap();
    }
}
