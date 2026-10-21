//! Retained settings follow the existing root/direct-component consumers.
//! Scanner state and SDK attribute scratch remain independent of row retention.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReserveSite {
    Name,
    Objects,
    Parts,
}

pub(super) enum SettingsError {
    Metadata(String),
    Source(StlError),
}
impl From<StlError> for SettingsError {
    fn from(error: StlError) -> Self {
        match error {
            StlError::ResourceStopped(_) | StlError::AllocationFailed | StlError::FundingBusy => {
                Self::Source(error)
            }
            ordinary => Self::Metadata(ordinary.to_string()),
        }
    }
}
impl From<ZipOpenError> for SettingsError {
    fn from(error: ZipOpenError) -> Self {
        StlError::from(error).into()
    }
}
impl std::fmt::Display for SettingsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Metadata(detail) => formatter.write_str(detail),
            Self::Source(error) => std::fmt::Display::fmt(error, formatter),
        }
    }
}

struct Control<'a> {
    observer: &'a ZipResourceObserver,
    #[cfg(test)]
    probe: Option<&'a SettingsOwnedTestProfile>,
}
impl Control<'_> {
    fn allocation(&self) -> SettingsError {
        let _ = self.observer.allocation_failed_io();
        self.observer
            .check()
            .expect_err("allocation is latched")
            .into()
    }
    fn input(&self) -> SettingsError {
        let _ = self.observer.stop_io(ResourceStop::InputLimitExceeded);
        self.observer
            .check()
            .expect_err("input stop is latched")
            .into()
    }
    #[cfg(test)]
    fn inject(&self, failure: ParserTestFailure) {
        match failure {
            ParserTestFailure::Stop(stop) => {
                let _ = self.observer.stop_io(stop);
            }
            ParserTestFailure::Allocation => {
                let _ = self.observer.allocation_failed_io();
            }
            ParserTestFailure::Funding => {
                let _ = self.observer.funding_busy_io();
            }
        }
    }
    fn before(&self, _site: ReserveSite, _amount: usize) -> Result<(), SettingsError> {
        self.observer.check()?;
        #[cfg(test)]
        if let Some(probe) = self.probe {
            probe.requests.borrow_mut().push((_site, _amount));
            if let Some((site, failure)) = probe.before_site.get() {
                if site == _site {
                    probe.before_site.set(None);
                    self.inject(failure);
                }
            }
        }
        self.observer.check().map_err(Into::into)
    }
    fn after(&self, _site: ReserveSite) -> Result<(), SettingsError> {
        #[cfg(test)]
        if let Some(probe) = self.probe {
            if let Some((site, failure)) = probe.after_site.get() {
                if site == _site {
                    probe.after_site.set(None);
                    self.inject(failure);
                }
            }
        }
        self.observer.check().map_err(Into::into)
    }
    fn name(&self, value: &str) -> Result<Option<String>, SettingsError> {
        let Some(value) = normalized_name(value) else {
            return Ok(None);
        };
        if value.len() > isize::MAX as usize {
            return Err(self.input());
        }
        let mut name = String::new();
        self.before(ReserveSite::Name, value.len())?;
        name.try_reserve_exact(value.len())
            .map_err(|_| self.allocation())?;
        self.after(ReserveSite::Name)?;
        name.push_str(value);
        #[cfg(test)]
        if let Some(probe) = self.probe {
            probe.name_copies.set(probe.name_copies.get() + 1);
        }
        Ok(Some(name))
    }
    fn update(
        &self,
        record: Option<&mut SettingsRecord>,
        key: &str,
        value: &str,
    ) -> Result<(), SettingsError> {
        self.observer.check()?;
        let Some(record) = record else {
            // Keep validation of values the scanner already selected, without retained backing.
            match key {
                "name" => {
                    let _ = normalized_name(value);
                }
                "extruder" => {
                    let _ = value.trim().parse::<u32>();
                }
                _ => {}
            }
            return Ok(());
        };
        match key {
            "name" => {
                if record.name_invalid || record.name.is_some() {
                    record.name = None;
                    record.name_invalid = true;
                } else if let Some(name) = self.name(value)? {
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
        Ok(())
    }
    fn merge(
        &self,
        records: &mut HashMap<u32, SettingsRecord>,
        open: OpenRecord,
        site: ReserveSite,
    ) -> Result<(), SettingsError> {
        self.observer.check()?;
        let OpenRecord { id, record } = open;
        let Some(record) = record else {
            return Ok(());
        };
        if let Some(existing) = records.get_mut(&id) {
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
        } else {
            records.len().checked_add(1).ok_or_else(|| self.input())?;
            if records.len() == records.capacity() {
                self.before(site, 1)?;
                records.try_reserve(1).map_err(|_| self.allocation())?;
                self.after(site)?;
            }
            self.observer.check()?;
            records.insert(id, record);
            #[cfg(test)]
            if let Some(probe) = self.probe {
                let count = if site == ReserveSite::Parts {
                    &probe.part_inserts
                } else {
                    &probe.object_inserts
                };
                count.set(count.get() + 1);
            }
        }
        Ok(())
    }
}

fn normalized_name(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let end = value
        .char_indices()
        .nth(256)
        .map_or(value.len(), |(index, _)| index);
    Some(&value[..end])
}
struct OpenRecord {
    id: u32,
    record: Option<SettingsRecord>,
}

pub(super) fn parse_slicer_metadata(
    text: &str,
    observer: &ZipResourceObserver,
    objects: &HashMap<u32, Object>,
    component_paths: &HashMap<u32, HashSet<SharedPath>>,
    #[cfg(test)] probe: Option<&SettingsOwnedTestProfile>,
) -> Result<SlicerMetadata, SettingsError> {
    observer.check()?;
    #[cfg(test)]
    if let Some(probe) = probe {
        probe.entries.set(probe.entries.get() + 1);
    }
    let control = Control {
        observer,
        #[cfg(test)]
        probe,
    };
    let result = parse_inner(text, objects, component_paths, &control);
    observer.finish(result)
}

fn parse_inner(
    text: &str,
    objects: &HashMap<u32, Object>,
    component_paths: &HashMap<u32, HashSet<SharedPath>>,
    control: &Control<'_>,
) -> Result<SlicerMetadata, SettingsError> {
    let mut xml = XmlEventReader::new(text.as_bytes(), control.observer.clone())?;
    xml.set_trim_text(true);
    let mut metadata = SlicerMetadata::default();
    let mut object: Option<OpenRecord> = None;
    let mut part: Option<OpenRecord> = None;
    let mut ignored_part_depth = 0u32;
    loop {
        let event = match xml.next_event() {
            Ok(event) => event,
            Err(error) => {
                control.observer.check()?;
                return Err(SettingsError::Metadata(format!(
                    "invalid slicer metadata XML ({error})"
                )));
            }
        };
        match event {
            Event::Start(e) => match e.local_name().as_ref() {
                b"object" if object.is_none() && part.is_none() && ignored_part_depth == 0 => {
                    let Some(id) = attr_str(&e, b"id")?.and_then(|v| v.trim().parse().ok()) else {
                        continue;
                    };
                    object = Some(OpenRecord {
                        id,
                        record: objects.contains_key(&id).then(SettingsRecord::default),
                    });
                }
                b"part" if object.is_some() => {
                    if ignored_part_depth > 0 {
                        ignored_part_depth = ignored_part_depth.saturating_add(1);
                    } else if part.is_some() {
                        ignored_part_depth = 1;
                    } else if let Some(id) =
                        attr_str(&e, b"id")?.and_then(|v| v.trim().parse().ok())
                    {
                        part = Some(OpenRecord {
                            id,
                            record: component_paths
                                .contains_key(&id)
                                .then(SettingsRecord::default),
                        });
                    } else {
                        ignored_part_depth = 1;
                    }
                }
                b"metadata" if ignored_part_depth == 0 => {
                    update_node(&e, &mut object, &mut part, control)?
                }
                _ => {}
            },
            Event::Empty(e)
                if e.local_name().as_ref() == b"metadata" && ignored_part_depth == 0 =>
            {
                update_node(&e, &mut object, &mut part, control)?;
            }
            Event::End(e) => match e.local_name().as_ref() {
                b"part" => {
                    if ignored_part_depth > 0 {
                        ignored_part_depth -= 1;
                    } else if let Some(open) = part.take() {
                        control.merge(&mut metadata.parts, open, ReserveSite::Parts)?;
                    }
                }
                b"object" => {
                    if part.is_some() || ignored_part_depth > 0 {
                        return Err(SettingsError::Metadata(
                            "unclosed slicer metadata part".into(),
                        ));
                    }
                    if let Some(open) = object.take() {
                        control.merge(&mut metadata.objects, open, ReserveSite::Objects)?;
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
fn update_node(
    node: &BytesStart<'_>,
    object: &mut Option<OpenRecord>,
    part: &mut Option<OpenRecord>,
    control: &Control<'_>,
) -> Result<(), SettingsError> {
    let key = attr_str(node, b"key")?;
    let value = attr_str(node, b"value")?;
    if let (Some(key), Some(value)) = (key, value) {
        if let Some(open) = part.as_mut() {
            control.update(open.record.as_mut(), &key, &value)?;
        } else if let Some(open) = object.as_mut() {
            control.update(open.record.as_mut(), &key, &value)?;
        }
    }
    Ok(())
}
