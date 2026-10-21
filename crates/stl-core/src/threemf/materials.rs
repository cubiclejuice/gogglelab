//! Existing local/display material policies with controlled retained-field copies.
//! SDK attribute normalization may own bounded scratch; it is not allocator recovery.
use super::*;
use std::borrow::Cow;
use std::fmt::Write as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReserveSite {
    LocalRows,
    Lookup,
    Id,
    Name,
    Color,
    GlobalRows,
    GlobalId,
    GlobalName,
    GlobalColor,
}

struct Control<'a> {
    observer: &'a ZipResourceObserver,
    #[cfg(test)]
    probe: Option<&'a MaterialOwnedTestProfile>,
}
impl Control<'_> {
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
            .expect_err("allocation stop is latched")
            .into()
    }
    fn before(&self, _site: ReserveSite, _target: usize) -> Result<(), StlError> {
        self.observer.check()?;
        #[cfg(test)]
        if let Some(probe) = self.probe {
            probe.requests.borrow_mut().push((_site, _target));
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
            }
        }
        self.observer.check().map_err(Into::into)
    }
    fn rows(&self, output: &mut Vec<ParsedMaterial>, site: ReserveSite) -> Result<(), StlError> {
        let needed = output
            .len()
            .checked_add(1)
            .filter(|needed| *needed <= MAX_STANDARD_MATERIALS)
            .ok_or_else(|| self.input())?;
        if output.capacity() >= needed {
            return Ok(());
        }
        let target = needed
            .max(
                output
                    .capacity()
                    .checked_mul(2)
                    .unwrap_or(MAX_STANDARD_MATERIALS),
            )
            .min(MAX_STANDARD_MATERIALS);
        self.before(site, target)?;
        output
            .try_reserve_exact(target - output.len())
            .map_err(|_| self.allocation())?;
        self.after(site)
    }
    fn lookup(&self, map: &mut HashMap<(u32, u32), usize>) -> Result<(), StlError> {
        if map.len() < map.capacity() {
            return Ok(());
        }
        self.before(ReserveSite::Lookup, 1)?;
        map.try_reserve(1).map_err(|_| self.allocation())?;
        self.after(ReserveSite::Lookup)
    }
    fn backing(&self, length: usize, site: ReserveSite) -> Result<String, StlError> {
        if length > isize::MAX as usize {
            return Err(self.input());
        }
        let mut text = String::new();
        if length != 0 {
            self.before(site, length)?;
            text.try_reserve_exact(length)
                .map_err(|_| self.allocation())?;
            self.after(site)?;
        }
        Ok(text)
    }
    fn text(&self, value: &str, site: ReserveSite) -> Result<String, StlError> {
        let mut text = self.backing(value.len(), site)?;
        text.push_str(value);
        #[cfg(test)]
        if let Some(probe) = self.probe {
            match site {
                ReserveSite::Name => probe.normalized_names.set(probe.normalized_names.get() + 1),
                ReserveSite::Color => probe
                    .normalized_colors
                    .set(probe.normalized_colors.get() + 1),
                _ => {}
            }
        }
        Ok(text)
    }
    fn id(&self, path: &str, group: u32, index: u32) -> Result<String, StlError> {
        fn digits(mut value: u32) -> usize {
            let mut count = 1;
            while value >= 10 {
                value /= 10;
                count += 1;
            }
            count
        }
        let length = 4usize
            .checked_add(path.len())
            .and_then(|n| n.checked_add(2))
            .and_then(|n| n.checked_add(digits(group)))
            .and_then(|n| n.checked_add(digits(index)))
            .ok_or_else(|| self.input())?;
        let mut id = self.backing(length, ReserveSite::Id)?;
        // The formatter borrows its pieces and cannot exceed the exact reserve.
        write!(&mut id, "3mf:{path}:{group}:{index}").expect("String formatting is infallible");
        debug_assert_eq!(id.len(), length);
        Ok(id)
    }
}

