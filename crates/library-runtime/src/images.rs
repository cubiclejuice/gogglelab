use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{Cursor, Read, Seek, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use image::{ImageDecoder, ImageFormat, ImageReader, Limits};
use quick_xml::events::Event;
use sha2::{Digest, Sha256};
use stl_core::package_bounds::{BinaryRole, PackageProcessingContext, TextRole};
use stl_core::xml_bounds::XmlEventReader;

use crate::png_output::BoundedPngOutput;
use crate::secure_fs::Directory;
use crate::ProcessingError;
use stl_core::zip_bounds::{
    BoundedZipArchive, BoundedZipMember, ZipOpenError, ZipResourceObserver,
};

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
pub fn normalize_image(bytes: &[u8]) -> Result<Vec<u8>, String> {
    normalize_image_typed(bytes).map_err(|error| error.to_string())
}

pub(crate) fn normalize_image_typed(bytes: &[u8]) -> Result<Vec<u8>, ProcessingError> {
    normalize_image_with_output_limit_typed(bytes, MAX_OUTPUT_BYTES)
}

#[cfg(test)]
fn normalize_image_with_output_limit(bytes: &[u8], output_limit: usize) -> Result<Vec<u8>, String> {
    normalize_image_with_output_limit_typed(bytes, output_limit).map_err(|error| error.to_string())
}

fn normalize_image_with_output_limit_typed(
    bytes: &[u8],
    output_limit: usize,
) -> Result<Vec<u8>, ProcessingError> {
    normalize_image_with_output(bytes, BoundedPngOutput::new(output_limit))
}

fn normalize_image_with_output(
    bytes: &[u8],
    mut output: BoundedPngOutput,
) -> Result<Vec<u8>, ProcessingError> {
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
    let encoded = (|| {
        let mut encoder = png::Encoder::new(&mut output, rgba.width(), rgba.height());
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(rgba.as_raw())?;
        writer.finish()
    })();
    if output.allocation_failed() {
        return Err(ProcessingError::AllocationFailed);
    }
    if output.limit_exceeded() {
        return Err("Normalized preview exceeds its output limit".into());
    }
    encoded.map_err(|error| error.to_string())?;
    output.into_bytes().map_err(ProcessingError::from_io)
}

pub fn import_custom_image_file(root: &Path, path: &Path) -> Result<(PathBuf, String), String> {
    let _import = IMPORT_LOCK
        .lock()
        .map_err(|_| "Preview importer is unavailable".to_owned())?;
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_INPUT_BYTES
    {
        return Err("Choose an ordinary PNG/JPEG file no larger than 16 MiB".into());
    }
    let mut input = Vec::new();
    fs::File::open(path)
        .map_err(|error| error.to_string())?
        .take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut input)
        .map_err(|error| error.to_string())?;
    let png = normalize_image(&input)?;
    let png_hash = format!("{:x}", Sha256::digest(&png));
    let relative = PathBuf::from("thumbnails").join(format!("custom-{png_hash}.png"));
    store_png(root, &relative, &png, &png_hash)?;
    Ok((relative, png_hash))
}

pub fn store_png(root: &Path, relative: &Path, png: &[u8], hash: &str) -> Result<(), String> {
    store_png_with_publish_hook(root, relative, png, hash, || {})
}

