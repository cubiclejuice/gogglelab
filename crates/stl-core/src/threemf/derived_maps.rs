//! Derived indices and diagnostic sets; no source-record or owned-byte quota.
use super::*;
use std::hash::Hash;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReserveSite {
    Counts,
    Occurrences,
    Items,
    Claimed,
    Plates,
    Indices,
    DiagnosticMap,
    DiagnosticSet,
    Path,
    OutputPlates,
}

struct Control<'a> {
    observer: &'a ZipResourceObserver,
    #[cfg(test)]
    probe: Option<&'a DerivedMapsTestProfile>,
}
impl Control<'_> {
    fn before(&self, _site: ReserveSite) -> Result<(), StlError> {
        self.observer.check()?;
        #[cfg(test)]
        if let Some(probe) = self.probe {
            probe.reserves.set(probe.reserves.get() + 1);
            if probe.fail_site.get() == Some(_site) {
                probe.fail_site.set(None);
                return Err(self.allocation());
            }
        }
        Ok(())
    }
    fn after(&self, _site: ReserveSite) -> Result<(), StlError> {
        #[cfg(test)]
        if let Some(probe) = self.probe {
            if let Some((site, failure)) = probe.stop_site.get() {
                if site == _site {
                    probe.stop_site.set(None);
                    latch(self.observer, failure);
                }
            }
        }
        self.observer.check().map_err(Into::into)
    }
    fn input(&self) -> StlError {
        let _ = self.observer.stop_io(ResourceStop::InputLimitExceeded);
        self.observer
            .check()
            .expect_err("input stop is latched")
            .into()
    }
    fn allocation(&self) -> StlError {
        let _ = self.observer.allocation_failed_io();
        self.observer
            .check()
            .expect_err("allocation is latched")
            .into()
    }
    fn map<K: Eq + Hash, V>(
        &self,
        map: &mut HashMap<K, V>,
        key: &K,
        site: ReserveSite,
    ) -> Result<(), StlError> {
        if map.contains_key(key) || map.len() < map.capacity() {
            return Ok(());
        }
        self.before(site)?;
        map.try_reserve(1).map_err(|_| self.allocation())?;
        self.after(site)
    }
    fn set<T: Eq + Hash>(&self, set: &mut HashSet<T>, site: ReserveSite) -> Result<(), StlError> {
        if set.len() < set.capacity() {
            return Ok(());
        }
        self.before(site)?;
        set.try_reserve(1).map_err(|_| self.allocation())?;
        self.after(site)
    }
    fn text(&self, text: &str) -> Result<String, StlError> {
        if text.len() > isize::MAX as usize {
            return input_stop(self.observer);
        }
        let mut owned = String::new();
        if !text.is_empty() {
            self.before(ReserveSite::Path)?;
            owned
                .try_reserve_exact(text.len())
                .map_err(|_| self.allocation())?;
            self.after(ReserveSite::Path)?;
            owned.push_str(text);
        }
        Ok(owned)
    }
}

#[cfg(test)]
fn latch(observer: &ZipResourceObserver, failure: ParserTestFailure) {
    match failure {
        ParserTestFailure::Stop(stop) => {
            let _ = observer.stop_io(stop);
        }
        ParserTestFailure::Allocation => {
            let _ = observer.allocation_failed_io();
        }
    }
}

pub(super) fn reserve_vec<T>(
    output: &mut Vec<T>,
    additional: usize,
    ceiling: usize,
    observer: &ZipResourceObserver,
    site: ReserveSite,
    #[cfg(test)] probe: Option<&DerivedMapsTestProfile>,
) -> Result<(), StlError> {
    let control = Control {
        observer,
        #[cfg(test)]
        probe,
    };
    let Some(needed) = output
        .len()
        .checked_add(additional)
        .filter(|n| *n <= ceiling)
    else {
        return input_stop(observer);
    };
    if output.capacity() >= needed {
        return Ok(());
    }
    let target = needed
        .max(output.capacity().checked_mul(2).unwrap_or(ceiling))
        .min(ceiling);
    if std::mem::size_of::<T>() != 0 && target > isize::MAX as usize / std::mem::size_of::<T>() {
        return input_stop(observer);
    }
    control.before(site)?;
    output
        .try_reserve_exact(target - output.len())
        .map_err(|_| control.allocation())?;
    control.after(site)
}

