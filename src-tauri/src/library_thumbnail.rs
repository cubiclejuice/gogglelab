use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use library_core::CatalogSettings;
use sha2::{Digest, Sha256};
use stl_core::ParsedStl;

const RENDER_VERSION: u32 = 1;
const MAX_RENDER_TRIANGLES: usize = 200_000;
const BASE_MAX_RASTER_SAMPLES: usize = 8_000_000;
const MAX_CACHED_PNG_BYTES: u64 = 4 * 1024 * 1024;
const MAX_PNG_DIMENSION: u32 = 1024;
const MAX_DECODED_PNG_BYTES: usize = 4 * 1024 * 1024;
static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub struct ThumbnailArtifact {
    pub relative_path: PathBuf,
    pub png_hash: String,
}

pub fn thumbnail_relative_path(content_hash: &str, settings: &CatalogSettings) -> PathBuf {
    let background = settings
        .thumbnail_background
        .trim_start_matches('#')
        .to_ascii_lowercase();
    PathBuf::from("thumbnails").join(format!(
        "{content_hash}-isometric-v{RENDER_VERSION}-{}-{background}.png",
        settings.thumbnail_size
    ))
}

pub fn ensure_thumbnail(
    library_root: &Path,
    source_path: &Path,
    content_hash: &str,
    expected_source_bytes: u64,
    settings: &CatalogSettings,
    regenerate: bool,
) -> Result<ThumbnailArtifact, String> {
    let relative = thumbnail_relative_path(content_hash, settings);
    let output = library_root.join(&relative);
    if !regenerate {
        match fs::symlink_metadata(&output) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err("Thumbnail cache entry is a symlink".into());
            }
            Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_CACHED_PNG_BYTES => {
                let bytes = read_bounded(&output, MAX_CACHED_PNG_BYTES)?;
                if validate_png_bytes(&bytes).is_ok() {
                    return Ok(ThumbnailArtifact {
                        relative_path: relative,
                        png_hash: hex_digest(&bytes),
                    });
                }
            }
            Ok(metadata) if metadata.is_file() => {
                return Err("Thumbnail cache entry exceeds the size limit".into());
            }
            Ok(_) => return Err("Thumbnail cache entry is not a regular file".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }

    let source_metadata = fs::symlink_metadata(source_path).map_err(|error| error.to_string())?;
    if source_metadata.file_type().is_symlink() || !source_metadata.is_file() {
        return Err("Model source is not a regular file".into());
    }
    if source_metadata.len() != expected_source_bytes
        || source_metadata.len() > stl_core::limits::MAX_FILE_BYTES
    {
        return Err("Model source size changed or exceeds the safe render limit".into());
    }
    let bytes = read_bounded(source_path, expected_source_bytes)?;
    if bytes.len() as u64 != expected_source_bytes || hex_digest(&bytes) != content_hash {
        return Err("Model source changed since it was indexed".into());
    }
    let parsed = stl_core::parse_model(&bytes).map_err(|error| error.to_string())?;
    let png = render_png(&parsed, settings)?;
    let parent = output
        .parent()
        .ok_or_else(|| "Thumbnail output has no parent directory".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let staging = output.with_extension(format!(
        "{}.{}.part",
        std::process::id(),
        STAGING_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let png_hash = hex_digest(&png);
    fs::write(&staging, png).map_err(|error| error.to_string())?;
    if output.exists() {
        fs::remove_file(&output).map_err(|error| error.to_string())?;
    }
    if let Err(error) = fs::rename(&staging, &output) {
        let _ = fs::remove_file(&staging);
        return Err(error.to_string());
    }
    Ok(ThumbnailArtifact {
        relative_path: relative,
        png_hash,
    })
}

pub fn png_data_uri(path: &Path) -> Result<String, String> {
    png_data_uri_validated(path, None)
}

pub fn png_data_uri_with_hash(path: &Path, expected_hash: &str) -> Result<String, String> {
    png_data_uri_validated(path, Some(expected_hash))
}

fn png_data_uri_validated(path: &Path, expected_hash: Option<&str>) -> Result<String, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("Cached thumbnail is not a regular file".into());
    }
    if metadata.len() > MAX_CACHED_PNG_BYTES {
        return Err("Cached thumbnail exceeds the size limit".into());
    }
    let bytes = read_bounded(path, MAX_CACHED_PNG_BYTES)?;
    validate_png_bytes(&bytes)?;
    if expected_hash.is_some_and(|expected| hex_digest(&bytes) != expected) {
        return Err("Cached thumbnail hash does not match the catalog".into());
    }
    Ok(format!("data:image/png;base64,{}", STANDARD.encode(bytes)))
}

fn validate_png_bytes(bytes: &[u8]) -> Result<(), String> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("Cached thumbnail is not a PNG".into());
    }
    let limits = png::Limits {
        bytes: MAX_DECODED_PNG_BYTES,
    };
    let decoder = png::Decoder::new_with_limits(std::io::Cursor::new(bytes), limits);
    let mut reader = decoder.read_info().map_err(|error| error.to_string())?;
    let info = reader.info();
    if info.width == 0
        || info.height == 0
        || info.width > MAX_PNG_DIMENSION
        || info.height > MAX_PNG_DIMENSION
    {
        return Err("Cached thumbnail dimensions exceed the limit".into());
    }
    let output_size = reader
        .output_buffer_size()
        .ok_or_else(|| "Cached thumbnail output size overflowed".to_string())?;
    if output_size > MAX_DECODED_PNG_BYTES {
        return Err("Cached thumbnail decoded output exceeds the limit".into());
    }
    let mut decoded = vec![0_u8; output_size];
    reader
        .next_frame(&mut decoded)
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("File grew beyond the allowed size while being read".into());
    }
    Ok(bytes)
}

