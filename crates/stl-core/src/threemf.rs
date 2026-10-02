//! 3MF reader (3D Manufacturing Format, 3mf.io core spec + the production
//! extension's `p:path` cross-part references, which every Bambu / Orca /
//! PrusaSlicer project file uses).
//!
//! A 3MF is an OPC zip package. `_rels/.rels` names the root model part
//! (conventionally `3D/3dmodel.model`), whose XML holds `<object>` resources —
//! each either a `<mesh>` of indexed vertices/triangles or a list of
//! `<component>` references to other objects, possibly in other parts — and a
//! `<build>` of `<item>`s placing objects with a 4x3 affine transform.
//!
//! The reader flattens the build into the same non-indexed `ParsedStl`
//! triangle soup the STL parsers produce, in millimetres, so everything
//! downstream is format-agnostic. It streams the XML rather than building a
//! DOM: a real project's mesh part is tens of megabytes of `<vertex>` lines.

use std::collections::{HashMap, HashSet};
use std::io::{BufReader, Cursor, Read};
use std::rc::Rc;

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use quick_xml::XmlVersion;
use serde_json::Value;

use crate::error::StlError;
use crate::limits::{MAX_3MF_INFLATED_BYTES, MAX_TRIANGLES};
use crate::model::{
    MaterialSource, ParsedFilament, ParsedMaterial, ParsedModel, ParsedPart, Plate,
};
use crate::parse::{ParsedStl, StlFormat};
use crate::vec3::{cross, dot, sub, V3};

/// Zip local-file-header signature. Every 3MF (and every other OPC package)
/// starts with it; no STL can, so this is the format discriminator.
pub fn is_zip(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
}

const ROOT_MODEL_FALLBACK: &str = "3D/3dmodel.model";
/// Component graphs are shallow in practice (one or two levels). Anything
/// deeper than this is a cycle or a hostile file, and is refused rather than
/// recursed into.
const MAX_COMPONENT_DEPTH: u32 = 32;

/// Row-major 4x3 affine matrix in the order 3MF writes it:
/// `m00 m01 m02 m10 m11 m12 m20 m21 m22 m30 m31 m32`, applied to row vectors
/// (`[x y z 1] * M`), so the last three entries are the translation.
type Mat = [f32; 12];
const IDENTITY: Mat = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];

fn apply(m: &Mat, p: V3) -> V3 {
    [
        p[0] * m[0] + p[1] * m[3] + p[2] * m[6] + m[9],
        p[0] * m[1] + p[1] * m[4] + p[2] * m[7] + m[10],
        p[0] * m[2] + p[1] * m[5] + p[2] * m[8] + m[11],
    ]
}

/// `a` then `b`: `p * (a ∘ b) == (p * a) * b`. Used to fold a component's own
/// transform into the transform of whatever placed its parent.
fn compose(a: &Mat, b: &Mat) -> Mat {
    let mut out = [0.0f32; 12];
    for r in 0..4 {
        for c in 0..3 {
            let mut s = 0.0;
            for k in 0..3 {
                s += a[r * 3 + k] * b[k * 3 + c];
            }
            if r == 3 {
                s += b[9 + c];
            }
            out[r * 3 + c] = s;
        }
    }
    out
}

/// Sign of the linear part's determinant. A mirror (negative) reverses the
/// winding of every triangle it places, which would otherwise read as a fully
/// inside-out mesh to `integrity()` and a negative signed volume.
fn is_mirror(m: &Mat) -> bool {
    let det = m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6])
        + m[2] * (m[3] * m[7] - m[4] * m[6]);
    det < 0.0
}

fn malformed(detail: impl Into<String>) -> StlError {
    StlError::Malformed3mf {
        detail: detail.into(),
    }
}

fn unit_to_mm(unit: &str) -> Option<f32> {
    Some(match unit {
        "micron" => 0.001,
        "millimeter" => 1.0,
        "centimeter" => 10.0,
        "meter" => 1000.0,
        "inch" => 25.4,
        "foot" => 304.8,
        _ => return None,
    })
}

struct Component {
    /// `p:path` from the production extension: the part holding the target
    /// object, or `None` for "this part".
    path: Option<String>,
    object_id: u32,
    transform: Mat,
}

enum ObjectKind {
    Mesh {
        vertices: Vec<V3>,
        triangles: Vec<[u32; 3]>,
    },
    Components(Vec<Component>),
}

struct Object {
    kind: ObjectKind,
    /// `type` attribute. Only "model" (the default) and "solidsupport" are
    /// printed geometry; "support", "surface" and "other" are skipped.
    printed: bool,
    name: Option<String>,
    material: Option<(u32, u32)>,
    has_triangle_properties: bool,
}

struct BuildItem {
    path: Option<String>,
    object_id: u32,
    transform: Mat,
    /// Bambu/Orca write `printable="0"` on items the user disabled. Not core
    /// spec, but honoring it matches what the slicer would actually print.
    printable: bool,
}

struct ModelPart {
    /// Normalized zip entry name, without an OPC leading slash.
    path: String,
    /// Multiplier from this part's `unit` to millimetres.
    scale: f32,
    objects: HashMap<u32, Object>,
    build: Vec<BuildItem>,
    materials: Vec<ParsedMaterial>,
    material_lookup: HashMap<(u32, u32), usize>,
}

/// The open package plus a cache of parsed parts, so a part referenced by
/// several components is parsed once.
struct Package<'a> {
    archive: zip::ZipArchive<Cursor<&'a [u8]>>,
    parts: HashMap<String, Rc<ModelPart>>,
    materials: Vec<ParsedMaterial>,
    inflated: u64,
}

impl<'a> Package<'a> {
    fn open(bytes: &'a [u8]) -> Result<Self, StlError> {
        let archive = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|e| malformed(format!("not a readable zip package ({e})")))?;
        Ok(Package {
            archive,
            parts: HashMap::new(),
            materials: Vec::new(),
            inflated: 0,
        })
    }

    /// Zip entry names never carry the leading slash that OPC part URIs do.
    fn entry_name(path: &str) -> &str {
        path.trim_start_matches('/')
    }

    fn has(&self, path: &str) -> bool {
        let name = Self::entry_name(path);
        self.archive.file_names().any(|n| n == name)
    }

    /// Opens a part for streaming, charging its declared inflated size
    /// against the package-wide cap first.
    fn reader(
        &mut self,
        path: &str,
    ) -> Result<BufReader<zip::read::ZipFile<'_, Cursor<&'a [u8]>>>, StlError> {
        let name = Self::entry_name(path).to_string();
        let file = self
            .archive
            .by_name(&name)
            .map_err(|_| malformed(format!("missing part {path}")))?;
        self.inflated = self.inflated.saturating_add(file.size());
        if self.inflated > MAX_3MF_INFLATED_BYTES {
            return Err(malformed(format!(
                "model parts expand to more than {} MB",
                MAX_3MF_INFLATED_BYTES / (1024 * 1024)
            )));
        }
        Ok(BufReader::new(file))
    }

    fn part(&mut self, path: &str) -> Result<Rc<ModelPart>, StlError> {
        let key = Self::entry_name(path).to_string();
        if let Some(p) = self.parts.get(&key) {
            return Ok(p.clone());
        }
        let parsed = {
            let reader = self.reader(path)?;
            parse_model_part(reader, path)?
        };
        self.materials.extend(
            parsed
                .materials
                .iter()
                .take(MAX_STANDARD_MATERIALS.saturating_sub(self.materials.len()))
                .cloned(),
        );
        let rc = Rc::new(parsed);
        self.parts.insert(key, rc.clone());
        Ok(rc)
    }

    fn optional_text(&mut self, path: &str) -> Result<Option<String>, StlError> {
        if !self.has(path) {
            return Ok(None);
        }
        let mut text = String::new();
        self.reader(path)?
            .read_to_string(&mut text)
            .map_err(|_| malformed(format!("{path} is not valid UTF-8 text")))?;
        Ok(Some(text))
    }

    fn optional_bounded_text(
        &mut self,
        path: &str,
        max_bytes: u64,
    ) -> Result<Option<String>, StlError> {
        if !self.has(path) {
            return Ok(None);
        }
        let name = Self::entry_name(path).to_string();
        let size = self
            .archive
            .by_name(&name)
            .map_err(|_| malformed(format!("missing part {path}")))?
            .size();
        if size > max_bytes {
            return Ok(None);
        }
        self.optional_text(path)
    }

    /// The root model part per `_rels/.rels`; falls back to the conventional
    /// path when the relationships part is absent or does not name one.
    fn root_model_path(&mut self) -> Result<String, StlError> {
        if self.has("_rels/.rels") {
            let mut text = String::new();
            self.reader("_rels/.rels")?
                .read_to_string(&mut text)
                .map_err(|_| malformed("_rels/.rels is not valid text"))?;
            if let Some(target) = model_relationship_target(&text) {
                return Ok(target);
            }
        }
        if self.has(ROOT_MODEL_FALLBACK) {
            return Ok(ROOT_MODEL_FALLBACK.to_string());
        }
        Err(malformed("no 3D model part in the package"))
    }
}

const PLATE_SETTINGS_PATH: &str = "Metadata/model_settings.config";
// Bound per-plate UI/renderer resources even when a package contains no triangles.
const MAX_VIEWER_PLATES: usize = 256;

const MODEL_SETTINGS_PATH: &str = PLATE_SETTINGS_PATH;
const PROJECT_SETTINGS_PATH: &str = "Metadata/project_settings.config";
const MAX_APPEARANCE_METADATA_BYTES: u64 = 8 * 1024 * 1024;
const MAX_VIEWER_PARTS: usize = 1024;
const MAX_STANDARD_MATERIALS: usize = 1024;
const MAX_EMBEDDED_FILAMENTS: usize = 256;
const PART_WARNING: &str =
    "This 3MF has too many or unusable part boundaries; model-wide color is available.";

#[derive(Clone, Default)]
struct SettingsRecord {
    name: Option<String>,
    extruder: Option<u32>,
    name_invalid: bool,
    extruder_invalid: bool,
}

#[derive(Default)]
struct SlicerMetadata {
    objects: HashMap<u32, SettingsRecord>,
    parts: HashMap<u32, SettingsRecord>,
}
const NO_PLATE_METADATA_WARNING: &str =
    "This 3MF has no usable plate metadata; showing the whole build.";

#[derive(Debug)]
struct PlateAssignment {
    object_id: u32,
    instance_id: u32,
}

#[derive(Debug)]
struct PlateSpec {
    slicer_id: u32,
    name: String,
    assignments: Vec<PlateAssignment>,
}

#[derive(Default)]
struct OpenPlate {
    slicer_id: Option<u32>,
    name: Option<String>,
    assignments: Vec<PlateAssignment>,
}

#[derive(Default)]
struct OpenInstance {
    object_id: Option<u32>,
    instance_id: Option<u32>,
}

fn set_once<T: PartialEq>(slot: &mut Option<T>, value: T, what: &str) -> Result<(), String> {
    match slot {
        Some(existing) if existing != &value => Err(format!("conflicting {what}")),
        Some(_) => Err(format!("duplicated {what}")),
        None => {
            *slot = Some(value);
            Ok(())
        }
    }
}