pub(super) struct BuildIndex {
    pub(super) occurrences: Vec<u32>,
    items: HashMap<(u32, u32), usize>,
}
impl BuildIndex {
    pub(super) fn new(
        build: &[BuildItem],
        observer: &ZipResourceObserver,
        #[cfg(test)] probe: Option<&DerivedMapsTestProfile>,
    ) -> Result<Self, StlError> {
        observer.check()?;
        let control = Control {
            observer,
            #[cfg(test)]
            probe,
        };
        let mut counts: HashMap<u32, u32> = HashMap::new();
        let mut occurrences = Vec::new();
        let mut items = HashMap::new();
        for (index, item) in build.iter().enumerate() {
            observer.check()?;
            let occurrence = counts.get(&item.object_id).copied().unwrap_or(0);
            let next = occurrence
                .checked_add(1)
                .ok_or_else(|| malformed("too many instances for one object"))?;
            control.map(&mut counts, &item.object_id, ReserveSite::Counts)?;
            control.map(
                &mut items,
                &(item.object_id, occurrence),
                ReserveSite::Items,
            )?;
            reserve_vec(
                &mut occurrences,
                1,
                build.len(),
                observer,
                ReserveSite::Occurrences,
                #[cfg(test)]
                probe,
            )?;
            counts.insert(item.object_id, next);
            items.insert((item.object_id, occurrence), index);
            occurrences.push(occurrence);
        }
        Ok(Self { occurrences, items })
    }
}

pub(super) struct ResolvedPlate {
    pub(super) name: String,
    pub(super) items: Vec<usize>,
}
#[derive(Default)]
struct OpenResolvedPlate {
    slicer_id: Option<u32>,
    name: Option<String>,
    items: Vec<usize>,
}
fn source(error: impl Into<StlError>) -> PlateResolutionError {
    PlateResolutionError::Source(error.into())
}
fn attr(
    node: &BytesStart<'_>,
    key: &[u8],
    observer: &ZipResourceObserver,
) -> Result<Option<String>, PlateResolutionError> {
    let result = attr_str(node, key);
    observer.check().map_err(source)?;
    result.map_err(|error| PlateResolutionError::Metadata(error.to_string()))
}

pub(super) fn parse_and_resolve_plates(
    text: &str,
    root_path: &str,
    root: &ModelPart,
    index: &BuildIndex,
    pkg: &mut Package<'_, '_>,
) -> Result<Vec<ResolvedPlate>, PlateResolutionError> {
    let observer = pkg.observer.clone();
    #[cfg(test)]
    let probe = pkg.derived_maps_test.clone();
    let control = Control {
        observer: &observer,
        #[cfg(test)]
        probe: probe.as_deref(),
    };
    observer.check().map_err(source)?;
    #[cfg(test)]
    if let Some(probe) = &probe {
        probe
            .plate_parser_entries
            .set(probe.plate_parser_entries.get() + 1);
    }
    let result = parse_inner(text, root_path, root, index, pkg, &control);
    observer.check().map_err(source)?;
    result
}

