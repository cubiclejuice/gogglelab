//! Printer detection from installed slicers. Bambu Studio and ElegooSlicer
//! (an OrcaSlicer fork of Bambu Studio) share one on-disk layout under
//! ~/Library/Application Support/<Slicer>/:
//!   <Slicer>.conf                      -> presets.machine = selected preset name
//!   user/<id>/machine/*.json           -> custom presets, `inherits` a system one
//!   system/<vendor>/machine/**/*.json  -> vendor profiles with printable_area /
//!                                         printable_height, chained via `inherits`
//! Read-only: nothing here writes to a slicer's files.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::slicers;

#[derive(Debug, Clone, Serialize)]
pub struct DetectedPrinter {
    /// Stable id: "<bundle_id>:<preset name>", so it never collides with a
    /// manual printer id and stays the same across launches.
    pub id: String,
    pub name: String,
    pub bed_mm: [f32; 3],
    pub source_slicer: String, // bundle id
    pub source_display: String,
}

#[tauri::command]
pub fn detect_printers() -> Vec<DetectedPrinter> {
    let installed: HashSet<String> = slicers::detect_slicers()
        .into_iter()
        .map(|s| s.bundle_id)
        .collect();
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return vec![];
    };
    let mut out = Vec::new();
    for definition in slicers::slicer_definitions() {
        if !installed.contains(definition.bundle_id) {
            continue;
        }
        let support = home
            .join("Library/Application Support")
            .join(definition.support_dir);
        if let Some(r) = detect_in(&support, definition.config_name) {
            out.push(DetectedPrinter {
                id: format!("{}:{}", definition.bundle_id, r.preset),
                name: r.model,
                bed_mm: r.bed,
                source_slicer: definition.bundle_id.to_string(),
                source_display: definition.display_name.to_string(),
            });
        }
    }
    out
}

pub struct Resolved {
    pub preset: String,
    /// `printer_model` from the profile chain -- the machine's real name
    /// ("Bambu Lab X2D"), as opposed to the preset name the user picked
    /// ("Bambu Lab X2D 0.4 nozzle"), which is a print-profile identity.
    pub model: String,
    pub bed: [f32; 3],
}

/// Selected preset, resolved model name, and bed for one slicer's support folder.
pub fn detect_in(support: &Path, conf_name: &str) -> Option<Resolved> {
    let conf: Value = serde_json::from_slice(&std::fs::read(support.join(conf_name)).ok()?).ok()?;
    let preset = conf.get("presets")?.get("machine")?.as_str()?.to_string();
    let (bed, model) = resolve(support, &preset)?;
    Some(Resolved {
        model: model.unwrap_or_else(|| preset.clone()),
        preset,
        bed,
    })
}

fn machine_dirs(support: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for sub in ["user", "system"] {
        walk(&support.join(sub), &mut dirs);
    }
    dirs
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().and_then(|n| n.to_str()) == Some("machine") {
                collect_json(&p, out);
            } else {
                walk(&p, out);
            }
        }
    }
}

fn collect_json(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_json(&p, out);
        } else if p.extension().and_then(|x| x.to_str()) == Some("json") {
            out.push(p);
        }
    }
}

/// Follows `inherits` until printable_area and printable_height are both
/// known, picking up the first `printer_model` seen on the way. Bounded by a
/// visited set so a cyclic profile can't loop.
fn resolve(support: &Path, preset: &str) -> Option<([f32; 3], Option<String>)> {
    let files = machine_dirs(support);
    let mut name = preset.to_string();
    let mut seen = HashSet::new();
    let (mut area, mut height): (Option<[f32; 2]>, Option<f32>) = (None, None);
    let mut model: Option<String> = None;
    while seen.insert(name.clone()) {
        let json = find_profile(&files, &name)?;
        if model.is_none() {
            model = json
                .get("printer_model")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
        }
        if area.is_none() {
            area = json.get("printable_area").and_then(bbox_of_area);
        }
        if height.is_none() {
            height = json.get("printable_height").and_then(|v| {
                v.as_str()
                    .and_then(|s| s.trim().parse().ok())
                    .or_else(|| v.as_f64().map(|f| f as f32))
            });
        }
        if let (Some(a), Some(h)) = (area, height) {
            // Keep walking only if the model name is still missing.
            if model.is_some() {
                return Some(([a[0], a[1], h], model));
            }
            match json.get("inherits").and_then(|v| v.as_str()) {
                Some(next) => {
                    name = next.to_string();
                    continue;
                }
                None => return Some(([a[0], a[1], h], None)),
            }
        }
        name = json.get("inherits")?.as_str()?.to_string();
    }
    None
}

