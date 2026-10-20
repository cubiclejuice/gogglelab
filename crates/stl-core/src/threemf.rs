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
use std::io::{self, BufReader, Cursor, Read, Seek, SeekFrom};
use std::rc::Rc;

use quick_xml::events::{BytesStart, Event};
use quick_xml::XmlVersion;
#[cfg(test)]
use serde_json::Value;

mod derived_maps;
mod materials;
mod project_json;
mod settings;
use derived_maps::BuildIndex;

use crate::error::{ResourceStop, StlError};
use crate::limits::{MAX_FILE_BYTES, MAX_TRIANGLES};
use crate::model::{
    MaterialSource, ParsedFilament, ParsedMaterial, ParsedModel, ParsedPart, Plate,
};
use crate::package_bounds::{PackageProcessingContext, TextRole};
use crate::parse::{ParsedStl, StlFormat};
use crate::vec3::{cross, dot, sub, V3};
use crate::xml_bounds::XmlEventReader;
use crate::zip_bounds::{BoundedZipArchive, ZipOpenError, ZipResourceObserver};

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
// Bounds traversal work, including shared subgraphs with no output triangles.
// This is not a bound on parser memory or process RSS.
const MAX_COMPONENT_VISITS: u64 = 10_000_000;

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

fn missing_part_error(error: ZipOpenError, path: &str) -> StlError {
    match error {
        ZipOpenError::Archive(_) => malformed(format!("missing part {path}")),
        terminal => terminal.into(),
    }
}

fn preserve_terminal<T>(result: &Result<T, StlError>) -> Result<(), StlError> {
    match result {
        Err(StlError::ResourceStopped(stop)) => Err(StlError::ResourceStopped(*stop)),
        Err(StlError::AllocationFailed) => Err(StlError::AllocationFailed),
        _ => Ok(()),
    }
}

#[cfg(test)]
#[derive(Clone, Copy)]
enum ParserTestFailure {
    Stop(ResourceStop),
    Allocation,
}

#[cfg(test)]
impl ParserTestFailure {
    fn io_error(self) -> io::Error {
        match self {
            Self::Stop(stop) => stop.into_io_error(),
            Self::Allocation => crate::zip_bounds::ZipAllocationFailed.into_io_error(),
        }
    }

    fn error(self) -> StlError {
        match self {
            Self::Stop(stop) => StlError::ResourceStopped(stop),
            Self::Allocation => StlError::AllocationFailed,
        }
    }
}

#[cfg(test)]
#[derive(Default)]
struct ParserTestState {
    constructor_completed: bool,
    armed: bool,
    fired: bool,
    after_io: usize,
    work_after_stop: usize,
    total_work: usize,
    root_cached: bool,
}

#[cfg(test)]
struct ParserTestProbe {
    member: &'static str,
    failure: ParserTestFailure,
    state: std::cell::RefCell<ParserTestState>,
    guard_observer: Option<ZipResourceObserver>,
}

#[cfg(test)]
impl ParserTestProbe {
    fn before_io(&self, read: bool) -> io::Result<()> {
        let mut state = self.state.borrow_mut();
        if state.fired
            || self
                .guard_observer
                .as_ref()
                .is_some_and(|observer| observer.check().is_err())
        {
            state.after_io += 1;
        } else if state.armed && read {
            state.fired = true;
            return Err(self.failure.io_error());
        }
        Ok(())
    }

    fn record_work(&self) {
        let mut state = self.state.borrow_mut();
        state.total_work += 1;
        if state.fired
            || self
                .guard_observer
                .as_ref()
                .is_some_and(|observer| observer.check().is_err())
        {
            state.work_after_stop += 1;
        }
    }
}

#[cfg(test)]
thread_local! {
    static PARSER_TEST_PROBE: std::cell::RefCell<Option<Rc<ParserTestProbe>>> = const {
        std::cell::RefCell::new(None)
    };
}

#[derive(Clone, Copy)]
#[repr(usize)]
enum InputKind {
    Vertex,
    Triangle,
    Object,
    Component,
    BuildItem,
}

const INPUT_LIMITS: [usize; 5] = [6_000_000, 2_000_000, 10_000, 100_000, 100_000];

// Physical source records belong to one Package, not the shared read context.
struct InputCollections {
    counts: [usize; 5],
    limits: [usize; 5],
    observer: ZipResourceObserver,
    #[cfg(test)]
    test: Option<InputCollectionsTestSession>,
}

impl InputCollections {
    fn new(
        observer: ZipResourceObserver,
        #[cfg(test)] test: Option<InputCollectionsTestSession>,
    ) -> Self {
        #[cfg(not(test))]
        let limits = INPUT_LIMITS;
        #[cfg(test)]
        let limits = test
            .as_ref()
            .map_or(INPUT_LIMITS, |test| test.profile.limits);
        Self {
            counts: [0; 5],
            limits,
            observer,
            #[cfg(test)]
            test,
        }
    }

    fn admit(&mut self, kind: InputKind) -> Result<(), StlError> {
        let index = kind as usize;
        #[cfg(test)]
        if let Some(test) = &self.test {
            test.record(index);
        }
        let next = self.counts[index]
            .checked_add(1)
            .filter(|&next| next <= self.limits[index]);
        let Some(next) = next else {
            return input_stop(&self.observer);
        };
        self.counts[index] = next;
        Ok(())
    }

    fn ceiling(&self, kind: InputKind, prospective: usize) -> Result<usize, StlError> {
        prospective
            .checked_add(self.limits[kind as usize] - self.counts[kind as usize])
            .ok_or_else(|| input_error(&self.observer))
    }
}

fn input_error(observer: &ZipResourceObserver) -> StlError {
    let _ = observer.stop_io(ResourceStop::InputLimitExceeded);
    observer.check().expect_err("terminal input stop").into()
}
fn input_stop<T>(observer: &ZipResourceObserver) -> Result<T, StlError> {
    Err(input_error(observer))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
enum InputReserveSite {
    Vertices,
    Triangles,
    Components,
    Build,
    Objects,
    Parts,
    Positions,
    Normals,
}

fn reserve_input(
    observer: &ZipResourceObserver,
    _site: InputReserveSite,
    #[cfg(test)] test: Option<&InputCollectionsTestSession>,
    reserve: impl FnOnce() -> Result<(), std::collections::TryReserveError>,
) -> Result<(), StlError> {
    observer.check()?;
    #[cfg(test)]
    let attempt = test.map(|test| {
        let mut attempts = test.profile.reserves.borrow_mut();
        attempts[_site as usize] += 1;
        attempts[_site as usize]
    });
    #[cfg(test)]
    if let Some(test) = test {
        if test.profile.fail_reserve == Some((_site, attempt.unwrap())) {
            let _ = observer.allocation_failed_io();
            observer.check()?;
        }
    }
    if reserve().is_err() {
        let _ = observer.allocation_failed_io();
        observer.check()?;
    }
    #[cfg(test)]
    if let Some(test) = test {
        if let Some((site, at, failure)) = test.profile.stop_after_reserve {
            if site == _site && Some(at) == attempt {
                observer.observe_io(&failure.io_error());
            }
        }
    }
    observer.check()?;
    Ok(())
}

fn reserve_input_vec<T>(
    values: &mut Vec<T>,
    additional: usize,
    ceiling: usize,
    observer: &ZipResourceObserver,
    site: InputReserveSite,
    #[cfg(test)] test: Option<&InputCollectionsTestSession>,
) -> Result<(), StlError> {
    let prospective = values
        .len()
        .checked_add(additional)
        .filter(|&n| n <= ceiling)
        .ok_or_else(|| input_error(observer))?;
    if prospective <= values.capacity() {
        return Ok(());
    }
    let target = values
        .capacity()
        .checked_mul(2)
        .unwrap_or(ceiling)
        .max(prospective)
        .min(ceiling);
    let _bytes = target
        .checked_mul(std::mem::size_of::<T>())
        .filter(|&n| n <= isize::MAX as usize)
        .ok_or_else(|| input_error(observer))?;
    reserve_input(
        observer,
        site,
        #[cfg(test)]
        test,
        || values.try_reserve_exact(target - values.len()),
    )
}

fn reserve_input_map<K: std::hash::Hash + Eq, V>(
    values: &mut HashMap<K, V>,
    key: &K,
    observer: &ZipResourceObserver,
    site: InputReserveSite,
    #[cfg(test)] test: Option<&InputCollectionsTestSession>,
) -> Result<(), StlError> {
    if values.contains_key(key) || values.len() < values.capacity() {
        return Ok(());
    }
    values
        .len()
        .checked_add(1)
        .and_then(|len| len.checked_mul(std::mem::size_of::<(K, V)>()))
        .filter(|&bytes| bytes <= isize::MAX as usize)
        .ok_or_else(|| input_error(observer))?;
    reserve_input(
        observer,
        site,
        #[cfg(test)]
        test,
        || values.try_reserve(1),
    )
}

// Private scoped test policy captured once at Package entry, with causal record
// and reserve observations. Production always uses the fixed supported-input caps.
#[cfg(test)]
struct InputCollectionsTestProfile {
    limits: [usize; 5],
    packages: std::cell::RefCell<Vec<[usize; 5]>>,
    reserves: std::cell::RefCell<[usize; 8]>,
    fail_reserve: Option<(InputReserveSite, usize)>,
    stop_after_reserve: Option<(InputReserveSite, usize, ParserTestFailure)>,
    appended: std::cell::RefCell<[usize; 8]>,
}

#[cfg(test)]
impl InputCollectionsTestProfile {
    fn new(limits: [usize; 5]) -> Self {
        Self {
            limits,
            packages: std::cell::RefCell::new(Vec::new()),
            reserves: std::cell::RefCell::new([0; 8]),
            fail_reserve: None,
            stop_after_reserve: None,
            appended: std::cell::RefCell::new([0; 8]),
        }
    }
}

#[cfg(test)]
#[derive(Clone)]
struct InputCollectionsTestSession {
    profile: Rc<InputCollectionsTestProfile>,
    package: usize,
}

#[cfg(test)]
impl InputCollectionsTestSession {
    fn capture() -> Option<Self> {
        INPUT_COLLECTIONS_TEST_PROFILE.with(|slot| {
            let profile = slot.borrow().clone()?;
            let package = {
                let mut packages = profile.packages.borrow_mut();
                let package = packages.len();
                packages.push([0; 5]);
                package
            };
            Some(Self { profile, package })
        })
    }

    fn record(&self, kind: usize) {
        self.profile.packages.borrow_mut()[self.package][kind] += 1;
    }
    fn appended(&self, site: InputReserveSite, count: usize) {
        self.profile.appended.borrow_mut()[site as usize] += count;
    }
}

#[cfg(test)]
thread_local! {
    static INPUT_COLLECTIONS_TEST_PROFILE:
        std::cell::RefCell<Option<Rc<InputCollectionsTestProfile>>> = const {
            std::cell::RefCell::new(None)
        };
}

#[cfg(test)]
struct PartRetentionTestProfile {
    limit: usize,
    high_water: std::cell::Cell<usize>,
    name_builders: std::cell::Cell<usize>,
    id_builders: std::cell::Cell<usize>,
    appearance_copies: std::cell::Cell<usize>,
    reserves: std::cell::Cell<usize>,
    requested_capacity: std::cell::RefCell<Vec<usize>>,
    fail_reserve_at: Option<usize>,
    stop_after_reserve: Option<(usize, ParserTestFailure)>,
}

#[cfg(test)]
impl PartRetentionTestProfile {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            high_water: std::cell::Cell::new(0),
            name_builders: std::cell::Cell::new(0),
            id_builders: std::cell::Cell::new(0),
            appearance_copies: std::cell::Cell::new(0),
            reserves: std::cell::Cell::new(0),
            requested_capacity: std::cell::RefCell::new(Vec::new()),
            fail_reserve_at: None,
            stop_after_reserve: None,
        }
    }
}

#[cfg(test)]
impl PartRetentionTestProfile {
    fn record_retained(&self, len: usize) {
        self.high_water.set(self.high_water.get().max(len));
    }
}

#[cfg(test)]
thread_local! {
    static PART_RETENTION_TEST_PROFILE: std::cell::RefCell<Option<Rc<PartRetentionTestProfile>>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
#[derive(Default)]
struct DerivedMapsTestProfile {
    plate_parser_entries: std::cell::Cell<usize>,
    retained_assignments: std::cell::Cell<usize>,
    assignment_high_water: std::cell::Cell<usize>,
    diagnostic_path_copies: std::cell::Cell<usize>,
    diagnostic_unique_pairs: std::cell::Cell<usize>,
    reserves: std::cell::Cell<usize>,
    fail_site: std::cell::Cell<Option<derived_maps::ReserveSite>>,
    stop_site: std::cell::Cell<Option<(derived_maps::ReserveSite, ParserTestFailure)>>,
    after_invalid: std::cell::Cell<Option<ParserTestFailure>>,
    membership_invalid: std::cell::Cell<usize>,
    invalid_tail_events: std::cell::Cell<usize>,
    invalid_stop_at: std::cell::Cell<usize>,
    fallback_entries: std::cell::Cell<usize>,
}

#[cfg(test)]
thread_local! {
    static DERIVED_MAPS_TEST_PROFILE: std::cell::RefCell<Option<Rc<DerivedMapsTestProfile>>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
#[derive(Default)]
struct MaterialOwnedTestProfile {
    normalized_names: std::cell::Cell<usize>,
    normalized_colors: std::cell::Cell<usize>,
    requests: std::cell::RefCell<Vec<(materials::ReserveSite, usize)>>,
    fail_site: std::cell::Cell<Option<materials::ReserveSite>>,
    stop_site: std::cell::Cell<Option<(materials::ReserveSite, ParserTestFailure)>>,
    local_commits: std::cell::Cell<usize>,
    global_commits: std::cell::Cell<usize>,
}

#[cfg(test)]
thread_local! {
    static MATERIAL_OWNED_TEST_PROFILE: std::cell::RefCell<Option<Rc<MaterialOwnedTestProfile>>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
#[derive(Default)]
struct SettingsOwnedTestProfile {
    name_copies: std::cell::Cell<usize>,
    object_inserts: std::cell::Cell<usize>,
    part_inserts: std::cell::Cell<usize>,
    requests: std::cell::RefCell<Vec<(settings::ReserveSite, usize)>>,
    before_site: std::cell::Cell<Option<(settings::ReserveSite, ParserTestFailure)>>,
    after_site: std::cell::Cell<Option<(settings::ReserveSite, ParserTestFailure)>>,
    finish_failure: std::cell::Cell<Option<ParserTestFailure>>,
    entries: std::cell::Cell<usize>,
    ordinary_fallbacks: std::cell::Cell<usize>,
}

#[cfg(test)]
thread_local! {
    static SETTINGS_OWNED_TEST_PROFILE: std::cell::RefCell<Option<Rc<SettingsOwnedTestProfile>>> = const {
        std::cell::RefCell::new(None)
    };
}

/// Original input cursor; its private fault seam exists only in parser tests.
struct PackageInput<'a> {
    inner: Cursor<&'a [u8]>,
    #[cfg(test)]
    probe: Option<Rc<ParserTestProbe>>,
}

impl Read for PackageInput<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        #[cfg(test)]
        if let Some(probe) = &self.probe {
            probe.before_io(true)?;
        }
        self.inner.read(output)
    }
}

impl Seek for PackageInput<'_> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        #[cfg(test)]
        if let Some(probe) = &self.probe {
            probe.before_io(false)?;
        }
        self.inner.seek(from)
    }
}

/// The open package plus a cache of parsed parts, so a part referenced by
/// several components is parsed once.
struct Package<'a, 'context> {
    archive: BoundedZipArchive<PackageInput<'a>>,
    observer: ZipResourceObserver,
    #[cfg(test)]
    probe: Option<Rc<ParserTestProbe>>,
    #[cfg(test)]
    input_test: Option<InputCollectionsTestSession>,
    #[cfg(test)]
    part_retention_test: Option<Rc<PartRetentionTestProfile>>,
    #[cfg(test)]
    derived_maps_test: Option<Rc<DerivedMapsTestProfile>>,
    #[cfg(test)]
    material_owned_test: Option<Rc<MaterialOwnedTestProfile>>,
    #[cfg(test)]
    settings_owned_test: Option<Rc<SettingsOwnedTestProfile>>,
    collections: InputCollections,
    parts: HashMap<String, Rc<ModelPart>>,
    materials: Vec<ParsedMaterial>,
    context: &'context mut PackageProcessingContext,
    component_visits_remaining: u64,
}

impl<'a, 'context> Package<'a, 'context> {
    fn open(
        bytes: &'a [u8],
        context: &'context mut PackageProcessingContext,
    ) -> Result<Self, StlError> {
        let observer = context.observer();
        #[cfg(test)]
        let probe = PARSER_TEST_PROBE.with(|probe| probe.borrow().clone());
        let input = PackageInput {
            inner: Cursor::new(bytes),
            #[cfg(test)]
            probe: probe.clone(),
        };
        let archive = observer.open_bounded_zip(input)?;
        #[cfg(test)]
        if let Some(probe) = &probe {
            probe.state.borrow_mut().constructor_completed = true;
        }
        #[cfg(test)]
        let input_test = InputCollectionsTestSession::capture();
        let collections = InputCollections::new(
            observer.clone(),
            #[cfg(test)]
            input_test.clone(),
        );
        Ok(Package {
            archive,
            observer,
            #[cfg(test)]
            probe,
            #[cfg(test)]
            input_test,
            #[cfg(test)]
            part_retention_test: PART_RETENTION_TEST_PROFILE.with(|slot| slot.borrow().clone()),
            #[cfg(test)]
            derived_maps_test: DERIVED_MAPS_TEST_PROFILE.with(|slot| slot.borrow().clone()),
            #[cfg(test)]
            material_owned_test: MATERIAL_OWNED_TEST_PROFILE.with(|slot| slot.borrow().clone()),
            #[cfg(test)]
            settings_owned_test: SETTINGS_OWNED_TEST_PROFILE.with(|slot| slot.borrow().clone()),
            collections,
            parts: HashMap::new(),
            materials: Vec::new(),
            context,
            component_visits_remaining: MAX_COMPONENT_VISITS,
        })
    }

    fn charge_component_work(&mut self) -> Result<(), StlError> {
        self.component_visits_remaining = self
            .component_visits_remaining
            .checked_sub(1)
            .ok_or(ResourceStop::InputLimitExceeded)?;
        Ok(())
    }

    /// Zip entry names never carry the leading slash that OPC part URIs do.
    fn entry_name(path: &str) -> &str {
        path.trim_start_matches('/')
    }

    fn has(&self, path: &str) -> bool {
        let name = Self::entry_name(path);
        self.archive.file_names().any(|n| n == name)
    }