// Like attr_str: stop at the FIRST iterator error or matching local name. A
// normalized Cow owns its scratch or borrows the event; no self-referential view.
fn attribute<'a>(node: &'a BytesStart<'_>, key: &[u8]) -> Result<Option<Cow<'a, str>>, StlError> {
    for attribute in node.attributes() {
        let attribute = attribute.map_err(|_| malformed("malformed XML attribute"))?;
        if attribute.key.local_name().as_ref() == key {
            return attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .map(Some)
                .map_err(|_| malformed("malformed XML attribute value"));
        }
    }
    Ok(None)
}
fn color(value: &str) -> Option<[u8; 7]> {
    let hex = value.trim().strip_prefix('#')?;
    if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut color = [b'#'; 7];
    for (output, byte) in color[1..].iter_mut().zip(hex.bytes()) {
        *output = byte.to_ascii_uppercase();
    }
    Some(color)
}
struct Candidate<'a> {
    name: Option<Cow<'a, str>>,
    color: [u8; 7],
    source: MaterialSource,
}
fn classify<'a>(node: &'a BytesStart<'_>, kind: MaterialGroupKind) -> Option<Candidate<'a>> {
    match kind {
        MaterialGroupKind::Base => {
            let name = attribute(node, b"name")
                .ok()
                .flatten()
                .filter(|name| !name.trim().is_empty());
            // Preserve both independent optional lookups even when name is invalid.
            let color = attribute(node, b"displaycolor")
                .ok()
                .flatten()
                .and_then(|value| color(&value));
            Some(Candidate {
                name: Some(name?),
                color: color?,
                source: MaterialSource::ThreeMfBaseMaterial,
            })
        }
        MaterialGroupKind::Color => {
            let color = attribute(node, b"color")
                .ok()
                .flatten()
                .and_then(|value| color(&value))?;
            Some(Candidate {
                name: None,
                color,
                source: MaterialSource::ThreeMfColorGroup,
            })
        }
    }
}

pub(super) fn retain(
    part: &mut ModelPartBuilder,
    node: &BytesStart<'_>,
    kind: MaterialGroupKind,
    group: u32,
    index: u32,
    observer: &ZipResourceObserver,
    #[cfg(test)] probe: Option<&MaterialOwnedTestProfile>,
) -> Result<(), StlError> {
    observer.check()?;
    let candidate = classify(node, kind);
    observer.check()?;
    let Some(candidate) = candidate else {
        return Ok(());
    };
    if part.material_lookup.contains_key(&(group, index))
        || part.materials.len() >= MAX_STANDARD_MATERIALS
    {
        return Ok(());
    }
    let control = Control {
        observer,
        #[cfg(test)]
        probe,
    };
    control.rows(&mut part.materials, ReserveSite::LocalRows)?;
    control.lookup(&mut part.material_lookup)?;
    let id = control.id(&part.path, group, index)?;
    let name = match candidate.name.as_deref() {
        Some(name) => {
            let trimmed = name.trim();
            let end = trimmed
                .char_indices()
                .nth(256)
                .map_or(trimmed.len(), |(offset, _)| offset);
            Some(control.text(&trimmed[..end], ReserveSite::Name)?)
        }
        None => None,
    };
    let color = Some(control.text(
        std::str::from_utf8(&candidate.color).expect("ASCII hex"),
        ReserveSite::Color,
    )?);
    observer.check()?;
    let material_index = part.materials.len();
    part.materials.push(ParsedMaterial {
        id,
        name,
        color,
        source: candidate.source,
    });
    part.material_lookup.insert((group, index), material_index);
    #[cfg(test)]
    if let Some(probe) = probe {
        probe.local_commits.set(probe.local_commits.get() + 1);
    }
    Ok(())
}

pub(super) fn copy_selected(
    output: &mut Vec<ParsedMaterial>,
    rows: &[ParsedMaterial],
    observer: &ZipResourceObserver,
    #[cfg(test)] probe: Option<&MaterialOwnedTestProfile>,
) -> Result<(), StlError> {
    observer.check()?;
    let control = Control {
        observer,
        #[cfg(test)]
        probe,
    };
    for row in rows
        .iter()
        .take(MAX_STANDARD_MATERIALS.saturating_sub(output.len()))
    {
        control.rows(output, ReserveSite::GlobalRows)?;
        let id = control.text(&row.id, ReserveSite::GlobalId)?;
        let name = row
            .name
            .as_deref()
            .map(|value| control.text(value, ReserveSite::GlobalName))
            .transpose()?;
        let color = row
            .color
            .as_deref()
            .map(|value| control.text(value, ReserveSite::GlobalColor))
            .transpose()?;
        observer.check()?;
        output.push(ParsedMaterial {
            id,
            name,
            color,
            source: row.source.clone(),
        });
        #[cfg(test)]
        if let Some(probe) = probe {
            probe.global_commits.set(probe.global_commits.get() + 1);
        }
    }
    Ok(())
}