fn find_profile(files: &[PathBuf], name: &str) -> Option<Value> {
    files.iter().find_map(|p| {
        let v: Value = serde_json::from_slice(&std::fs::read(p).ok()?).ok()?;
        (v.get("name")?.as_str()? == name).then_some(v)
    })
}

/// printable_area is a list of "XxY" corner points. Some beds aren't
/// rectangular (deltas), so take the bounding box.
fn bbox_of_area(v: &Value) -> Option<[f32; 2]> {
    let (mut maxx, mut maxy) = (f32::MIN, f32::MIN);
    let (mut minx, mut miny) = (f32::MAX, f32::MAX);
    for pt in v.as_array()? {
        let s = pt.as_str()?.trim();
        let (x, y) = s.split_once('x')?;
        let (x, y): (f32, f32) = (x.trim().parse().ok()?, y.trim().parse().ok()?);
        maxx = maxx.max(x);
        maxy = maxy.max(y);
        minx = minx.min(x);
        miny = miny.min(y);
    }
    (maxx > minx && maxy > miny).then_some([maxx - minx, maxy - miny])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("gogglelab-printers-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let sys = root.join("system/Vendor/machine/sub");
        let usr = root.join("user/123/machine");
        std::fs::create_dir_all(&sys).unwrap();
        std::fs::create_dir_all(&usr).unwrap();
        std::fs::write(
            root.join("Slicer.conf"),
            r#"{"presets":{"machine":"My Custom"}}"#,
        )
        .unwrap();
        std::fs::write(
            usr.join("custom.json"),
            r#"{"name":"My Custom","inherits":"Vendor X 0.4"}"#,
        )
        .unwrap();
        std::fs::write(sys.join("x.json"), r#"{"name":"Vendor X 0.4","inherits":"vendor_common","printable_height":"261","printer_model":"Vendor X"}"#).unwrap();
        std::fs::write(
            sys.join("common.json"),
            r#"{"name":"vendor_common","printable_area":["0x0","256x0","256x256","0x256 "]}"#,
        )
        .unwrap();
        root
    }

    #[test]
    fn resolves_selected_preset_through_user_and_system_inherits_chain() {
        let root = fixture("chain");
        let r = detect_in(&root, "Slicer.conf").unwrap();
        assert_eq!(r.preset, "My Custom");
        assert_eq!(
            r.model, "Vendor X",
            "display name is the printer model, not the preset"
        );
        assert_eq!(r.bed, [256.0, 256.0, 261.0]);
    }

    #[test]
    fn non_rectangular_area_uses_bounding_box() {
        let v: Value = serde_json::from_str(r#"["-100x0","0x100","100x0","0x-100"]"#).unwrap();
        assert_eq!(bbox_of_area(&v), Some([200.0, 200.0]));
    }

    #[test]
    fn cyclic_inherits_terminates_with_none() {
        let root = fixture("cycle");
        let sys = root.join("system/Vendor/machine/sub");
        std::fs::write(
            sys.join("x.json"),
            r#"{"name":"Vendor X 0.4","inherits":"My Custom"}"#,
        )
        .unwrap();
        assert!(detect_in(&root, "Slicer.conf").is_none());
    }
}