fn parse_inner(
    text: &str,
    root_path: &str,
    root: &ModelPart,
    index: &BuildIndex,
    pkg: &mut Package<'_, '_>,
    control: &Control<'_>,
) -> Result<Vec<ResolvedPlate>, PlateResolutionError> {
    let observer = control.observer;
    let reader = XmlEventReader::new(text.as_bytes(), observer.clone());
    observer.check().map_err(source)?;
    let mut xml = reader.map_err(|error| error.to_string())?;
    xml.set_trim_text(true);
    let mut plates: Vec<ResolvedPlate> = Vec::new();
    let mut plate: Option<OpenResolvedPlate> = None;
    let mut instance: Option<OpenInstance> = None;
    let mut completed = 0usize;
    let mut ids = [0u32; MAX_VIEWER_PLATES];
    let mut duplicate_ids = false;
    let mut membership_error: Option<String> = None;
    let mut claimed = HashSet::new();
    let mut retained = 0usize;
    loop {
        let event = xml.next_event();
        observer.check().map_err(source)?;
        #[cfg(test)]
        if membership_error.is_some() {
            if let Some(probe) = control.probe {
                probe
                    .invalid_tail_events
                    .set(probe.invalid_tail_events.get() + 1);
                if probe.invalid_stop_at.get() == 0
                    || probe.invalid_tail_events.get() == probe.invalid_stop_at.get()
                {
                    if let Some(failure) = probe.after_invalid.take() {
                        latch(observer, failure);
                    }
                }
                observer.check().map_err(source)?;
            }
        }
        let event = event.map_err(|error| format!("invalid plate metadata XML ({error})"))?;
        match event {
            Event::Start(ref node) | Event::Empty(ref node) => {
                let empty = matches!(&event, Event::Empty(_));
                match node.local_name().as_ref() {
                    b"plate" => {
                        if plate.is_some() {
                            return Err("nested plate metadata".into());
                        }
                        plate = Some(OpenResolvedPlate::default());
                        if empty {
                            return Err("plate metadata has no id or name".into());
                        }
                    }
                    b"model_instance" if plate.is_some() => {
                        if instance.is_some() {
                            return Err("nested model instance metadata".into());
                        }
                        instance = Some(OpenInstance::default());
                        if empty {
                            return Err("model instance metadata is empty".into());
                        }
                    }
                    b"metadata" if plate.is_some() => {
                        let key = attr(node, b"key", observer)?;
                        let value = attr(node, b"value", observer)?;
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
            Event::End(ref node) => match node.local_name().as_ref() {
                b"model_instance" if plate.is_some() => {
                    let open = instance
                        .take()
                        .ok_or("unbalanced model instance metadata")?;
                    let object_id = open.object_id.ok_or("model instance has no object_id")?;
                    let instance_id = open
                        .instance_id
                        .ok_or("model instance has no instance_id")?;
                    if membership_error.is_none() {
                        let error = match index.items.get(&(object_id, instance_id)).copied() {
                            None => Some(format!("plate references unknown object {object_id} instance {instance_id}")),
                            Some(item_index) if claimed.contains(&item_index) => Some(format!("object {object_id} instance {instance_id} is assigned more than once")),
                            Some(item_index) => {
                                let current = plate.as_mut().expect("matched open plate");
                                let ceiling = current.items.len().checked_add(root.build.len().checked_sub(retained).ok_or_else(|| source(control.input()))?).ok_or_else(|| source(control.input()))?;
                                control.set(&mut claimed, ReserveSite::Claimed).map_err(source)?;
                                reserve_vec(&mut current.items, 1, ceiling, observer, ReserveSite::Indices,
                                    #[cfg(test)] control.probe).map_err(source)?;
                                claimed.insert(item_index);
                                current.items.push(item_index);
                                retained += 1;
                                #[cfg(test)] if let Some(probe) = control.probe {
                                    probe.retained_assignments.set(retained);
                                    probe.assignment_high_water.set(probe.assignment_high_water.get().max(retained));
                                }
                                None
                            }
                        };
                        if let Some(error) = error {
                            membership_error = Some(error);
                            claimed = HashSet::new();
                            plates = Vec::new();
                            if let Some(open) = &mut plate {
                                open.items = Vec::new();
                            }
                            #[cfg(test)]
                            if let Some(probe) = control.probe {
                                probe.retained_assignments.set(0);
                                probe
                                    .membership_invalid
                                    .set(probe.membership_invalid.get() + 1);
                            }
                        }
                    }
                }
                b"plate" => {
                    if instance.is_some() {
                        return Err("unclosed model instance metadata".into());
                    }
                    let open = plate.take().ok_or("unbalanced plate metadata")?;
                    if completed >= MAX_VIEWER_PLATES {
                        return Err(format!("more than {MAX_VIEWER_PLATES} plates").into());
                    }
                    let id = open.slicer_id.ok_or("plate has no plater_id")?;
                    duplicate_ids |= ids[..completed].contains(&id);
                    ids[completed] = id;
                    completed += 1;
                    if membership_error.is_none() {
                        let name = open
                            .name
                            .filter(|name| !name.is_empty())
                            .unwrap_or_else(|| format!("Plate {completed}"));
                        reserve_vec(
                            &mut plates,
                            1,
                            MAX_VIEWER_PLATES,
                            observer,
                            ReserveSite::Plates,
                            #[cfg(test)]
                            control.probe,
                        )
                        .map_err(source)?;
                        plates.push(ResolvedPlate {
                            name,
                            items: open.items,
                        });
                    }
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
    if completed == 0 {
        return Err("plate metadata contains no plates".into());
    }
    if duplicate_ids {
        return Err("duplicated plate plater_id".into());
    }
    if let Some(error) = membership_error {
        return Err(error.into());
    }
    for (item_index, item) in root.build.iter().enumerate() {
        if item.printable
            && !claimed.contains(&item_index)
            && object_has_geometry(
                pkg,
                item.path.as_deref().unwrap_or(root_path),
                item.object_id,
                0,
            )
            .map_err(PlateResolutionError::Source)?
        {
            return Err("plate metadata does not assign every printable build item".into());
        }
    }
    Ok(plates)
}

pub(super) fn diagnostic_paths(
    pkg: &mut Package<'_, '_>,
    root: &ModelPart,
    root_path: &str,
) -> Result<HashMap<u32, HashSet<String>>, StlError> {
    let root_path = Package::entry_name(root_path);
    let observer = pkg.observer.clone();
    #[cfg(test)]
    let probe = pkg.derived_maps_test.clone();
    let control = Control {
        observer: &observer,
        #[cfg(test)]
        probe: probe.as_deref(),
    };
    let mut paths: HashMap<u32, HashSet<String>> = HashMap::new();
    for item in &root.build {
        pkg.charge_component_work()?;
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
            pkg.charge_component_work()?;
            let path = Package::entry_name(component.path.as_deref().unwrap_or(item_path));
            if paths
                .get(&component.object_id)
                .is_some_and(|set| set.contains(path))
            {
                continue;
            }
            control.map(&mut paths, &component.object_id, ReserveSite::DiagnosticMap)?;
            let set = paths.entry(component.object_id).or_default();
            control.set(set, ReserveSite::DiagnosticSet)?;
            let owned = control.text(path)?;
            #[cfg(test)]
            if let Some(probe) = &probe {
                probe
                    .diagnostic_path_copies
                    .set(probe.diagnostic_path_copies.get() + 1);
            }
            set.insert(owned);
        }
    }
    #[cfg(test)]
    if let Some(probe) = &probe {
        probe
            .diagnostic_unique_pairs
            .set(paths.values().map(HashSet::len).sum());
    }
    observer.check()?;
    Ok(paths)
}