    fn collect_text(&mut self, path: &str, role: TextRole) -> Result<String, StlError> {
        let name = Self::entry_name(path).to_string();
        #[cfg(test)]
        if let Some(probe) = &self.probe {
            if probe.member == name {
                probe.state.borrow_mut().armed = true;
            }
        }
        let file = self
            .archive
            .by_name(&name)
            .map_err(|error| missing_part_error(error, path))?;
        if file.size() > role.byte_limit() as u64 {
            let error = self.observer.stop_io(ResourceStop::InputLimitExceeded);
            self.observer.observe_io(&error);
            self.observer.check()?;
        }
        let ordinary_message = match role {
            TextRole::Relationships => "_rels/.rels is not valid text".to_owned(),
            TextRole::ModelSettings | TextRole::ProjectSettings => {
                format!("{path} is not valid UTF-8 text")
            }
        };
        self.context
            .collect_text(file, role)
            .map_err(|error| match error {
                error @ (StlError::ResourceStopped(_) | StlError::AllocationFailed) => error,
                _ => malformed(ordinary_message),
            })
    }

    fn part(&mut self, path: &str) -> Result<Rc<ModelPart>, StlError> {
        self.observer.check()?;
        let key = Self::entry_name(path);
        if let Some(p) = self.parts.get(key) {
            return Ok(p.clone());
        }
        let observer = self.observer.clone();
        #[cfg(test)]
        if let Some(probe) = &self.probe {
            if probe.member == key {
                probe.state.borrow_mut().armed = true;
            }
        }
        let parsed = {
            let member = self
                .archive
                .by_name(key)
                .map_err(|error| missing_part_error(error, path))?;
            if member.size() > MAX_FILE_BYTES {
                return input_stop(&observer);
            }
            let reader = BufReader::new(self.context.model_reader(member));
            parse_model_part(
                reader,
                path,
                &observer,
                &mut self.collections,
                #[cfg(test)]
                self.material_owned_test.as_deref(),
            )
        };
        observer.check()?;
        let parsed = parsed?;
        materials::copy_selected(
            &mut self.materials,
            &parsed.materials,
            &observer,
            #[cfg(test)]
            self.material_owned_test.as_deref(),
        )?;
        // Cache keys are owned only on misses; bucket growth is fallible.
        let key = key.to_string();
        reserve_input_map(
            &mut self.parts,
            &key,
            &observer,
            InputReserveSite::Parts,
            #[cfg(test)]
            self.input_test.as_ref(),
        )?;
        let rc = Rc::new(parsed);
        self.parts.insert(key, rc.clone());
        #[cfg(test)]
        if let Some(test) = &self.input_test {
            test.appended(InputReserveSite::Parts, 1);
        }
        #[cfg(test)]
        if path == ROOT_MODEL_FALLBACK {
            if let Some(probe) = &self.probe {
                probe.state.borrow_mut().root_cached = true;
            }
        }
        Ok(rc)
    }

    fn optional_text(&mut self, path: &str, role: TextRole) -> Result<Option<String>, StlError> {
        self.observer.check()?;
        if !self.has(path) {
            return Ok(None);
        }
        self.collect_text(path, role).map(Some)
    }

    fn optional_bounded_text(
        &mut self,
        path: &str,
        max_bytes: u64,
    ) -> Result<Option<String>, StlError> {
        self.observer.check()?;
        if !self.has(path) {
            return Ok(None);
        }
        let name = Self::entry_name(path).to_string();
        let size = self
            .archive
            .by_name(&name)
            .map_err(|error| missing_part_error(error, path))?
            .size();
        if size > max_bytes {
            return Ok(None);
        }
        self.optional_text(path, TextRole::ProjectSettings)
    }