fn store_png_with_publish_hook<F>(
    root: &Path,
    relative: &Path,
    png: &[u8],
    hash: &str,
    after_staging: F,
) -> Result<(), String>
where
    F: FnOnce(),
{
    if root.canonicalize().map_err(|error| error.to_string())? != root {
        return Err("Library root identity changed".into());
    }
    if format!("{:x}", Sha256::digest(png)) != hash {
        return Err("Custom preview bytes do not match their content hash".into());
    }
    let expected_relative = PathBuf::from("thumbnails").join(format!("custom-{hash}.png"));
    if relative != expected_relative {
        return Err("Custom preview path does not match its content hash".into());
    }
    let root_directory = Directory::open(root).map_err(|error| error.to_string())?;
    let directory = root_directory
        .open_child(OsStr::new("thumbnails"))
        .map_err(|_| "Preview directory is not an owned ordinary directory")?;
    let output = root.join(relative);
    let output_name = output
        .file_name()
        .ok_or_else(|| "Custom preview filename is invalid".to_string())?;
    match directory.open_file(output_name) {
        Ok(file) => {
            let bytes = read_bounded_file(file, MAX_OUTPUT_BYTES as u64)?;
            if format!("{:x}", Sha256::digest(&bytes)) != hash {
                return Err("Custom preview cache entry failed its hash check".into());
            }
            return Ok(());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    let staging_name = OsString::from(format!(
        ".custom-{}-{}.part",
        std::process::id(),
        FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = directory
        .create_new_file(&staging_name)
        .map_err(|error| error.to_string())?;
    let write_result = file.write_all(png).and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = write_result {
        let _ = directory.hold_entry(&staging_name);
        return Err(error.to_string());
    }
    after_staging();
    if let Err(error) = directory.rename_child_exclusive(&staging_name, output_name) {
        let _ = directory.hold_entry(&staging_name);
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            let file = directory
                .open_file(output_name)
                .map_err(|open_error| open_error.to_string())?;
            let existing = read_bounded_file(file, MAX_OUTPUT_BYTES as u64)?;
            if format!("{:x}", Sha256::digest(&existing)) == hash {
                return Ok(());
            }
            return Err("Custom preview cache entry failed its hash check".into());
        }
        return Err(error.to_string());
    }
    let published = match directory.open_file(output_name) {
        Ok(file) => file,
        Err(error) => {
            let _ = directory.hold_entry(output_name);
            return Err(error.to_string());
        }
    };
    let published_bytes = match read_bounded_file(published, MAX_OUTPUT_BYTES as u64) {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = directory.hold_entry(output_name);
            return Err(error);
        }
    };
    if format!("{:x}", Sha256::digest(&published_bytes)) != hash {
        let _ = directory.hold_entry(output_name);
        return Err("Published custom preview failed its content hash check".into());
    }
    Ok(())
}

fn read_bounded_file(file: fs::File, limit: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("Custom preview cache grew beyond its limit".into());
    }
    Ok(bytes)
}

/// Package thumbnails stay inside the ZIP. No external relationships or paths
/// are followed, and failed optional images can fall back to mesh rendering.
pub fn embedded_3mf_thumbnail(bytes: &[u8]) -> Result<Option<Vec<u8>>, String> {
    embedded_3mf_thumbnail_typed(bytes).map_err(|error| error.to_string())
}

pub(crate) fn embedded_3mf_thumbnail_typed(
    bytes: &[u8],
) -> Result<Option<Vec<u8>>, ProcessingError> {
    let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
    embedded_3mf_thumbnail_in_context_typed(bytes, &mut context)
}

pub(crate) fn embedded_3mf_thumbnail_in_context_typed(
    bytes: &[u8],
    context: &mut PackageProcessingContext,
) -> Result<Option<Vec<u8>>, ProcessingError> {
    let observer = context.observer();
    observer.check()?;
    let result = if bytes.starts_with(b"PK") {
        select_embedded_reader_in_context(Cursor::new(bytes), context, normalize_image_typed)
    } else {
        Ok(None)
    };
    observer.finish(result)
}

#[cfg(test)]
fn select_embedded_thumbnail(
    bytes: &[u8],
    observer: &ZipResourceObserver,
    normalize: impl FnMut(&[u8]) -> Result<Vec<u8>, ProcessingError>,
) -> Result<Option<Vec<u8>>, ProcessingError> {
    let mut context = PackageProcessingContext::new(observer.clone());
    if !bytes.starts_with(b"PK") {
        return observer.finish(Ok(None));
    }
    select_embedded_reader_in_context(Cursor::new(bytes), &mut context, normalize)
}

#[cfg(test)]
fn select_embedded_reader<R: Read + Seek>(
    source: R,
    observer: &ZipResourceObserver,
    normalize: impl FnMut(&[u8]) -> Result<Vec<u8>, ProcessingError>,
) -> Result<Option<Vec<u8>>, ProcessingError> {
    let mut context = PackageProcessingContext::new(observer.clone());
    select_embedded_reader_in_context(source, &mut context, normalize)
}

fn select_embedded_reader_in_context<R: Read + Seek>(
    source: R,
    context: &mut PackageProcessingContext,
    mut normalize: impl FnMut(&[u8]) -> Result<Vec<u8>, ProcessingError>,
) -> Result<Option<Vec<u8>>, ProcessingError> {
    let observer = context.observer();
    let result = (|| {
        observer.check()?;
        let mut archive = observer.open_bounded_zip(source)?;
        if archive.len() > 10_000 {
            return Err("3MF preview package has too many entries".into());
        }
        let mut candidates = Vec::new();
        if let Some(entry) = checked_package_entry(
            &mut archive,
            "_rels/.rels",
            TextRole::Relationships.byte_limit() as u64,
        )? {
            let relations = context
                .collect_text(entry, TextRole::Relationships)
                .map_err(ProcessingError::from)?;
            let mut reader = XmlEventReader::new(relations.as_bytes(), observer.clone())
                .map_err(ProcessingError::from)?;
            let mut xml_version = quick_xml::XmlVersion::Implicit1_0;
            reader.set_trim_text(true);
            loop {
                let event = reader.next_event();
                observer.check()?;
                match event.map_err(|error| error.to_string())? {
                    Event::Empty(node) | Event::Start(node)
                        if node.local_name().as_ref() == b"Relationship" =>
                    {
                        let mut kind = None;
                        let mut target = None;
                        let mut external = false;
                        for attribute in node.attributes().with_checks(true) {
                            let attribute = attribute.map_err(|error| error.to_string())?;
                            let value = attribute
                                .normalized_value(xml_version)
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
                            if let Some(target) =
                                target.and_then(|target| safe_package_image(&target))
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
                                return Err(
                                    "Unsupported XML version in thumbnail relationships".into()
                                );
                            }
                        };
                    }
                    Event::DocType(_) => {
                        return Err("3MF thumbnail relationships cannot contain a DTD".into());
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
            if let Some(entry) = checked_package_entry(&mut archive, &name, MAX_INPUT_BYTES)? {
                let bytes = context
                    .collect_binary(entry, BinaryRole::EmbeddedImage)
                    .map_err(ProcessingError::from)?;
                match normalize(&bytes) {
                    Ok(png) => return Ok(Some(png)),
                    Err(error) if error.is_terminal() => return Err(error),
                    Err(_) => {}
                }
            }
        }
        Ok(None)
    })();
    observer.finish(result)
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

fn checked_package_entry<'a, R: Read + Seek>(
    archive: &'a mut BoundedZipArchive<R>,
    name: &str,
    limit: u64,
) -> Result<Option<BoundedZipMember<'a, R>>, ProcessingError> {
    let entry = match archive.by_name(name) {
        Ok(entry) => entry,
        Err(ZipOpenError::Archive(zip::result::ZipError::FileNotFound)) => return Ok(None),
        Err(error) => return Err(error.into()),
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
    Ok(Some(entry))
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
    fn normalization_bounded_output_preserves_bytes_and_requires_complete_iend() {
        let input = png(48, 24);
        let baseline = input.clone();
        let output = normalize_image_with_output_limit(&input, baseline.len()).unwrap();
        assert_eq!(output, baseline);
        assert_eq!(normalize_image(&input).unwrap(), baseline);
        assert_eq!(
            normalize_image_with_output_limit(&input, baseline.len() - 1).unwrap_err(),
            "Normalized preview exceeds its output limit"
        );
        assert_eq!(
            &output[output.len() - 12..],
            b"\0\0\0\0IEND\xae\x42\x60\x82"
        );
        let mut reader = png::Decoder::new(Cursor::new(&output)).read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut pixels).unwrap();
        reader.finish().unwrap();
        assert_eq!(pixels, vec![127; 48 * 24 * 4]);
        drop(reader);

        let mut bad_crc = output;
        *bad_crc.last_mut().unwrap() ^= 1;
        let mut reader = png::Decoder::new(Cursor::new(&bad_crc))
            .read_info()
            .unwrap();
        reader.next_frame(&mut pixels).unwrap();
        assert!(reader.finish().is_err());
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

    #[test]
    fn custom_cache_rejects_a_hash_that_does_not_match_the_published_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("catalog");
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("thumbnails")).unwrap();
        let root = root.canonicalize().unwrap();
        let bytes = png(8, 8);
        let relative = PathBuf::from(format!("thumbnails/custom-{}.png", "0".repeat(64)));

        assert!(store_png(&root, &relative, &bytes, &"0".repeat(64)).is_err());
        assert_eq!(fs::read_dir(root.join("thumbnails")).unwrap().count(), 0);
    }

    #[test]
    fn custom_cache_verifies_and_holds_bytes_replaced_before_publication() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("catalog");
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("thumbnails")).unwrap();
        let root = root.canonicalize().unwrap();
        let bytes = png(8, 8);
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let relative = PathBuf::from(format!("thumbnails/custom-{hash}.png"));
        let replacement = b"concurrent replacement bytes";
        let staging_directory = root.join("thumbnails");

        let result = store_png_with_publish_hook(&root, &relative, &bytes, &hash, || {
            let staging = fs::read_dir(&staging_directory)
                .unwrap()
                .filter_map(Result::ok)
                .find(|entry| entry.file_name().to_string_lossy().ends_with(".part"))
                .unwrap()
                .path();
            fs::remove_file(&staging).unwrap();
            fs::write(staging, replacement).unwrap();
        });

        assert!(result.is_err());
        assert!(!root.join(&relative).exists());
        let held = fs::read_dir(staging_directory)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".gogglelab-held-")
                    && entry.file_name().to_string_lossy().ends_with(".data")
            })
            .find(|entry| fs::read(entry.path()).is_ok_and(|bytes| bytes == replacement));
        assert!(held.is_some(), "the raced bytes must remain recoverable");
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
    #[cfg(unix)]
    #[test]
    fn custom_import_refuses_symlinked_input_or_cache_ancestor_without_outside_writes() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("catalog");
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("thumbnails")).unwrap();
        let image = root.join("image.png");
        fs::write(&image, png(8, 8)).unwrap();
        let input_link = root.join("image-link.png");
        symlink(&image, &input_link).unwrap();
        assert!(import_custom_image_file(&root, &input_link).is_err());
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::remove_dir(root.join("thumbnails")).unwrap();
        symlink(&outside, root.join("thumbnails")).unwrap();
        assert!(import_custom_image_file(&root, &image).is_err());
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    }
    fn terminal_errors() -> Vec<ProcessingError> {
        vec![
            ProcessingError::ResourceStopped(stl_core::ResourceStop::InputLimitExceeded),
            ProcessingError::ResourceStopped(stl_core::ResourceStop::ReadWorkExceeded),
            ProcessingError::ResourceStopped(stl_core::ResourceStop::EnvelopeChanged),
            ProcessingError::AllocationFailed,
            ProcessingError::FundingBusy,
        ]
    }
    #[test]
    fn typed_last_candidate_and_normalizer_stops_never_try_another_candidate() {
        for error in terminal_errors() {
            let package = package(&[
                ("Metadata/thumbnail.png", png(4, 4)),
                ("Metadata/thumbnail.jpg", jpeg(4, 4)),
            ]);
            let expected = error.to_string();
            let mut failure = Some(error);
            let calls = std::cell::Cell::new(0);
            let observer = ZipResourceObserver::new();
            let result = select_embedded_thumbnail(&package, &observer, |_| {
                calls.set(calls.get() + 1);
                Err(failure.take().unwrap())
            });
            let error = observer.finish(result).unwrap_err();
            assert!(error.is_terminal());
            assert_eq!(error.to_string(), expected);
            assert_eq!(calls.get(), 1);
        }
        for error in terminal_errors() {
            let package = package(&[("Metadata/thumbnail.png", png(4, 4))]);
            let mut failure = Some(error);
            let observer = ZipResourceObserver::new();
            let result =
                select_embedded_thumbnail(&package, &observer, |_| Err(failure.take().unwrap()));
            assert!(observer.finish(result).unwrap_err().is_terminal());
        }
        let package = package(&[
            ("Metadata/thumbnail.png", png(4, 4)),
            ("Metadata/thumbnail.jpg", jpeg(4, 4)),
        ]);
        let calls = std::cell::Cell::new(0);
        let observer = ZipResourceObserver::new();
        let result = select_embedded_thumbnail(&package, &observer, |bytes| {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                Err("ordinary invalid image".into())
            } else {
                normalize_image_typed(bytes)
            }
        });
        assert!(observer.finish(result).unwrap().is_some());
        assert_eq!(calls.get(), 2);
    }
    struct CandidateFaultReader {
        inner: Cursor<Vec<u8>>,
        target: u64,
        error: Option<std::io::Error>,
        fired: std::rc::Rc<std::cell::Cell<bool>>,
    }
    impl Read for CandidateFaultReader {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            if self.inner.position() == self.target {
                if let Some(error) = self.error.take() {
                    self.fired.set(true);
                    return Err(error);
                }
            }
            self.inner.read(output)
        }
    }
    impl Seek for CandidateFaultReader {
        fn seek(&mut self, from: std::io::SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(from)
        }
    }
    #[test]
    fn typed_final_member_read_stop_is_preserved_by_complete_selection() {
        for error in terminal_errors() {
            let bytes = package(&[("Metadata/thumbnail.png", png(4, 4))]);
            let target = 30
                + u16::from_le_bytes(bytes[26..28].try_into().unwrap()) as u64
                + u16::from_le_bytes(bytes[28..30].try_into().unwrap()) as u64;
            let expected = error.to_string();
            let io = match error {
                ProcessingError::ResourceStopped(stop) => stop.into_io_error(),
                ProcessingError::AllocationFailed => {
                    stl_core::zip_bounds::ZipAllocationFailed.into_io_error()
                }
                ProcessingError::FundingBusy => {
                    stl_core::zip_bounds::ZipFundingBusy.into_io_error()
                }
                _ => unreachable!(),
            };
            let fired = std::rc::Rc::new(std::cell::Cell::new(false));
            let calls = std::cell::Cell::new(0);
            let source = CandidateFaultReader {
                inner: Cursor::new(bytes),
                target,
                error: Some(io),
                fired: fired.clone(),
            };
            let observer = ZipResourceObserver::new();
            let result = select_embedded_reader(source, &observer, |bytes| {
                calls.set(calls.get() + 1);
                normalize_image_typed(bytes)
            });
            let error = observer.finish(result).unwrap_err();
            assert!(fired.get());
            assert!(error.is_terminal());
            assert_eq!(error.to_string(), expected);
            assert_eq!(calls.get(), 0);
        }
    }
    #[test]
    fn typed_normalization_sink_allocation_is_distinct_from_local_output_limit() {
        let input = png(4, 4);
        let error =
            normalize_image_with_output(&input, BoundedPngOutput::allocation_failure_for_test())
                .unwrap_err();
        assert!(matches!(error, ProcessingError::AllocationFailed));
        assert!(!normalize_image_with_output_limit_typed(&input, 1)
            .unwrap_err()
            .is_terminal());
    }
    #[test]
    fn image_context_xml_versions_preserve_utf8_attribute_newline_normalization() {
        for (version, expected) in [("1.0", false), ("1.1", true)] {
            let rels=format!("<?xml version='{version}'?><Relationships><Relationship Type='{THUMBNAIL_RELATION}' Target='/Metadata/\u{85}custom.png'/></Relationships>");
            let bytes = package(&[
                ("_rels/.rels", rels.as_bytes().to_vec()),
                ("Metadata/ custom.png", png(2, 2)),
            ]);
            let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
            let result = embedded_3mf_thumbnail_in_context_typed(&bytes, &mut context).unwrap();
            assert_eq!(result.is_some(), expected, "XML{version}");
            assert_eq!(context.usage().retained_input_bytes, rels.len());
        }
    }
    #[test]
    fn image_context_relationship_guard_never_reaches_normalizer_or_later_io() {
        let rels = format!(
            "<Relationships><Relationship unused='{}'/></Relationships>",
            "x".repeat(4097)
        );
        let bytes = package(&[
            ("_rels/.rels", rels.as_bytes().to_vec()),
            ("Metadata/thumbnail.png", png(2, 2)),
        ]);
        let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
        let calls = std::cell::Cell::new(0);
        let error = select_embedded_reader_in_context(Cursor::new(&bytes), &mut context, |_| {
            calls.set(calls.get() + 1);
            Ok(Vec::new())
        })
        .unwrap_err();
        assert!(matches!(
            error,
            ProcessingError::ResourceStopped(stl_core::ResourceStop::InputLimitExceeded)
        ));
        assert_eq!(calls.get(), 0);
        let before = context.usage();
        assert!(embedded_3mf_thumbnail_in_context_typed(&bytes, &mut context).is_err());
        assert_eq!(context.usage(), before);
    }
    #[test]
    fn image_context_declared_oversize_remains_ordinary_without_payload_collection() {
        let mut bytes = package(&[("Metadata/thumbnail.png", png(2, 2))]);
        let central = bytes
            .windows(4)
            .position(|window| window == b"PK\x01\x02")
            .unwrap();
        let declared = (MAX_INPUT_BYTES as u32 + 1).to_le_bytes();
        bytes[22..26].copy_from_slice(&declared);
        bytes[central + 24..central + 28].copy_from_slice(&declared);
        let target = 30
            + u16::from_le_bytes(bytes[26..28].try_into().unwrap()) as u64
            + u16::from_le_bytes(bytes[28..30].try_into().unwrap()) as u64;
        let fired = std::rc::Rc::new(std::cell::Cell::new(false));
        let source = CandidateFaultReader {
            inner: Cursor::new(bytes),
            target,
            error: Some(stl_core::ResourceStop::EnvelopeChanged.into_io_error()),
            fired: fired.clone(),
        };
        let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
        let error = select_embedded_reader_in_context(source, &mut context, normalize_image_typed)
            .unwrap_err();
        assert!(!error.is_terminal());
        assert!(!fired.get());
        assert_eq!(context.usage().inflation_bytes, 0);
        assert_eq!(context.usage().retained_input_bytes, 0);
        assert!(context.observer().check().is_ok());
    }
}