fn render_png(model: &ParsedStl, settings: &CatalogSettings) -> Result<Vec<u8>, String> {
    let size = usize::try_from(settings.thumbnail_size)
        .map_err(|_| "Thumbnail size is invalid".to_string())?;
    if !matches!(size, 128 | 256 | 512) {
        return Err("Thumbnail size is invalid".into());
    }
    let background = parse_background(&settings.thumbnail_background)?;
    let mut rgba = vec![0_u8; size * size * 4];
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[background[0], background[1], background[2], 255]);
    }
    let mut depth = vec![f32::NEG_INFINITY; size * size];

    if model.positions.len() >= 9 {
        let (min_x, max_x, min_y, max_y) = bounds(
            model
                .positions
                .chunks_exact(3)
                .map(|point| project([point[0], point[1], point[2]])),
        );
        let width = (max_x - min_x).max(f32::EPSILON);
        let height = (max_y - min_y).max(f32::EPSILON);
        let margin = (size as f32 * 0.055).max(7.0);
        let scale =
            ((size as f32 - margin * 2.0) / width).min((size as f32 - margin * 2.0) / height);
        let offset_x = (size as f32 - width * scale) * 0.5 - min_x * scale;
        let offset_y = (size as f32 - height * scale) * 0.5 + max_y * scale;
        let triangle_count = model.positions.len() / 9;
        let stride = triangle_count.div_ceil(MAX_RENDER_TRIANGLES).max(1);
        let mut raster_budget = BASE_MAX_RASTER_SAMPLES
            .saturating_mul(size * size)
            .checked_div(256 * 256)
            .unwrap_or(BASE_MAX_RASTER_SAMPLES);

        for triangle in (0..triangle_count).step_by(stride) {
            let first = triangle * 9;
            let vertices = [
                screen(
                    project(&model.positions[first..first + 3]),
                    scale,
                    offset_x,
                    offset_y,
                ),
                screen(
                    project(&model.positions[first + 3..first + 6]),
                    scale,
                    offset_x,
                    offset_y,
                ),
                screen(
                    project(&model.positions[first + 6..first + 9]),
                    scale,
                    offset_x,
                    offset_y,
                ),
            ];
            let source = &model.positions[first..first + 9];
            let shade = triangle_shade(source);
            if !rasterize(
                vertices,
                shade,
                size,
                &mut depth,
                &mut rgba,
                &mut raster_budget,
            ) {
                break;
            }
        }
    }

    let mut output = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut output, size as u32, size as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
        writer
            .write_image_data(&rgba)
            .map_err(|error| error.to_string())?;
    }
    Ok(output)
}