    /// The root model part per `_rels/.rels`; falls back to the conventional
    /// path when the relationships part is absent or does not name one.
    fn root_model_path(&mut self) -> Result<String, StlError> {
        self.observer.check()?;
        if self.has("_rels/.rels") {
            let text = self.collect_text("_rels/.rels", TextRole::Relationships)?;
            if let Some(target) = model_relationship_target(&text, &self.observer)? {
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

#[cfg(test)]
#[derive(Debug)]
struct PlateSpec {
    slicer_id: u32,
    name: String,
}

#[cfg(test)]
#[derive(Default)]
struct OpenPlate {
    slicer_id: Option<u32>,
    name: Option<String>,
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

#[cfg(test)]
fn parse_plate_settings(text: &str) -> Result<Vec<PlateSpec>, String> {
    parse_plate_settings_observed(text, &ZipResourceObserver::new())
}

#[cfg(test)]
fn parse_plate_settings_observed(
    text: &str,
    observer: &ZipResourceObserver,
) -> Result<Vec<PlateSpec>, String> {
    let mut xml = XmlEventReader::new(text.as_bytes(), observer.clone())
        .map_err(|error| error.to_string())?;
    xml.set_trim_text(true);
    let mut plates = Vec::new();
    let mut plate: Option<OpenPlate> = None;
    let mut instance: Option<OpenInstance> = None;

    loop {
        let event = xml
            .next_event()
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
                    open.object_id
                        .ok_or_else(|| "model instance has no object_id".to_string())?;
                    open.instance_id
                        .ok_or_else(|| "model instance has no instance_id".to_string())?;
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

fn normalize_label(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    Some(value.chars().take(256).collect())
}

#[cfg(test)]
thread_local! {
    static PROJECT_AUX_NORMALIZATION_PROBE: std::cell::RefCell<Option<Rc<std::cell::Cell<usize>>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn record_project_aux_normalization() {
    // Copy the handle, releasing the TLS borrow before normalization/work.
    let probe = PROJECT_AUX_NORMALIZATION_PROBE.with(|slot| slot.borrow().clone());
    if let Some(probe) = probe {
        probe.set(probe.get() + 1);
    }
}

#[cfg(test)]
fn json_string_array(value: Option<&Value>) -> Option<Vec<Option<String>>> {
    let Value::Array(values) = value? else {
        return None;
    };
    Some(
        values
            .iter()
            .map(|value| {
                value.as_str().and_then(|value| {
                    #[cfg(test)]
                    record_project_aux_normalization();
                    normalize_label(value)
                })
            })
            .collect(),
    )
}

#[cfg(test)]
fn parse_project_filaments_reference(text: &str) -> Option<Vec<ParsedFilament>> {
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

#[cfg(test)]
fn parse_project_filaments(text: &str) -> Option<Vec<ParsedFilament>> {
    project_json::parse(text, &ZipResourceObserver::new()).expect("bounded fixture must not stop")
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
    #[cfg(test)] probe: Option<&PartRetentionTestProfile>,
) -> String {
    #[cfg(test)]
    if let Some(probe) = probe {
        probe.name_builders.set(probe.name_builders.get() + 1);
    }
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
    pkg: &mut Package<'_, '_>,
    root: &ModelPart,
    root_path: &str,
) -> Result<HashMap<u32, HashSet<String>>, StlError> {
    derived_maps::diagnostic_paths(pkg, root, root_path)
}

fn component_part_metadata_ambiguous(
    component_paths: &HashMap<u32, HashSet<String>>,
    object_id: u32,
) -> bool {
    component_paths
        .get(&object_id)
        .is_some_and(|paths| paths.len() > 1)
}

#[derive(Debug)]
enum PlateResolutionError {
    Metadata(String),
    Source(StlError),
}

impl From<String> for PlateResolutionError {
    fn from(detail: String) -> Self {
        Self::Metadata(detail)
    }
}

impl From<&str> for PlateResolutionError {
    fn from(detail: &str) -> Self {
        Self::Metadata(detail.to_owned())
    }
}

impl std::fmt::Display for PlateResolutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Metadata(detail) => formatter.write_str(detail),
            Self::Source(error) => std::fmt::Display::fmt(error, formatter),
        }
    }
}

/// Membership is required only for build items that contribute printed triangles.
fn object_has_geometry(
    pkg: &mut Package,
    path: &str,
    id: u32,
    depth: u32,
) -> Result<bool, StlError> {
    pkg.charge_component_work()?;
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
fn model_relationship_target(
    rels: &str,
    observer: &ZipResourceObserver,
) -> Result<Option<String>, StlError> {
    let mut reader = XmlEventReader::new(rels.as_bytes(), observer.clone())?;
    let target = (|| loop {
        match reader.next_event() {
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
    })();
    observer.finish(Ok(target))
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

#[cfg(test)]
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
    let mut m = [0.0; 12];
    let mut count = 0usize;
    for token in s.split_ascii_whitespace() {
        let value = token
            .parse::<f32>()
            .map_err(|_| malformed("transform has a non-numeric entry"))?;
        if count < m.len() {
            m[count] = value;
        }
        count += 1;
    }
    if count != m.len() {
        return Err(malformed("transform must have 12 entries"));
    }
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
fn parse_model_part<R: Read>(
    reader: R,
    path: &str,
    observer: &ZipResourceObserver,
    collections: &mut InputCollections,
    #[cfg(test)] material_probe: Option<&MaterialOwnedTestProfile>,
) -> Result<ModelPart, StlError> {
    let mut xml = XmlEventReader::new(reader, observer.clone())?;
    xml.set_trim_text(true);

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
        let event = xml.next_event();
        observer.check()?;
        let event = event.map_err(|e| malformed(format!("{path}: invalid XML ({e})")))?;
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
                    b"base" | b"color" => {
                        if let Some(group) = material_group.as_mut().filter(|group| {
                            matches!(
                                (e.local_name().as_ref(), group.kind),
                                (b"base", MaterialGroupKind::Base)
                                    | (b"color", MaterialGroupKind::Color)
                            )
                        }) {
                            let index = group.next_index;
                            group.next_index = group.next_index.saturating_add(1);
                            materials::retain(
                                &mut part,
                                e,
                                group.kind,
                                group.id,
                                index,
                                observer,
                                #[cfg(test)]
                                material_probe,
                            )?;
                        }
                    }
                    b"object" => {
                        collections.admit(InputKind::Object)?;
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
                            reserve_input_map(
                                &mut part.objects,
                                &id,
                                observer,
                                InputReserveSite::Objects,
                                #[cfg(test)]
                                collections.test.as_ref(),
                            )?;
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
                            #[cfg(test)]
                            if let Some(test) = &collections.test {
                                test.appended(InputReserveSite::Objects, 1);
                            }
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
                        collections.admit(InputKind::Vertex)?;
                        let v = [
                            attr_f32(e, "x", "vertex")?,
                            attr_f32(e, "y", "vertex")?,
                            attr_f32(e, "z", "vertex")?,
                        ];
                        let ceiling =
                            collections.ceiling(InputKind::Vertex, cur.vertices.len() + 1)?;
                        reserve_input_vec(
                            &mut cur.vertices,
                            1,
                            ceiling,
                            observer,
                            InputReserveSite::Vertices,
                            #[cfg(test)]
                            collections.test.as_ref(),
                        )?;
                        cur.vertices.push(v);
                        #[cfg(test)]
                        if let Some(test) = &collections.test {
                            test.appended(InputReserveSite::Vertices, 1);
                        }
                    }
                    b"triangle" => {
                        let Some(cur) = current.as_mut() else {
                            return Err(malformed("triangle outside of an object"));
                        };
                        collections.admit(InputKind::Triangle)?;
                        cur.has_triangle_properties |=
                            has_any_attr(e, &[b"pid", b"p1", b"p2", b"p3"]);
                        let triangle = [
                            attr_u32(e, "v1", "triangle")?,
                            attr_u32(e, "v2", "triangle")?,
                            attr_u32(e, "v3", "triangle")?,
                        ];
                        let ceiling =
                            collections.ceiling(InputKind::Triangle, cur.triangles.len() + 1)?;
                        reserve_input_vec(
                            &mut cur.triangles,
                            1,
                            ceiling,
                            observer,
                            InputReserveSite::Triangles,
                            #[cfg(test)]
                            collections.test.as_ref(),
                        )?;
                        cur.triangles.push(triangle);
                        #[cfg(test)]
                        if let Some(test) = &collections.test {
                            test.appended(InputReserveSite::Triangles, 1);
                        }
                    }
                    b"component" => {
                        let Some(cur) = current.as_mut() else {
                            return Err(malformed("component outside of an object"));
                        };
                        collections.admit(InputKind::Component)?;
                        let component = Component {
                            path: attr_str(e, b"path")?,
                            object_id: attr_u32(e, "objectid", "component")?,
                            transform: attr_transform(e)?,
                        };
                        let ceiling =
                            collections.ceiling(InputKind::Component, cur.components.len() + 1)?;
                        reserve_input_vec(
                            &mut cur.components,
                            1,
                            ceiling,
                            observer,
                            InputReserveSite::Components,
                            #[cfg(test)]
                            collections.test.as_ref(),
                        )?;
                        cur.components.push(component);
                        #[cfg(test)]
                        if let Some(test) = &collections.test {
                            test.appended(InputReserveSite::Components, 1);
                        }
                    }
                    b"build" => in_build = true,
                    b"item" if in_build => {
                        collections.admit(InputKind::BuildItem)?;
                        let printable = attr_str(e, b"printable")?
                            .map(|p| p.trim() != "0" && !p.trim().eq_ignore_ascii_case("false"))
                            .unwrap_or(true);
                        let item = BuildItem {
                            path: attr_str(e, b"path")?,
                            object_id: attr_u32(e, "objectid", "build item")?,
                            transform: attr_transform(e)?,
                            printable,
                        };
                        let ceiling =
                            collections.ceiling(InputKind::BuildItem, part.build.len() + 1)?;
                        reserve_input_vec(
                            &mut part.build,
                            1,
                            ceiling,
                            observer,
                            InputReserveSite::Build,
                            #[cfg(test)]
                            collections.test.as_ref(),
                        )?;
                        part.build.push(item);
                        #[cfg(test)]
                        if let Some(test) = &collections.test {
                            test.appended(InputReserveSite::Build, 1);
                        }
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
                    reserve_input_map(
                        &mut part.objects,
                        &id,
                        observer,
                        InputReserveSite::Objects,
                        #[cfg(test)]
                        collections.test.as_ref(),
                    )?;
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
                    #[cfg(test)]
                    if let Some(test) = &collections.test {
                        test.appended(InputReserveSite::Objects, 1);
                    }
                }
                b"basematerials" | b"colorgroup" => material_group = None,
                b"build" => in_build = false,
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(part)
}

/// Flattened output accumulated across the whole build.
struct Out {
    observer: ZipResourceObserver,
    #[cfg(test)]
    test: Option<InputCollectionsTestSession>,
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
        reserve_input_vec(
            &mut self.positions,
            9,
            MAX_TRIANGLES as usize * 9,
            &self.observer,
            InputReserveSite::Positions,
            #[cfg(test)]
            self.test.as_ref(),
        )?;
        reserve_input_vec(
            &mut self.normals,
            3,
            MAX_TRIANGLES as usize * 3,
            &self.observer,
            InputReserveSite::Normals,
            #[cfg(test)]
            self.test.as_ref(),
        )?;
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
        #[cfg(test)]
        if let Some(test) = &self.test {
            test.appended(InputReserveSite::Positions, 9);
            test.appended(InputReserveSite::Normals, 3);
        }
        Ok(())
    }
}

enum StandardAppearance {
    Empty,
    Uniform(String),
    NonUniform,
}

impl StandardAppearance {
    fn observe(
        &mut self,
        material_id: Option<&str>,
        #[cfg(test)] probe: Option<&PartRetentionTestProfile>,
    ) {
        if matches!(self, Self::NonUniform) {
            return;
        }
        let Some(material_id) = material_id else {
            *self = Self::NonUniform;
            return;
        };
        match self {
            Self::Empty => {
                #[cfg(test)]
                if let Some(probe) = probe {
                    probe
                        .appearance_copies
                        .set(probe.appearance_copies.get() + 1);
                }
                *self = Self::Uniform(material_id.to_string());
            }
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
    pkg: &Package<'_, '_>,
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
    pkg.charge_component_work()?;
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
                appearance.observe(
                    material_id,
                    #[cfg(test)]
                    pkg.part_retention_test.as_deref(),
                );
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

fn new_out(scale: f32, collections: &InputCollections) -> Out {
    Out {
        observer: collections.observer.clone(),
        #[cfg(test)]
        test: collections.test.clone(),
        positions: Vec::new(),
        normals: Vec::new(),
        triangle_count: 0,
        scale,
    }
}

// Optional viewer rows may be omitted without omitting any geometry work.
// The state is shared across all authored plates, and cannot revive after overflow.
enum PartRows {
    Collecting(Vec<ParsedPart>),
    Omitted,
}

struct PartOutput {
    rows: PartRows,
    limit: usize,
    observer: ZipResourceObserver,
    #[cfg(test)]
    test: Option<Rc<PartRetentionTestProfile>>,
}

impl PartOutput {
    fn new(pkg: &Package<'_, '_>) -> Self {
        #[cfg(not(test))]
        let limit = MAX_VIEWER_PARTS;
        #[cfg(test)]
        let limit = pkg
            .part_retention_test
            .as_ref()
            .map_or(MAX_VIEWER_PARTS, |test| test.limit);
        Self {
            rows: PartRows::Collecting(Vec::new()),
            limit,
            observer: pkg.observer.clone(),
            #[cfg(test)]
            test: pkg.part_retention_test.clone(),
        }
    }

    fn can_collect_next(&self) -> bool {
        matches!(&self.rows, PartRows::Collecting(parts) if parts.len() < self.limit)
    }

    fn appearance(&self) -> StandardAppearance {
        // Below capacity one in-flight Uniform ID remains a separate scratch
        // contribution. At capacity/after omission it need not be constructed.
        if self.can_collect_next() {
            StandardAppearance::Empty
        } else {
            StandardAppearance::NonUniform
        }
    }

    // Called only AFTER a candidate emitted nonzero geometry.
    fn admit_nonempty(&mut self) -> Result<bool, StlError> {
        self.observer.check()?;
        let PartRows::Collecting(parts) = &mut self.rows else {
            return Ok(false);
        };
        if parts.len() == self.limit {
            self.rows = PartRows::Omitted; // drop retained text and backing permanently
            return Ok(false);
        }
        let needed = parts
            .len()
            .checked_add(1)
            .filter(|&n| n <= self.limit)
            .ok_or_else(|| input_error(&self.observer))?;
        if needed > parts.capacity() {
            let target = parts
                .capacity()
                .checked_mul(2)
                .unwrap_or(self.limit)
                .max(needed)
                .min(self.limit);
            target
                .checked_mul(std::mem::size_of::<ParsedPart>())
                .filter(|&bytes| bytes <= isize::MAX as usize)
                .ok_or_else(|| input_error(&self.observer))?;
            self.observer.check()?;
            #[cfg(test)]
            let attempt = self.test.as_ref().map(|test| {
                let n = test.reserves.get() + 1;
                test.reserves.set(n);
                test.requested_capacity.borrow_mut().push(target);
                n
            });
            #[cfg(test)]
            if let Some(test) = &self.test {
                if test.fail_reserve_at == attempt {
                    let _ = self.observer.allocation_failed_io();
                    self.observer.check()?;
                }
            }
            if parts.try_reserve_exact(target - parts.len()).is_err() {
                let _ = self.observer.allocation_failed_io();
                self.observer.check()?;
            }
            #[cfg(test)]
            if let Some(test) = &self.test {
                if let Some((at, failure)) = test.stop_after_reserve {
                    if Some(at) == attempt {
                        self.observer.observe_io(&failure.io_error());
                    }
                }
            }
            self.observer.check()?;
        }
        Ok(true)
    }

    // Admission has already reserved a slot; this append cannot grow the Vec.
    fn push_admitted(&mut self, part: ParsedPart) {
        let PartRows::Collecting(parts) = &mut self.rows else {
            unreachable!("admitted part output");
        };
        debug_assert!(parts.len() < self.limit && parts.len() < parts.capacity());
        parts.push(part);
        #[cfg(test)]
        if let Some(test) = &self.test {
            test.record_retained(parts.len());
        }
    }

    fn finish(
        self,
        plates: &[Plate],
        total: u32,
    ) -> Result<(Vec<ParsedPart>, Option<String>), StlError> {
        self.observer.check()?;
        match self.rows {
            PartRows::Omitted => Ok((Vec::new(), Some(PART_WARNING.into()))),
            PartRows::Collecting(mut parts) => {
                if validate_parts(
                    &parts,
                    plates,
                    total,
                    #[cfg(test)]
                    self.limit,
                ) {
                    Ok((parts, None))
                } else {
                    parts.clear();
                    Ok((parts, Some(PART_WARNING.into())))
                }
            }
        }
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
    parts: &mut PartOutput,
) -> Result<(), StlError> {
    #[cfg(test)]
    if let Some(probe) = &pkg.probe {
        probe.record_work();
    }
    if !item.printable {
        return Ok(());
    }
    // Build-item emission handles a component root directly instead of calling
    // emit_object for it. Charge that lookup against the same package budget.
    pkg.charge_component_work()?;
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
            let mut appearance = parts.appearance();
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
            if triangle_count != 0 && parts.admit_nonempty()? {
                #[cfg(test)]
                if let Some(probe) = &pkg.part_retention_test {
                    probe.id_builders.set(probe.id_builders.get() + 1);
                }
                parts.push_admitted(ParsedPart {
                    id: format!("object:{}:instance:{occurrence}", item.object_id),
                    name: boundary_name(
                        object,
                        None,
                        root_metadata,
                        None,
                        item.object_id,
                        #[cfg(test)]
                        pkg.part_retention_test.as_deref(),
                    ),
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
                let child_transform = compose(&component.transform, &item.transform);
                let triangle_start = out.triangle_count;
                let mut appearance = parts.appearance();
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
                if triangle_count != 0 && parts.admit_nonempty()? {
                    let name = boundary_name(
                        object,
                        Some(child_object),
                        root_metadata,
                        part_metadata,
                        component.object_id,
                        #[cfg(test)]
                        pkg.part_retention_test.as_deref(),
                    );
                    let filament_slot =
                        setting_extruder(part_metadata).or_else(|| setting_extruder(root_metadata));
                    #[cfg(test)]
                    if let Some(probe) = &pkg.part_retention_test {
                        probe.id_builders.set(probe.id_builders.get() + 1);
                    }
                    parts.push_admitted(ParsedPart {
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

fn finish_model(out: Out) -> ParsedStl {
    ParsedStl {
        format: StlFormat::ThreeMf,
        triangle_count: out.triangle_count,
        positions: out.positions,
        stored_normals: out.normals,
    }
}

fn validate_parts(
    parts: &[ParsedPart],
    plates: &[Plate],
    total: u32,
    #[cfg(test)] limit: usize,
) -> bool {
    #[cfg(not(test))]
    let limit = MAX_VIEWER_PARTS;
    if parts.len() > limit {
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

struct BuildView<'a> {
    root_path: &'a str,
    root: &'a ModelPart,
    index: &'a BuildIndex,
}

fn whole_build(
    pkg: &mut Package,
    build: &BuildView<'_>,
    slicer: &SlicerMetadata,
    component_paths: &HashMap<u32, HashSet<String>>,
    filaments: Vec<ParsedFilament>,
    warning: String,
) -> Result<ParsedModel, StlError> {
    let root_path = build.root_path;
    let root = build.root;
    let index = build.index;
    #[cfg(test)]
    if let Some(probe) = &pkg.derived_maps_test {
        probe.fallback_entries.set(probe.fallback_entries.get() + 1);
    }
    #[cfg(test)]
    if let Some(probe) = &pkg.probe {
        probe.record_work();
    }
    let mut out = new_out(root.scale, &pkg.collections);
    let mut parts = PartOutput::new(pkg);
    let occurrences = &index.occurrences;
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
    let mut plates = Vec::new();
    derived_maps::reserve_vec(
        &mut plates,
        1,
        MAX_VIEWER_PLATES,
        &pkg.observer,
        derived_maps::ReserveSite::OutputPlates,
        #[cfg(test)]
        pkg.derived_maps_test.as_deref(),
    )?;
    plates.push(Plate {
        id: 0,
        name: "Whole build".into(),
        triangle_start: 0,
        triangle_count,
    });
    let (parts, part_warning) = parts.finish(&plates, triangle_count)?;
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
    let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
    parse_3mf_with_plates_in_context(bytes, &mut context)
}

#[doc(hidden)]
pub fn parse_3mf_with_plates_in_context(
    bytes: &[u8],
    context: &mut PackageProcessingContext,
) -> Result<ParsedModel, StlError> {
    parse_3mf_in_context_with_component_visit_limit(bytes, MAX_COMPONENT_VISITS, context)
}

#[cfg(test)]
fn parse_3mf_with_component_visit_limit(
    bytes: &[u8],
    component_visit_limit: u64,
) -> Result<ParsedModel, StlError> {
    let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
    parse_3mf_in_context_with_component_visit_limit(bytes, component_visit_limit, &mut context)
}

fn parse_3mf_in_context_with_component_visit_limit(
    bytes: &[u8],
    component_visit_limit: u64,
    context: &mut PackageProcessingContext,
) -> Result<ParsedModel, StlError> {
    let observer = context.observer();
    observer.check()?;
    let result = parse_3mf_with_context(bytes, component_visit_limit, context);
    observer.finish(result)
}

fn parse_3mf_with_context(
    bytes: &[u8],
    component_visit_limit: u64,
    context: &mut PackageProcessingContext,
) -> Result<ParsedModel, StlError> {
    if bytes.is_empty() {
        return Err(StlError::EmptyFile);
    }
    let mut pkg = Package::open(bytes, context)?;
    pkg.component_visits_remaining = component_visit_limit;
    let root_path = pkg.root_model_path()?;
    let root = pkg.part(&root_path)?;
    let component_paths = component_part_metadata_paths(&mut pkg, &root, &root_path)?;
    let build_index = BuildIndex::new(
        &root.build,
        &pkg.observer,
        #[cfg(test)]
        pkg.derived_maps_test.as_deref(),
    )?;
    let build_view = BuildView {
        root_path: &root_path,
        root: &root,
        index: &build_index,
    };

    let settings_result = pkg.optional_text(MODEL_SETTINGS_PATH, TextRole::ModelSettings);
    pkg.observer.check()?;
    preserve_terminal(&settings_result)?;
    let slicer_result = settings_result
        .as_ref()
        .ok()
        .and_then(|text| text.as_deref())
        .map(|text| {
            settings::parse_slicer_metadata(
                text,
                &pkg.observer,
                &root.objects,
                &component_paths,
                #[cfg(test)]
                pkg.settings_owned_test.as_deref(),
            )
        });
    #[cfg(test)]
    if let Some(failure) = pkg
        .settings_owned_test
        .as_ref()
        .and_then(|probe| probe.finish_failure.take())
    {
        match failure {
            ParserTestFailure::Stop(stop) => {
                let _ = pkg.observer.stop_io(stop);
            }
            ParserTestFailure::Allocation => {
                let _ = pkg.observer.allocation_failed_io();
            }
        }
    }
    pkg.observer.check()?;
    let slicer = match slicer_result {
        Some(Ok(metadata)) => metadata,
        Some(Err(settings::SettingsError::Source(error))) => return Err(error),
        Some(Err(settings::SettingsError::Metadata(_))) => {
            #[cfg(test)]
            if let Some(probe) = &pkg.settings_owned_test {
                probe
                    .ordinary_fallbacks
                    .set(probe.ordinary_fallbacks.get() + 1);
            }
            SlicerMetadata::default()
        }
        None => SlicerMetadata::default(),
    };
    let project_result =
        pkg.optional_bounded_text(PROJECT_SETTINGS_PATH, MAX_APPEARANCE_METADATA_BYTES);
    pkg.observer.check()?;
    preserve_terminal(&project_result)?;
    let filaments = match project_result {
        Ok(Some(text)) => project_json::parse(&text, &pkg.observer)?.unwrap_or_default(),
        Ok(None) | Err(_) => Vec::new(), // terminal errors were preserved above
    };

    let settings = match settings_result {
        Ok(Some(text)) => {
            let result = derived_maps::parse_and_resolve_plates(
                &text,
                &root_path,
                &root,
                &build_index,
                &mut pkg,
            );
            pkg.observer.check()?;
            match result {
                Ok(specs) => Some(specs),
                Err(PlateResolutionError::Source(
                    error @ (StlError::ResourceStopped(_) | StlError::AllocationFailed),
                )) => return Err(error),
                Err(detail) => {
                    return whole_build(
                        &mut pkg,
                        &build_view,
                        &slicer,
                        &component_paths,
                        filaments,
                        format!("Plate metadata is unusable ({detail}); showing the whole build."),
                    );
                }
            }
        }
        Ok(None) => {
            return whole_build(
                &mut pkg,
                &build_view,
                &slicer,
                &component_paths,
                filaments,
                NO_PLATE_METADATA_WARNING.into(),
            );
        }
        Err(err @ (StlError::ResourceStopped(_) | StlError::AllocationFailed)) => {
            return Err(err);
        }
        Err(err) => {
            return whole_build(
                &mut pkg,
                &build_view,
                &slicer,
                &component_paths,
                filaments,
                format!("Plate metadata is unusable ({err}); showing the whole build."),
            );
        }
    };

    let resolved = settings.expect("all other metadata branches return");

    // Transforms are expressed in the model's unit, so unit conversion is
    // applied last, to already-placed coordinates.
    let mut out = new_out(root.scale, &pkg.collections);
    let mut parts = PartOutput::new(&pkg);
    let occurrences = &build_index.occurrences;
    let mut plates = Vec::new();
    for (plate_index, spec) in resolved.into_iter().enumerate() {
        let triangle_start = out.triangle_count;
        for &item_index in &spec.items {
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
        derived_maps::reserve_vec(
            &mut plates,
            1,
            MAX_VIEWER_PLATES,
            &pkg.observer,
            derived_maps::ReserveSite::OutputPlates,
            #[cfg(test)]
            pkg.derived_maps_test.as_deref(),
        )?;
        plates.push(Plate {
            id: u32::try_from(plate_index).map_err(|_| malformed("too many plates in metadata"))?,
            name: spec.name,
            triangle_start,
            triangle_count: out.triangle_count - triangle_start,
        });
    }

    let (parts, part_warning) = parts.finish(&plates, out.triangle_count)?;
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

    const PARSER_TEST_FAILURES: [ParserTestFailure; 4] = [
        ParserTestFailure::Stop(ResourceStop::InputLimitExceeded),
        ParserTestFailure::Stop(ResourceStop::ReadWorkExceeded),
        ParserTestFailure::Stop(ResourceStop::EnvelopeChanged),
        ParserTestFailure::Allocation,
    ];

    struct ParserProbeGuard(Option<Rc<ParserTestProbe>>);

    impl ParserProbeGuard {
        fn install(probe: Rc<ParserTestProbe>) -> Self {
            Self(PARSER_TEST_PROBE.with(|slot| slot.replace(Some(probe))))
        }
    }

    impl Drop for ParserProbeGuard {
        fn drop(&mut self) {
            drop(PARSER_TEST_PROBE.with(|slot| slot.replace(self.0.take())));
        }
    }

    fn assert_public_parser_stop(
        bytes: &[u8],
        member: &'static str,
        failure: ParserTestFailure,
        aggregate: bool,
        root_cached: bool,
    ) {
        let probe = Rc::new(ParserTestProbe {
            member,
            failure,
            state: std::cell::RefCell::new(ParserTestState::default()),
            guard_observer: None,
        });
        let _guard = ParserProbeGuard::install(probe.clone());
        let result = if aggregate {
            parse_3mf(bytes).map(|_| ())
        } else {
            parse_3mf_with_plates(bytes).map(|_| ())
        };
        assert_eq!(result, Err(failure.error()));
        let state = probe.state.borrow();
        assert!(state.constructor_completed);
        assert!(
            state.fired,
            "the selected member payload fault must be reached"
        );
        assert_eq!(state.root_cached, root_cached);
        assert_eq!(
            state.after_io, 0,
            "no underlying IO after the observed stop"
        );
        assert_eq!(
            state.work_after_stop, 0,
            "no fallback or emission after stop"
        );
    }

    #[test]
    fn typed_zip_parser_last_settings_stop_never_enters_cached_whole_build() {
        let root = model_xml("millimeter", &cube_mesh_xml(1), r#"<item objectid="1"/>"#);
        for deflate in [false, true] {
            let bytes = package(
                &[
                    (ROOT_MODEL_FALLBACK, &root),
                    (MODEL_SETTINGS_PATH, "<config/>"),
                ],
                deflate,
            );
            // Without the fault, the empty plate configuration falls back to
            // the already parsed root and succeeds with all twelve triangles.
            assert_eq!(
                parse_3mf_with_plates(&bytes).unwrap().model.triangle_count,
                12
            );
            for failure in PARSER_TEST_FAILURES {
                for aggregate in [false, true] {
                    assert_public_parser_stop(
                        &bytes,
                        MODEL_SETTINGS_PATH,
                        failure,
                        aggregate,
                        true,
                    );
                }
            }
        }
    }

    #[test]
    fn typed_zip_parser_last_project_stop_never_becomes_empty_metadata() {
        let root = model_xml("millimeter", &cube_mesh_xml(1), r#"<item objectid="1"/>"#);
        for deflate in [false, true] {
            let bytes = package(
                &[(ROOT_MODEL_FALLBACK, &root), (PROJECT_SETTINGS_PATH, "{}")],
                deflate,
            );
            let normal = parse_3mf_with_plates(&bytes).unwrap();
            assert_eq!(normal.model.triangle_count, 12);
            assert_eq!(
                normal.plate_warning.as_deref(),
                Some(NO_PLATE_METADATA_WARNING)
            );
            for failure in PARSER_TEST_FAILURES {
                for aggregate in [false, true] {
                    assert_public_parser_stop(
                        &bytes,
                        PROJECT_SETTINGS_PATH,
                        failure,
                        aggregate,
                        true,
                    );
                }
            }
        }
    }

    #[test]
    fn typed_zip_parser_preserves_stops_erased_by_xml_error_context() {
        for deflate in [false, true] {
            let bytes = simple_cube("millimeter", deflate);
            for failure in PARSER_TEST_FAILURES {
                assert_public_parser_stop(&bytes, ROOT_MODEL_FALLBACK, failure, false, false);
            }
        }
    }

    #[test]
    fn typed_zip_parser_plate_resolution_stop_never_enters_fallback() {
        const CHILD: &str = "3D/Objects/child.model";
        let root = model_xml(
            "millimeter",
            r#"<object id="1" type="model"><components><component objectid="2" p:path="/3D/Objects/child.model"/></components></object>"#,
            r#"<item objectid="1"/>"#,
        );
        let child = model_xml("millimeter", &cube_mesh_xml(2), "");
        let settings = plate_settings(&plate_xml(1, "Unassigned", &[]));
        for deflate in [false, true] {
            let bytes = package(
                &[
                    (ROOT_MODEL_FALLBACK, &root),
                    (CHILD, &child),
                    (MODEL_SETTINGS_PATH, &settings),
                ],
                deflate,
            );
            let normal = parse_3mf_with_plates(&bytes).unwrap();
            assert_eq!(normal.model.triangle_count, 12);
            assert!(!normal.plate_metadata);
            for failure in PARSER_TEST_FAILURES {
                assert_public_parser_stop(&bytes, CHILD, failure, false, true);
            }
        }
    }

    #[test]
    fn typed_zip_parser_constructor_limit_is_not_formatted_as_malformed() {
        let mut bytes = simple_cube("millimeter", false);
        let footer = bytes
            .windows(4)
            .rposition(|value| value == b"PK\x05\x06")
            .unwrap();
        bytes[footer + 8..footer + 10].copy_from_slice(&10_001u16.to_le_bytes());
        bytes[footer + 10..footer + 12].copy_from_slice(&10_001u16.to_le_bytes());
        assert!(matches!(
            parse_3mf_with_plates(&bytes),
            Err(StlError::ResourceStopped(ResourceStop::InputLimitExceeded))
        ));
    }

    #[test]
    fn typed_zip_parser_keeps_ordinary_optional_metadata_fallback() {
        let root = model_xml("millimeter", &cube_mesh_xml(1), r#"<item objectid="1"/>"#);
        for deflate in [false, true] {
            let bytes = package(
                &[
                    (ROOT_MODEL_FALLBACK, &root),
                    (MODEL_SETTINGS_PATH, "<config><plate>"),
                    (PROJECT_SETTINGS_PATH, "{"),
                ],
                deflate,
            );
            let parsed = parse_3mf_with_plates(&bytes).unwrap();
            assert_eq!(parsed.model.triangle_count, 12);
            assert!(!parsed.plate_metadata);
            assert!(parsed
                .plate_warning
                .unwrap()
                .starts_with("Plate metadata is unusable ("));
            assert!(parsed.embedded_filaments.is_empty());
        }
    }

    const RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
 <Relationship Target="/3D/3dmodel.model" Id="rel-1" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
 <Relationship Target="/Metadata/thumb.png" Id="rel-2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail"/>
</Relationships>"#;

    struct InputCollectionsProfileGuard(Option<Rc<InputCollectionsTestProfile>>);

    impl InputCollectionsProfileGuard {
        fn install(profile: Rc<InputCollectionsTestProfile>) -> Self {
            Self(INPUT_COLLECTIONS_TEST_PROFILE.with(|slot| slot.replace(Some(profile))))
        }
    }

    impl Drop for InputCollectionsProfileGuard {
        fn drop(&mut self) {
            INPUT_COLLECTIONS_TEST_PROFILE.with(|slot| slot.replace(self.0.take()));
        }
    }

    fn input_collection_fixture(kind: usize, count: usize) -> Vec<u8> {
        let (resources, build) = match kind {
            0 => (format!("<object id=\"1\" type=\"support\"><mesh><vertices>{}</vertices></mesh></object>",
                "<vertex x=\"0\" y=\"0\" z=\"0\"/>".repeat(count)), String::new()),
            1 => (format!("<object id=\"1\" type=\"support\"><mesh><vertices><vertex x=\"0\" y=\"0\" z=\"0\"/></vertices><triangles>{}</triangles></mesh></object>",
                "<triangle v1=\"0\" v2=\"0\" v3=\"0\"/>".repeat(count)), String::new()),
            2 => ((0..count).map(|id| format!("<object id=\"{}\"/>", id + 1)).collect::<String>(), String::new()),
            3 => (format!("<object id=\"2\"/><object id=\"1\"><components>{}</components></object>",
                "<component objectid=\"2\"/>".repeat(count)), String::new()),
            4 => ("<object id=\"1\"/>".into(), "<item objectid=\"1\" printable=\"0\"/>".repeat(count)),
            _ => unreachable!(),
        };
        let model = model_xml("millimeter", &resources, &build);
        package(&[(ROOT_MODEL_FALLBACK, &model)], false)
    }

    fn assert_input_collection_boundary(kind: usize) {
        let mut limits = [100; 5];
        limits[kind] = 2;
        let profile = Rc::new(InputCollectionsTestProfile::new(limits));
        let _guard = InputCollectionsProfileGuard::install(profile.clone());
        let exact = parse_3mf(&input_collection_fixture(kind, 2)).unwrap();
        assert_eq!(exact.triangle_count, 0, "records are unused or disabled");
        assert_eq!(profile.packages.borrow()[0][kind], profile.limits[kind]);
        let exact_reserves = *profile.reserves.borrow();
        let exact_appended = *profile.appended.borrow();
        let excess = parse_3mf(&input_collection_fixture(kind, 3));
        assert_eq!(profile.packages.borrow().len(), 2, "fresh Package snapshot");
        assert_eq!(
            profile.packages.borrow()[1][kind],
            3,
            "physical branch reached"
        );
        assert!(
            matches!(
                excess,
                Err(StlError::ResourceStopped(ResourceStop::InputLimitExceeded))
            ),
            "physical input category {kind} must stop at its captured limit; got {excess:?}"
        );
        let site = [
            InputReserveSite::Vertices,
            InputReserveSite::Triangles,
            InputReserveSite::Objects,
            InputReserveSite::Components,
            InputReserveSite::Build,
        ][kind] as usize;
        assert_eq!(
            profile.reserves.borrow()[site] - exact_reserves[site],
            exact_reserves[site],
            "rejected third record must not start another category reserve"
        );
        assert_eq!(
            profile.appended.borrow()[site] - exact_appended[site],
            exact_appended[site],
            "rejected third record must not append"
        );
        assert_eq!(
            profile.appended.borrow()[InputReserveSite::Parts as usize],
            exact_appended[InputReserveSite::Parts as usize],
            "failed parse must not commit its part"
        );
        assert_eq!(
            profile.reserves.borrow()[InputReserveSite::Parts as usize],
            exact_reserves[InputReserveSite::Parts as usize],
            "failed parse must not reserve a cache slot"
        );
    }

    #[test]
    fn input_collections_vertices_exact_and_one_over() {
        assert_input_collection_boundary(0);
    }
    #[test]
    fn input_collections_triangles_exact_and_one_over() {
        assert_input_collection_boundary(1);
    }
    #[test]
    fn input_collections_objects_exact_and_one_over() {
        assert_input_collection_boundary(2);
    }
    #[test]
    fn input_collections_components_exact_and_one_over() {
        assert_input_collection_boundary(3);
    }
    #[test]
    fn input_collections_build_items_exact_and_one_over() {
        assert_input_collection_boundary(4);
    }

    fn assert_input_failure(result: Result<ParsedStl, StlError>, failure: ParserTestFailure) {
        match (result, failure) {
            (Err(StlError::ResourceStopped(actual)), ParserTestFailure::Stop(expected)) => {
                assert_eq!(actual, expected)
            }
            (Err(StlError::AllocationFailed), ParserTestFailure::Allocation) => {}
            (actual, _) => panic!("wrong terminal result: {actual:?}"),
        }
    }

    fn input_fault_fixture(site: InputReserveSite) -> Vec<u8> {
        match site {
            InputReserveSite::Vertices => input_collection_fixture(0, 3),
            InputReserveSite::Triangles => input_collection_fixture(1, 3),
            InputReserveSite::Components => input_collection_fixture(3, 3),
            InputReserveSite::Build => input_collection_fixture(4, 3),
            InputReserveSite::Objects | InputReserveSite::Parts => input_collection_fixture(2, 4),
            InputReserveSite::Positions | InputReserveSite::Normals => {
                simple_cube("millimeter", false)
            }
        }
    }

    const INPUT_RESERVE_SITES: [InputReserveSite; 8] = [
        InputReserveSite::Vertices,
        InputReserveSite::Triangles,
        InputReserveSite::Components,
        InputReserveSite::Build,
        InputReserveSite::Objects,
        InputReserveSite::Parts,
        InputReserveSite::Positions,
        InputReserveSite::Normals,
    ];

    #[test]
    fn input_collections_all_reserve_sites_fail_before_commit() {
        for site in INPUT_RESERVE_SITES {
            let mut profile = InputCollectionsTestProfile::new([100; 5]);
            profile.fail_reserve = Some((site, 1));
            let profile = Rc::new(profile);
            let _guard = InputCollectionsProfileGuard::install(profile.clone());
            assert_input_failure(
                parse_3mf(&input_fault_fixture(site)),
                ParserTestFailure::Allocation,
            );
            assert_eq!(profile.reserves.borrow()[site as usize], 1, "{site:?}");
            assert_eq!(profile.appended.borrow()[site as usize], 0, "{site:?}");
            if matches!(site, InputReserveSite::Normals) {
                assert_eq!(
                    profile.appended.borrow()[InputReserveSite::Positions as usize],
                    0
                );
            }
            if !matches!(
                site,
                InputReserveSite::Positions | InputReserveSite::Normals
            ) {
                assert_eq!(
                    profile.appended.borrow()[InputReserveSite::Parts as usize],
                    0
                );
            }
        }
    }

    #[test]
    fn input_collections_post_reserve_first_cause_stops_before_append() {
        for site in INPUT_RESERVE_SITES {
            for failure in PARSER_TEST_FAILURES {
                let mut profile = InputCollectionsTestProfile::new([100; 5]);
                profile.stop_after_reserve = Some((site, 1, failure));
                let profile = Rc::new(profile);
                let _guard = InputCollectionsProfileGuard::install(profile.clone());
                assert_input_failure(parse_3mf(&input_fault_fixture(site)), failure);
                assert_eq!(profile.reserves.borrow()[site as usize], 1);
                assert_eq!(profile.appended.borrow()[site as usize], 0);
                if site == InputReserveSite::Positions {
                    assert_eq!(
                        profile.reserves.borrow()[InputReserveSite::Normals as usize],
                        0
                    );
                }
                if site == InputReserveSite::Normals {
                    assert_eq!(
                        profile.appended.borrow()[InputReserveSite::Positions as usize],
                        0
                    );
                }
            }
        }
    }

    #[test]
    fn input_collections_second_source_growth_keeps_only_previous_records() {
        for (site, previous) in [
            (InputReserveSite::Vertices, 1),
            (InputReserveSite::Triangles, 1),
            (InputReserveSite::Components, 1),
            (InputReserveSite::Build, 1),
        ] {
            let mut profile = InputCollectionsTestProfile::new([100; 5]);
            profile.fail_reserve = Some((site, 2));
            let profile = Rc::new(profile);
            let _guard = InputCollectionsProfileGuard::install(profile.clone());
            assert_input_failure(
                parse_3mf(&input_fault_fixture(site)),
                ParserTestFailure::Allocation,
            );
            assert_eq!(profile.reserves.borrow()[site as usize], 2);
            assert_eq!(profile.appended.borrow()[site as usize], previous);
            assert_eq!(
                profile.appended.borrow()[InputReserveSite::Parts as usize],
                0
            );
        }
    }

    #[test]
    fn input_collections_cross_part_cache_and_fresh_package_share_read_context_only() {
        let root = model_xml(
            "millimeter",
            r#"<object id="1"><components><component objectid="2" path="/3D/child.model"/><component objectid="2" path="3D/child.model"/></components></object>"#,
            r#"<item objectid="1"/>"#,
        );
        let child = model_xml("millimeter", &cube_mesh_xml(2), "");
        let bytes = package(
            &[(ROOT_MODEL_FALLBACK, &root), ("3D/child.model", &child)],
            true,
        );
        let profile = Rc::new(InputCollectionsTestProfile::new([8, 12, 2, 2, 1]));
        let _guard = InputCollectionsProfileGuard::install(profile.clone());
        let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
        let first = parse_3mf_with_plates_in_context(&bytes, &mut context).unwrap();
        let first_usage = context.usage();
        assert_eq!(first.model.triangle_count, 24);
        let second = parse_3mf_with_plates_in_context(&bytes, &mut context).unwrap();
        assert_eq!(second.model.triangle_count, 24);
        assert_eq!(
            context.usage().inflation_bytes,
            first_usage.inflation_bytes * 2
        );
        assert_eq!(*profile.packages.borrow(), vec![[8, 12, 2, 2, 1]; 2]);
        assert_eq!(
            profile.appended.borrow()[InputReserveSite::Parts as usize],
            4
        );
    }

    #[test]
    fn input_collections_distinct_parts_cumulate_before_geometry() {
        let root = model_xml(
            "millimeter",
            r#"<object id="1"><mesh><vertices><vertex x="0" y="0" z="0"/></vertices></mesh><components><component objectid="2" path="/3D/child.model"/></components></object>"#,
            r#"<item objectid="1"/>"#,
        );
        let child = model_xml("millimeter", &cube_mesh_xml(2), "");
        let bytes = package(
            &[(ROOT_MODEL_FALLBACK, &root), ("3D/child.model", &child)],
            false,
        );
        let profile = Rc::new(InputCollectionsTestProfile::new([8, 100, 100, 100, 100]));
        let _guard = InputCollectionsProfileGuard::install(profile.clone());
        assert_input_failure(
            parse_3mf(&bytes),
            ParserTestFailure::Stop(ResourceStop::InputLimitExceeded),
        );
        assert_eq!(profile.packages.borrow()[0][0], 9);
        assert_eq!(
            profile.appended.borrow()[InputReserveSite::Positions as usize],
            0
        );
        assert_eq!(
            profile.appended.borrow()[InputReserveSite::Parts as usize],
            1
        );
    }

    #[test]
    fn input_collections_discarded_mesh_duplicates_and_disabled_records_still_count() {
        let xml = model_xml(
            "millimeter",
            r#"<object id="1"><mesh><vertices><vertex x="NaN" y="0" z="0"/></vertices><triangles><triangle v1="99" v2="99" v3="99"/></triangles></mesh><components><component objectid="2"/></components></object><object id="1"/><object id="2"/>"#,
            r#"<item objectid="1" printable="0" path="missing.model"/>"#,
        );
        let bytes = package(&[(ROOT_MODEL_FALLBACK, &xml)], false);
        let profile = Rc::new(InputCollectionsTestProfile::new([1, 1, 3, 1, 1]));
        let _guard = InputCollectionsProfileGuard::install(profile.clone());
        assert_eq!(parse_3mf(&bytes).unwrap().triangle_count, 0);
        assert_eq!(profile.packages.borrow()[0], [1, 1, 3, 1, 1]);
        assert_eq!(
            profile.appended.borrow()[InputReserveSite::Parts as usize],
            1
        );
    }

    #[test]
    fn input_collections_nested_open_object_overwrite_does_not_refund_physical_records() {
        let xml = model_xml(
            "millimeter",
            r#"<object id="1"><mesh><vertices><vertex x="0" y="0" z="0"/></vertices></mesh><object id="2"><mesh><vertices><vertex x="1" y="0" z="0"/></vertices></mesh></object></object>"#,
            "",
        );
        let bytes = package(&[(ROOT_MODEL_FALLBACK, &xml)], false);
        let profile = Rc::new(InputCollectionsTestProfile::new([2, 0, 2, 0, 0]));
        {
            let _guard = InputCollectionsProfileGuard::install(profile.clone());
            // Existing permissive opening behavior overwrites OpenObject; the
            // final outer close still reports its existing unbalanced error.
            let result = parse_3mf(&bytes);
            assert!(
                matches!(result, Err(StlError::Malformed3mf { ref detail }) if detail == "unbalanced </object>")
            );
            assert_eq!(profile.packages.borrow()[0], [2, 0, 2, 0, 0]);
            assert_eq!(
                profile.appended.borrow()[InputReserveSite::Vertices as usize],
                2
            );
            assert_eq!(
                profile.appended.borrow()[InputReserveSite::Objects as usize],
                1
            );
            assert_eq!(
                profile.appended.borrow()[InputReserveSite::Parts as usize],
                0
            );
        }
        let profile = Rc::new(InputCollectionsTestProfile::new([2, 0, 1, 0, 0]));
        let _guard = InputCollectionsProfileGuard::install(profile.clone());
        assert_input_failure(
            parse_3mf(&bytes),
            ParserTestFailure::Stop(ResourceStop::InputLimitExceeded),
        );
        assert_eq!(profile.packages.borrow()[0], [1, 0, 2, 0, 0]);
        assert_eq!(
            profile.appended.borrow()[InputReserveSite::Vertices as usize],
            1
        );
        assert_eq!(
            profile.appended.borrow()[InputReserveSite::Objects as usize],
            0
        );
    }

    #[test]
    fn input_collections_overcap_attributes_stop_but_structural_errors_keep_priority() {
        for (kind, resources, build) in [
            (0, "<object id=\"1\"><mesh><vertices><vertex x=\"bad\"/></vertices></mesh></object>", ""),
            (1, "<object id=\"1\"><mesh><triangles><triangle v1=\"bad\"/></triangles></mesh></object>", ""),
            (2, "<object id=\"bad\"/>", ""),
            (3, "<object id=\"1\"><components><component objectid=\"bad\"/></components></object>", ""),
            (4, "<object id=\"1\"/>", "<item objectid=\"bad\"/>"),
        ] {
            let xml = model_xml("millimeter", resources, build);
            let bytes = package(&[(ROOT_MODEL_FALLBACK, &xml)], false);
            assert!(matches!(parse_3mf(&bytes), Err(StlError::Malformed3mf { .. })));
            let mut limits = [100; 5]; limits[kind] = 0;
            let profile = Rc::new(InputCollectionsTestProfile::new(limits));
            let _guard = InputCollectionsProfileGuard::install(profile.clone());
            assert_input_failure(parse_3mf(&bytes), ParserTestFailure::Stop(ResourceStop::InputLimitExceeded));
            assert_eq!(profile.packages.borrow()[0][kind], 1);
        }
        for record in ["<vertex/>", "<triangle/>", "<component/>"] {
            let xml = model_xml("millimeter", record, "");
            let bytes = package(&[(ROOT_MODEL_FALLBACK, &xml)], false);
            let profile = Rc::new(InputCollectionsTestProfile::new([0; 5]));
            let _guard = InputCollectionsProfileGuard::install(profile.clone());
            assert!(matches!(
                parse_3mf(&bytes),
                Err(StlError::Malformed3mf { .. })
            ));
            assert_eq!(profile.packages.borrow()[0], [0; 5]);
        }
    }

    #[test]
    fn input_collections_second_map_growth_is_fallible_without_committing_pending_key() {
        for site in [InputReserveSite::Objects, InputReserveSite::Parts] {
            // Discover the current standard library layout using the actual map
            // types. Bucket capacity is not part of the supported-input policy.
            let initial_capacity = if site == InputReserveSite::Objects {
                let mut map = HashMap::<u32, Object>::new();
                map.try_reserve(1).unwrap();
                map.capacity()
            } else {
                let mut map = HashMap::<String, Rc<ModelPart>>::new();
                map.try_reserve(1).unwrap();
                map.capacity()
            };
            assert!(
                (1..=32).contains(&initial_capacity),
                "keep this a tiny fixture"
            );
            let mut profile = InputCollectionsTestProfile::new([100; 5]);
            profile.fail_reserve = Some((site, 2));
            let profile = Rc::new(profile);
            let _guard = InputCollectionsProfileGuard::install(profile.clone());
            if site == InputReserveSite::Objects {
                assert_input_failure(
                    parse_3mf(&input_collection_fixture(2, initial_capacity + 1)),
                    ParserTestFailure::Allocation,
                );
                assert_eq!(profile.appended.borrow()[site as usize], initial_capacity);
                assert_eq!(
                    profile.appended.borrow()[InputReserveSite::Parts as usize],
                    0
                );
            } else {
                let model = model_xml("millimeter", "<object id=\"1\"/>", "");
                let names = (0..=initial_capacity)
                    .map(|index| format!("part-{index}.model"))
                    .collect::<Vec<_>>();
                let entries = names
                    .iter()
                    .map(|name| (name.as_str(), model.as_str()))
                    .collect::<Vec<_>>();
                let bytes = package(&entries, false);
                let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
                let mut pkg = Package::open(&bytes, &mut context).unwrap();
                for path in &names[..initial_capacity] {
                    pkg.part(path).unwrap();
                }
                let pending = &names[initial_capacity];
                assert!(matches!(pkg.part(pending), Err(StlError::AllocationFailed)));
                assert_eq!(pkg.parts.len(), initial_capacity);
                assert!(!pkg.parts.contains_key(pending));
            }
            assert_eq!(profile.reserves.borrow()[site as usize], 2);
        }
    }

    #[test]
    fn input_collections_output_pair_never_partially_appends_on_first_or_second_reserve() {
        for site in [InputReserveSite::Positions, InputReserveSite::Normals] {
            for failure in PARSER_TEST_FAILURES {
                let mut profile = InputCollectionsTestProfile::new([100; 5]);
                profile.stop_after_reserve = Some((site, 1, failure));
                let profile = Rc::new(profile);
                let _guard = InputCollectionsProfileGuard::install(profile.clone());
                let collections = InputCollections::new(
                    ZipResourceObserver::new(),
                    InputCollectionsTestSession::capture(),
                );
                let mut out = new_out(1.0, &collections);
                let result = out.push_triangle([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
                assert!(out.positions.is_empty());
                assert!(out.normals.is_empty());
                assert_eq!(out.triangle_count, 0);
                assert_input_failure(result.map(|()| finish_model(out)), failure);
                // Probe lengths reflect the actual append sites, not capacity changes.
                assert_eq!(
                    profile.appended.borrow()[InputReserveSite::Positions as usize],
                    0
                );
                assert_eq!(
                    profile.appended.borrow()[InputReserveSite::Normals as usize],
                    0
                );
            }
        }
    }

    #[test]
    fn input_collections_prior_stop_starts_no_package_or_reserve_work() {
        for failure in PARSER_TEST_FAILURES {
            let observer = ZipResourceObserver::new();
            observer.observe_io(&failure.io_error());
            let profile = Rc::new(InputCollectionsTestProfile::new([100; 5]));
            let _guard = InputCollectionsProfileGuard::install(profile.clone());
            let mut context = PackageProcessingContext::new(observer);
            assert_input_failure(
                parse_3mf_with_plates_in_context(&simple_cube("millimeter", false), &mut context)
                    .map(|model| model.model),
                failure,
            );
            assert_eq!(*profile.reserves.borrow(), [0; 8]);
            assert!(profile.packages.borrow().is_empty());
        }
    }

    #[test]
    fn input_collections_failed_part_keeps_admissions_and_cache_hits_charge_nothing() {
        let invalid = model_xml(
            "millimeter",
            "<object id=\"1\"><mesh><vertices><vertex x=\"bad\"/></vertices></mesh></object>",
            "",
        );
        let valid = model_xml("millimeter", "<object id=\"1\"><mesh><vertices><vertex x=\"0\" y=\"0\" z=\"0\"/></vertices></mesh></object>", "");
        let bytes = package(&[("bad.model", &invalid), ("good.model", &valid)], false);
        let profile = Rc::new(InputCollectionsTestProfile::new([1, 10, 10, 10, 10]));
        let _guard = InputCollectionsProfileGuard::install(profile.clone());
        let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
        let mut pkg = Package::open(&bytes, &mut context).unwrap();
        assert!(matches!(
            pkg.part("bad.model"),
            Err(StlError::Malformed3mf { .. })
        ));
        assert_eq!(pkg.collections.counts[0], 1);
        assert!(pkg.parts.is_empty());
        assert!(matches!(
            pkg.part("good.model"),
            Err(StlError::ResourceStopped(ResourceStop::InputLimitExceeded))
        ));
        assert!(pkg.parts.is_empty());
        assert_eq!(profile.packages.borrow()[0][0], 2);

        let profile = Rc::new(InputCollectionsTestProfile::new([1, 10, 10, 10, 10]));
        let _guard = InputCollectionsProfileGuard::install(profile.clone());
        let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
        let mut pkg = Package::open(&bytes, &mut context).unwrap();
        let first = pkg.part("good.model").unwrap();
        let usage = pkg.context.usage();
        let second = pkg.part("/good.model").unwrap();
        assert!(Rc::ptr_eq(&first, &second));
        assert_eq!(pkg.context.usage().inflation_bytes, usage.inflation_bytes);
        assert_eq!(profile.packages.borrow()[0][0], 1);
    }

    #[test]
    fn input_collections_local_growth_ceiling_includes_pending_and_remaining() {
        let observer = ZipResourceObserver::new();
        let mut collections = InputCollections::new(observer.clone(), None);
        collections.limits[InputKind::Vertex as usize] = 3;
        let mut vertices = Vec::<[f32; 3]>::new();
        for _ in 0..3 {
            collections.admit(InputKind::Vertex).unwrap();
            let ceiling = collections
                .ceiling(InputKind::Vertex, vertices.len() + 1)
                .unwrap();
            assert_eq!(ceiling, 3);
            reserve_input_vec(
                &mut vertices,
                1,
                ceiling,
                &observer,
                InputReserveSite::Vertices,
                None,
            )
            .unwrap();
            vertices.push([0.0; 3]);
        }
        assert_eq!(vertices.len(), 3);
        assert!(matches!(
            collections.admit(InputKind::Vertex),
            Err(StlError::ResourceStopped(ResourceStop::InputLimitExceeded))
        ));
    }

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

    // Seven stored objects/twelve edges, but 127 visits when expanded. Keep
    // this small; never exercise a depth-32 exponential fixture.
    fn shared_binary_dag(depth: u32, geometry: bool) -> String {
        let mut resources = String::new();
        for id in 1..=depth {
            let child = id + 1;
            resources.push_str(&format!(
                r#"<object id="{id}" type="model"><components><component objectid="{child}"/><component objectid="{child}"/></components></object>"#
            ));
        }
        let leaf = depth + 1;
        if geometry {
            resources.push_str(&cube_mesh_xml(leaf));
        } else {
            resources.push_str(&format!(
                r#"<object id="{leaf}" type="model"><mesh><vertices/><triangles/></mesh></object>"#
            ));
        }
        resources
    }

    fn assert_visit_stop(result: Result<ParsedModel, StlError>) {
        assert!(matches!(
            result,
            Err(StlError::ResourceStopped(ResourceStop::InputLimitExceeded))
        ));
    }

    #[test]
    fn component_visit_budget_bounds_empty_dag_whole_build_and_plate_emission() {
        let root = model_xml(
            "millimeter",
            &shared_binary_dag(6, false),
            r#"<item objectid="1"/>"#,
        );
        let bytes = package(&[("3D/3dmodel.model", &root)], false);
        let parsed = parse_3mf_with_component_visit_limit(&bytes, 130).unwrap();
        assert!(!parsed.plate_metadata);
        assert_eq!(parsed.model.triangle_count, 0);
        assert_visit_stop(parse_3mf_with_component_visit_limit(&bytes, 64));
        assert_visit_stop(parse_3mf_with_component_visit_limit(&bytes, 129));

        let settings = plate_settings(&plate_xml(1, "Assigned", &[(1, 0)]));
        let bytes = package(
            &[
                ("3D/3dmodel.model", &root),
                (MODEL_SETTINGS_PATH, &settings),
            ],
            false,
        );
        assert!(
            parse_3mf_with_component_visit_limit(&bytes, 130)
                .unwrap()
                .plate_metadata
        );
        assert_visit_stop(parse_3mf_with_component_visit_limit(&bytes, 64));
        assert_visit_stop(parse_3mf_with_component_visit_limit(&bytes, 129));
    }

    #[test]
    fn component_visit_budget_membership_stop_is_terminal_not_plate_fallback() {
        let root = model_xml(
            "millimeter",
            &shared_binary_dag(6, false),
            r#"<item objectid="1"/>"#,
        );
        let settings = plate_settings(&plate_xml(1, "Empty", &[]));
        let bytes = package(
            &[
                ("3D/3dmodel.model", &root),
                (MODEL_SETTINGS_PATH, &settings),
            ],
            false,
        );
        let parsed = parse_3mf_with_component_visit_limit(&bytes, 130).unwrap();
        assert!(parsed.plate_metadata);
        assert_eq!(parsed.model.triangle_count, 0);
        assert_visit_stop(parse_3mf_with_component_visit_limit(&bytes, 64));
        assert_visit_stop(parse_3mf_with_component_visit_limit(&bytes, 129));
    }

    #[test]
    fn component_visit_budget_is_shared_across_parts_build_items_and_plates() {
        let child = model_xml("millimeter", &shared_binary_dag(4, false), "");
        let item = r#"<item p:path="/3D/Objects/child.model" objectid="1"/>"#;
        let single = model_xml("millimeter", "", item);
        let bytes = package(
            &[
                ("3D/3dmodel.model", &single),
                ("3D/Objects/child.model", &child),
            ],
            false,
        );
        parse_3mf_with_component_visit_limit(&bytes, 32).unwrap();
        let root = model_xml("millimeter", "", &item.repeat(2));
        let settings = plate_settings(&format!(
            "{}{}",
            plate_xml(1, "First", &[(1, 0)]),
            plate_xml(2, "Second", &[(1, 1)])
        ));
        for with_plates in [false, true] {
            let mut entries = vec![
                ("3D/3dmodel.model", root.as_str()),
                ("3D/Objects/child.model", child.as_str()),
            ];
            if with_plates {
                entries.push((MODEL_SETTINGS_PATH, settings.as_str()));
            }
            let bytes = package(&entries, false);
            let parsed = parse_3mf_with_component_visit_limit(&bytes, 64).unwrap();
            assert_eq!(parsed.plate_metadata, with_plates);
            assert_visit_stop(parse_3mf_with_component_visit_limit(&bytes, 50));
            assert_visit_stop(parse_3mf_with_component_visit_limit(&bytes, 63));
        }
    }

    #[test]
    fn component_visit_budget_is_not_reset_between_membership_and_fallback() {
        let root = model_xml(
            "millimeter",
            &shared_binary_dag(6, true),
            r#"<item objectid="1"/>"#,
        );
        let settings = plate_settings(&plate_xml(1, "Unassigned", &[]));
        let bytes = package(
            &[
                ("3D/3dmodel.model", &root),
                (MODEL_SETTINGS_PATH, &settings),
            ],
            false,
        );
        // Diagnostics charge three units; membership visits seven; fallback emits 127.
        let parsed = parse_3mf_with_component_visit_limit(&bytes, 137).unwrap();
        assert!(!parsed.plate_metadata);
        assert_eq!(parsed.model.triangle_count, 64 * 12);
        assert_eq!(
            parsed.plate_warning.as_deref(),
            Some("Plate metadata is unusable (plate metadata does not assign every printable build item); showing the whole build.")
        );
        assert_visit_stop(parse_3mf_with_component_visit_limit(&bytes, 136));
    }

    #[test]
    fn component_visit_budget_bounds_repeated_diagnostic_component_edges() {
        let components = r#"<component objectid="2"/>"#.repeat(4);
        let resources = format!(
            r#"<object id="1" type="model"><components>{components}</components></object><object id="2" type="model"><mesh><vertices/><triangles/></mesh></object>"#
        );
        let root = model_xml(
            "millimeter",
            &resources,
            &r#"<item objectid="1"/>"#.repeat(4),
        );
        let bytes = package(&[("3D/3dmodel.model", &root)], false);
        let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
        let mut pkg = Package::open(&bytes, &mut context).unwrap();
        let root = pkg.part("3D/3dmodel.model").unwrap();
        // Four build roots plus sixteen diagnostic edge iterations. Repeated
        // paths still cost work even though the resulting set contains one path.
        pkg.component_visits_remaining = 19;
        assert!(matches!(
            component_part_metadata_paths(&mut pkg, &root, "3D/3dmodel.model"),
            Err(StlError::ResourceStopped(ResourceStop::InputLimitExceeded))
        ));
        pkg.component_visits_remaining = 20;
        let paths = component_part_metadata_paths(&mut pkg, &root, "3D/3dmodel.model").unwrap();
        assert_eq!(paths[&2].len(), 1);
        assert_eq!(pkg.component_visits_remaining, 0);
        assert_visit_stop(parse_3mf_with_component_visit_limit(&bytes, 19));
        // The same budget must also fund four five-visit emissions afterward.
        parse_3mf_with_component_visit_limit(&bytes, 40).unwrap();
        assert_visit_stop(parse_3mf_with_component_visit_limit(&bytes, 39));
    }

    #[test]
    fn component_visit_budget_direct_walk_exact_boundary_is_127() {
        let root = model_xml("millimeter", &shared_binary_dag(6, false), "");
        let bytes = package(&[("3D/3dmodel.model", &root)], false);
        let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
        let mut pkg = Package::open(&bytes, &mut context).unwrap();
        pkg.component_visits_remaining = 127;
        assert!(!object_has_geometry(&mut pkg, "3D/3dmodel.model", 1, 0).unwrap());
        assert_eq!(pkg.component_visits_remaining, 0);
        pkg.component_visits_remaining = 126;
        assert!(matches!(
            object_has_geometry(&mut pkg, "3D/3dmodel.model", 1, 0),
            Err(StlError::ResourceStopped(ResourceStop::InputLimitExceeded))
        ));
        pkg.component_visits_remaining = 127;
        emit_object(
            &mut pkg,
            "3D/3dmodel.model",
            1,
            &IDENTITY,
            0,
            &mut new_out(
                1.0,
                &InputCollections::new(ZipResourceObserver::new(), None),
            ),
            &mut StandardAppearance::Empty,
        )
        .unwrap();
        assert_eq!(pkg.component_visits_remaining, 0);
    }

    #[test]
    fn component_visit_budget_debits_before_lookup_and_counts_cached_empty_objects() {
        let root = model_xml(
            "millimeter",
            r#"<object id="1" type="support"><mesh><vertices/><triangles/></mesh></object>"#,
            "",
        );
        let bytes = package(&[("3D/3dmodel.model", &root)], false);
        let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
        let mut pkg = Package::open(&bytes, &mut context).unwrap();
        pkg.part("3D/3dmodel.model").unwrap();
        pkg.component_visits_remaining = 1;
        assert!(!object_has_geometry(&mut pkg, "3D/3dmodel.model", 1, 0).unwrap());
        assert_eq!(pkg.component_visits_remaining, 0);
        assert!(matches!(
            object_has_geometry(&mut pkg, "missing.model", 999, 0),
            Err(StlError::ResourceStopped(ResourceStop::InputLimitExceeded))
        ));
        assert!(matches!(
            emit_object(
                &mut pkg,
                "missing.model",
                999,
                &IDENTITY,
                0,
                &mut new_out(
                    1.0,
                    &InputCollections::new(ZipResourceObserver::new(), None)
                ),
                &mut StandardAppearance::Empty,
            ),
            Err(StlError::ResourceStopped(ResourceStop::InputLimitExceeded))
        ));
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

    struct DerivedMapsProfileGuard(Option<Rc<DerivedMapsTestProfile>>);

    impl DerivedMapsProfileGuard {
        fn install(profile: Rc<DerivedMapsTestProfile>) -> Self {
            Self(DERIVED_MAPS_TEST_PROFILE.with(|slot| slot.replace(Some(profile))))
        }
    }
    impl Drop for DerivedMapsProfileGuard {
        fn drop(&mut self) {
            DERIVED_MAPS_TEST_PROFILE.with(|slot| slot.replace(self.0.take()));
        }
    }

    #[test]
    fn derived_maps_assignment_retention_never_exceeds_physical_build_membership() {
        let model = model_xml(
            "millimeter",
            part_retention_triangle(),
            &r#"<item objectid="1"/>"#.repeat(2),
        );
        let settings = plate_settings(&plate_xml(10, "Duplicates", &[(1, 0), (1, 1), (1, 0)]));
        let bytes = package(
            &[
                (ROOT_MODEL_FALLBACK, &model),
                (PLATE_SETTINGS_PATH, &settings),
            ],
            false,
        );
        let profile = Rc::new(DerivedMapsTestProfile::default());
        let _guard = DerivedMapsProfileGuard::install(profile.clone());
        let parsed = crate::parse_model_with_plates(&bytes).unwrap();
        assert_eq!(parsed.model.triangle_count, 2);
        assert_eq!(parsed.model.positions.len(), 18);
        assert_eq!(parsed.parts.len(), 2);
        assert!(parsed.part_warning.is_none());
        assert!(!parsed.plate_metadata);
        assert_eq!(parsed.plates.len(), 1);
        assert_eq!(parsed.plates[0].triangle_count, 2);
        assert_eq!(parsed.plate_warning.as_deref(), Some("Plate metadata is unusable (object 1 instance 0 is assigned more than once); showing the whole build."));
        assert_eq!(
            profile.plate_parser_entries.get(),
            1,
            "reached actual plate parser"
        );
        assert!(
            profile.assignment_high_water.get() <= 2,
            "retained {} assignments for only2 physical build members",
            profile.assignment_high_water.get()
        );
    }

    #[test]
    fn derived_maps_diagnostic_paths_copy_each_normalized_pair_only_once() {
        let root = model_xml(
            "millimeter",
            r#"<object id="10"><components><component objectid="2" path="/3D/child.model"/><component objectid="2" path="3D/child.model"/></components></object>"#,
            &r#"<item objectid="10"/>"#.repeat(2),
        );
        let child = model_xml(
            "millimeter",
            &part_retention_triangle().replacen("id=\"1\"", "id=\"2\"", 1),
            "",
        );
        let bytes = package(
            &[(ROOT_MODEL_FALLBACK, &root), ("3D/child.model", &child)],
            true,
        );
        let profile = Rc::new(DerivedMapsTestProfile::default());
        let _guard = DerivedMapsProfileGuard::install(profile.clone());
        let parsed = crate::parse_model_with_plates(&bytes).unwrap();
        assert_eq!(parsed.model.triangle_count, 4);
        assert_eq!(parsed.model.positions.len(), 36);
        assert_eq!(parsed.parts.len(), 4);
        assert!(parsed
            .parts
            .iter()
            .all(|part| part.name == "triangle" && part.triangle_count == 1));
        assert!(parsed.part_warning.is_none());
        assert_eq!(
            parsed.plate_warning.as_deref(),
            Some(NO_PLATE_METADATA_WARNING)
        );
        assert_eq!(profile.diagnostic_unique_pairs.get(), 1);
        assert!(
            profile.diagnostic_path_copies.get() <= profile.diagnostic_unique_pairs.get(),
            "copied {} paths for {} unique normalized pairs",
            profile.diagnostic_path_copies.get(),
            profile.diagnostic_unique_pairs.get()
        );
    }

    struct PartRetentionProfileGuard(Option<Rc<PartRetentionTestProfile>>);

    impl PartRetentionProfileGuard {
        fn install(profile: Rc<PartRetentionTestProfile>) -> Self {
            Self(PART_RETENTION_TEST_PROFILE.with(|slot| slot.replace(Some(profile))))
        }
    }
    impl Drop for PartRetentionProfileGuard {
        fn drop(&mut self) {
            PART_RETENTION_TEST_PROFILE.with(|slot| slot.replace(self.0.take()));
        }
    }

    fn tiny_part_retention_fixture(assigned: bool) -> Vec<u8> {
        let resources = r#"<object id="1" name="triangle"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/></vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object>"#;
        let model = model_xml(
            "millimeter",
            resources,
            &r#"<item objectid="1"/>"#.repeat(3),
        );
        let settings = plate_settings(&format!(
            "{}{}",
            plate_xml(10, "First", &[(1, 0), (1, 2)]),
            plate_xml(20, "Second", &[(1, 1)])
        ));
        let mut entries = vec![(ROOT_MODEL_FALLBACK, model.as_str())];
        if assigned {
            entries.push((PLATE_SETTINGS_PATH, settings.as_str()));
        }
        package(&entries, assigned)
    }

    fn assert_part_retention_high_water(assigned: bool) {
        let profile = Rc::new(PartRetentionTestProfile::new(2));
        let _guard = PartRetentionProfileGuard::install(profile.clone());
        let parsed =
            crate::parse_model_with_plates(&tiny_part_retention_fixture(assigned)).unwrap();
        assert_eq!(parsed.model.triangle_count, 3);
        assert_eq!(parsed.model.positions.len(), 27);
        assert!(parsed.parts.is_empty());
        assert_eq!(parsed.part_warning.as_deref(), Some(PART_WARNING));
        assert_eq!(parsed.plate_metadata, assigned);
        if assigned {
            assert!(parsed.plate_warning.is_none());
            assert_eq!(
                parsed
                    .plates
                    .iter()
                    .map(|plate| (
                        plate.name.as_str(),
                        plate.triangle_start,
                        plate.triangle_count
                    ))
                    .collect::<Vec<_>>(),
                vec![("First", 0, 2), ("Second", 2, 1)]
            );
        } else {
            assert!(parsed.plate_warning.is_some());
            assert_eq!(parsed.plates.len(), 1);
            assert_eq!(parsed.plates[0].triangle_count, 3);
        }
        // Observe the actual retained-row high-water, not the already-cleared
        // final output. Geometry/warning compatibility was asserted above.
        assert!(
            profile.high_water.get() <= profile.limit,
            "actual retained high-water {} exceeds {} (name builders {})",
            profile.high_water.get(),
            profile.limit,
            profile.name_builders.get()
        );
        assert_eq!(
            profile.name_builders.get(),
            2,
            "third optional name must never be built"
        );
        assert_eq!(profile.id_builders.get(), 2);
        assert!(!profile.requested_capacity.borrow().is_empty());
        assert!(profile
            .requested_capacity
            .borrow()
            .iter()
            .all(|&target| target <= profile.limit));
    }

    #[test]
    fn part_retention_whole_build_never_retains_third_optional_boundary() {
        assert_part_retention_high_water(false);
    }

    #[test]
    fn part_retention_assigned_plates_share_one_bounded_accumulator() {
        assert_part_retention_high_water(true);
    }

    fn part_retention_triangle() -> &'static str {
        r#"<object id="1" name="triangle"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/></vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object>"#
    }

    fn part_retention_fixture_with_build(resources: &str, build: &str) -> Vec<u8> {
        let model = model_xml("millimeter", resources, build);
        package(&[(ROOT_MODEL_FALLBACK, &model)], false)
    }

    #[test]
    fn part_retention_exact_capacity_empty_disabled_unprinted_do_not_omit() {
        for later_overflow in [false, true] {
            let resources = format!(
                "{}<object id=\"2\"/>{}",
                part_retention_triangle(),
                part_retention_triangle().replace("id=\"1\"", "id=\"3\" type=\"support\"")
            );
            let mut build = format!(
                "{}{}",
                r#"<item objectid="1"/>"#.repeat(2),
                r#"<item objectid="2"/><item objectid="1" printable="0"/><item objectid="3"/>"#
            );
            if later_overflow {
                build.push_str(r#"<item objectid="1"/><item objectid="2"/><item objectid="1"/>"#);
            }
            let profile = Rc::new(PartRetentionTestProfile::new(2));
            let _guard = PartRetentionProfileGuard::install(profile.clone());
            let parsed = crate::parse_model_with_plates(&part_retention_fixture_with_build(
                &resources, &build,
            ))
            .unwrap();
            assert_eq!(
                parsed.model.triangle_count,
                if later_overflow { 4 } else { 2 }
            );
            assert_eq!(profile.high_water.get(), 2);
            assert_eq!(profile.name_builders.get(), 2);
            assert_eq!(profile.id_builders.get(), 2);
            if later_overflow {
                assert!(parsed.parts.is_empty());
                assert_eq!(parsed.part_warning.as_deref(), Some(PART_WARNING));
            } else {
                assert_eq!(parsed.parts.len(), 2);
                assert!(parsed.part_warning.is_none());
            }
        }
    }

    #[test]
    fn part_retention_component_boundaries_use_the_same_nonempty_admission() {
        let resources = format!("{}<object id=\"2\"/><object id=\"3\"><components><component objectid=\"1\"/><component objectid=\"1\"/><component objectid=\"2\"/><component objectid=\"1\"/></components></object>", part_retention_triangle());
        let profile = Rc::new(PartRetentionTestProfile::new(2));
        let _guard = PartRetentionProfileGuard::install(profile.clone());
        let parsed = crate::parse_model_with_plates(&part_retention_fixture_with_build(
            &resources,
            r#"<item objectid="3"/>"#,
        ))
        .unwrap();
        assert_eq!(parsed.model.triangle_count, 3);
        assert!(parsed.parts.is_empty());
        assert_eq!(parsed.part_warning.as_deref(), Some(PART_WARNING));
        assert_eq!(profile.high_water.get(), 2);
        assert_eq!(
            profile.name_builders.get(),
            2,
            "empty child and overflow do not build names"
        );
        assert_eq!(profile.id_builders.get(), 2);
    }

    #[test]
    fn part_retention_capacity_and_omission_suppress_uniform_appearance_copies() {
        let resources = format!(
            "{}{}",
            r##"<basematerials id="5"><base name="Red" displaycolor="#ff0000"/></basematerials>"##,
            part_retention_triangle().replacen(
                "<object id=\"1\"",
                "<object id=\"1\" pid=\"5\" pindex=\"0\"",
                1
            )
        );
        let profile = Rc::new(PartRetentionTestProfile::new(2));
        let _guard = PartRetentionProfileGuard::install(profile.clone());
        let parsed = crate::parse_model_with_plates(&part_retention_fixture_with_build(
            &resources,
            &r#"<item objectid="1"/>"#.repeat(5),
        ))
        .unwrap();
        assert_eq!(parsed.model.triangle_count, 5);
        assert_eq!(
            parsed.materials.len(),
            1,
            "global material behavior stays intact"
        );
        assert!(parsed.parts.is_empty());
        assert_eq!(parsed.part_warning.as_deref(), Some(PART_WARNING));
        assert_eq!(
            profile.appearance_copies.get(),
            2,
            "only below-cap in-flight Uniform scratch remains"
        );
        assert_eq!(profile.name_builders.get(), 2);
        assert_eq!(profile.id_builders.get(), 2);
    }

    #[test]
    fn part_retention_row_reserve_failures_are_terminal_before_record_building() {
        for assigned in [false, true] {
            for attempt in [1, 2] {
                let mut profile = PartRetentionTestProfile::new(2);
                profile.fail_reserve_at = Some(attempt);
                let profile = Rc::new(profile);
                let _guard = PartRetentionProfileGuard::install(profile.clone());
                assert_input_failure(
                    parse_3mf(&tiny_part_retention_fixture(assigned)),
                    ParserTestFailure::Allocation,
                );
                assert_eq!(profile.reserves.get(), attempt);
                assert_eq!(profile.high_water.get(), attempt - 1);
                assert_eq!(profile.name_builders.get(), attempt - 1);
                assert_eq!(profile.id_builders.get(), attempt - 1);
            }
        }
    }

    #[test]
    fn part_retention_post_reserve_first_cause_is_not_optional_fallback() {
        for assigned in [false, true] {
            for attempt in [1, 2] {
                for failure in PARSER_TEST_FAILURES {
                    let mut profile = PartRetentionTestProfile::new(2);
                    profile.stop_after_reserve = Some((attempt, failure));
                    let profile = Rc::new(profile);
                    let _guard = PartRetentionProfileGuard::install(profile.clone());
                    assert_input_failure(
                        parse_3mf(&tiny_part_retention_fixture(assigned)),
                        failure,
                    );
                    assert_eq!(profile.reserves.get(), attempt);
                    assert_eq!(profile.high_water.get(), attempt - 1);
                    assert_eq!(profile.name_builders.get(), attempt - 1);
                    assert_eq!(profile.id_builders.get(), attempt - 1);
                }
            }
        }
    }

    #[test]
    fn part_retention_omitted_metadata_never_hides_later_geometry_errors() {
        for nonfinite in [false, true] {
            let resources = if nonfinite {
                format!(
                    "{}{}",
                    part_retention_triangle(),
                    part_retention_triangle()
                        .replacen("id=\"1\"", "id=\"4\"", 1)
                        .replacen("x=\"0\"", "x=\"NaN\"", 1)
                )
            } else {
                part_retention_triangle().to_owned()
            };
            let build = format!(
                "{}<item objectid=\"{}\"/>",
                r#"<item objectid="1"/>"#.repeat(3),
                if nonfinite { 4 } else { 999 }
            );
            let profile = Rc::new(PartRetentionTestProfile::new(2));
            let _guard = PartRetentionProfileGuard::install(profile.clone());
            let result = crate::parse_model_with_plates(&part_retention_fixture_with_build(
                &resources, &build,
            ));
            if nonfinite {
                assert!(matches!(
                    result,
                    Err(StlError::NonFiniteCoordinate { triangle_index: 3 })
                ));
            } else {
                assert!(matches!(result, Err(StlError::Malformed3mf { .. })));
            }
            assert_eq!(profile.high_water.get(), 2);
            assert_eq!(profile.name_builders.get(), 2);
            assert_eq!(profile.id_builders.get(), 2);
        }
    }

    #[test]
    fn part_retention_omitted_state_still_obeys_existing_flattened_triangle_limit() {
        let bytes =
            part_retention_fixture_with_build(part_retention_triangle(), r#"<item objectid="1"/>"#);
        let profile = Rc::new(PartRetentionTestProfile::new(0));
        let _guard = PartRetentionProfileGuard::install(profile.clone());
        let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
        let mut pkg = Package::open(&bytes, &mut context).unwrap();
        let root = pkg.part(ROOT_MODEL_FALLBACK).unwrap();
        let mut rows = PartOutput::new(&pkg);
        let mut out = new_out(1.0, &pkg.collections);
        let slicer = SlicerMetadata::default();
        let paths = HashMap::new();
        emit_build_item(
            &mut pkg,
            ROOT_MODEL_FALLBACK,
            &root.build[0],
            0,
            &slicer,
            &paths,
            &mut out,
            &mut rows,
        )
        .unwrap();
        assert!(matches!(rows.rows, PartRows::Omitted));
        out.triangle_count = MAX_TRIANGLES; // tiny seam into existing output check
        let error = emit_build_item(
            &mut pkg,
            ROOT_MODEL_FALLBACK,
            &root.build[0],
            1,
            &slicer,
            &paths,
            &mut out,
            &mut rows,
        );
        assert!(
            matches!(error, Err(StlError::TooManyTriangles { count, limit }) if count == MAX_TRIANGLES + 1 && limit == MAX_TRIANGLES)
        );
        assert_eq!(profile.name_builders.get(), 0);
        assert_eq!(profile.id_builders.get(), 0);
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
    fn guard_probe(observer: ZipResourceObserver) -> (Rc<ParserTestProbe>, ParserProbeGuard) {
        let probe = Rc::new(ParserTestProbe {
            member: "never-inject-a-source-fault",
            failure: ParserTestFailure::Stop(ResourceStop::EnvelopeChanged),
            state: std::cell::RefCell::new(ParserTestState::default()),
            guard_observer: Some(observer),
        });
        let guard = ParserProbeGuard::install(probe.clone());
        (probe, guard)
    }

    #[test]
    fn guarded_core_public_settings_xml_stop_preserves_cached_root_without_fallback() {
        let root = model_xml("millimeter", &cube_mesh_xml(1), r#"<item objectid="1"/>"#);
        let settings = format!(
            "<config><metadata key='unused' value='{}'/></config>",
            "x".repeat(4097)
        );
        for deflate in [false, true] {
            let bytes = package(
                &[
                    (ROOT_MODEL_FALLBACK, &root),
                    (MODEL_SETTINGS_PATH, &settings),
                ],
                deflate,
            );
            for aggregate in [false, true] {
                let (probe, _guard) = guard_probe(ZipResourceObserver::new());
                let result = if aggregate {
                    crate::parse_model(&bytes).map(|_| ())
                } else {
                    crate::parse_model_with_plates(&bytes).map(|_| ())
                };
                assert_eq!(
                    result,
                    Err(StlError::ResourceStopped(ResourceStop::InputLimitExceeded))
                );
                let state = probe.state.borrow();
                assert!(state.root_cached);
                assert_eq!(
                    state.total_work, 0,
                    "must not call cached whole-build fallback"
                );
            }
        }
    }

    #[test]
    fn guarded_core_context_actual_inflation_and_retained_text_are_not_double_debited() {
        use crate::package_bounds::PackageTestLimits;
        let root = model_xml("millimeter", &cube_mesh_xml(1), r#"<item objectid="1"/>"#);
        let settings = "<config/>";
        for deflate in [false, true] {
            let bytes = package(
                &[
                    ("_rels/.rels", RELS),
                    (ROOT_MODEL_FALLBACK, &root),
                    (MODEL_SETTINGS_PATH, settings),
                ],
                deflate,
            );
            let observer = ZipResourceObserver::new();
            let (probe, _guard) = guard_probe(observer.clone());
            let mut context = PackageProcessingContext::with_test_limits(
                observer.clone(),
                PackageTestLimits {
                    inflation: (RELS.len() + root.len() + settings.len()) as u64,
                    text_total: RELS.len() + settings.len(),
                    ..Default::default()
                },
            );
            let parsed = crate::parse_model_with_plates_in_context(&bytes, &mut context).unwrap();
            assert_eq!(parsed.model.triangle_count, 12);
            assert!(!parsed.plate_metadata);
            assert!(observer.check().is_ok());
            // A new parse really reinflates, so the same context is exhausted.
            assert_eq!(
                crate::parse_model_in_context(&bytes, &mut context),
                Err(StlError::ResourceStopped(ResourceStop::ReadWorkExceeded))
            );
            assert_eq!(probe.state.borrow().after_io, 0);
        }
    }

    #[test]
    fn guarded_core_context_actual_model_part_and_last_text_limits_are_terminal() {
        use crate::package_bounds::PackageTestLimits;
        let root = model_xml("millimeter", &cube_mesh_xml(1), r#"<item objectid="1"/>"#);
        let settings = "<config/>";
        let bytes = package(
            &[
                (ROOT_MODEL_FALLBACK, &root),
                (MODEL_SETTINGS_PATH, settings),
            ],
            true,
        );
        for (limits, cached) in [
            (
                PackageTestLimits {
                    model: root.len() as u64 - 1,
                    ..Default::default()
                },
                false,
            ),
            (
                PackageTestLimits {
                    model_settings: settings.len() - 1,
                    ..Default::default()
                },
                true,
            ),
            (
                PackageTestLimits {
                    text_total: settings.len() - 1,
                    ..Default::default()
                },
                true,
            ),
        ] {
            let observer = ZipResourceObserver::new();
            let (probe, _guard) = guard_probe(observer.clone());
            let mut context = PackageProcessingContext::with_test_limits(observer, limits);
            assert_eq!(
                crate::parse_model_with_plates_in_context(&bytes, &mut context),
                Err(StlError::ResourceStopped(ResourceStop::InputLimitExceeded))
            );
            let state = probe.state.borrow();
            assert_eq!(state.root_cached, cached);
            assert_eq!(state.total_work, 0);
            assert_eq!(state.after_io, 0);
            assert_eq!(state.work_after_stop, 0);
        }
        let observer = ZipResourceObserver::new();
        let mut context = PackageProcessingContext::with_test_limits(
            observer,
            PackageTestLimits {
                model: root.len() as u64,
                ..Default::default()
            },
        );
        assert_eq!(
            crate::parse_model_in_context(&bytes, &mut context)
                .unwrap()
                .triangle_count,
            12
        );
    }

    #[test]
    fn guarded_core_relationship_xml_limit_never_becomes_conventional_root_fallback() {
        let root = model_xml("millimeter", &cube_mesh_xml(1), r#"<item objectid="1"/>"#);
        let rels = format!(
            "<Relationships><Unknown ignored='{}'/></Relationships>",
            "x".repeat(4097)
        );
        let bytes = package(
            &[("_rels/.rels", &rels), (ROOT_MODEL_FALLBACK, &root)],
            false,
        );
        let observer = ZipResourceObserver::new();
        let (probe, _guard) = guard_probe(observer.clone());
        let mut context = PackageProcessingContext::new(observer);
        assert_eq!(
            crate::parse_model_with_plates_in_context(&bytes, &mut context),
            Err(StlError::ResourceStopped(ResourceStop::InputLimitExceeded))
        );
        let state = probe.state.borrow();
        assert!(!state.root_cached);
        assert_eq!(state.total_work, 0);
        assert_eq!(state.after_io, 0);
    }

    #[test]
    fn guarded_core_retained_reserve_failure_is_terminal_not_optional_metadata() {
        use crate::package_bounds::PackageTestLimits;
        let root = model_xml("millimeter", &cube_mesh_xml(1), r#"<item objectid="1"/>"#);
        let bytes = package(
            &[
                (ROOT_MODEL_FALLBACK, &root),
                (MODEL_SETTINGS_PATH, "<config/>"),
            ],
            false,
        );
        let observer = ZipResourceObserver::new();
        let (probe, _guard) = guard_probe(observer.clone());
        let mut context = PackageProcessingContext::with_test_limits(
            observer,
            PackageTestLimits {
                fail_reserve: true,
                ..Default::default()
            },
        );
        assert_eq!(
            crate::parse_model_with_plates_in_context(&bytes, &mut context),
            Err(StlError::AllocationFailed)
        );
        let state = probe.state.borrow();
        assert!(state.root_cached);
        assert_eq!(state.total_work, 0);
        assert_eq!(state.after_io, 0);
    }
    #[test]
    fn guarded_core_declared_optional_project_oversize_stays_a_no_read_skip() {
        let bytes = package(&[(PROJECT_SETTINGS_PATH, "{}")], false);
        let probe = Rc::new(ParserTestProbe {
            member: PROJECT_SETTINGS_PATH,
            failure: ParserTestFailure::Stop(ResourceStop::EnvelopeChanged),
            state: std::cell::RefCell::new(ParserTestState::default()),
            guard_observer: None,
        });
        let _guard = ParserProbeGuard::install(probe.clone());
        let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
        let mut pkg = Package::open(&bytes, &mut context).unwrap();
        assert_eq!(
            pkg.optional_bounded_text(PROJECT_SETTINGS_PATH, 1).unwrap(),
            None
        );
        assert!(!probe.state.borrow().armed);
        assert!(!probe.state.borrow().fired);
    }
    struct ProjectAuxProbeGuard(Option<Rc<std::cell::Cell<usize>>>);
    impl ProjectAuxProbeGuard {
        fn install() -> (Self, Rc<std::cell::Cell<usize>>) {
            let probe = Rc::new(std::cell::Cell::new(0));
            let previous =
                PROJECT_AUX_NORMALIZATION_PROBE.with(|slot| slot.replace(Some(probe.clone())));
            (Self(previous), probe)
        }
    }
    impl Drop for ProjectAuxProbeGuard {
        fn drop(&mut self) {
            PROJECT_AUX_NORMALIZATION_PROBE.with(|slot| slot.replace(self.0.take()));
        }
    }

    #[test]
    fn project_json_auxiliary_retention_stops_at_selected_256_position_prefix() {
        let array = |label: &str| {
            std::iter::repeat_n(format!("\"{label}\""), 257)
                .collect::<Vec<_>>()
                .join(",")
        };
        let text = format!(
            r##"{{"filament_colour":["#123456"],"filament_settings_id":[{}],"filament_type":[{}],"filament_vendor":[{}]}}"##,
            array(" Basic "),
            array(" PLA "),
            array(" Acme ")
        );
        let (_guard, probe) = ProjectAuxProbeGuard::install();
        let parsed = parse_project_filaments(&text).unwrap();
        // Preserve actual output while separately measuring the unwanted copy
        // work. Output-only checks already passed before this correction.
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].slot, 1);
        assert_eq!(parsed[0].color, "#123456");
        assert_eq!(parsed[0].name.as_deref(), Some("Acme Basic"));
        assert_eq!(parsed[0].material.as_deref(), Some("PLA"));
        let normalized = probe.get();
        assert!(
            normalized >= 3,
            "probe must observe actual auxiliary normalization"
        );
        assert!(normalized <= 3*MAX_EMBEDDED_FILAMENTS,
            "only selected positional prefixes may be retained: normalized {normalized} labels, allowed {}", 3*MAX_EMBEDDED_FILAMENTS);
    }
    #[test]
    fn derived_maps_reserve_failures_are_source_terminal_not_metadata_fallback() {
        use derived_maps::ReserveSite;
        let model = model_xml(
            "millimeter",
            part_retention_triangle(),
            &r#"<item objectid="1"/>"#.repeat(2),
        );
        let settings = plate_settings(&plate_xml(1, "Complete", &[(1, 0), (1, 1)]));
        let bytes = package(
            &[
                (ROOT_MODEL_FALLBACK, &model),
                (PLATE_SETTINGS_PATH, &settings),
            ],
            true,
        );
        for site in [
            ReserveSite::Counts,
            ReserveSite::Occurrences,
            ReserveSite::Items,
            ReserveSite::Claimed,
            ReserveSite::Plates,
            ReserveSite::Indices,
            ReserveSite::OutputPlates,
        ] {
            let profile = Rc::new(DerivedMapsTestProfile::default());
            profile.fail_site.set(Some(site));
            let _guard = DerivedMapsProfileGuard::install(profile.clone());
            let observer = ZipResourceObserver::new();
            let mut context = PackageProcessingContext::new(observer.clone());
            assert_eq!(
                crate::parse_model_with_plates_in_context(&bytes, &mut context),
                Err(StlError::AllocationFailed)
            );
            assert!(matches!(
                observer.check(),
                Err(ZipOpenError::AllocationFailed)
            ));
            assert!(
                profile.fail_site.get().is_none(),
                "actual target reserve was reached"
            );
            assert_eq!(profile.fallback_entries.get(), 0);
            assert!(profile.reserves.get() > 0);
        }
    }

    #[test]
    fn derived_maps_post_reserve_stops_preserve_first_cause_in_public_parser() {
        use derived_maps::ReserveSite;
        let model = model_xml(
            "millimeter",
            part_retention_triangle(),
            &r#"<item objectid="1"/>"#.repeat(2),
        );
        let settings = plate_settings(&plate_xml(1, "Complete", &[(1, 0), (1, 1)]));
        let bytes = package(
            &[
                (ROOT_MODEL_FALLBACK, &model),
                (PLATE_SETTINGS_PATH, &settings),
            ],
            false,
        );
        for site in [
            ReserveSite::Counts,
            ReserveSite::Occurrences,
            ReserveSite::Items,
            ReserveSite::Claimed,
            ReserveSite::Plates,
            ReserveSite::Indices,
            ReserveSite::OutputPlates,
        ] {
            for failure in PARSER_TEST_FAILURES {
                let profile = Rc::new(DerivedMapsTestProfile::default());
                profile.stop_site.set(Some((site, failure)));
                let _guard = DerivedMapsProfileGuard::install(profile.clone());
                let observer = ZipResourceObserver::new();
                let mut context = PackageProcessingContext::new(observer.clone());
                assert_eq!(
                    crate::parse_model_with_plates_in_context(&bytes, &mut context),
                    Err(failure.error())
                );
                assert!(
                    profile.stop_site.get().is_none(),
                    "actual successful target reserve was reached"
                );
                assert_eq!(
                    StlError::from(observer.check().unwrap_err()),
                    failure.error()
                );
                assert_eq!(profile.fallback_entries.get(), 0);
            }
        }
    }

    #[test]
    fn derived_maps_actual_plate_invalidity_observes_later_terminal_event_without_fallback() {
        let model = model_xml(
            "millimeter",
            part_retention_triangle(),
            &r#"<item objectid="1"/>"#.repeat(2),
        );
        let settings = plate_settings(&plate_xml(1, "Duplicate", &[(1, 0), (1, 0)]));
        let bytes = package(
            &[
                (ROOT_MODEL_FALLBACK, &model),
                (PLATE_SETTINGS_PATH, &settings),
            ],
            true,
        );
        for failure in PARSER_TEST_FAILURES {
            let profile = Rc::new(DerivedMapsTestProfile::default());
            profile.after_invalid.set(Some(failure));
            let _guard = DerivedMapsProfileGuard::install(profile.clone());
            let observer = ZipResourceObserver::new();
            let mut context = PackageProcessingContext::new(observer.clone());
            assert_eq!(
                crate::parse_model_with_plates_in_context(&bytes, &mut context),
                Err(failure.error())
            );
            assert_eq!(profile.plate_parser_entries.get(), 1);
            assert_eq!(profile.membership_invalid.get(), 1);
            assert_eq!(profile.invalid_tail_events.get(), 1);
            assert!(profile.after_invalid.get().is_none());
            assert_eq!(profile.fallback_entries.get(), 0);
            assert_eq!(
                StlError::from(observer.check().unwrap_err()),
                failure.error()
            );
        }
    }

    #[test]
    fn derived_maps_diagnostic_allocation_boundaries_are_terminal() {
        use derived_maps::ReserveSite;
        let root = model_xml(
            "millimeter",
            r#"<object id="10"><components><component objectid="2" path="/3D/child.model"/></components></object>"#,
            r#"<item objectid="10"/>"#,
        );
        let child = model_xml(
            "millimeter",
            &part_retention_triangle().replacen("id=\"1\"", "id=\"2\"", 1),
            "",
        );
        let bytes = package(
            &[(ROOT_MODEL_FALLBACK, &root), ("3D/child.model", &child)],
            false,
        );
        for site in [
            ReserveSite::DiagnosticMap,
            ReserveSite::DiagnosticSet,
            ReserveSite::Path,
        ] {
            let profile = Rc::new(DerivedMapsTestProfile::default());
            profile.fail_site.set(Some(site));
            let _guard = DerivedMapsProfileGuard::install(profile.clone());
            assert_eq!(
                crate::parse_model_with_plates(&bytes),
                Err(StlError::AllocationFailed)
            );
            assert!(profile.fail_site.get().is_none());
            assert_eq!(profile.diagnostic_path_copies.get(), 0);
            assert_eq!(profile.fallback_entries.get(), 0);
        }
    }
    #[test]
    fn derived_maps_immediate_structural_error_does_not_scan_to_later_terminal_hook() {
        let model = model_xml(
            "millimeter",
            part_retention_triangle(),
            r#"<item objectid="1"/>"#,
        );
        let settings = plate_settings(&format!(
            "{}<plate><plate></plate></plate>",
            plate_xml(1, "Unknown", &[(1, 99)])
        ));
        let bytes = package(
            &[
                (ROOT_MODEL_FALLBACK, &model),
                (PLATE_SETTINGS_PATH, &settings),
            ],
            false,
        );
        let profile = Rc::new(DerivedMapsTestProfile::default());
        profile.after_invalid.set(Some(ParserTestFailure::Stop(
            ResourceStop::ReadWorkExceeded,
        )));
        // End first plate, Start second plate, nested Start are the three old
        // observed events. The armed fourth event must never be requested.
        profile.invalid_stop_at.set(4);
        let _guard = DerivedMapsProfileGuard::install(profile.clone());
        let observer = ZipResourceObserver::new();
        let mut context = PackageProcessingContext::new(observer.clone());
        let parsed = crate::parse_model_with_plates_in_context(&bytes, &mut context).unwrap();
        assert_eq!(
            parsed.plate_warning.as_deref(),
            Some("Plate metadata is unusable (nested plate metadata); showing the whole build.")
        );
        assert_eq!(profile.plate_parser_entries.get(), 1);
        assert_eq!(profile.membership_invalid.get(), 1);
        assert_eq!(profile.invalid_tail_events.get(), 3);
        assert!(profile.after_invalid.get().is_some());
        assert!(observer.check().is_ok());
        assert_eq!(profile.fallback_entries.get(), 1);
        assert_eq!(parsed.model.triangle_count, 1);
    }

    #[test]
    fn derived_maps_diagnostic_post_reserve_stops_prevent_path_copy() {
        use derived_maps::ReserveSite;
        let root = model_xml(
            "millimeter",
            r#"<object id="10"><components><component objectid="2" path="/3D/child.model"/></components></object>"#,
            r#"<item objectid="10"/>"#,
        );
        let child = model_xml(
            "millimeter",
            &part_retention_triangle().replacen("id=\"1\"", "id=\"2\"", 1),
            "",
        );
        let bytes = package(
            &[(ROOT_MODEL_FALLBACK, &root), ("3D/child.model", &child)],
            true,
        );
        for site in [
            ReserveSite::DiagnosticMap,
            ReserveSite::DiagnosticSet,
            ReserveSite::Path,
        ] {
            for failure in PARSER_TEST_FAILURES {
                let profile = Rc::new(DerivedMapsTestProfile::default());
                profile.stop_site.set(Some((site, failure)));
                let _guard = DerivedMapsProfileGuard::install(profile.clone());
                let observer = ZipResourceObserver::new();
                let mut context = PackageProcessingContext::new(observer.clone());
                assert_eq!(
                    crate::parse_model_with_plates_in_context(&bytes, &mut context),
                    Err(failure.error())
                );
                assert!(profile.stop_site.get().is_none());
                assert_eq!(profile.diagnostic_path_copies.get(), 0);
                assert_eq!(profile.fallback_entries.get(), 0);
            }
        }
    }
    struct SettingsOwnedProfileGuard(Option<Rc<SettingsOwnedTestProfile>>);
    impl SettingsOwnedProfileGuard {
        fn install(profile: Rc<SettingsOwnedTestProfile>) -> Self {
            Self(SETTINGS_OWNED_TEST_PROFILE.with(|slot| slot.replace(Some(profile))))
        }
    }
    impl Drop for SettingsOwnedProfileGuard {
        fn drop(&mut self) {
            drop(SETTINGS_OWNED_TEST_PROFILE.with(|slot| slot.replace(self.0.take())));
        }
    }

    #[test]
    fn settings_relevance_owned_irrelevant_object_names_and_rows_are_not_retained() {
        let root = model_xml("millimeter", &cube_mesh_xml(1), r#"<item objectid="1"/>"#);
        let settings = r#"<config><object id="1"><metadata key="name" value="Useful"/><metadata key="extruder" value="2"/></object><object id="999"><metadata key="name" value="Unused A"/></object><object id="1000"><metadata key="name" value="Unused B"/></object></config>"#;
        for deflate in [false, true] {
            let profile = Rc::new(SettingsOwnedTestProfile::default());
            let _guard = SettingsOwnedProfileGuard::install(profile.clone());
            let parsed = crate::parse_model_with_plates(&package(
                &[
                    (ROOT_MODEL_FALLBACK, &root),
                    (MODEL_SETTINGS_PATH, settings),
                ],
                deflate,
            ))
            .unwrap();
            assert_eq!(parsed.model.triangle_count, 12);
            assert_eq!(parsed.model.positions.len(), 108);
            assert_eq!(
                &parsed.model.positions[..9],
                &[0., 0., 0., 10., 10., 0., 10., 0., 0.]
            );
            assert_eq!(parsed.parts.len(), 1);
            assert_eq!(parsed.parts[0].id, "object:1:instance:0");
            assert_eq!(
                (
                    parsed.parts[0].triangle_start,
                    parsed.parts[0].triangle_count
                ),
                (0, 12)
            );
            assert_eq!(parsed.parts[0].name, "Useful");
            assert_eq!(parsed.parts[0].filament_slot, Some(2));
            assert!(parsed.part_warning.is_none());
            assert_eq!(
                (
                    profile.name_copies.get(),
                    profile.object_inserts.get(),
                    profile.part_inserts.get()
                ),
                (1, 1, 0),
                "only the consumed root record needs retained name backing and a map slot"
            );
        }
    }

    #[test]
    fn settings_relevance_owned_part_retention_is_independent_of_outer_object() {
        let resources = format!(
            r#"{}<object id="1"><components><component objectid="2"/></components></object>"#,
            cube_mesh_xml(2)
        );
        let root = model_xml("millimeter", &resources, r#"<item objectid="1"/>"#);
        let settings = r#"<config><object id="1"><metadata key="name" value="Root label"/><metadata key="extruder" value="2"/></object><object id="999"><part id="2"><metadata key="name" value="Useful part"/><metadata key="extruder" value="3"/></part><part id="998"><metadata key="name" value="Unused A"/></part><part id="997"><metadata key="name" value="Unused B"/></part></object></config>"#;
        for deflate in [false, true] {
            let profile = Rc::new(SettingsOwnedTestProfile::default());
            let _guard = SettingsOwnedProfileGuard::install(profile.clone());
            let parsed = crate::parse_model_with_plates(&package(
                &[
                    (ROOT_MODEL_FALLBACK, &root),
                    (MODEL_SETTINGS_PATH, settings),
                ],
                deflate,
            ))
            .unwrap();
            assert_eq!(parsed.model.triangle_count, 12);
            assert_eq!(parsed.model.positions.len(), 108);
            assert_eq!(
                &parsed.model.positions[..9],
                &[0., 0., 0., 10., 10., 0., 10., 0., 0.]
            );
            assert_eq!(parsed.parts.len(), 1);
            assert_eq!(
                parsed.parts[0].id,
                "object:1:instance:0/component:0:object:2"
            );
            assert_eq!(
                (
                    parsed.parts[0].triangle_start,
                    parsed.parts[0].triangle_count
                ),
                (0, 12)
            );
            assert_eq!(parsed.parts[0].name, "Root label · Useful part");
            assert_eq!(parsed.parts[0].filament_slot, Some(3));
            assert!(parsed.part_warning.is_none());
            assert_eq!(
                (
                    profile.name_copies.get(),
                    profile.object_inserts.get(),
                    profile.part_inserts.get()
                ),
                (2, 1, 1),
                "part relevance must not depend on its unrelated outer object record"
            );
        }
    }

    fn settings_owned_fault_fixture() -> (String, &'static str) {
        let resources = format!(
            r#"{}<object id="1"><components><component objectid="2"/></components></object>"#,
            cube_mesh_xml(2)
        );
        let root = model_xml("millimeter", &resources, r#"<item objectid="1"/>"#);
        let settings = r#"<config><object id="1"><metadata key="name" value="Root label"/><metadata key="extruder" value="2"/></object><object id="999"><part id="2"><metadata key="name" value="Useful part"/><metadata key="extruder" value="3"/></part></object></config>"#;
        (root, settings)
    }

    #[test]
    fn settings_relevance_owned_actual_reserve_stops_remain_terminal_before_and_after() {
        for deflate in [false, true] {
            let (root, settings) = settings_owned_fault_fixture();
            let bytes = package(
                &[
                    (ROOT_MODEL_FALLBACK, &root),
                    (MODEL_SETTINGS_PATH, settings),
                ],
                deflate,
            );
            for site in [
                settings::ReserveSite::Name,
                settings::ReserveSite::Objects,
                settings::ReserveSite::Parts,
            ] {
                for after in [false, true] {
                    for failure in PARSER_TEST_FAILURES {
                        let profile = Rc::new(SettingsOwnedTestProfile::default());
                        if after {
                            profile.after_site.set(Some((site, failure)));
                        } else {
                            profile.before_site.set(Some((site, failure)));
                        }
                        profile.finish_failure.set(Some(match failure {
                            ParserTestFailure::Stop(_) => ParserTestFailure::Allocation,
                            ParserTestFailure::Allocation => {
                                ParserTestFailure::Stop(ResourceStop::EnvelopeChanged)
                            }
                        }));
                        let _guard = SettingsOwnedProfileGuard::install(profile.clone());
                        let observer = ZipResourceObserver::new();
                        let mut context = PackageProcessingContext::new(observer.clone());
                        assert_eq!(
                            crate::parse_model_with_plates_in_context(&bytes, &mut context)
                                .map(|_| ()),
                            Err(failure.error())
                        );
                        assert_eq!(
                            observer.check().map_err(StlError::from),
                            Err(failure.error())
                        );
                        assert_eq!(profile.entries.get(), 1);
                        assert_eq!(profile.ordinary_fallbacks.get(), 0);
                        assert!(profile.before_site.get().is_none());
                        assert!(profile.after_site.get().is_none());
                        assert!(profile.finish_failure.get().is_none());
                        let requests = profile.requests.borrow();
                        assert_eq!(requests.last().map(|(actual, _)| *actual), Some(site));
                        assert_eq!(
                            requests.len(),
                            match site {
                                settings::ReserveSite::Name => 1,
                                settings::ReserveSite::Objects => 2,
                                settings::ReserveSite::Parts => 4,
                            }
                        );
                        match site {
                            settings::ReserveSite::Name => assert_eq!(profile.name_copies.get(), 0),
                            settings::ReserveSite::Objects => {
                                assert_eq!(profile.object_inserts.get(), 0)
                            }
                            settings::ReserveSite::Parts => {
                                assert_eq!(profile.part_inserts.get(), 0)
                            }
                        }
                        assert_eq!(
                            context.usage().inflation_bytes,
                            (root.len() + settings.len()) as u64
                        );
                        assert_eq!(context.usage().retained_input_bytes, settings.len());
                    }
                }
            }
        }
    }

    #[test]
    fn settings_relevance_owned_shared_context_usage_is_not_recharged() {
        for deflate in [false, true] {
            let (root, settings) = settings_owned_fault_fixture();
            let bytes = package(
                &[
                    (ROOT_MODEL_FALLBACK, &root),
                    (MODEL_SETTINGS_PATH, settings),
                ],
                deflate,
            );
            let profile = Rc::new(SettingsOwnedTestProfile::default());
            let _guard = SettingsOwnedProfileGuard::install(profile.clone());
            let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
            let parsed = crate::parse_model_with_plates_in_context(&bytes, &mut context).unwrap();
            assert_eq!(parsed.model.triangle_count, 12);
            assert_eq!(parsed.parts[0].name, "Root label · Useful part");
            assert_eq!(parsed.parts[0].filament_slot, Some(3));
            assert_eq!(
                context.usage().inflation_bytes,
                (root.len() + settings.len()) as u64
            );
            assert_eq!(context.usage().retained_input_bytes, settings.len());
            assert_eq!(profile.entries.get(), 1);
            assert_eq!(
                (
                    profile.name_copies.get(),
                    profile.object_inserts.get(),
                    profile.part_inserts.get()
                ),
                (2, 1, 1)
            );
        }
    }

    #[test]
    fn settings_relevance_owned_duplicates_and_irrelevant_records_do_not_reserve_map_slots() {
        let root = model_xml("millimeter", &cube_mesh_xml(1), r#"<item objectid="1"/>"#);
        let settings = r#"<config><object id="1"><metadata key="name" value="Useful"/><metadata key="extruder" value="2"/></object><object id="999"><metadata key="name" value="Unused A"/></object><object id="1"><metadata key="extruder" value="2"/></object><object id="1000"><metadata key="name" value="Unused B"/></object><object id="1"><metadata key="extruder" value="3"/></object></config>"#;
        for deflate in [false, true] {
            let profile = Rc::new(SettingsOwnedTestProfile::default());
            let _guard = SettingsOwnedProfileGuard::install(profile.clone());
            let parsed = crate::parse_model_with_plates(&package(
                &[
                    (ROOT_MODEL_FALLBACK, &root),
                    (MODEL_SETTINGS_PATH, settings),
                ],
                deflate,
            ))
            .unwrap();
            assert_eq!(parsed.model.triangle_count, 12);
            assert_eq!(parsed.parts[0].name, "Useful");
            assert_eq!(parsed.parts[0].filament_slot, None);
            assert_eq!(
                &*profile.requests.borrow(),
                &[
                    (settings::ReserveSite::Name, 6),
                    (settings::ReserveSite::Objects, 1)
                ]
            );
            assert_eq!(
                (
                    profile.name_copies.get(),
                    profile.object_inserts.get(),
                    profile.part_inserts.get()
                ),
                (1, 1, 0)
            );
        }
    }

    #[test]
    fn settings_relevance_owned_ordinary_tail_still_discards_optional_settings() {
        let root = model_xml("millimeter", &cube_mesh_xml(1), r#"<item objectid="1"/>"#);
        let settings = r#"<config><object id="1"><metadata key="name" value="Useful"/><metadata key="extruder" value="2"/></object><object id="999"><part id="999"></object></config>"#;
        for deflate in [false, true] {
            let profile = Rc::new(SettingsOwnedTestProfile::default());
            let _guard = SettingsOwnedProfileGuard::install(profile.clone());
            let observer = ZipResourceObserver::new();
            let mut context = PackageProcessingContext::new(observer.clone());
            let parsed = crate::parse_model_with_plates_in_context(
                &package(
                    &[
                        (ROOT_MODEL_FALLBACK, &root),
                        (MODEL_SETTINGS_PATH, settings),
                    ],
                    deflate,
                ),
                &mut context,
            )
            .unwrap();
            assert_eq!(parsed.model.triangle_count, 12);
            assert_eq!(parsed.parts[0].name, "Object 1");
            assert_eq!(parsed.parts[0].filament_slot, None);
            assert_eq!(profile.ordinary_fallbacks.get(), 1);
            assert_eq!(profile.object_inserts.get(), 1);
            assert!(observer.check().is_ok());
        }
    }

    struct MaterialOwnedProfileGuard(Option<Rc<MaterialOwnedTestProfile>>);
    impl MaterialOwnedProfileGuard {
        fn install(profile: Rc<MaterialOwnedTestProfile>) -> Self {
            Self(MATERIAL_OWNED_TEST_PROFILE.with(|slot| slot.replace(Some(profile))))
        }
    }
    impl Drop for MaterialOwnedProfileGuard {
        fn drop(&mut self) {
            drop(MATERIAL_OWNED_TEST_PROFILE.with(|slot| slot.replace(self.0.take())));
        }
    }

    #[test]
    fn material_owned_duplicate_base_keys_do_not_construct_unretained_values() {
        let resources = format!(
            r##"<basematerials id="5"><base name="First" displaycolor="#112233"/></basematerials><basematerials id="5"><base name="DiscardedSecond" displaycolor="#abcdef"/></basematerials><basematerials id="5"><base name="DiscardedThird" displaycolor="#fedcba"/></basematerials>{}"##,
            cube_mesh_with_object_attrs_xml(1, r#"pid="5" pindex="0""#)
        );
        let model = model_xml("millimeter", &resources, r#"<item objectid="1"/>"#);
        for deflate in [false, true] {
            let profile = Rc::new(MaterialOwnedTestProfile::default());
            let _guard = MaterialOwnedProfileGuard::install(profile.clone());
            let parsed =
                crate::parse_model_with_plates(&package(&[(ROOT_MODEL_FALLBACK, &model)], deflate))
                    .unwrap();
            assert_eq!(parsed.model.triangle_count, 12);
            assert_eq!(parsed.materials.len(), 1);
            let material = &parsed.materials[0];
            assert_eq!(material.id, "3mf:3D/3dmodel.model:5:0");
            assert_eq!(material.name.as_deref(), Some("First"));
            assert_eq!(material.color.as_deref(), Some("#112233"));
            assert_eq!(material.source, MaterialSource::ThreeMfBaseMaterial);
            assert_eq!(
                parsed.parts[0].material_id.as_deref(),
                Some(material.id.as_str())
            );
            assert_eq!((profile.normalized_names.get(), profile.normalized_colors.get()), (1, 1),
                "only the accepted row needs owned normalized field backing; duplicate source rows still validate/advance");
        }
    }

    #[test]
    fn material_owned_base_tail_does_not_construct_values_after_local_1024_clip() {
        let bases = (0..=MAX_STANDARD_MATERIALS)
            .map(|index| format!(r##"<base name="M{index}" displaycolor="#112233"/>"##))
            .collect::<String>();
        let resources = format!(
            r#"<basematerials id="5">{bases}</basematerials>{}"#,
            cube_mesh_with_object_attrs_xml(1, r#"pid="5" pindex="1023""#)
        );
        let model = model_xml("millimeter", &resources, r#"<item objectid="1"/>"#);
        for deflate in [false, true] {
            let profile = Rc::new(MaterialOwnedTestProfile::default());
            let _guard = MaterialOwnedProfileGuard::install(profile.clone());
            let parsed =
                crate::parse_model_with_plates(&package(&[(ROOT_MODEL_FALLBACK, &model)], deflate))
                    .unwrap();
            assert_eq!(parsed.model.triangle_count, 12);
            assert_eq!(parsed.materials.len(), MAX_STANDARD_MATERIALS);
            let last = parsed.materials.last().unwrap();
            assert_eq!(last.id, "3mf:3D/3dmodel.model:5:1023");
            assert_eq!(last.name.as_deref(), Some("M1023"));
            assert_eq!(last.color.as_deref(), Some("#112233"));
            assert_eq!(
                parsed.parts[0].material_id.as_deref(),
                Some(last.id.as_str())
            );
            assert_eq!(
                (
                    profile.normalized_names.get(),
                    profile.normalized_colors.get()
                ),
                (MAX_STANDARD_MATERIALS, MAX_STANDARD_MATERIALS),
                "valid clipped source rows do not need owned normalized values"
            );
        }
    }

    #[test]
    fn material_owned_color_tail_does_not_construct_values_after_local_1024_clip() {
        let colors = r##"<color color="#11223380"/>"##.repeat(MAX_STANDARD_MATERIALS + 1);
        let resources = format!(
            r#"<colorgroup id="9">{colors}</colorgroup>{}"#,
            cube_mesh_with_object_attrs_xml(1, r#"pid="9" pindex="1023""#)
        );
        let model = model_xml("millimeter", &resources, r#"<item objectid="1"/>"#);
        for deflate in [false, true] {
            let profile = Rc::new(MaterialOwnedTestProfile::default());
            let _guard = MaterialOwnedProfileGuard::install(profile.clone());
            let parsed =
                crate::parse_model_with_plates(&package(&[(ROOT_MODEL_FALLBACK, &model)], deflate))
                    .unwrap();
            assert_eq!(parsed.model.triangle_count, 12);
            assert_eq!(parsed.materials.len(), MAX_STANDARD_MATERIALS);
            let last = parsed.materials.last().unwrap();
            assert_eq!(last.id, "3mf:3D/3dmodel.model:9:1023");
            assert!(last.name.is_none());
            assert_eq!(last.color.as_deref(), Some("#112233"));
            assert_eq!(last.source, MaterialSource::ThreeMfColorGroup);
            assert_eq!(
                parsed.parts[0].material_id.as_deref(),
                Some(last.id.as_str())
            );
            assert_eq!(
                (
                    profile.normalized_names.get(),
                    profile.normalized_colors.get()
                ),
                (0, MAX_STANDARD_MATERIALS),
                "color-only clipped rows do not need a normalized String result"
            );
        }
    }
    #[test]
    fn material_owned_actual_reserve_sites_fail_terminally_before_commit() {
        use materials::ReserveSite;
        let resources = format!(
            r##"<basematerials id="5"><base name="First" displaycolor="#112233"/></basematerials>{}"##,
            cube_mesh_with_object_attrs_xml(1, r#"pid="5" pindex="0""#)
        );
        let model = model_xml("millimeter", &resources, r#"<item objectid="1"/>"#);
        let bytes = package(&[(ROOT_MODEL_FALLBACK, &model)], false);
        for site in [
            ReserveSite::LocalRows,
            ReserveSite::Lookup,
            ReserveSite::Id,
            ReserveSite::Name,
            ReserveSite::Color,
            ReserveSite::GlobalRows,
            ReserveSite::GlobalId,
            ReserveSite::GlobalName,
            ReserveSite::GlobalColor,
        ] {
            let profile = Rc::new(MaterialOwnedTestProfile::default());
            profile.fail_site.set(Some(site));
            let _guard = MaterialOwnedProfileGuard::install(profile.clone());
            let observer = ZipResourceObserver::new();
            let mut context = PackageProcessingContext::new(observer.clone());
            assert_eq!(
                crate::parse_model_with_plates_in_context(&bytes, &mut context),
                Err(StlError::AllocationFailed)
            );
            assert!(
                profile.fail_site.get().is_none(),
                "actual allocation site reached"
            );
            assert!(matches!(
                observer.check(),
                Err(ZipOpenError::AllocationFailed)
            ));
            assert_eq!(profile.global_commits.get(), 0);
            if matches!(
                site,
                ReserveSite::LocalRows
                    | ReserveSite::Lookup
                    | ReserveSite::Id
                    | ReserveSite::Name
                    | ReserveSite::Color
            ) {
                assert_eq!(profile.local_commits.get(), 0);
            }
            let requests = profile.requests.borrow();
            assert_eq!(requests.last().map(|(site, _)| *site), Some(site));
        }
    }

    #[test]
    fn material_owned_post_reserve_stops_preserve_exact_first_cause() {
        use materials::ReserveSite;
        let resources = format!(
            r##"<basematerials id="5"><base name="First" displaycolor="#112233"/></basematerials>{}"##,
            cube_mesh_with_object_attrs_xml(1, r#"pid="5" pindex="0""#)
        );
        let model = model_xml("millimeter", &resources, r#"<item objectid="1"/>"#);
        let bytes = package(&[(ROOT_MODEL_FALLBACK, &model)], true);
        for site in [
            ReserveSite::LocalRows,
            ReserveSite::Lookup,
            ReserveSite::Id,
            ReserveSite::Name,
            ReserveSite::Color,
            ReserveSite::GlobalRows,
            ReserveSite::GlobalId,
            ReserveSite::GlobalName,
            ReserveSite::GlobalColor,
        ] {
            for failure in PARSER_TEST_FAILURES {
                let profile = Rc::new(MaterialOwnedTestProfile::default());
                profile.stop_site.set(Some((site, failure)));
                let _guard = MaterialOwnedProfileGuard::install(profile.clone());
                let observer = ZipResourceObserver::new();
                let mut context = PackageProcessingContext::new(observer.clone());
                assert_eq!(
                    crate::parse_model_with_plates_in_context(&bytes, &mut context),
                    Err(failure.error())
                );
                assert!(
                    profile.stop_site.get().is_none(),
                    "actual successful reserve reached"
                );
                assert_eq!(profile.global_commits.get(), 0);
                assert_eq!(
                    StlError::from(observer.check().unwrap_err()),
                    failure.error()
                );
                let _ = observer.stop_io(ResourceStop::EnvelopeChanged);
                assert_eq!(
                    StlError::from(observer.check().unwrap_err()),
                    failure.error()
                );
                let requests = profile.requests.borrow();
                assert_eq!(requests.last().map(|(site, _)| *site), Some(site));
            }
        }
    }

    #[test]
    fn material_owned_duplicate_and_clipped_rows_do_not_request_final_fields() {
        use materials::ReserveSite;
        let bases = (0..=MAX_STANDARD_MATERIALS)
            .map(|index| format!(r##"<base name="M{index}" displaycolor="#112233"/>"##))
            .collect::<String>();
        let resources = format!(
            r##"<basematerials id="5">{bases}</basematerials><basematerials id="5"><base name="Duplicate" displaycolor="#abcdef"/></basematerials>{}"##,
            cube_mesh_with_object_attrs_xml(1, r#"pid="5" pindex="0""#)
        );
        let model = model_xml("millimeter", &resources, r#"<item objectid="1"/>"#);
        let profile = Rc::new(MaterialOwnedTestProfile::default());
        let _guard = MaterialOwnedProfileGuard::install(profile.clone());
        let parsed =
            crate::parse_model_with_plates(&package(&[(ROOT_MODEL_FALLBACK, &model)], true))
                .unwrap();
        assert_eq!(parsed.materials.len(), MAX_STANDARD_MATERIALS);
        assert_eq!(profile.local_commits.get(), MAX_STANDARD_MATERIALS);
        assert_eq!(profile.global_commits.get(), MAX_STANDARD_MATERIALS);
        let requests = profile.requests.borrow();
        for site in [
            ReserveSite::Id,
            ReserveSite::Name,
            ReserveSite::Color,
            ReserveSite::GlobalId,
            ReserveSite::GlobalName,
            ReserveSite::GlobalColor,
        ] {
            assert_eq!(
                requests.iter().filter(|(s, _)| *s == site).count(),
                MAX_STANDARD_MATERIALS
            );
        }
        for &(site, target) in requests.iter() {
            match site {
                ReserveSite::LocalRows | ReserveSite::GlobalRows => {
                    assert!((1..=MAX_STANDARD_MATERIALS).contains(&target))
                }
                ReserveSite::Lookup => assert_eq!(target, 1),
                ReserveSite::Color | ReserveSite::GlobalColor => assert_eq!(target, 7),
                ReserveSite::Name | ReserveSite::GlobalName => assert!(target <= 1024),
                ReserveSite::Id | ReserveSite::GlobalId => {
                    assert!(target <= 4 + ROOT_MODEL_FALLBACK.len() + 2 + 10 + 10)
                }
            }
        }
    }

    #[test]
    fn material_owned_full_global_display_still_constructs_child_local_rows_only() {
        use materials::ReserveSite;
        let bases = (0..MAX_STANDARD_MATERIALS)
            .map(|index| format!(r##"<base name="R{index}" displaycolor="#112233"/>"##))
            .collect::<String>();
        let root = model_xml(
            "millimeter",
            &format!(
                r#"<basematerials id="5">{bases}</basematerials><object id="10"><components><component objectid="2" path="3D/child.model"/></components></object>"#
            ),
            r#"<item objectid="10"/>"#,
        );
        let child = model_xml(
            "millimeter",
            &format!(
                r##"<basematerials id="6"><base name="Child" displaycolor="#abcdef"/></basematerials>{}"##,
                cube_mesh_with_object_attrs_xml(2, r#"pid="6" pindex="0""#)
            ),
            "",
        );
        let profile = Rc::new(MaterialOwnedTestProfile::default());
        let _guard = MaterialOwnedProfileGuard::install(profile.clone());
        let parsed = crate::parse_model_with_plates(&package(
            &[(ROOT_MODEL_FALLBACK, &root), ("3D/child.model", &child)],
            true,
        ))
        .unwrap();
        assert_eq!(parsed.model.triangle_count, 12);
        assert_eq!(parsed.materials.len(), MAX_STANDARD_MATERIALS);
        assert_eq!(profile.local_commits.get(), MAX_STANDARD_MATERIALS + 1);
        assert_eq!(profile.global_commits.get(), MAX_STANDARD_MATERIALS);
        assert!(parsed.parts.iter().all(|part| part.material_id.is_none()));
        let requests = profile.requests.borrow();
        assert_eq!(
            requests
                .iter()
                .filter(|(site, _)| *site == ReserveSite::Id)
                .count(),
            MAX_STANDARD_MATERIALS + 1
        );
        assert_eq!(
            requests
                .iter()
                .filter(|(site, _)| *site == ReserveSite::GlobalId)
                .count(),
            MAX_STANDARD_MATERIALS
        );
    }

    #[test]
    fn material_owned_deferred_child_membership_failure_never_enters_fallback() {
        use materials::ReserveSite;
        let root = model_xml(
            "millimeter",
            r#"<object id="10"><components><component objectid="2" path="3D/child.model"/></components></object>"#,
            r#"<item objectid="10"/>"#,
        );
        let child = model_xml(
            "millimeter",
            &format!(
                r##"<basematerials id="6"><base name="Child" displaycolor="#abcdef"/></basematerials>{}"##,
                cube_mesh_with_object_attrs_xml(2, r#"pid="6" pindex="0""#)
            ),
            "",
        );
        let settings = plate_settings(&plate_xml(1, "Empty", &[]));
        let bytes = package(
            &[
                (ROOT_MODEL_FALLBACK, &root),
                ("3D/child.model", &child),
                (PLATE_SETTINGS_PATH, &settings),
            ],
            true,
        );
        for failure in PARSER_TEST_FAILURES {
            let profile = Rc::new(MaterialOwnedTestProfile::default());
            profile.stop_site.set(Some((ReserveSite::Name, failure)));
            let _guard = MaterialOwnedProfileGuard::install(profile.clone());
            let derived = Rc::new(DerivedMapsTestProfile::default());
            let _derived_guard = DerivedMapsProfileGuard::install(derived.clone());
            let observer = ZipResourceObserver::new();
            let probe = Rc::new(ParserTestProbe {
                member: "do-not-inject",
                failure: ParserTestFailure::Allocation,
                state: std::cell::RefCell::new(ParserTestState::default()),
                guard_observer: Some(observer.clone()),
            });
            let _parser_guard = ParserProbeGuard::install(probe.clone());
            let mut context = PackageProcessingContext::new(observer.clone());
            assert_eq!(
                crate::parse_model_with_plates_in_context(&bytes, &mut context),
                Err(failure.error())
            );
            assert!(profile.stop_site.get().is_none());
            assert!(probe.state.borrow().root_cached);
            assert_eq!(derived.plate_parser_entries.get(), 1);
            assert_eq!(derived.fallback_entries.get(), 0);
            assert_eq!(profile.local_commits.get(), 0);
            assert_eq!(profile.global_commits.get(), 0);
            assert_eq!(
                StlError::from(observer.check().unwrap_err()),
                failure.error()
            );
        }
    }
}