fn parse_plate_settings(text: &str) -> Result<Vec<PlateSpec>, String> {
    let mut xml = Reader::from_str(text);
    xml.config_mut().trim_text(true);
    let mut plates = Vec::new();
    let mut plate: Option<OpenPlate> = None;
    let mut instance: Option<OpenInstance> = None;

    loop {
        let event = xml
            .read_event()
            .map_err(|e| format!("invalid plate metadata XML ({e})"))?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let is_empty = matches!(event, Event::Empty(_));
                match e.local_name().as_ref() {
                    b"plate" => {
                        if plate.is_some() {
                            return Err("nested plate metadata".into());
                        }
                        plate = Some(OpenPlate::default());
                        if is_empty {
                            return Err("plate metadata has no id or name".into());
                        }
                    }
                    b"model_instance" if plate.is_some() => {
                        if instance.is_some() {
                            return Err("nested model instance metadata".into());
                        }
                        instance = Some(OpenInstance::default());
                        if is_empty {
                            return Err("model instance metadata is empty".into());
                        }
                    }
                    b"metadata" if plate.is_some() => {
                        let key = attr_str(e, b"key").map_err(|e| e.to_string())?;
                        let value = attr_str(e, b"value").map_err(|e| e.to_string())?;
                        let (Some(key), Some(value)) = (key, value) else {
                            continue;
                        };
                        if let Some(open) = instance.as_mut() {
                            match key.as_str() {
                                "object_id" => {
                                    let id = value
                                        .trim()
                                        .parse()
                                        .map_err(|_| "invalid model instance object_id")?;
                                    set_once(&mut open.object_id, id, "model instance object_id")?;
                                }
                                "instance_id" => {
                                    let id = value
                                        .trim()
                                        .parse()
                                        .map_err(|_| "invalid model instance instance_id")?;
                                    set_once(
                                        &mut open.instance_id,
                                        id,
                                        "model instance instance_id",
                                    )?;
                                }
                                _ => {}
                            }
                        } else if let Some(open) = plate.as_mut() {
                            match key.as_str() {
                                "plater_id" => {
                                    let id = value
                                        .trim()
                                        .parse()
                                        .map_err(|_| "invalid plate plater_id")?;
                                    set_once(&mut open.slicer_id, id, "plate plater_id")?;
                                }
                                "plater_name" | "name" => {
                                    let name = normalize_label(&value).unwrap_or_default();
                                    set_once(&mut open.name, name, "plate name")?;
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::End(ref e) => match e.local_name().as_ref() {
                b"model_instance" if plate.is_some() => {
                    let open = instance
                        .take()
                        .ok_or_else(|| "unbalanced model instance metadata".to_string())?;
                    plate.as_mut().unwrap().assignments.push(PlateAssignment {
                        object_id: open
                            .object_id
                            .ok_or_else(|| "model instance has no object_id".to_string())?,
                        instance_id: open
                            .instance_id
                            .ok_or_else(|| "model instance has no instance_id".to_string())?,
                    });
                }
                b"plate" => {
                    if instance.is_some() {
                        return Err("unclosed model instance metadata".into());
                    }
                    let open = plate
                        .take()
                        .ok_or_else(|| "unbalanced plate metadata".to_string())?;
                    if plates.len() >= MAX_VIEWER_PLATES {
                        return Err(format!("more than {MAX_VIEWER_PLATES} plates"));
                    }
                    plates.push(PlateSpec {
                        slicer_id: open
                            .slicer_id
                            .ok_or_else(|| "plate has no plater_id".to_string())?,
                        name: open
                            .name
                            .filter(|name| !name.is_empty())
                            .unwrap_or_else(|| format!("Plate {}", plates.len() + 1)),
                        assignments: open.assignments,
                    });
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    if plate.is_some() || instance.is_some() {
        return Err("unclosed plate metadata".into());
    }
    if plates.is_empty() {
        return Err("plate metadata contains no plates".into());
    }
    let mut ids = HashSet::with_capacity(plates.len());
    if plates.iter().any(|plate| !ids.insert(plate.slicer_id)) {
        return Err("duplicated plate plater_id".into());
    }
    Ok(plates)
}

fn update_settings_record(record: &mut SettingsRecord, key: &str, value: &str) {
    match key {
        "name" => {
            if record.name_invalid || record.name.is_some() {
                record.name = None;
                record.name_invalid = true;
            } else if let Some(name) = normalize_label(value) {
                record.name = Some(name);
            } else {
                record.name_invalid = true;
            }
        }
        "extruder" => {
            if record.extruder_invalid || record.extruder.is_some() {
                record.extruder = None;
                record.extruder_invalid = true;
            } else if let Ok(slot) = value.trim().parse::<u32>() {
                if slot > 0 {
                    record.extruder = Some(slot);
                } else {
                    record.extruder_invalid = true;
                }
            } else {
                record.extruder_invalid = true;
            }
        }
        _ => {}
    }
}
fn merge_settings_record(
    records: &mut HashMap<u32, SettingsRecord>,
    id: u32,
    record: SettingsRecord,
) {
    let Some(existing) = records.get_mut(&id) else {
        records.insert(id, record);
        return;
    };
    if existing.name_invalid
        || record.name_invalid
        || (existing.name.is_some() && record.name.is_some())
    {
        existing.name = None;
        existing.name_invalid = true;
    } else if existing.name.is_none() {
        existing.name = record.name;
    }
    if existing.extruder_invalid
        || record.extruder_invalid
        || (existing.extruder.is_some() && record.extruder.is_some())
    {
        existing.extruder = None;
        existing.extruder_invalid = true;
    } else if existing.extruder.is_none() {
        existing.extruder = record.extruder;
    }
}

fn parse_slicer_metadata(text: &str) -> Result<SlicerMetadata, String> {
    let mut xml = Reader::from_str(text);
    xml.config_mut().trim_text(true);
    let mut metadata = SlicerMetadata::default();
    let mut object: Option<(u32, SettingsRecord)> = None;
    let mut part: Option<(u32, SettingsRecord)> = None;
    let mut ignored_part_depth = 0u32;
    loop {
        match xml
            .read_event()
            .map_err(|e| format!("invalid slicer metadata XML ({e})"))?
        {
            Event::Start(e) => match e.local_name().as_ref() {
                b"object" if object.is_none() && part.is_none() && ignored_part_depth == 0 => {
                    let Some(id) = attr_str(&e, b"id")
                        .map_err(|e| e.to_string())?
                        .and_then(|v| v.trim().parse().ok())
                    else {
                        continue;
                    };
                    object = Some((id, SettingsRecord::default()));
                }
                b"part" if object.is_some() => {
                    if ignored_part_depth > 0 {
                        ignored_part_depth = ignored_part_depth.saturating_add(1);
                    } else if part.is_some() {
                        ignored_part_depth = 1;
                    } else if let Some(id) = attr_str(&e, b"id")
                        .map_err(|e| e.to_string())?
                        .and_then(|v| v.trim().parse().ok())
                    {
                        part = Some((id, SettingsRecord::default()));
                    } else {
                        ignored_part_depth = 1;
                    }
                }
                b"metadata" if ignored_part_depth == 0 => {
                    let key = attr_str(&e, b"key").map_err(|e| e.to_string())?;
                    let value = attr_str(&e, b"value").map_err(|e| e.to_string())?;
                    if let (Some(key), Some(value)) = (key, value) {
                        if let Some((_, record)) = part.as_mut() {
                            update_settings_record(record, &key, &value);
                        } else if let Some((_, record)) = object.as_mut() {
                            update_settings_record(record, &key, &value);
                        }
                    }
                }
                _ => {}
            },
            Event::Empty(e) => match e.local_name().as_ref() {
                b"metadata" if ignored_part_depth == 0 => {
                    let key = attr_str(&e, b"key").map_err(|e| e.to_string())?;
                    let value = attr_str(&e, b"value").map_err(|e| e.to_string())?;
                    if let (Some(key), Some(value)) = (key, value) {
                        if let Some((_, record)) = part.as_mut() {
                            update_settings_record(record, &key, &value);
                        } else if let Some((_, record)) = object.as_mut() {
                            update_settings_record(record, &key, &value);
                        }
                    }
                }
                _ => {}
            },
            Event::End(e) => match e.local_name().as_ref() {
                b"part" => {
                    if ignored_part_depth > 0 {
                        ignored_part_depth -= 1;
                    } else if let Some((id, record)) = part.take() {
                        merge_settings_record(&mut metadata.parts, id, record);
                    }
                }
                b"object" => {
                    if part.is_some() || ignored_part_depth > 0 {
                        return Err("unclosed slicer metadata part".into());
                    }
                    if let Some((id, record)) = object.take() {
                        merge_settings_record(&mut metadata.objects, id, record);
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(metadata)
}

fn normalize_label(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    Some(value.chars().take(256).collect())
}

fn json_string_array(value: Option<&Value>) -> Option<Vec<Option<String>>> {
    let Value::Array(values) = value? else {
        return None;
    };
    Some(
        values
            .iter()
            .map(|value| value.as_str().and_then(normalize_label))
            .collect(),
    )
}

fn parse_project_filaments(text: &str) -> Option<Vec<ParsedFilament>> {
    let value: Value = serde_json::from_str(text).ok()?;
    let colors = value.get("filament_colour")?.as_array()?;
    let settings = json_string_array(value.get("filament_settings_id"));
    let kinds = json_string_array(value.get("filament_type"));
    let vendors = json_string_array(value.get("filament_vendor"));
    let mut filaments = Vec::new();
    for (index, color) in colors.iter().take(MAX_EMBEDDED_FILAMENTS).enumerate() {
        let Some(color) = color.as_str().and_then(normalize_standard_color) else {
            continue;
        };
        let setting = settings
            .as_ref()
            .and_then(|values| values.get(index))
            .cloned()
            .flatten();
        let vendor = vendors
            .as_ref()
            .and_then(|values| values.get(index))
            .cloned()
            .flatten();
        let name = match (vendor, setting) {
            (Some(vendor), Some(setting))
                if setting == vendor
                    || setting
                        .strip_prefix(&vendor)
                        .and_then(|suffix| suffix.chars().next())
                        .is_some_and(char::is_whitespace) =>
            {
                Some(setting)
            }
            (Some(vendor), Some(setting)) => normalize_label(&format!("{vendor} {setting}")),
            (Some(vendor), None) | (None, Some(vendor)) => Some(vendor),
            (None, None) => None,
        };
        let material = kinds
            .as_ref()
            .and_then(|values| values.get(index))
            .cloned()
            .flatten();
        filaments.push(ParsedFilament {
            slot: u32::try_from(index + 1).unwrap(),
            name,
            material,
            color,
        });
    }
    Some(filaments)
}

fn setting_name(record: Option<&SettingsRecord>) -> Option<&str> {
    record
        .filter(|record| !record.name_invalid)
        .and_then(|record| record.name.as_deref())
}

fn setting_extruder(record: Option<&SettingsRecord>) -> Option<u32> {
    record
        .filter(|record| !record.extruder_invalid)
        .and_then(|record| record.extruder)
}

fn boundary_name(
    root_object: &Object,
    child_object: Option<&Object>,
    slicer_object: Option<&SettingsRecord>,
    slicer_part: Option<&SettingsRecord>,
    object_id: u32,
) -> String {
    let object_name = setting_name(slicer_object);
    let part_name = setting_name(slicer_part);
    if let (Some(object_name), Some(part_name)) = (object_name, part_name) {
        if object_name != part_name {
            let combined = format!("{object_name} · {part_name}");
            return normalize_label(&combined).unwrap_or_else(|| format!("Object {object_id}"));
        }
    }
    part_name
        .or_else(|| child_object.and_then(|object| object.name.as_deref()))
        .or(object_name)
        .or(root_object.name.as_deref())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("Object {object_id}"))
}

fn component_part_metadata_paths(
    root: &ModelPart,
    root_path: &str,
) -> HashMap<u32, HashSet<String>> {
    let root_path = Package::entry_name(root_path);
    let mut paths = HashMap::new();
    for item in &root.build {
        if !item.printable {
            continue;
        }
        let item_path = item.path.as_deref().unwrap_or(root_path);
        if Package::entry_name(item_path) != root_path {
            continue;
        }
        let Some(Object {
            kind: ObjectKind::Components(components),
            ..
        }) = root.objects.get(&item.object_id)
        else {
            continue;
        };
        for component in components {
            let path =
                Package::entry_name(component.path.as_deref().unwrap_or(item_path)).to_owned();
            paths
                .entry(component.object_id)
                .or_insert_with(HashSet::new)
                .insert(path);
        }
    }
    paths
}

fn component_part_metadata_ambiguous(
    component_paths: &HashMap<u32, HashSet<String>>,
    object_id: u32,
) -> bool {
    component_paths
        .get(&object_id)
        .is_some_and(|paths| paths.len() > 1)
}

fn resolve_plate_items(
    pkg: &mut Package,
    root_path: &str,
    root: &ModelPart,
    specs: &[PlateSpec],
) -> Result<Vec<Vec<usize>>, String> {
    // Slicer instance ids are occurrence numbers for a given object id. Count
    // every build entry, including disabled entries, before geometry filtering.
    let mut occurrences: HashMap<u32, u32> = HashMap::new();
    let mut items = HashMap::with_capacity(root.build.len());
    for (index, item) in root.build.iter().enumerate() {
        let occurrence = occurrences.entry(item.object_id).or_default();
        items.insert((item.object_id, *occurrence), index);
        *occurrence = occurrence
            .checked_add(1)
            .ok_or_else(|| "too many instances for one object".to_string())?;
    }

    let mut claimed = HashSet::with_capacity(root.build.len());
    let mut resolved = Vec::with_capacity(specs.len());
    for plate in specs {
        let mut plate_items = Vec::with_capacity(plate.assignments.len());
        for assignment in &plate.assignments {
            let index = *items
                .get(&(assignment.object_id, assignment.instance_id))
                .ok_or_else(|| {
                    format!(
                        "plate references unknown object {} instance {}",
                        assignment.object_id, assignment.instance_id
                    )
                })?;
            if !claimed.insert(index) {
                return Err(format!(
                    "object {} instance {} is assigned more than once",
                    assignment.object_id, assignment.instance_id
                ));
            }
            plate_items.push(index);
        }
        resolved.push(plate_items);
    }

    for (index, item) in root.build.iter().enumerate() {
        if item.printable
            && !claimed.contains(&index)
            && object_has_geometry(
                pkg,
                item.path.as_deref().unwrap_or(root_path),
                item.object_id,
                0,
            )
            .map_err(|e| e.to_string())?
        {
            return Err("plate metadata does not assign every printable build item".into());
        }
    }
    Ok(resolved)
}

/// Membership is required only for build items that contribute printed triangles.
fn object_has_geometry(
    pkg: &mut Package,
    path: &str,
    id: u32,
    depth: u32,
) -> Result<bool, StlError> {
    if depth > MAX_COMPONENT_DEPTH {
        return Err(malformed("component nesting is too deep or cyclic"));
    }
    let part = pkg.part(path)?;
    let object = part
        .objects
        .get(&id)
        .ok_or_else(|| malformed(format!("object {id} not found in {path}")))?;
    if !object.printed {
        return Ok(false);
    }
    match &object.kind {
        ObjectKind::Mesh { triangles, .. } => Ok(!triangles.is_empty()),
        ObjectKind::Components(components) => {
            for component in components {
                if object_has_geometry(
                    pkg,
                    component.path.as_deref().unwrap_or(path),
                    component.object_id,
                    depth + 1,
                )? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    }
}

/// Finds the `Relationship` whose `Type` is the 3dmodel relationship and
/// returns its `Target`. Small file, so a DOM-free attribute scan is enough.
fn model_relationship_target(rels: &str) -> Option<String> {
    let mut reader = Reader::from_str(rels);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if e.local_name().as_ref() != b"Relationship" {
                    continue;
                }
                let mut target = None;
                let mut is_model = false;
                for attr in e.attributes().flatten() {
                    let value = attr.normalized_value(XmlVersion::Implicit1_0).ok()?;
                    match attr.key.local_name().as_ref() {
                        b"Target" => target = Some(value.into_owned()),
                        b"Type" => is_model = value.ends_with("/3dmodel"),
                        _ => {}
                    }
                }
                if is_model {
                    return target;
                }
            }
            Ok(Event::Eof) | Err(_) => return None,
            _ => {}
        }
    }
}

fn attr_str(e: &BytesStart, name: &[u8]) -> Result<Option<String>, StlError> {
    for attr in e.attributes() {
        let attr = attr.map_err(|_| malformed("malformed XML attribute"))?;
        if attr.key.local_name().as_ref() == name {
            let v = attr
                .normalized_value(XmlVersion::Implicit1_0)
                .map_err(|_| malformed("malformed XML attribute value"))?;
            return Ok(Some(v.into_owned()));
        }
    }
    Ok(None)
}

fn attr_u32(e: &BytesStart, name: &str, what: &str) -> Result<u32, StlError> {
    attr_str(e, name.as_bytes())?
        .and_then(|s| s.trim().parse::<u32>().ok())
        .ok_or_else(|| malformed(format!("{what} has no valid {name}")))
}

fn attr_f32(e: &BytesStart, name: &str, what: &str) -> Result<f32, StlError> {
    attr_str(e, name.as_bytes())?
        .and_then(|s| s.trim().parse::<f32>().ok())
        .ok_or_else(|| malformed(format!("{what} has no valid {name}")))
}

fn optional_appearance_u32(e: &BytesStart, name: &[u8]) -> Option<u32> {
    attr_str(e, name)
        .ok()
        .flatten()
        .and_then(|value| value.trim().parse().ok())
}

fn has_any_attr(e: &BytesStart, names: &[&[u8]]) -> bool {
    e.attributes().flatten().any(|attr| {
        names
            .iter()
            .any(|name| attr.key.local_name().as_ref() == *name)
    })
}

fn normalize_standard_color(value: &str) -> Option<String> {
    let hex = value.trim().strip_prefix('#')?;
    if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut color = String::with_capacity(7);
    color.push('#');
    for byte in hex.as_bytes()[..6].iter().copied() {
        color.push((byte as char).to_ascii_uppercase());
    }
    Some(color)
}

fn attr_transform(e: &BytesStart) -> Result<Mat, StlError> {
    let Some(s) = attr_str(e, b"transform")? else {
        return Ok(IDENTITY);
    };
    let values: Vec<f32> = s
        .split_ascii_whitespace()
        .map(|t| t.parse::<f32>())
        .collect::<Result<_, _>>()
        .map_err(|_| malformed("transform has a non-numeric entry"))?;
    let m: Mat = values
        .try_into()
        .map_err(|_| malformed("transform must have 12 entries"))?;
    if m.iter().any(|v| !v.is_finite()) {
        return Err(malformed("transform has a non-finite entry"));
    }
    Ok(m)
}

#[derive(Clone, Copy)]
enum MaterialGroupKind {
    Base,
    Color,
}

struct OpenMaterialGroup {
    id: u32,
    next_index: u32,
    kind: MaterialGroupKind,
}

/// An `<object>` whose end tag has not arrived yet.
struct OpenObject {
    id: u32,
    printed: bool,
    name: Option<String>,
    material: Option<(u32, u32)>,
    has_triangle_properties: bool,
    vertices: Vec<V3>,
    triangles: Vec<[u32; 3]>,
    components: Vec<Component>,
}

/// Streams one `.model` part into its objects and build items.
fn parse_model_part<R: std::io::BufRead>(reader: R, path: &str) -> Result<ModelPart, StlError> {
    let mut xml = Reader::from_reader(reader);
    xml.config_mut().trim_text(true);
    let mut buf = Vec::new();

    let mut part = ModelPart {
        path: Package::entry_name(path).to_string(),
        scale: 1.0,
        objects: HashMap::new(),
        build: Vec::new(),
        materials: Vec::new(),
        material_lookup: HashMap::new(),
    };
    let mut current: Option<OpenObject> = None;
    let mut material_group: Option<OpenMaterialGroup> = None;
    let mut in_build = false;

    loop {
        let event = xml
            .read_event_into(&mut buf)
            .map_err(|e| malformed(format!("{path}: invalid XML ({e})")))?;
        match &event {
            Event::Start(e) | Event::Empty(e) => {
                let is_empty = matches!(&event, Event::Empty(_));
                match e.local_name().as_ref() {
                    b"model" => {
                        let unit = attr_str(e, b"unit")?.unwrap_or_else(|| "millimeter".into());
                        part.scale = unit_to_mm(&unit)
                            .ok_or_else(|| malformed(format!("unknown unit \"{unit}\"")))?;
                    }
                    b"basematerials" | b"colorgroup" => {
                        material_group = if is_empty {
                            None
                        } else {
                            optional_appearance_u32(e, b"id").map(|id| OpenMaterialGroup {
                                id,
                                next_index: 0,
                                kind: if e.local_name().as_ref() == b"basematerials" {
                                    MaterialGroupKind::Base
                                } else {
                                    MaterialGroupKind::Color
                                },
                            })
                        };
                    }
                    b"base" => {
                        if let Some(group) = material_group
                            .as_mut()
                            .filter(|group| matches!(group.kind, MaterialGroupKind::Base))
                        {
                            let index = group.next_index;
                            group.next_index = group.next_index.saturating_add(1);
                            let name = attr_str(e, b"name")
                                .ok()
                                .flatten()
                                .and_then(|name| normalize_label(&name));
                            let color = attr_str(e, b"displaycolor")
                                .ok()
                                .flatten()
                                .and_then(|color| normalize_standard_color(&color));
                            if let (Some(name), Some(color)) = (name, color) {
                                let key = (group.id, index);
                                if !part.material_lookup.contains_key(&key) {
                                    if part.materials.len() < MAX_STANDARD_MATERIALS {
                                        let material_index = part.materials.len();
                                        part.materials.push(ParsedMaterial {
                                            id: format!("3mf:{}:{}:{}", part.path, group.id, index),
                                            name: Some(name),
                                            color: Some(color),
                                            source: MaterialSource::ThreeMfBaseMaterial,
                                        });
                                        part.material_lookup.insert(key, material_index);
                                    }
                                }
                            }
                        }
                    }
                    b"color" => {
                        if let Some(group) = material_group
                            .as_mut()
                            .filter(|group| matches!(group.kind, MaterialGroupKind::Color))
                        {
                            let index = group.next_index;
                            group.next_index = group.next_index.saturating_add(1);
                            let color = attr_str(e, b"color")
                                .ok()
                                .flatten()
                                .and_then(|color| normalize_standard_color(&color));
                            if let Some(color) = color {
                                let key = (group.id, index);
                                if !part.material_lookup.contains_key(&key) {
                                    if part.materials.len() < MAX_STANDARD_MATERIALS {
                                        let material_index = part.materials.len();
                                        part.materials.push(ParsedMaterial {
                                            id: format!("3mf:{}:{}:{}", part.path, group.id, index),
                                            name: None,
                                            color: Some(color),
                                            source: MaterialSource::ThreeMfColorGroup,
                                        });
                                        part.material_lookup.insert(key, material_index);
                                    }
                                }
                            }
                        }
                    }
                    b"object" => {
                        let id = attr_u32(e, "id", "object")?;
                        let kind = attr_str(e, b"type")?.unwrap_or_else(|| "model".into());
                        let printed = matches!(kind.as_str(), "model" | "solidsupport");
                        let name = attr_str(e, b"name")
                            .ok()
                            .flatten()
                            .and_then(|name| normalize_label(&name));
                        let material = optional_appearance_u32(e, b"pid")
                            .zip(optional_appearance_u32(e, b"pindex"));
                        if is_empty {
                            part.objects.insert(
                                id,
                                Object {
                                    kind: ObjectKind::Mesh {
                                        vertices: vec![],
                                        triangles: vec![],
                                    },
                                    printed,
                                    name,
                                    material,
                                    has_triangle_properties: false,
                                },
                            );
                        } else {
                            current = Some(OpenObject {
                                id,
                                printed,
                                name,
                                material,
                                has_triangle_properties: false,
                                vertices: Vec::new(),
                                triangles: Vec::new(),
                                components: Vec::new(),
                            });
                        }
                    }
                    b"vertex" => {
                        let Some(cur) = current.as_mut() else {
                            return Err(malformed("vertex outside of an object"));
                        };
                        let v = [
                            attr_f32(e, "x", "vertex")?,
                            attr_f32(e, "y", "vertex")?,
                            attr_f32(e, "z", "vertex")?,
                        ];
                        cur.vertices.push(v);
                    }
                    b"triangle" => {
                        let Some(cur) = current.as_mut() else {
                            return Err(malformed("triangle outside of an object"));
                        };
                        cur.has_triangle_properties |=
                            has_any_attr(e, &[b"pid", b"p1", b"p2", b"p3"]);
                        cur.triangles.push([
                            attr_u32(e, "v1", "triangle")?,
                            attr_u32(e, "v2", "triangle")?,
                            attr_u32(e, "v3", "triangle")?,
                        ]);
                    }
                    b"component" => {
                        let Some(cur) = current.as_mut() else {
                            return Err(malformed("component outside of an object"));
                        };
                        cur.components.push(Component {
                            path: attr_str(e, b"path")?,
                            object_id: attr_u32(e, "objectid", "component")?,
                            transform: attr_transform(e)?,
                        });
                    }
                    b"build" => in_build = true,
                    b"item" if in_build => {
                        let printable = attr_str(e, b"printable")?
                            .map(|p| p.trim() != "0" && !p.trim().eq_ignore_ascii_case("false"))
                            .unwrap_or(true);
                        part.build.push(BuildItem {
                            path: attr_str(e, b"path")?,
                            object_id: attr_u32(e, "objectid", "build item")?,
                            transform: attr_transform(e)?,
                            printable,
                        });
                    }
                    _ => {}
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                b"object" => {
                    let OpenObject {
                        id,
                        printed,
                        name,
                        material,
                        has_triangle_properties,
                        vertices,
                        triangles,
                        components,
                    } = current
                        .take()
                        .ok_or_else(|| malformed("unbalanced </object>"))?;
                    let kind = if !components.is_empty() {
                        ObjectKind::Components(components)
                    } else {
                        for (i, t) in triangles.iter().enumerate() {
                            if t.iter().any(|&v| v as usize >= vertices.len()) {
                                return Err(malformed(format!(
                                    "object {id} triangle {i} references a missing vertex"
                                )));
                            }
                        }
                        ObjectKind::Mesh {
                            vertices,
                            triangles,
                        }
                    };
                    part.objects.insert(
                        id,
                        Object {
                            kind,
                            printed,
                            name,
                            material,
                            has_triangle_properties,
                        },
                    );
                }
                b"basematerials" | b"colorgroup" => material_group = None,
                b"build" => in_build = false,
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(part)
}

/// Flattened output accumulated across the whole build.
struct Out {
    positions: Vec<f32>,
    normals: Vec<f32>,
    triangle_count: u32,
    scale: f32,
}

impl Out {
    fn push_triangle(&mut self, verts: [V3; 3]) -> Result<(), StlError> {
        if self.triangle_count == MAX_TRIANGLES {
            return Err(StlError::TooManyTriangles {
                count: self.triangle_count + 1,
                limit: MAX_TRIANGLES,
            });
        }
        for v in &verts {
            for c in v {
                if !c.is_finite() {
                    return Err(StlError::NonFiniteCoordinate {
                        triangle_index: self.triangle_count,
                    });
                }
            }
        }
        for v in &verts {
            self.positions.extend_from_slice(v);
        }
        // 3MF stores no normals. Record the winding normal so integrity()'s
        // stored-vs-winding comparison is trivially satisfied and only the
        // topology checks (which 3MF can genuinely fail) carry signal.
        let n = cross(sub(verts[1], verts[0]), sub(verts[2], verts[0]));
        let len = dot(n, n).sqrt();
        if len > 0.0 {
            self.normals
                .extend_from_slice(&[n[0] / len, n[1] / len, n[2] / len]);
        } else {
            self.normals.extend_from_slice(&[0.0, 0.0, 0.0]);
        }
        self.triangle_count += 1;
        Ok(())
    }
}

enum StandardAppearance {
    Empty,
    Uniform(String),
    NonUniform,
}

impl StandardAppearance {
    fn observe(&mut self, material_id: Option<&str>) {
        if matches!(self, Self::NonUniform) {
            return;
        }
        let Some(material_id) = material_id else {
            *self = Self::NonUniform;
            return;
        };
        match self {
            Self::Empty => *self = Self::Uniform(material_id.to_string()),
            Self::Uniform(current) if current.as_str() != material_id => {
                *self = Self::NonUniform;
            }
            Self::Uniform(_) | Self::NonUniform => {}
        }
    }

    fn into_material_id(self) -> Option<String> {
        match self {
            Self::Uniform(material_id) => Some(material_id),
            Self::Empty | Self::NonUniform => None,
        }
    }
}

fn standard_material_id<'a>(
    pkg: &Package<'_>,
    part: &'a ModelPart,
    object: &Object,
) -> Option<&'a str> {
    let key = object.material?;
    let index = *part.material_lookup.get(&key)?;
    let material_id = part.materials.get(index)?.id.as_str();
    pkg.materials
        .iter()
        .any(|material| material.id == material_id)
        .then_some(material_id)
}

fn emit_object(
    pkg: &mut Package,
    part_path: &str,
    object_id: u32,
    transform: &Mat,
    depth: u32,
    out: &mut Out,
    appearance: &mut StandardAppearance,
) -> Result<(), StlError> {
    if depth > MAX_COMPONENT_DEPTH {
        return Err(malformed("component nesting is too deep or cyclic"));
    }
    let part = pkg.part(part_path)?;
    let object = part
        .objects
        .get(&object_id)
        .ok_or_else(|| malformed(format!("object {object_id} not found in {part_path}")))?;
    if !object.printed {
        return Ok(());
    }
    match &object.kind {
        ObjectKind::Mesh {
            vertices,
            triangles,
        } => {
            if !triangles.is_empty() {
                let material_id = if object.has_triangle_properties {
                    None
                } else {
                    standard_material_id(pkg, &part, object)
                };
                appearance.observe(material_id);
            }
            let flip = is_mirror(transform);
            for t in triangles {
                let mut verts = [
                    apply(transform, vertices[t[0] as usize]),
                    apply(transform, vertices[t[1] as usize]),
                    apply(transform, vertices[t[2] as usize]),
                ];
                if flip {
                    verts.swap(1, 2);
                }
                for v in &mut verts {
                    for c in v.iter_mut() {
                        *c *= out.scale;
                    }
                }
                out.push_triangle(verts)?;
            }
        }
        ObjectKind::Components(components) => {
            for component in components {
                let child_path = component.path.as_deref().unwrap_or(part_path);
                let child_transform = compose(&component.transform, transform);
                emit_object(
                    pkg,
                    child_path,
                    component.object_id,
                    &child_transform,
                    depth + 1,
                    out,
                    appearance,
                )?;
            }
        }
    }
    Ok(())
}

fn new_out(scale: f32) -> Out {
    Out {
        positions: Vec::new(),
        normals: Vec::new(),
        triangle_count: 0,
        scale,
    }
}

fn emit_build_item(
    pkg: &mut Package,
    root_path: &str,
    item: &BuildItem,
    occurrence: u32,
    slicer: &SlicerMetadata,
    component_paths: &HashMap<u32, HashSet<String>>,
    out: &mut Out,
    parts: &mut Vec<ParsedPart>,
) -> Result<(), StlError> {
    if !item.printable {
        return Ok(());
    }
    let path = item.path.as_deref().unwrap_or(root_path);
    let source_part = pkg.part(path)?;
    let object = source_part
        .objects
        .get(&item.object_id)
        .ok_or_else(|| malformed(format!("object {} not found in {path}", item.object_id)))?;
    if !object.printed {
        return Ok(());
    }
    let root_metadata = (Package::entry_name(path) == Package::entry_name(root_path))
        .then(|| slicer.objects.get(&item.object_id))
        .flatten();

    match &object.kind {
        ObjectKind::Mesh { .. } => {
            let triangle_start = out.triangle_count;
            let mut appearance = StandardAppearance::Empty;
            emit_object(
                pkg,
                path,
                item.object_id,
                &item.transform,
                0,
                out,
                &mut appearance,
            )?;
            let triangle_count = out.triangle_count - triangle_start;
            if triangle_count != 0 {
                parts.push(ParsedPart {
                    id: format!("object:{}:instance:{occurrence}", item.object_id),
                    name: boundary_name(object, None, root_metadata, None, item.object_id),
                    triangle_start,
                    triangle_count,
                    material_id: appearance.into_material_id(),
                    filament_slot: setting_extruder(root_metadata),
                });
            }
        }
        ObjectKind::Components(components) => {
            for (component_index, component) in components.iter().enumerate() {
                let child_path = component.path.as_deref().unwrap_or(path);
                let child_part = pkg.part(child_path)?;
                let child_object =
                    child_part
                        .objects
                        .get(&component.object_id)
                        .ok_or_else(|| {
                            malformed(format!(
                                "object {} not found in {child_path}",
                                component.object_id
                            ))
                        })?;
                let part_metadata = (Package::entry_name(path) == Package::entry_name(root_path)
                    && !component_part_metadata_ambiguous(component_paths, component.object_id))
                .then(|| slicer.parts.get(&component.object_id))
                .flatten();
                let name = boundary_name(
                    object,
                    Some(child_object),
                    root_metadata,
                    part_metadata,
                    component.object_id,
                );
                let filament_slot =
                    setting_extruder(part_metadata).or_else(|| setting_extruder(root_metadata));
                let child_transform = compose(&component.transform, &item.transform);
                let triangle_start = out.triangle_count;
                let mut appearance = StandardAppearance::Empty;
                emit_object(
                    pkg,
                    child_path,
                    component.object_id,
                    &child_transform,
                    1,
                    out,
                    &mut appearance,
                )?;
                let triangle_count = out.triangle_count - triangle_start;
                if triangle_count != 0 {
                    parts.push(ParsedPart {
                        id: format!(
                            "object:{}:instance:{occurrence}/component:{component_index}:object:{}",
                            item.object_id, component.object_id
                        ),
                        name,
                        triangle_start,
                        triangle_count,
                        material_id: appearance.into_material_id(),
                        filament_slot,
                    });
                }
            }
        }
    }
    Ok(())
}

fn build_item_occurrences(build: &[BuildItem]) -> Result<Vec<u32>, StlError> {
    let mut counts: HashMap<u32, u32> = HashMap::new();
    let mut occurrences = Vec::with_capacity(build.len());
    for item in build {
        let occurrence = counts.entry(item.object_id).or_default();
        occurrences.push(*occurrence);
        *occurrence = (*occurrence)
            .checked_add(1)
            .ok_or_else(|| malformed("too many instances for one object"))?;
    }
    Ok(occurrences)
}

fn finish_model(out: Out) -> ParsedStl {
    ParsedStl {
        format: StlFormat::ThreeMf,
        triangle_count: out.triangle_count,
        positions: out.positions,
        stored_normals: out.normals,
    }
}

fn validate_parts(parts: &[ParsedPart], plates: &[Plate], total: u32) -> bool {
    if parts.len() > MAX_VIEWER_PARTS {
        return false;
    }
    let mut index = 0;
    for plate in plates {
        let Some(plate_end) = plate.triangle_start.checked_add(plate.triangle_count) else {
            return false;
        };
        let mut cursor = plate.triangle_start;
        while index < parts.len() && parts[index].triangle_start < plate_end {
            let part = &parts[index];
            let Some(part_end) = part.triangle_start.checked_add(part.triangle_count) else {
                return false;
            };
            if part.triangle_count == 0
                || part.triangle_start != cursor
                || part_end > plate_end
                || part.triangle_start < plate.triangle_start
            {
                return false;
            }
            cursor = part_end;
            index += 1;
        }
        if cursor != plate_end {
            return false;
        }
    }
    index == parts.len()
        && plates.last().is_some_and(|plate| {
            plate
                .triangle_start
                .checked_add(plate.triangle_count)
                .is_some_and(|end| end == total)
        })
}

fn whole_build(
    pkg: &mut Package,
    root_path: &str,
    root: &ModelPart,
    slicer: &SlicerMetadata,
    component_paths: &HashMap<u32, HashSet<String>>,
    filaments: Vec<ParsedFilament>,
    warning: String,
) -> Result<ParsedModel, StlError> {
    let mut out = new_out(root.scale);
    let mut parts = Vec::new();
    let occurrences = build_item_occurrences(&root.build)?;
    for (index, item) in root.build.iter().enumerate() {
        emit_build_item(
            pkg,
            root_path,
            item,
            occurrences[index],
            slicer,
            component_paths,
            &mut out,
            &mut parts,
        )?;
    }
    let triangle_count = out.triangle_count;
    let plates = vec![Plate {
        id: 0,
        name: "Whole build".into(),
        triangle_start: 0,
        triangle_count,
    }];
    let part_warning = if validate_parts(&parts, &plates, triangle_count) {
        None
    } else {
        parts.clear();
        Some(PART_WARNING.into())
    };
    Ok(ParsedModel {
        model: finish_model(out),
        plates,
        parts,
        materials: std::mem::take(&mut pkg.materials),
        embedded_filaments: filaments,
        part_warning,
        plate_metadata: false,
        plate_warning: Some(warning),
    })
}

/// Parse a 3MF and retain validated slicer plate membership when available.
/// Geometry is emitted once, grouped in display order so each plate is a
/// contiguous range of the aggregate model.
pub fn parse_3mf_with_plates(bytes: &[u8]) -> Result<ParsedModel, StlError> {
    if bytes.is_empty() {
        return Err(StlError::EmptyFile);
    }
    let mut pkg = Package::open(bytes)?;
    let root_path = pkg.root_model_path()?;
    let root = pkg.part(&root_path)?;
    let component_paths = component_part_metadata_paths(&root, &root_path);

    let settings_result = pkg.optional_text(MODEL_SETTINGS_PATH);
    let slicer = settings_result
        .as_ref()
        .ok()
        .and_then(|text| text.as_deref())
        .and_then(|text| parse_slicer_metadata(text).ok())
        .unwrap_or_default();
    let filaments = pkg
        .optional_bounded_text(PROJECT_SETTINGS_PATH, MAX_APPEARANCE_METADATA_BYTES)
        .ok()
        .flatten()
        .and_then(|text| parse_project_filaments(&text))
        .unwrap_or_default();

    let settings = match settings_result {
        Ok(Some(text)) => match parse_plate_settings(&text) {
            Ok(specs) => Some(specs),
            Err(detail) => {
                return whole_build(
                    &mut pkg,
                    &root_path,
                    &root,
                    &slicer,
                    &component_paths,
                    filaments,
                    format!("Plate metadata is unusable ({detail}); showing the whole build."),
                );
            }
        },
        Ok(None) => {
            return whole_build(
                &mut pkg,
                &root_path,
                &root,
                &slicer,
                &component_paths,
                filaments,
                NO_PLATE_METADATA_WARNING.into(),
            );
        }
        Err(err) => {
            return whole_build(
                &mut pkg,
                &root_path,
                &root,
                &slicer,
                &component_paths,
                filaments,
                format!("Plate metadata is unusable ({err}); showing the whole build."),
            );
        }
    };

    let specs = settings.expect("all other metadata branches return");
    let resolved = match resolve_plate_items(&mut pkg, &root_path, &root, &specs) {
        Ok(resolved) => resolved,
        Err(detail) => {
            return whole_build(
                &mut pkg,
                &root_path,
                &root,
                &slicer,
                &component_paths,
                filaments,
                format!("Plate metadata is unusable ({detail}); showing the whole build."),
            );
        }
    };

    // Transforms are expressed in the model's unit, so unit conversion is
    // applied last, to already-placed coordinates.
    let mut out = new_out(root.scale);
    let mut parts = Vec::new();
    let occurrences = build_item_occurrences(&root.build)?;
    let mut plates = Vec::with_capacity(specs.len());
    for (plate_index, (spec, item_indices)) in specs.iter().zip(&resolved).enumerate() {
        let triangle_start = out.triangle_count;
        for &item_index in item_indices {
            emit_build_item(
                &mut pkg,
                &root_path,
                &root.build[item_index],
                occurrences[item_index],
                &slicer,
                &component_paths,
                &mut out,
                &mut parts,
            )?;
        }
        plates.push(Plate {
            id: u32::try_from(plate_index).map_err(|_| malformed("too many plates in metadata"))?,
            name: spec.name.clone(),
            triangle_start,
            triangle_count: out.triangle_count - triangle_start,
        });
    }

    let part_warning = if validate_parts(&parts, &plates, out.triangle_count) {
        None
    } else {
        parts.clear();
        Some(PART_WARNING.into())
    };
    Ok(ParsedModel {
        model: finish_model(out),
        plates,
        parts,
        materials: std::mem::take(&mut pkg.materials),
        embedded_filaments: filaments,
        part_warning,
        plate_metadata: true,
        plate_warning: None,
    })
}

/// Compatibility entry point returning the aggregate triangle soup.
pub fn parse_3mf(bytes: &[u8]) -> Result<ParsedStl, StlError> {
    Ok(parse_3mf_with_plates(bytes)?.model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MaterialSource, ParsedMaterial};
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    const RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
 <Relationship Target="/3D/3dmodel.model" Id="rel-1" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
 <Relationship Target="/Metadata/thumb.png" Id="rel-2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail"/>
</Relationships>"#;

    /// Axis-aligned 10 mm cube with outward-consistent winding, as a 3MF mesh.
    fn cube_mesh_xml(id: u32) -> String {
        format!(
            r#"<object id="{id}" type="model"><mesh>
<vertices>
<vertex x="0" y="0" z="0"/><vertex x="10" y="0" z="0"/><vertex x="10" y="10" z="0"/><vertex x="0" y="10" z="0"/>
<vertex x="0" y="0" z="10"/><vertex x="10" y="0" z="10"/><vertex x="10" y="10" z="10"/><vertex x="0" y="10" z="10"/>
</vertices>
<triangles>
<triangle v1="0" v2="2" v3="1"/><triangle v1="0" v2="3" v3="2"/>
<triangle v1="4" v2="5" v3="6"/><triangle v1="4" v2="6" v3="7"/>
<triangle v1="0" v2="1" v3="5"/><triangle v1="0" v2="5" v3="4"/>
<triangle v1="1" v2="2" v3="6"/><triangle v1="1" v2="6" v3="5"/>
<triangle v1="2" v2="3" v3="7"/><triangle v1="2" v2="7" v3="6"/>
<triangle v1="3" v2="0" v3="4"/><triangle v1="3" v2="4" v3="7"/>
</triangles></mesh></object>"#
        )
    }

    fn model_xml(unit: &str, resources: &str, build: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="{unit}" xml:lang="en-US" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06">
 <metadata name="Title">test</metadata>
 <resources>{resources}</resources>
 <build>{build}</build>
</model>"#
        )
    }

    fn package(parts: &[(&str, &str)], deflate: bool) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let method = if deflate {
            zip::CompressionMethod::Deflated
        } else {
            zip::CompressionMethod::Stored
        };
        let opts = SimpleFileOptions::default().compression_method(method);
        for (name, body) in parts {
            w.start_file(*name, opts).unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn simple_cube(unit: &str, deflate: bool) -> Vec<u8> {
        let model = model_xml(unit, &cube_mesh_xml(1), r#"<item objectid="1"/>"#);
        package(
            &[("_rels/.rels", RELS), ("3D/3dmodel.model", &model)],
            deflate,
        )
    }

    fn plate_settings(plates: &str) -> String {
        format!(r#"<?xml version="1.0" encoding="UTF-8"?><config>{plates}</config>"#)
    }

    fn plate_xml(id: u32, name: &str, instances: &[(u32, u32)]) -> String {
        let instances = instances
            .iter()
            .map(|(object_id, instance_id)| {
                format!(
                    r#"<model_instance><metadata key="object_id" value="{object_id}"/><metadata key="instance_id" value="{instance_id}"/></model_instance>"#
                )
            })
            .collect::<String>();
        format!(
            r#"<plate><metadata key="plater_id" value="{id}"/><metadata key="plater_name" value="{name}"/>{instances}</plate>"#
        )
    }

    fn cube_mesh_with_object_attrs_xml(id: u32, attrs: &str) -> String {
        cube_mesh_xml(id).replacen(
            &format!(r#"<object id="{id}" type="model">"#),
            &format!(r#"<object id="{id}" type="model" {attrs}>"#),
            1,
        )
    }

    #[test]
    fn object_level_base_material_resolves_to_namespaced_part_appearance() {
        let resources = format!(
            r##"<basematerials id="5"><base name="Signal Red" displaycolor="#C02030"/></basematerials>{}"##,
            cube_mesh_with_object_attrs_xml(1, r#"name="Base Cube" pid="5" pindex="0""#)
        );
        let model = model_xml("millimeter", &resources, r#"<item objectid="1"/>"#);
        let parsed =
            parse_3mf_with_plates(&package(&[("3D/3dmodel.model", &model)], false)).unwrap();

        assert_eq!(parsed.model.triangle_count, 12);
        assert_eq!(
            parsed.materials,
            vec![ParsedMaterial {
                id: "3mf:3D/3dmodel.model:5:0".into(),
                name: Some("Signal Red".into()),
                color: Some("#C02030".into()),
                source: MaterialSource::ThreeMfBaseMaterial,
            }]
        );
        assert_eq!(parsed.parts.len(), 1);
        assert_eq!(
            parsed.parts[0].material_id.as_deref(),
            Some("3mf:3D/3dmodel.model:5:0")
        );
    }

    #[test]
    fn object_level_color_group_resolves_zero_based_opaque_namespaced_appearance() {
        let resources = format!(
            r##"<m:colorgroup xmlns:m="http://schemas.microsoft.com/3dmanufacturing/material/2015/02" id="7"><m:color color="#2A6FDB40"/></m:colorgroup>{}"##,
            cube_mesh_with_object_attrs_xml(1, r#"name="Color Cube" pid="7" pindex="0""#)
        );
        let model = model_xml("millimeter", &resources, r#"<item objectid="1"/>"#);
        let parsed =
            parse_3mf_with_plates(&package(&[("3D/3dmodel.model", &model)], false)).unwrap();

        assert_eq!(parsed.model.triangle_count, 12);
        assert_eq!(
            parsed.materials,
            vec![ParsedMaterial {
                id: "3mf:3D/3dmodel.model:7:0".into(),
                name: None,
                color: Some("#2A6FDB".into()),
                source: MaterialSource::ThreeMfColorGroup,
            }]
        );
        assert_eq!(parsed.parts.len(), 1);
        assert_eq!(
            parsed.parts[0].material_id.as_deref(),
            Some("3mf:3D/3dmodel.model:7:0")
        );
    }

    #[test]
    fn reused_resource_ids_in_distinct_model_parts_resolve_independently() {
        let root = model_xml(
            "millimeter",
            "",
            r#"<item p:path="/3D/Objects/a.model" objectid="1"/><item p:path="/3D/Objects/b.model" objectid="1" transform="1 0 0 0 1 0 0 0 1 100 0 0"/>"#,
        );
        let a_resources = format!(
            r##"<basematerials id="5"><base name="Part A" displaycolor="#AA1122"/></basematerials>{}"##,
            cube_mesh_with_object_attrs_xml(1, r#"pid="5" pindex="0""#)
        );
        let b_resources = format!(
            r##"<basematerials id="5"><base name="Part B" displaycolor="#22AA33"/></basematerials>{}"##,
            cube_mesh_with_object_attrs_xml(1, r#"pid="5" pindex="0""#)
        );
        let a = model_xml("millimeter", &a_resources, "");
        let b = model_xml("millimeter", &b_resources, "");
        let parsed = parse_3mf_with_plates(&package(
            &[
                ("_rels/.rels", RELS),
                ("3D/3dmodel.model", &root),
                ("3D/Objects/a.model", &a),
                ("3D/Objects/b.model", &b),
            ],
            false,
        ))
        .unwrap();

        assert_eq!(parsed.model.triangle_count, 24);
        let mut materials = parsed.materials.clone();
        materials.sort_by(|left, right| left.id.cmp(&right.id));
        assert_eq!(
            materials,
            vec![
                ParsedMaterial {
                    id: "3mf:3D/Objects/a.model:5:0".into(),
                    name: Some("Part A".into()),
                    color: Some("#AA1122".into()),
                    source: MaterialSource::ThreeMfBaseMaterial,
                },
                ParsedMaterial {
                    id: "3mf:3D/Objects/b.model:5:0".into(),
                    name: Some("Part B".into()),
                    color: Some("#22AA33".into()),
                    source: MaterialSource::ThreeMfBaseMaterial,
                },
            ]
        );
        assert_eq!(
            parsed
                .parts
                .iter()
                .map(|part| part.material_id.as_deref())
                .collect::<Vec<_>>(),
            vec![
                Some("3mf:3D/Objects/a.model:5:0"),
                Some("3mf:3D/Objects/b.model:5:0"),
            ]
        );
    }

    #[test]
    fn out_of_range_object_material_index_preserves_geometry_without_assignment() {
        let resources = format!(
            r##"<basematerials id="5"><base name="Only Entry" displaycolor="#445566"/></basematerials>{}"##,
            cube_mesh_with_object_attrs_xml(1, r#"pid="5" pindex="1""#)
        );
        let model = model_xml("millimeter", &resources, r#"<item objectid="1"/>"#);
        let parsed =
            parse_3mf_with_plates(&package(&[("3D/3dmodel.model", &model)], false)).unwrap();

        assert_eq!(parsed.model.triangle_count, 12);
        assert_eq!(
            parsed.materials,
            vec![ParsedMaterial {
                id: "3mf:3D/3dmodel.model:5:0".into(),
                name: Some("Only Entry".into()),
                color: Some("#445566".into()),
                source: MaterialSource::ThreeMfBaseMaterial,
            }]
        );
        assert_eq!(parsed.parts.len(), 1);
        assert_eq!(parsed.parts[0].material_id, None);
    }

    #[test]
    fn non_color_object_property_resource_preserves_geometry_without_assignment() {
        let resources = format!(
            r#"<m:texture2d xmlns:m="http://schemas.microsoft.com/3dmanufacturing/material/2015/02" id="8" path="/3D/Textures/texture.png" contenttype="image/png"/><m:texture2dgroup xmlns:m="http://schemas.microsoft.com/3dmanufacturing/material/2015/02" id="9" texid="8"><m:tex2coord u="0" v="0"/></m:texture2dgroup>{}"#,
            cube_mesh_with_object_attrs_xml(1, r#"pid="9" pindex="0""#)
        );
        let model = model_xml("millimeter", &resources, r#"<item objectid="1"/>"#);
        let parsed =
            parse_3mf_with_plates(&package(&[("3D/3dmodel.model", &model)], false)).unwrap();

        assert_eq!(parsed.model.triangle_count, 12);
        assert!(parsed.materials.is_empty());
        assert_eq!(parsed.parts.len(), 1);
        assert_eq!(parsed.parts[0].material_id, None);
    }

    #[test]
    fn any_triangle_property_override_suppresses_uniform_object_appearance() {
        for triangle_property in [r#"pid="5""#, r#"p1="0""#, r#"p2="0""#, r#"p3="0""#] {
            let mesh = cube_mesh_with_object_attrs_xml(1, r#"pid="5" pindex="0""#).replacen(
                r#"<triangle v1="0" v2="2" v3="1"/>"#,
                &format!(r#"<triangle v1="0" v2="2" v3="1" {triangle_property}/>"#),
                1,
            );
            let resources = format!(
                r##"<basematerials id="5"><base name="Object Default" displaycolor="#778899"/></basematerials>{mesh}"##
            );
            let model = model_xml("millimeter", &resources, r#"<item objectid="1"/>"#);
            let parsed =
                parse_3mf_with_plates(&package(&[("3D/3dmodel.model", &model)], false)).unwrap();

            assert_eq!(
                parsed.model.triangle_count, 12,
                "geometry changed for {triangle_property}"
            );
            assert_eq!(
                parsed.materials,
                vec![ParsedMaterial {
                    id: "3mf:3D/3dmodel.model:5:0".into(),
                    name: Some("Object Default".into()),
                    color: Some("#778899".into()),
                    source: MaterialSource::ThreeMfBaseMaterial,
                }],
                "resource changed for {triangle_property}"
            );
            assert_eq!(
                parsed.parts.len(),
                1,
                "part boundary missing for {triangle_property}"
            );
            assert_eq!(
                parsed.parts[0].material_id, None,
                "{triangle_property} did not suppress the uniform appearance"
            );
        }
    }

    #[test]
    fn divergent_descendant_object_defaults_suppress_wrapper_wide_appearance() {
        let first = cube_mesh_with_object_attrs_xml(2, r#"pid="5" pindex="0""#);
        let second = cube_mesh_with_object_attrs_xml(3, r#"pid="5" pindex="1""#);
        let resources = format!(
            r##"<basematerials id="5"><base name="First" displaycolor="#AA0000"/><base name="Second" displaycolor="#00AA00"/></basematerials>
<object id="1" name="Assembly" type="model"><components><component objectid="2"/><component objectid="3"/></components></object>
<object id="10" type="model"><components><component objectid="1"/></components></object>
{first}{second}"##
        );
        let model = model_xml("millimeter", &resources, r#"<item objectid="10"/>"#);
        let parsed =
            parse_3mf_with_plates(&package(&[("3D/3dmodel.model", &model)], false)).unwrap();

        assert_eq!(parsed.model.triangle_count, 24);
        assert_eq!(
            parsed.materials,
            vec![
                ParsedMaterial {
                    id: "3mf:3D/3dmodel.model:5:0".into(),
                    name: Some("First".into()),
                    color: Some("#AA0000".into()),
                    source: MaterialSource::ThreeMfBaseMaterial,
                },
                ParsedMaterial {
                    id: "3mf:3D/3dmodel.model:5:1".into(),
                    name: Some("Second".into()),
                    color: Some("#00AA00".into()),
                    source: MaterialSource::ThreeMfBaseMaterial,
                },
            ]
        );
        assert_eq!(parsed.parts.len(), 1);
        assert_eq!(parsed.parts[0].triangle_start, 0);
        assert_eq!(parsed.parts[0].triangle_count, 24);
        assert_eq!(parsed.parts[0].material_id, None);
    }

    #[test]
    fn reads_a_deflated_cube_and_measures_it_in_mm() {
        let parsed = parse_3mf(&simple_cube("millimeter", true)).unwrap();
        assert_eq!(parsed.format, StlFormat::ThreeMf);
        assert_eq!(parsed.triangle_count, 12);
        assert_eq!(parsed.positions.len(), 12 * 9);
        assert_eq!(parsed.stored_normals.len(), 12 * 3);
        let stats = crate::measure(&parsed).unwrap();
        assert_eq!(stats.dimensions, [10.0, 10.0, 10.0]);
        assert!((stats.volume_mm3 - 1000.0).abs() < 1e-3);
        let crate::IntegrityResult::Computed(i) = crate::integrity(&parsed) else {
            panic!()
        };
        assert!(i.watertight());
        assert_eq!(i.inconsistent_orientation, 0);
        assert_eq!(i.normal_disagreements, 0);
    }

    #[test]
    fn converts_units_to_millimetres() {
        let parsed = parse_3mf(&simple_cube("inch", false)).unwrap();
        let stats = crate::measure(&parsed).unwrap();
        assert!((stats.dimensions[0] - 254.0).abs() < 1e-3);
    }

    #[test]
    fn unknown_unit_is_rejected() {
        let err = parse_3mf(&simple_cube("furlong", false)).unwrap_err();
        assert!(matches!(err, StlError::Malformed3mf { .. }), "{err:?}");
    }

    #[test]
    fn follows_p_path_components_and_applies_item_transforms() {
        // Bambu/Orca layout: the root part holds only component wrappers, the
        // mesh lives in 3D/Objects/*.model, and the build item translates it.
        let root = model_xml(
            "millimeter",
            r#"<object id="2" type="model"><components>
<component p:path="/3D/Objects/object_1.model" objectid="1" transform="1 0 0 0 1 0 0 0 1 5 0 0"/>
</components></object>"#,
            r#"<item objectid="2" transform="1 0 0 0 1 0 0 0 1 100 200 0" printable="1"/>"#,
        );
        let sub_part = model_xml("millimeter", &cube_mesh_xml(1), "");
        let bytes = package(
            &[
                ("_rels/.rels", RELS),
                ("3D/3dmodel.model", &root),
                ("3D/Objects/object_1.model", &sub_part),
            ],
            true,
        );
        let parsed = parse_3mf(&bytes).unwrap();
        assert_eq!(parsed.triangle_count, 12);
        let stats = crate::measure(&parsed).unwrap();
        // component +5 in x, then item +100 x / +200 y
        assert_eq!(stats.bbox.min, [105.0, 200.0, 0.0]);
        assert_eq!(stats.bbox.max, [115.0, 210.0, 10.0]);
    }

    #[test]
    fn mirrored_placement_keeps_outward_winding() {
        let model = model_xml(
            "millimeter",
            &cube_mesh_xml(1),
            r#"<item objectid="1" transform="-1 0 0 0 1 0 0 0 1 0 0 0"/>"#,
        );
        let bytes = package(&[("3D/3dmodel.model", &model)], false); // no .rels: fallback path
        let parsed = parse_3mf(&bytes).unwrap();
        let crate::IntegrityResult::Computed(i) = crate::integrity(&parsed) else {
            panic!()
        };
        assert!(i.watertight());
        assert_eq!(i.inconsistent_orientation, 0);
        // Outward winding: signed volume stays positive after the mirror.
        let mut signed = 0.0f64;
        for tri in parsed.positions.chunks_exact(9) {
            let v0 = [tri[0], tri[1], tri[2]];
            let v1 = [tri[3], tri[4], tri[5]];
            let v2 = [tri[6], tri[7], tri[8]];
            signed += dot(v0, cross(v1, v2)) as f64 / 6.0;
        }
        assert!(signed > 999.0, "signed volume {signed}");
    }

    #[test]
    fn unprintable_items_and_non_model_objects_are_skipped() {
        let resources = format!(
            "{}{}",
            cube_mesh_xml(1),
            cube_mesh_xml(2).replace(r#"type="model""#, r#"type="support""#)
        );
        let model = model_xml(
            "millimeter",
            &resources,
            r#"<item objectid="1"/><item objectid="1" printable="0" transform="1 0 0 0 1 0 0 0 1 500 0 0"/><item objectid="2"/>"#,
        );
        let bytes = package(
            &[("_rels/.rels", RELS), ("3D/3dmodel.model", &model)],
            false,
        );
        let parsed = parse_3mf(&bytes).unwrap();
        assert_eq!(parsed.triangle_count, 12);
        assert_eq!(
            crate::measure(&parsed).unwrap().dimensions,
            [10.0, 10.0, 10.0]
        );
    }

    #[test]
    fn splits_and_reorders_repeated_instances_into_contiguous_plate_ranges() {
        let model = model_xml(
            "millimeter",
            &cube_mesh_xml(1),
            r#"<item objectid="1"/><item objectid="1" transform="1 0 0 0 1 0 0 0 1 100 0 0"/>"#,
        );
        let settings = plate_settings(&format!(
            "{}{}",
            plate_xml(9, "Far", &[(1, 1)]),
            plate_xml(3, "Near", &[(1, 0)])
        ));
        let bytes = package(
            &[
                ("3D/3dmodel.model", &model),
                (PLATE_SETTINGS_PATH, &settings),
            ],
            true,
        );

        let parsed = parse_3mf_with_plates(&bytes).unwrap();
        assert!(parsed.plate_metadata);
        assert_eq!(parsed.plate_warning, None);
        assert_eq!(parsed.model.triangle_count, 24);
        assert_eq!(parsed.plates.len(), 2);
        assert_eq!(parsed.plates[0].id, 0);
        assert_eq!(parsed.plates[0].name, "Far");
        assert_eq!(parsed.plates[0].triangle_start, 0);
        assert_eq!(parsed.plates[0].triangle_count, 12);
        assert_eq!(parsed.plates[1].id, 1);
        assert_eq!(parsed.plates[1].triangle_start, 12);
        let far = parsed.plate_mesh(0).unwrap();
        let near = parsed.plate_mesh(1).unwrap();
        assert_eq!(crate::measure(&far).unwrap().bbox.min[0], 100.0);
        assert_eq!(crate::measure(&near).unwrap().bbox.min[0], 0.0);
        assert!(parsed.plate_mesh(2).is_none());
    }

    #[test]
    fn instance_occurrences_include_disabled_build_items_before_filtering() {
        let model = model_xml(
            "millimeter",
            &cube_mesh_xml(1),
            r#"<item objectid="1" printable="0"/><item objectid="1" transform="1 0 0 0 1 0 0 0 1 50 0 0"/>"#,
        );
        let settings = plate_settings(&plate_xml(1, "Enabled", &[(1, 1)]));
        let bytes = package(
            &[
                ("3D/3dmodel.model", &model),
                (PLATE_SETTINGS_PATH, &settings),
            ],
            false,
        );
        let parsed = parse_3mf_with_plates(&bytes).unwrap();
        assert!(parsed.plate_metadata);
        assert_eq!(parsed.plates[0].triangle_count, 12);
        assert_eq!(crate::measure(&parsed.model).unwrap().bbox.min[0], 50.0);
    }

    #[test]
    fn valid_empty_plates_are_preserved_in_display_order() {
        let model = model_xml("millimeter", &cube_mesh_xml(1), r#"<item objectid="1"/>"#);
        let settings = plate_settings(&format!(
            "{}{}",
            plate_xml(1, "Empty", &[]),
            plate_xml(2, "Model", &[(1, 0)])
        ));
        let bytes = package(
            &[
                ("3D/3dmodel.model", &model),
                (PLATE_SETTINGS_PATH, &settings),
            ],
            false,
        );
        let parsed = parse_3mf_with_plates(&bytes).unwrap();
        assert_eq!(parsed.plates[0].triangle_count, 0);
        assert_eq!(parsed.plates[1].triangle_start, 0);
        assert_eq!(parsed.plates[1].triangle_count, 12);
        assert_eq!(parsed.plate_mesh(0).unwrap().positions, Vec::<f32>::new());
    }

    #[test]
    fn unnamed_slicer_plates_get_display_names_without_losing_membership() {
        let specs = parse_plate_settings(
            r#"<config>
          <plate><metadata key="plater_id" value="1"/><metadata key="plater_name" value=""/></plate>
          <plate><metadata key="plater_id" value="2"/></plate>
        </config>"#,
        )
        .unwrap();
        assert_eq!(specs[0].name, "Plate 1");
        assert_eq!(specs[1].name, "Plate 2");
    }

    #[test]
    fn excessive_empty_plate_metadata_is_bounded() {
        let plates = (0..=MAX_VIEWER_PLATES)
            .map(|id| plate_xml(id as u32 + 1, "Empty", &[]))
            .collect::<String>();
        assert!(parse_plate_settings(&plate_settings(&plates))
            .unwrap_err()
            .contains("more than 256 plates"));
    }

    #[test]
    fn non_printed_unassigned_objects_do_not_invalidate_plate_membership() {
        let resources = format!(
            "{}{}",
            cube_mesh_xml(1),
            cube_mesh_xml(2).replace("type=\"model\"", "type=\"support\"")
        );
        let root = model_xml(
            "millimeter",
            &resources,
            r#"<item objectid="1"/><item objectid="2"/>"#,
        );
        let settings = plate_settings(&plate_xml(1, "Model", &[(1, 0)]));
        let bytes = package(
            &[
                ("3D/3dmodel.model", &root),
                (PLATE_SETTINGS_PATH, &settings),
            ],
            true,
        );
        let parsed = parse_3mf_with_plates(&bytes).unwrap();
        assert!(parsed.plate_metadata);
        assert_eq!(parsed.model.triangle_count, 12);
        assert_eq!(parsed.plates[0].triangle_count, 12);
    }

    #[test]
    fn missing_or_duplicated_plate_assignments_fall_back_without_geometry_loss() {
        let model = model_xml(
            "millimeter",
            &cube_mesh_xml(1),
            r#"<item objectid="1"/><item objectid="1" transform="1 0 0 0 1 0 0 0 1 100 0 0"/>"#,
        );
        let missing = package(&[("3D/3dmodel.model", &model)], false);
        let parsed = parse_3mf_with_plates(&missing).unwrap();
        assert!(!parsed.plate_metadata);
        assert_eq!(parsed.plates[0].name, "Whole build");
        assert_eq!(parsed.model.triangle_count, 24);
        assert!(parsed
            .plate_warning
            .as_deref()
            .unwrap()
            .contains("no usable"));

        let duplicated = plate_settings(&format!(
            "{}{}",
            plate_xml(1, "One", &[(1, 0)]),
            plate_xml(2, "Two", &[(1, 0), (1, 1)])
        ));
        let bytes = package(
            &[
                ("3D/3dmodel.model", &model),
                (PLATE_SETTINGS_PATH, &duplicated),
            ],
            false,
        );
        let parsed = parse_3mf_with_plates(&bytes).unwrap();
        assert!(!parsed.plate_metadata);
        assert_eq!(parsed.model.triangle_count, 24);
        assert!(parsed
            .plate_warning
            .as_deref()
            .unwrap()
            .contains("assigned more than once"));
    }

    #[test]
    fn malformed_unknown_and_incomplete_metadata_each_fall_back_safely() {
        let model = model_xml(
            "millimeter",
            &cube_mesh_xml(1),
            r#"<item objectid="1"/><item objectid="1" transform="1 0 0 0 1 0 0 0 1 100 0 0"/>"#,
        );
        let variants = [
            plate_settings(r#"<plate><metadata key="plater_id" value="bad"/></plate>"#),
            plate_settings(&plate_xml(1, "Unknown", &[(7, 0)])),
            plate_settings(&plate_xml(1, "Incomplete", &[(1, 0)])),
            plate_settings(""),
        ];
        for settings in variants {
            let bytes = package(
                &[
                    ("3D/3dmodel.model", &model),
                    (PLATE_SETTINGS_PATH, &settings),
                ],
                false,
            );
            let parsed = parse_3mf_with_plates(&bytes).unwrap();
            assert!(!parsed.plate_metadata);
            assert_eq!(parsed.model.triangle_count, 24);
            assert_eq!(parsed.plates[0].triangle_count, 24);
            assert!(parsed.plate_warning.is_some());
        }
    }

    #[test]
    fn plate_membership_handles_repeated_p_path_object_ids() {
        let root = model_xml(
            "millimeter",
            "",
            r#"<item p:path="/3D/Objects/a.model" objectid="1"/><item p:path="/3D/Objects/b.model" objectid="1" transform="1 0 0 0 1 0 0 0 1 100 0 0"/>"#,
        );
        let object = model_xml("millimeter", &cube_mesh_xml(1), "");
        let settings = plate_settings(&format!(
            "{}{}",
            plate_xml(1, "B", &[(1, 1)]),
            plate_xml(2, "A", &[(1, 0)])
        ));
        let bytes = package(
            &[
                ("3D/3dmodel.model", &root),
                ("3D/Objects/a.model", &object),
                ("3D/Objects/b.model", &object),
                (PLATE_SETTINGS_PATH, &settings),
            ],
            false,
        );
        let parsed = parse_3mf_with_plates(&bytes).unwrap();
        assert!(parsed.plate_metadata);
        assert_eq!(
            crate::measure(&parsed.plate_mesh(0).unwrap())
                .unwrap()
                .bbox
                .min[0],
            100.0
        );
        assert_eq!(
            crate::measure(&parsed.plate_mesh(1).unwrap())
                .unwrap()
                .bbox
                .min[0],
            0.0
        );
    }

    #[test]
    fn bad_vertex_index_and_missing_object_are_specific_errors() {
        let bad_index = model_xml(
            "millimeter",
            r#"<object id="1"><mesh><vertices><vertex x="0" y="0" z="0"/></vertices>
<triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object>"#,
            r#"<item objectid="1"/>"#,
        );
        let err = parse_3mf(&package(&[("3D/3dmodel.model", &bad_index)], false)).unwrap_err();
        assert!(
            matches!(&err, StlError::Malformed3mf { detail } if detail.contains("missing vertex")),
            "{err:?}"
        );

        let dangling = model_xml("millimeter", &cube_mesh_xml(1), r#"<item objectid="9"/>"#);
        let err = parse_3mf(&package(&[("3D/3dmodel.model", &dangling)], false)).unwrap_err();
        assert!(
            matches!(&err, StlError::Malformed3mf { detail } if detail.contains("object 9")),
            "{err:?}"
        );
    }

    #[test]
    fn component_cycles_are_refused() {
        let model = model_xml(
            "millimeter",
            r#"<object id="1"><components><component objectid="2"/></components></object>
<object id="2"><components><component objectid="1"/></components></object>"#,
            r#"<item objectid="1"/>"#,
        );
        let err = parse_3mf(&package(&[("3D/3dmodel.model", &model)], false)).unwrap_err();
        assert!(
            matches!(&err, StlError::Malformed3mf { detail } if detail.contains("cyclic")),
            "{err:?}"
        );
    }

    #[test]
    fn zip_without_a_model_part_is_not_a_3mf() {
        let bytes = package(&[("readme.txt", "hello")], false);
        let err = parse_3mf(&bytes).unwrap_err();
        assert!(matches!(err, StlError::Malformed3mf { .. }));
    }

    #[test]
    fn parse_model_sniffs_zip_magic_and_still_handles_stl() {
        let parsed = crate::parse_model(&simple_cube("millimeter", true)).unwrap();
        assert_eq!(parsed.format, StlFormat::ThreeMf);

        let ascii = b"solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid t\n";
        let parsed = crate::parse_model(ascii).unwrap();
        assert_eq!(parsed.format, StlFormat::Ascii);
        assert_eq!(parsed.triangle_count, 1);

        let parsed = crate::parse_model_with_plates(ascii).unwrap();
        assert_eq!(parsed.plates[0].id, 0);
        assert_eq!(parsed.plates[0].name, "Model");
        assert_eq!(parsed.plates[0].triangle_count, 1);
        assert!(!parsed.plate_metadata);
        assert_eq!(parsed.plate_warning, None);
    }

    /// Opt-in check against a real slicer project file:
    /// `GOGGLELAB_3MF_SAMPLE=/path/to/file.3mf cargo test -p stl-core -- --ignored`
    #[test]
    #[ignore]
    fn reads_a_real_project_file() {
        let path = std::env::var("GOGGLELAB_3MF_SAMPLE").expect("set GOGGLELAB_3MF_SAMPLE");
        let bytes = std::fs::read(&path).unwrap();
        let parsed = parse_3mf_with_plates(&bytes).unwrap();
        let stats = crate::measure(&parsed.model).unwrap();
        eprintln!(
            "{path}: {} triangles, dims {:?}, volume {:.2} cm3",
            parsed.model.triangle_count,
            stats.dimensions,
            stats.volume_mm3 / 1000.0
        );
        assert_eq!(parsed.model.triangle_count, 112_784);
        assert_eq!(parsed.plates.len(), 2);
        assert_eq!(parsed.plates[0].name, "Screw Side");
        assert_eq!(parsed.plates[0].triangle_count, 58_188);
        assert_eq!(parsed.plates[1].name, "Nut Side");
        assert_eq!(parsed.plates[1].triangle_count, 54_596);
        let screw = crate::measure(&parsed.plate_mesh(0).unwrap()).unwrap();
        let nut = crate::measure(&parsed.plate_mesh(1).unwrap()).unwrap();
        assert_eq!(screw.dimensions, [199.9913, 199.9928, 64.0]);
        assert_eq!(nut.dimensions, [199.99133, 199.99295, 27.0]);
    }

    #[test]
    fn optional_slicer_and_project_metadata_enriches_structural_parts() {
        let resources = format!(
            r#"<object id="10" name="Root"><components>
<component objectid="2"/><component objectid="3"/>
</components></object>
{}{}"#,
            cube_mesh_with_object_attrs_xml(2, r#"name="Child A""#),
            cube_mesh_with_object_attrs_xml(3, r#"name="Child B""#)
        );
        let model = model_xml("millimeter", &resources, r#"<item objectid="10"/>"#);
        let settings = plate_settings(
            r#"<object id="10">
              <metadata key="name" value="Assembly"/>
              <metadata key="extruder" value="2"/>
              <part id="2">
                <metadata key="name" value="Badge"/>
                <metadata key="extruder" value="3"/>
              </part>
              <part id="3">
                <metadata key="name" value="Button"/>
                <metadata key="extruder" value="0"/>
              </part>
            </object>"#,
        );
        let project = r##"{"filament_settings_id":["PLA Basic","Vendor PLA"],"filament_type":["PLA","PETG"],"filament_vendor":["Acme","Vendor"],"filament_colour":["#aabbcc80","#112233"]}"##;
        let parsed = parse_3mf_with_plates(&package(
            &[
                ("3D/3dmodel.model", &model),
                (PLATE_SETTINGS_PATH, &settings),
                ("Metadata/project_settings.config", project),
            ],
            false,
        ))
        .unwrap();

        assert_eq!(parsed.parts.len(), 2);
        assert_eq!(parsed.parts[0].name, "Assembly · Badge");
        assert_eq!(parsed.parts[0].filament_slot, Some(3));
        assert_eq!(parsed.parts[1].name, "Assembly · Button");
        assert_eq!(parsed.parts[1].filament_slot, Some(2));
        assert_eq!(parsed.embedded_filaments.len(), 2);
        assert_eq!(
            parsed.embedded_filaments[0].name.as_deref(),
            Some("Acme PLA Basic")
        );
        assert_eq!(
            parsed.embedded_filaments[1].name.as_deref(),
            Some("Vendor PLA")
        );
    }

    #[test]
    fn malformed_optional_metadata_keeps_geometry_and_omits_only_bad_fields() {
        let model = model_xml("millimeter", &cube_mesh_xml(1), r#"<item objectid="1"/>"#);
        let settings = plate_settings(
            r#"<object id="1">
              <metadata key="name" value="Good"/>
              <metadata key="name" value="Conflicting"/>
              <metadata key="extruder" value="-1"/>
            </object>"#,
        );
        let parsed = parse_3mf_with_plates(&package(
            &[
                ("3D/3dmodel.model", &model),
                (PLATE_SETTINGS_PATH, &settings),
                ("Metadata/project_settings.config", "not json"),
            ],
            false,
        ))
        .unwrap();
        assert_eq!(parsed.model.triangle_count, 12);
        assert_eq!(parsed.parts.len(), 1);
        assert_eq!(parsed.parts[0].name, "Object 1");
        assert_eq!(parsed.parts[0].filament_slot, None);
        assert!(parsed.embedded_filaments.is_empty());
    }

    #[test]
    fn excessive_structural_boundaries_clear_parts_with_exact_warning() {
        let build = (0..=MAX_VIEWER_PARTS)
            .map(|_| r#"<item objectid="1"/>"#)
            .collect::<String>();
        let model = model_xml("millimeter", &cube_mesh_xml(1), &build);
        let parsed =
            parse_3mf_with_plates(&package(&[("3D/3dmodel.model", &model)], false)).unwrap();
        assert_eq!(
            parsed.model.triangle_count,
            12 * (MAX_VIEWER_PARTS as u32 + 1)
        );
        assert!(parsed.parts.is_empty());
        assert_eq!(parsed.part_warning.as_deref(), Some(PART_WARNING));
    }

    #[test]
    fn standard_material_and_embedded_palette_entries_are_bounded() {
        let bases = (0..=MAX_STANDARD_MATERIALS)
            .map(|index| format!(r##"<base name="M{index}" displaycolor="#112233"/>"##))
            .collect::<String>();
        let resources = format!(
            r#"<basematerials id="5">{bases}</basematerials>{}"#,
            cube_mesh_with_object_attrs_xml(1, r#"pid="5" pindex="0""#)
        );
        let model = model_xml("millimeter", &resources, r#"<item objectid="1"/>"#);
        let parsed =
            parse_3mf_with_plates(&package(&[("3D/3dmodel.model", &model)], false)).unwrap();
        assert_eq!(parsed.materials.len(), MAX_STANDARD_MATERIALS);

        let colors = (0..300).map(|_| r#""#).collect::<Vec<_>>();
        let text = format!(
            r#"{{"filament_colour":[{}]}}"#,
            colors
                .iter()
                .map(|_| r#""#)
                .enumerate()
                .map(|(_, _)| r##""#112233""##)
                .collect::<Vec<_>>()
                .join(",")
        );
        assert_eq!(
            parse_project_filaments(&text).unwrap().len(),
            MAX_EMBEDDED_FILAMENTS
        );
    }

    #[test]
    fn root_metadata_matches_equivalent_explicit_build_paths() {
        let model = model_xml(
            "millimeter",
            &cube_mesh_with_object_attrs_xml(1, r#"name="Mesh Name""#),
            r#"<item p:path="3D/3dmodel.model" objectid="1"/>"#,
        );
        let settings = plate_settings(&format!(
            r##"{}<object id="1"><metadata key="name" value="Slicer Name"/><metadata key="extruder" value="4"/></object>"##,
            plate_xml(1, "Plate", &[(1, 0)])
        ));
        let parsed = parse_3mf_with_plates(&package(
            &[
                ("_rels/.rels", RELS),
                ("3D/3dmodel.model", &model),
                (PLATE_SETTINGS_PATH, &settings),
            ],
            false,
        ))
        .unwrap();
        assert_eq!(parsed.parts[0].name, "Slicer Name");
        assert_eq!(parsed.parts[0].filament_slot, Some(4));
    }

    #[test]
    fn dropped_global_materials_do_not_leave_dangling_part_ids() {
        let root_bases = (0..MAX_STANDARD_MATERIALS)
            .map(|index| format!(r##"<base name="M{index}" displaycolor="#112233"/>"##))
            .collect::<String>();
        let root = model_xml(
            "millimeter",
            &format!(r#"<basematerials id="5">{root_bases}</basematerials>"#),
            r#"<item p:path="/3D/Objects/child.model" objectid="1"/>"#,
        );
        let child_resources = format!(
            r##"<basematerials id="8"><base name="Child" displaycolor="#445566"/></basematerials>{}"##,
            cube_mesh_with_object_attrs_xml(1, r#"pid="8" pindex="0""#)
        );
        let child = model_xml("millimeter", &child_resources, "");
        let parsed = parse_3mf_with_plates(&package(
            &[
                ("_rels/.rels", RELS),
                ("3D/3dmodel.model", &root),
                ("3D/Objects/child.model", &child),
            ],
            false,
        ))
        .unwrap();
        assert_eq!(parsed.materials.len(), MAX_STANDARD_MATERIALS);
        assert_eq!(parsed.parts.len(), 1);
        assert_eq!(parsed.parts[0].material_id, None);
    }

    #[test]
    fn malformed_part_id_does_not_fall_through_to_object_metadata() {
        let resources = format!(
            r#"<object id="10" type="model"><components><component objectid="2"/></components></object>{}"#,
            cube_mesh_with_object_attrs_xml(2, r#"name="Child""#)
        );
        let model = model_xml("millimeter", &resources, r#"<item objectid="10"/>"#);
        let settings = plate_settings(
            r#"<object id="10">
              <metadata key="extruder" value="2"/>
              <part id="bad">
                <metadata key="extruder" value="9"/>
              </part>
            </object>"#,
        );
        let parsed = parse_3mf_with_plates(&package(
            &[
                ("3D/3dmodel.model", &model),
                (PLATE_SETTINGS_PATH, &settings),
            ],
            false,
        ))
        .unwrap();
        assert_eq!(parsed.parts.len(), 1);
        assert_eq!(parsed.parts[0].filament_slot, Some(2));
    }

    #[test]
    fn ambiguous_same_id_components_do_not_use_pathless_part_metadata() {
        let root = model_xml(
            "millimeter",
            r#"<object id="10" type="model"><components>
<component p:path="/3D/Objects/a.model" objectid="2"/>
<component p:path="/3D/Objects/b.model" objectid="2"/>
</components></object>"#,
            r#"<item objectid="10"/>"#,
        );
        let child_a = model_xml(
            "millimeter",
            &cube_mesh_with_object_attrs_xml(2, r#"name="A""#),
            "",
        );
        let child_b = model_xml(
            "millimeter",
            &cube_mesh_with_object_attrs_xml(2, r#"name="B""#),
            "",
        );
        let settings = plate_settings(
            r#"<object id="10">
              <metadata key="extruder" value="2"/>
              <part id="2">
                <metadata key="name" value="Ambiguous"/>
                <metadata key="extruder" value="3"/>
              </part>
            </object>"#,
        );
        let parsed = parse_3mf_with_plates(&package(
            &[
                ("_rels/.rels", RELS),
                ("3D/3dmodel.model", &root),
                ("3D/Objects/a.model", &child_a),
                ("3D/Objects/b.model", &child_b),
                (PLATE_SETTINGS_PATH, &settings),
            ],
            false,
        ))
        .unwrap();
        assert_eq!(
            parsed
                .parts
                .iter()
                .map(|part| part.name.as_str())
                .collect::<Vec<_>>(),
            vec!["A", "B"]
        );
        assert_eq!(
            parsed
                .parts
                .iter()
                .map(|part| part.filament_slot)
                .collect::<Vec<_>>(),
            vec![Some(2), Some(2)]
        );
    }

    #[test]
    fn equivalent_normalized_component_paths_use_pathless_part_metadata() {
        let root = model_xml(
            "millimeter",
            r#"<object id="10" type="model"><components>
<component p:path="/3D/Objects/a.model" objectid="2"/>
<component p:path="3D/Objects/a.model" objectid="2"/>
</components></object>"#,
            r#"<item objectid="10"/>"#,
        );
        let child = model_xml("millimeter", &cube_mesh_xml(2), "");
        let settings = plate_settings(
            r#"<object id="10">
              <metadata key="name" value="Wrapper"/>
              <metadata key="extruder" value="2"/>
              <part id="2">
                <metadata key="name" value="Part"/>
                <metadata key="extruder" value="3"/>
              </part>
            </object>"#,
        );
        let parsed = parse_3mf_with_plates(&package(
            &[
                ("_rels/.rels", RELS),
                ("3D/3dmodel.model", &root),
                ("3D/Objects/a.model", &child),
                (PLATE_SETTINGS_PATH, &settings),
            ],
            false,
        ))
        .unwrap();
        assert_eq!(
            parsed
                .parts
                .iter()
                .map(|part| part.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Wrapper · Part", "Wrapper · Part"]
        );
        assert_eq!(
            parsed
                .parts
                .iter()
                .map(|part| part.filament_slot)
                .collect::<Vec<_>>(),
            vec![Some(3), Some(3)]
        );
    }

    #[test]
    fn cross_wrapper_component_paths_do_not_use_pathless_part_metadata() {
        let root = model_xml(
            "millimeter",
            r#"<object id="10" type="model"><components>
<component p:path="/3D/Objects/a.model" objectid="2"/>
</components></object>
<object id="11" type="model"><components>
<component p:path="/3D/Objects/b.model" objectid="2"/>
</components></object>"#,
            r#"<item objectid="10"/><item objectid="11"/>"#,
        );
        let child_a = model_xml("millimeter", &cube_mesh_xml(2), "");
        let child_b = model_xml("millimeter", &cube_mesh_xml(2), "");
        let settings = plate_settings(
            r#"<object id="10">
              <metadata key="name" value="Wrapper A"/>
              <metadata key="extruder" value="4"/>
              <part id="2">
                <metadata key="name" value="Pathless Part"/>
                <metadata key="extruder" value="3"/>
              </part>
            </object>
            <object id="11">
              <metadata key="name" value="Wrapper B"/>
              <metadata key="extruder" value="5"/>
            </object>"#,
        );
        let parsed = parse_3mf_with_plates(&package(
            &[
                ("_rels/.rels", RELS),
                ("3D/3dmodel.model", &root),
                ("3D/Objects/a.model", &child_a),
                ("3D/Objects/b.model", &child_b),
                (PLATE_SETTINGS_PATH, &settings),
            ],
            false,
        ))
        .unwrap();
        assert_eq!(
            parsed
                .parts
                .iter()
                .map(|part| part.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Wrapper A", "Wrapper B"]
        );
        assert_eq!(
            parsed
                .parts
                .iter()
                .map(|part| part.filament_slot)
                .collect::<Vec<_>>(),
            vec![Some(4), Some(5)]
        );
    }
}