fn project(point: impl AsRef<[f32]>) -> [f32; 3] {
    let point = point.as_ref();
    let x = (point[0] - point[2]) * 0.707_106_77;
    let y = point[1] * 0.816_496_6 - (point[0] + point[2]) * 0.408_248_3;
    let depth = (point[0] + point[1] + point[2]) * 0.577_350_26;
    [x, y, depth]
}

fn bounds(points: impl Iterator<Item = [f32; 3]>) -> (f32, f32, f32, f32) {
    points.fold(
        (
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
        ),
        |(min_x, max_x, min_y, max_y), point| {
            (
                min_x.min(point[0]),
                max_x.max(point[0]),
                min_y.min(point[1]),
                max_y.max(point[1]),
            )
        },
    )
}

fn screen(point: [f32; 3], scale: f32, offset_x: f32, offset_y: f32) -> [f32; 3] {
    [
        point[0] * scale + offset_x,
        offset_y - point[1] * scale,
        point[2],
    ]
}

fn triangle_shade(p: &[f32]) -> u8 {
    let a = [p[3] - p[0], p[4] - p[1], p[5] - p[2]];
    let b = [p[6] - p[0], p[7] - p[1], p[8] - p[2]];
    let normal = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2])
        .sqrt()
        .max(f32::EPSILON);
    let light = [0.35_f32, 0.75, 0.56];
    let dot = ((normal[0] * light[0] + normal[1] * light[1] + normal[2] * light[2]) / length).abs();
    (105.0 + 130.0 * dot) as u8
}

fn rasterize(
    vertices: [[f32; 3]; 3],
    shade: u8,
    size: usize,
    depth: &mut [f32],
    rgba: &mut [u8],
    budget: &mut usize,
) -> bool {
    let min_x = vertices
        .iter()
        .map(|point| point[0].floor() as i32)
        .min()
        .unwrap_or(0)
        .clamp(0, size as i32 - 1);
    let max_x = vertices
        .iter()
        .map(|point| point[0].ceil() as i32)
        .max()
        .unwrap_or(0)
        .clamp(0, size as i32 - 1);
    let min_y = vertices
        .iter()
        .map(|point| point[1].floor() as i32)
        .min()
        .unwrap_or(0)
        .clamp(0, size as i32 - 1);
    let max_y = vertices
        .iter()
        .map(|point| point[1].ceil() as i32)
        .max()
        .unwrap_or(0)
        .clamp(0, size as i32 - 1);
    let area = edge(vertices[0], vertices[1], vertices[2][0], vertices[2][1]);
    if area.abs() < f32::EPSILON {
        return true;
    }
    let samples = (max_x - min_x + 1) as usize * (max_y - min_y + 1) as usize;
    if samples > *budget {
        return false;
    }
    *budget -= samples;

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let w0 = edge(vertices[1], vertices[2], px, py) / area;
            let w1 = edge(vertices[2], vertices[0], px, py) / area;
            let w2 = 1.0 - w0 - w1;
            if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                let z = w0 * vertices[0][2] + w1 * vertices[1][2] + w2 * vertices[2][2];
                let index = y as usize * size + x as usize;
                if z > depth[index] {
                    depth[index] = z;
                    let pixel = &mut rgba[index * 4..index * 4 + 4];
                    pixel.copy_from_slice(&[shade / 2, shade, shade.saturating_add(18), 255]);
                }
            }
        }
    }
    true
}

fn parse_background(value: &str) -> Result<[u8; 3], String> {
    if value.len() != 7 || !value.starts_with('#') {
        return Err("Thumbnail background must be #RRGGBB".into());
    }
    let parse = |range| {
        u8::from_str_radix(&value[range], 16)
            .map_err(|_| "Thumbnail background must be #RRGGBB".to_string())
    };
    Ok([parse(1..3)?, parse(3..5)?, parse(5..7)?])
}

fn edge(a: [f32; 3], b: [f32; 3], x: f32, y: f32) -> f32 {
    (x - a[0]) * (b[1] - a[1]) - (y - a[1]) * (b[0] - a[0])
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
    use stl_core::{ParsedStl, StlFormat};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn settings() -> CatalogSettings {
        CatalogSettings {
            thumbnail_size: 256,
            thumbnail_background: "#12171F".into(),
            thumbnail_concurrency: 2,
            scan_batch_size: 128,
        }
    }

    fn one_triangle() -> ParsedStl {
        ParsedStl {
            format: StlFormat::Binary,
            triangle_count: 1,
            positions: vec![0.0, 0.0, 0.0, 20.0, 0.0, 0.0, 0.0, 20.0, 0.0],
            stored_normals: vec![0.0, 0.0, 1.0],
        }
    }

    #[test]
    fn renders_real_png_with_non_background_pixels() {
        let png = render_png(&one_triangle(), &settings()).unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(png.len() > 500);
        let decoder = png::Decoder::new(Cursor::new(&png));
        let mut reader = decoder.read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut pixels).unwrap();
        assert_eq!((info.width, info.height), (256, 256));
        assert!(pixels[..info.buffer_size()]
            .chunks_exact(4)
            .any(|pixel| pixel != [18, 23, 31, 255]));
    }

    fn binary_triangle() -> Vec<u8> {
        let mut bytes = vec![0_u8; 84];
        bytes[80..84].copy_from_slice(&1_u32.to_le_bytes());
        for value in [
            0.0_f32, 0.0, 1.0, 0.0, 0.0, 0.0, 20.0, 0.0, 0.0, 0.0, 20.0, 0.0,
        ] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&[0, 0]);
        bytes
    }

    fn three_mf_cube() -> Vec<u8> {
        let model = r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
<resources><object id="1" type="model"><mesh><vertices>
<vertex x="0" y="0" z="0"/><vertex x="20" y="0" z="0"/>
<vertex x="20" y="20" z="0"/><vertex x="0" y="20" z="0"/>
<vertex x="0" y="0" z="20"/><vertex x="20" y="0" z="20"/>
<vertex x="20" y="20" z="20"/><vertex x="0" y="20" z="20"/>
</vertices><triangles>
<triangle v1="0" v2="2" v3="1"/><triangle v1="0" v2="3" v3="2"/>
<triangle v1="4" v2="5" v3="6"/><triangle v1="4" v2="6" v3="7"/>
<triangle v1="0" v2="1" v3="5"/><triangle v1="0" v2="5" v3="4"/>
<triangle v1="1" v2="2" v3="6"/><triangle v1="1" v2="6" v3="5"/>
<triangle v1="2" v2="3" v3="7"/><triangle v1="2" v2="7" v3="6"/>
<triangle v1="3" v2="0" v3="4"/><triangle v1="3" v2="4" v3="7"/>
</triangles></mesh></object></resources><build><item objectid="1"/></build></model>"#;
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file("3D/3dmodel.model", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(model.as_bytes()).unwrap();
        archive.finish().unwrap().into_inner()
    }

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "gogglelab-thumbnail-{label}-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("thumbnails")).unwrap();
        root
    }

    #[test]
    fn generated_thumbnail_is_reused_across_calls() {
        let root = temp_root("cache");
        let source = root.join("source.stl");
        let source_bytes = binary_triangle();
        fs::write(&source, &source_bytes).unwrap();
        let source_hash = hex_digest(&source_bytes);
        let first = ensure_thumbnail(
            &root,
            &source,
            &source_hash,
            source_bytes.len() as u64,
            &settings(),
            false,
        )
        .unwrap();
        let first_bytes = fs::read(root.join(&first.relative_path)).unwrap();
        let second = ensure_thumbnail(
            &root,
            &source,
            &source_hash,
            source_bytes.len() as u64,
            &settings(),
            false,
        )
        .unwrap();
        assert_eq!(first.relative_path, second.relative_path);
        assert_eq!(first.png_hash, second.png_hash);
        assert_eq!(
            first_bytes,
            fs::read(root.join(second.relative_path)).unwrap()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn truncated_cached_png_is_rejected_and_regenerated_from_source() {
        let root = temp_root("truncated");
        let source = root.join("source.stl");
        let source_bytes = binary_triangle();
        fs::write(&source, &source_bytes).unwrap();
        let source_hash = hex_digest(&source_bytes);
        let cached = root.join(thumbnail_relative_path(&source_hash, &settings()));
        fs::write(&cached, b"\x89PNG\r\n\x1a\ntruncated").unwrap();
        assert!(png_data_uri(&cached).is_err());

        let regenerated = ensure_thumbnail(
            &root,
            &source,
            &source_hash,
            source_bytes.len() as u64,
            &settings(),
            false,
        )
        .unwrap();
        assert!(png_data_uri(&root.join(regenerated.relative_path)).is_ok());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn oversized_png_dimensions_are_rejected_before_data_uri_encoding() {
        let root = temp_root("dimensions");
        let path = root.join("thumbnails/oversized.png");
        let mut encoded = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut encoded, MAX_PNG_DIMENSION + 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&vec![0_u8; (MAX_PNG_DIMENSION as usize + 1) * 4])
                .unwrap();
        }
        fs::write(&path, encoded).unwrap();
        let error = png_data_uri(&path).unwrap_err();
        assert!(error.contains("dimensions"), "unexpected error: {error}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn bounded_reader_refuses_content_beyond_limit() {
        let root = temp_root("bounded-read");
        let path = root.join("growing.bin");
        fs::write(&path, b"12345").unwrap();
        assert!(read_bounded(&path, 4).unwrap_err().contains("grew"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cache_key_and_pixels_include_thumbnail_configuration() {
        let default = settings();
        let mut configured = default.clone();
        configured.thumbnail_size = 128;
        configured.thumbnail_background = "#FF0000".into();
        assert_ne!(
            thumbnail_relative_path("abc", &default),
            thumbnail_relative_path("abc", &configured)
        );

        let png = render_png(
            &ParsedStl {
                format: StlFormat::Binary,
                triangle_count: 0,
                positions: Vec::new(),
                stored_normals: Vec::new(),
            },
            &configured,
        )
        .unwrap();
        let decoder = png::Decoder::new(Cursor::new(png));
        let mut reader = decoder.read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut pixels).unwrap();
        assert_eq!((info.width, info.height), (128, 128));
        assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
    }

    #[cfg(unix)]
    #[test]
    fn cached_thumbnail_symlink_is_rejected() {
        use std::os::unix::fs::symlink;

        let root = temp_root("symlink");
        let source = root.join("source.stl");
        let source_bytes = binary_triangle();
        fs::write(&source, &source_bytes).unwrap();
        let source_hash = hex_digest(&source_bytes);
        let cached = root.join(thumbnail_relative_path(&source_hash, &settings()));
        symlink(&source, cached).unwrap();
        assert!(ensure_thumbnail(
            &root,
            &source,
            &source_hash,
            source_bytes.len() as u64,
            &settings(),
            false,
        )
        .unwrap_err()
        .contains("symlink"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn renders_3mf_geometry_and_optionally_writes_qa_artifact() {
        let parsed = stl_core::parse_model(&three_mf_cube()).unwrap();
        assert_eq!(parsed.format, StlFormat::ThreeMf);
        assert_eq!(parsed.triangle_count, 12);
        let png = render_png(&parsed, &settings()).unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        if let Some(path) = std::env::var_os("GOGGLELAB_THUMBNAIL_QA_OUTPUT") {
            let path = PathBuf::from(path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, png).unwrap();
        }
    }
}
