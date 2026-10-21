//! Project metadata without a JSON DOM. Limits derive from the existing input,
//! positional and Unicode-label policies; this is not a whole-producer budget.
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use std::fmt;

use super::{ParsedFilament, MAX_EMBEDDED_FILAMENTS};
use crate::package_bounds::TextRole;
use crate::zip_bounds::ZipResourceObserver;
use crate::{ResourceStop, StlError};

const LABEL_CHARS: usize = 256;
const LABEL_BYTES: usize = LABEL_CHARS * 4;
const JSON_DEPTH: usize = 127;

#[derive(Clone, Copy, PartialEq)]
enum Field {
    Colors,
    Settings,
    Kind,
    Vendor,
    Other,
}
enum Slot {
    Color([u8; 7]),
    Label(String),
}
type Prefix = Option<Vec<Option<Slot>>>;
#[derive(Default)]
struct Palette {
    colors: Prefix,
    settings: Prefix,
    kinds: Prefix,
    vendors: Prefix,
}
impl Palette {
    fn field(&mut self, field: Field) -> &mut Prefix {
        match field {
            Field::Colors => &mut self.colors,
            Field::Settings => &mut self.settings,
            Field::Kind => &mut self.kinds,
            Field::Vendor => &mut self.vendors,
            Field::Other => unreachable!(),
        }
    }
}

#[derive(Clone, Copy)]
enum Reserve {
    Prefix,
    Label,
    Result,
}
struct Control<'a> {
    observer: &'a ZipResourceObserver,
    #[cfg(test)]
    profile: Option<std::rc::Rc<TestProfile>>,
}
impl Control<'_> {
    fn check<E: de::Error>(&self) -> Result<(), E> {
        self.observer
            .check()
            .map_err(|_| E::custom("Model metadata processing stopped"))
    }
    fn allocation<E: de::Error>(&self) -> E {
        let _ = self.observer.allocation_failed_io();
        E::custom("Model metadata allocation failed")
    }
    fn before_reserve<E: de::Error>(&self, _kind: Reserve, _target: usize) -> Result<(), E> {
        self.check()?;
        #[cfg(test)]
        if let Some(profile) = &self.profile {
            let fail = {
                let mut state = profile.state.borrow_mut();
                state.reserves += 1;
                state.requests.push((_kind, _target));
                state.fail_at == Some(state.reserves)
            };
            if fail {
                return Err(self.allocation());
            }
        }
        Ok(())
    }
    fn vector<T, E: de::Error>(
        &self,
        output: &mut Vec<T>,
        needed: usize,
        kind: Reserve,
    ) -> Result<(), E> {
        if output.capacity() >= needed {
            return Ok(());
        }
        debug_assert!(needed <= MAX_EMBEDDED_FILAMENTS);
        self.before_reserve(kind, MAX_EMBEDDED_FILAMENTS)?;
        output
            .try_reserve_exact(MAX_EMBEDDED_FILAMENTS - output.len())
            .map_err(|_| self.allocation())?;
        self.check()
    }
    fn string<E: de::Error>(&self, value: &str) -> Result<String, E> {
        debug_assert!(value.len() <= LABEL_BYTES);
        let mut output = String::new();
        if !value.is_empty() {
            self.before_reserve(Reserve::Label, value.len())?;
            output
                .try_reserve_exact(value.len())
                .map_err(|_| self.allocation())?;
            self.check()?;
            output.push_str(value);
        }
        Ok(output)
    }
    fn label<E: de::Error>(&self, value: &str) -> Result<Option<String>, E> {
        #[cfg(test)]
        super::record_project_aux_normalization();
        let value = value.trim();
        if value.is_empty() {
            return Ok(None);
        }
        let end = value
            .char_indices()
            .nth(LABEL_CHARS)
            .map_or(value.len(), |(index, _)| index);
        self.string(&value[..end]).map(Some)
    }
    fn join<E: de::Error>(&self, vendor: &str, setting: &str) -> Result<String, E> {
        // normalize_label trims the full joined string BEFORE truncating. Both
        // inputs start non-whitespace; only setting's end needs full trim.
        let chars = vendor
            .chars()
            .chain(std::iter::once(' '))
            .chain(setting.trim_end().chars())
            .take(LABEL_CHARS);
        let bytes: usize = chars.clone().map(char::len_utf8).sum();
        let mut output = String::new();
        self.before_reserve(Reserve::Label, bytes)?;
        output
            .try_reserve_exact(bytes)
            .map_err(|_| self.allocation())?;
        self.check()?;
        output.extend(chars);
        Ok(output)
    }
}

// Allocation-free scan over already collected immutable input, before serde's
// decoded scratch or visitor output can allocate. Individual wire/decoded tokens
// are bounded by the existing 8 MiB document; no new smaller JSON policy.
fn preflight(bytes: &[u8], control: &Control<'_>, limit: usize) -> Result<bool, StlError> {
    control.observer.check()?;
    if bytes.len() > limit {
        let _ = control.observer.stop_io(ResourceStop::InputLimitExceeded);
        control.observer.check()?;
    }
    if std::str::from_utf8(bytes).is_err() {
        return Ok(false);
    }
    let mut stack = [0u8; JSON_DEPTH];
    let mut depth = 0;
    let mut string = false;
    let mut escape = false;
    let mut unicode = 0u8;
    for chunk in bytes.chunks(4096) {
        control.observer.check()?;
        for &byte in chunk {
            if string {
                if unicode > 0 {
                    if !byte.is_ascii_hexdigit() {
                        return Ok(false);
                    }
                    unicode -= 1;
                } else if escape {
                    escape = false;
                    match byte {
                        b'u' => unicode = 4,
                        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {}
                        _ => return Ok(false),
                    }
                } else {
                    match byte {
                        b'"' => string = false,
                        b'\\' => escape = true,
                        0..=31 => return Ok(false),
                        _ => {}
                    }
                }
            } else {
                match byte {
                    b'"' => string = true,
                    b'[' | b'{' => {
                        if depth == stack.len() {
                            return Ok(false);
                        }
                        stack[depth] = byte;
                        depth += 1;
                    }
                    b']' | b'}' => {
                        if depth == 0 || stack[depth - 1] != if byte == b']' { b'[' } else { b'{' }
                        {
                            return Ok(false);
                        }
                        depth -= 1;
                    }
                    _ => {}
                }
            }
        }
    }
    control.observer.check()?;
    Ok(!string && !escape && unicode == 0 && depth == 0)
}

struct FieldSeed<'a>(&'a Control<'a>);
impl<'de> DeserializeSeed<'de> for FieldSeed<'_> {
    type Value = Field;
    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Field, D::Error> {
        self.0.check()?;
        deserializer.deserialize_identifier(self)
    }
}
impl Visitor<'_> for FieldSeed<'_> {
    type Value = Field;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON object key")
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Field, E> {
        self.0.check()?;
        Ok(match value {
            "filament_colour" => Field::Colors,
            "filament_settings_id" => Field::Settings,
            "filament_type" => Field::Kind,
            "filament_vendor" => Field::Vendor,
            _ => Field::Other,
        })
    }
}

// Do not use IgnoredAny: its iterative skip stack bypasses default recursion
// and its number skip accepts out-of-range f64 that Value used to reject.
#[derive(Clone, Copy)]
struct Discard<'a>(&'a Control<'a>);
impl<'de> DeserializeSeed<'de> for Discard<'_> {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        self.0.check()?;
        #[cfg(test)]
        if let Some(profile) = &self.0.profile {
            profile.state.borrow_mut().discard_starts += 1;
        }
        deserializer.deserialize_any(self)
    }
}
impl Discard<'_> {
    fn scalar<E: de::Error>(self) -> Result<(), E> {
        self.0.check()?;
        // Test-only latch AFTER a completed scalar. Returning Ok deliberately
        // exercises the parent's next-operation boundary checks.
        #[cfg(test)]
        if let Some(profile) = &self.0.profile {
            let stop = profile.state.borrow_mut().after_discard_stop.take();
            match stop {
                Some(TestStop::Resource(stop)) => {
                    let _ = self.0.observer.stop_io(stop);
                }
                Some(TestStop::Allocation) => {
                    let _ = self.0.observer.allocation_failed_io();
                }
                None => {}
            }
        }
        Ok(())
    }
}
impl<'de> Visitor<'de> for Discard<'_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<(), E> {
        self.scalar()
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<(), E> {
        self.scalar()
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<(), E> {
        self.scalar()
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<(), E> {
        self.scalar()
    }
    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        self.scalar()
    }
    fn visit_str<E: de::Error>(self, _: &str) -> Result<(), E> {
        self.scalar()
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<(), A::Error> {
        loop {
            self.0.check()?;
            if sequence.next_element_seed(Discard(self.0))?.is_none() {
                break;
            }
        }
        Ok(())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        loop {
            self.0.check()?;
            if map.next_key_seed(FieldSeed(self.0))?.is_none() {
                break;
            }
            self.0.check()?;
            map.next_value_seed(Discard(self.0))?;
        }
        Ok(())
    }
}

macro_rules! scalar_none {
    ($member:tt) => {
        fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
            self.$member.check()?;
            Ok(None)
        }
        fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
            self.$member.check()?;
            Ok(None)
        }
        fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
            self.$member.check()?;
            Ok(None)
        }
        fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
            self.$member.check()?;
            Ok(None)
        }
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            self.$member.check()?;
            Ok(None)
        }
    };
}
struct RootSeed<'a>(&'a Control<'a>);
impl<'de> DeserializeSeed<'de> for RootSeed<'_> {
    type Value = Option<Palette>;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        self.0.check()?;
        deserializer.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for RootSeed<'_> {
    type Value = Option<Palette>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON value")
    }
    scalar_none!(0);
    fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
        self.0.check()?;
        Ok(None)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, sequence: A) -> Result<Self::Value, A::Error> {
        Discard(self.0).visit_seq(sequence)?;
        Ok(None)
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut palette = Palette::default();
        loop {
            self.0.check()?;
            let Some(field) = map.next_key_seed(FieldSeed(self.0))? else {
                break;
            };
            self.0.check()?;
            if field == Field::Other {
                map.next_value_seed(Discard(self.0))?;
            } else {
                *palette.field(field) = None;
                let next = map.next_value_seed(ArraySeed {
                    control: self.0,
                    field,
                })?;
                *palette.field(field) = next;
            }
        }
        Ok(Some(palette))
    }
}
struct ArraySeed<'a> {
    control: &'a Control<'a>,
    field: Field,
}
impl<'de> DeserializeSeed<'de> for ArraySeed<'_> {
    type Value = Prefix;
    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Prefix, D::Error> {
        self.control.check()?;
        deserializer.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for ArraySeed<'_> {
    type Value = Prefix;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON value")
    }
    scalar_none!(control);
    fn visit_str<E: de::Error>(self, _: &str) -> Result<Prefix, E> {
        self.control.check()?;
        Ok(None)
    }
    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Prefix, A::Error> {
        Discard(self.control).visit_map(map)?;
        Ok(None)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Prefix, A::Error> {
        let mut output = Vec::new();
        let mut index = 0usize;
        loop {
            self.control.check()?;
            let next = if index < MAX_EMBEDDED_FILAMENTS {
                sequence.next_element_seed(ElementSeed {
                    control: self.control,
                    field: self.field,
                })?
            } else {
                sequence
                    .next_element_seed(Discard(self.control))?
                    .map(|()| None)
            };
            let Some(next) = next else {
                break;
            };
            if index < MAX_EMBEDDED_FILAMENTS {
                self.control
                    .vector(&mut output, index + 1, Reserve::Prefix)?;
                output.push(next);
            }
            index += 1; // At most one element per distinct input byte, <=8MiB.
        }
        Ok(Some(output))
    }
}
struct ElementSeed<'a> {
    control: &'a Control<'a>,
    field: Field,
}
impl<'de> DeserializeSeed<'de> for ElementSeed<'_> {
    type Value = Option<Slot>;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        self.control.check()?;
        deserializer.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for ElementSeed<'_> {
    type Value = Option<Slot>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON value")
    }
    scalar_none!(control);
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        self.control.check()?;
        if self.field == Field::Colors {
            let Some(hex) = value.trim().strip_prefix('#') else {
                return Ok(None);
            };
            if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Ok(None);
            }
            let mut color = [b'#'; 7];
            for (output, byte) in color[1..].iter_mut().zip(hex.bytes()) {
                *output = byte.to_ascii_uppercase();
            }
            Ok(Some(Slot::Color(color)))
        } else {
            self.control
                .label(value)
                .map(|value| value.map(Slot::Label))
        }
    }
    fn visit_seq<A: SeqAccess<'de>>(self, sequence: A) -> Result<Self::Value, A::Error> {
        Discard(self.control).visit_seq(sequence)?;
        Ok(None)
    }
    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        Discard(self.control).visit_map(map)?;
        Ok(None)
    }
}

fn take_label(prefix: &mut Prefix, index: usize) -> Option<String> {
    match prefix.as_mut()?.get_mut(index)?.take()? {
        Slot::Label(label) => Some(label),
        Slot::Color(_) => None,
    }
}
fn finish<E: de::Error>(
    mut palette: Palette,
    control: &Control<'_>,
) -> Result<Option<Vec<ParsedFilament>>, E> {
    let Some(colors) = palette.colors.take() else {
        return Ok(None);
    };
    let mut output = Vec::new();
    for (index, slot) in colors.into_iter().enumerate() {
        let Some(Slot::Color(color)) = slot else {
            continue;
        };
        let setting = take_label(&mut palette.settings, index);
        let vendor = take_label(&mut palette.vendors, index);
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
            (Some(vendor), Some(setting)) => Some(control.join(&vendor, &setting)?),
            (Some(label), None) | (None, Some(label)) => Some(label),
            (None, None) => None,
        };
        let material = take_label(&mut palette.kinds, index);
        let color = control.string(std::str::from_utf8(&color).expect("ASCII hex"))?;
        control.vector(&mut output, index + 1, Reserve::Result)?;
        output.push(ParsedFilament {
            slot: index as u32 + 1,
            name,
            material,
            color,
        });
    }
    Ok(Some(output))
}

pub(super) fn parse(
    text: &str,
    observer: &ZipResourceObserver,
) -> Result<Option<Vec<ParsedFilament>>, StlError> {
    observer.check()?;
    let control = Control {
        observer,
        #[cfg(test)]
        profile: test_profile(),
    };
    #[cfg(test)]
    let limit = control
        .profile
        .as_ref()
        .map_or(TextRole::ProjectSettings.byte_limit(), |profile| {
            profile.input_limit
        });
    #[cfg(not(test))]
    let limit = TextRole::ProjectSettings.byte_limit();
    let result = (|| {
        if !preflight(text.as_bytes(), &control, limit)? {
            return Ok(None);
        }
        #[cfg(test)]
        if let Some(profile) = &control.profile {
            profile.state.borrow_mut().factories += 1;
        }
        let mut deserializer = serde_json::Deserializer::from_str(text);
        let parsed = RootSeed(&control).deserialize(&mut deserializer);
        observer.check()?;
        let root = match parsed {
            Ok(root) => root,
            Err(_) => return Ok(None),
        };
        let end = deserializer.end();
        observer.check()?;
        if end.is_err() {
            return Ok(None);
        }
        let Some(root) = root else {
            return Ok(None);
        };
        let converted = finish::<serde_json::Error>(root, &control);
        observer.check()?;
        match converted {
            Ok(value) => Ok(value),
            Err(_) => Ok(None),
        }
    })();
    #[cfg(test)]
    if let Some(profile) = &control.profile {
        let stop = profile.state.borrow().final_stop;
        match stop {
            Some(TestStop::Resource(stop)) => {
                let _ = observer.stop_io(stop);
            }
            Some(TestStop::Allocation) => {
                let _ = observer.allocation_failed_io();
            }
            None => {}
        }
    }
    observer.finish(result)
}

#[cfg(test)]
#[derive(Clone, Copy)]
enum TestStop {
    Resource(ResourceStop),
    Allocation,
}
#[cfg(test)]
#[derive(Default)]
struct TestState {
    reserves: usize,
    fail_at: Option<usize>,
    requests: Vec<(Reserve, usize)>,
    factories: usize,
    final_stop: Option<TestStop>,
    after_discard_stop: Option<TestStop>,
    discard_starts: usize,
}
#[cfg(test)]
struct TestProfile {
    input_limit: usize,
    state: std::cell::RefCell<TestState>,
}
#[cfg(test)]
thread_local! { static PROFILE: std::cell::RefCell<Option<std::rc::Rc<TestProfile>>> = const { std::cell::RefCell::new(None) }; }
#[cfg(test)]
fn test_profile() -> Option<std::rc::Rc<TestProfile>> {
    PROFILE.with(|slot| slot.borrow().clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;
    struct Guard(Option<Rc<TestProfile>>);
    impl Guard {
        fn install(limit: usize) -> (Self, Rc<TestProfile>) {
            let profile = Rc::new(TestProfile {
                input_limit: limit,
                state: std::cell::RefCell::new(TestState::default()),
            });
            let previous = PROFILE.with(|slot| slot.replace(Some(profile.clone())));
            (Self(previous), profile)
        }
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            PROFILE.with(|slot| slot.replace(self.0.take()));
        }
    }
    fn check(text: &str) {
        let observer = ZipResourceObserver::new();
        let expected = super::super::parse_project_filaments_reference(text);
        assert_eq!(parse(text, &observer).unwrap(), expected, "{text}");
        assert!(observer.check().is_ok());
    }
    #[test]
    fn project_json_differential_roots_shapes_slots_and_decoded_duplicates() {
        for text in [
            "null",
            "true",
            "42",
            "\"x\"",
            "[]",
            "{}",
            r#"{"filament_colour":false}"#,
            r#"{"filament_colour":[]}"#,
            r#"{"filament_colour":[null,{},true,[],7]}"#,
            r##"{"filament_colour":[null," #aAbBcC80 ","123456","#12345G","#112233"],"filament_settings_id":["z"," A ",true,"b","C"],"filament_vendor":[null,"V"],"filament_type":false}"##,
            r##"{"filament_colour":["#112233"],"filament_colour":null}"##,
            r##"{"filament_colour":null,"filament_\u0063olour":["#aabbcc"]}"##,
            r##"{"filament_colour":["#112233"],"filament_settings_id":["A"],"filament_settings_id":{},"filament_vendor":["V"],"unknown":{"filament_vendor":["BAD"]}}"##,
            r##"{"filament_vendor":"V","filament_type":["PLA"],"filament_colour":["#112233"]}"##,
        ] {
            check(text);
        }
        for count in [0, 1, 255, 256, 257, 300] {
            let values: Vec<_> = (0..count)
                .map(|index| {
                    if index % 3 == 0 {
                        serde_json::Value::Null
                    } else {
                        serde_json::json!("#112233")
                    }
                })
                .collect();
            let text = serde_json::json!({"filament_colour": values, "filament_settings_id": vec!["Label"; count],
                "filament_vendor": vec!["Vendor"; count], "filament_type": vec!["PLA"; count]}).to_string();
            check(&text);
        }
    }
    #[test]
    fn project_json_semantic_discard_preserves_numbers_escapes_and_complete_end() {
        let prefix = r##""filament_colour":["#112233"]"##;
        for value in [
            "1e400",
            "-1e400",
            "1e99999999999999999999999",
            "1e-400",
            "-0",
            "18446744073709551616",
            "0.00000000000000000001",
            "[1e400]",
            r#""\uD800""#,
            r#""\uDC00""#,
            r#""\uD83D\uDE00""#,
            r#"{"nested":[true,null,"é"]}"#,
        ] {
            check(&format!("{{{prefix},\"unknown\":{value}}}"));
            let tail = format!("{}{}", "null,".repeat(256), value);
            check(&format!("{{{prefix},\"filament_type\":[{tail}]}}"));
        }
        for tail in ["x", "{}", " null", ",", "\n{}", " "] {
            check(&format!("{{{prefix}}}{tail}"));
        }
        check(r##"{"filament_colour":["#112233"],"filament_type":["ok""bad"]}"##);
    }
    #[test]
    fn project_json_depth_guard_matches_value_and_stays_ordinary() {
        for arrays in [126, 127] {
            let text = format!(
                r##"{{"filament_colour":["#112233"],"unknown":{}0{}}}"##,
                "[".repeat(arrays),
                "]".repeat(arrays)
            );
            let (_guard, profile) = Guard::install(TextRole::ProjectSettings.byte_limit());
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&text).is_ok(),
                arrays == 126
            );
            check(&text);
            assert_eq!(profile.state.borrow().factories, usize::from(arrays == 126));
        }
    }
    #[test]
    fn project_json_lexical_chunk_edges_keep_utf8_escapes_and_delimiters() {
        let suffix = r##"{"filament_colour":["#112233"],"unknown":"é🙂[]{}\\\"\uD83D\uDE00","filament_settings_id":["\tLabel\n"]}"##;
        for prefix in 4070..4110 {
            check(&format!("{}{suffix}", " ".repeat(prefix)));
        }
        let observer = ZipResourceObserver::new();
        let control = Control {
            observer: &observer,
            profile: None,
        };
        assert!(!preflight(
            b"\"\xff\"",
            &control,
            TextRole::ProjectSettings.byte_limit()
        )
        .unwrap());
        assert!(observer.check().is_ok());
    }
    #[test]
    fn project_json_labels_join_and_colors_match_reference_without_full_join() {
        let labels = [
            "",
            " A ",
            "Vendor",
            "Vendor PLA",
            "Vendor\u{2003}PLA",
            " VendorPLA ",
            "x\t",
            "é🙂",
        ];
        for vendor in labels {
            for setting in labels {
                check(&serde_json::json!({"filament_colour":["#aAbBcC80"],"filament_vendor":[vendor],"filament_settings_id":[setting]}).to_string());
            }
        }
        for vendor in [
            "v".repeat(255),
            "v".repeat(256),
            "é".repeat(255),
            "🙂".repeat(256),
        ] {
            for setting in [
                "Setting".to_owned(),
                format!("{} x", " ".repeat(400)),
                format!("{} ", "s".repeat(255)),
                " ".to_owned(),
            ] {
                check(&serde_json::json!({"filament_colour":["#123456"],"filament_vendor":[vendor],"filament_settings_id":[setting]}).to_string());
            }
        }
    }
    #[test]
    fn project_json_guard_precedes_serde_factory_and_output_growth() {
        let (_guard, profile) = Guard::install(8);
        let observer = ZipResourceObserver::new();
        assert_eq!(
            parse(r#"{"long":"value"}"#, &observer),
            Err(StlError::ResourceStopped(ResourceStop::InputLimitExceeded))
        );
        let state = profile.state.borrow();
        assert_eq!(state.factories, 0);
        assert_eq!(state.reserves, 0);
    }
    #[test]
    fn project_json_owned_reserves_are_bounded_and_each_failure_is_terminal() {
        let text = r##"{"filament_colour":[null,"#112233"],"filament_settings_id":["unused","Basic"],"filament_vendor":["unused","Vendor"],"filament_type":["unused","PLA"]}"##;
        let count = {
            let (_guard, profile) = Guard::install(TextRole::ProjectSettings.byte_limit());
            check(text);
            let state = profile.state.borrow();
            for &(kind, target) in &state.requests {
                assert!(
                    target
                        <= match kind {
                            Reserve::Label => LABEL_BYTES,
                            Reserve::Prefix | Reserve::Result => MAX_EMBEDDED_FILAMENTS,
                        }
                );
            }
            state.reserves
        };
        assert!(count > 0);
        for fail_at in 1..=count {
            let (_guard, profile) = Guard::install(TextRole::ProjectSettings.byte_limit());
            profile.state.borrow_mut().fail_at = Some(fail_at);
            let observer = ZipResourceObserver::new();
            assert_eq!(parse(text, &observer), Err(StlError::AllocationFailed));
            assert!(matches!(
                observer.check(),
                Err(crate::zip_bounds::ZipOpenError::AllocationFailed)
            ));
            assert_eq!(profile.state.borrow().reserves, fail_at);
        }
    }
    #[test]
    fn project_json_prior_and_final_terminal_causes_never_return_palette() {
        for stop in [
            TestStop::Resource(ResourceStop::InputLimitExceeded),
            TestStop::Resource(ResourceStop::ReadWorkExceeded),
            TestStop::Resource(ResourceStop::EnvelopeChanged),
            TestStop::Allocation,
        ] {
            let expected = match stop {
                TestStop::Resource(stop) => StlError::ResourceStopped(stop),
                TestStop::Allocation => StlError::AllocationFailed,
            };
            let (_guard, profile) = Guard::install(TextRole::ProjectSettings.byte_limit());
            let observer = ZipResourceObserver::new();
            match stop {
                TestStop::Resource(stop) => {
                    let _ = observer.stop_io(stop);
                }
                TestStop::Allocation => {
                    let _ = observer.allocation_failed_io();
                }
            }
            assert_eq!(parse("{}", &observer), Err(expected));
            assert_eq!(profile.state.borrow().factories, 0);
            for text in ["{}", "{bad}", r##"{"filament_colour":["#112233"]}"##] {
                let observer = ZipResourceObserver::new();
                profile.state.borrow_mut().final_stop = Some(stop);
                let expected = match stop {
                    TestStop::Resource(stop) => StlError::ResourceStopped(stop),
                    TestStop::Allocation => StlError::AllocationFailed,
                };
                assert_eq!(parse(text, &observer), Err(expected));
            }
        }
    }
    #[test]
    fn project_json_profile_restores_nested_scope_and_panic() {
        assert!(test_profile().is_none());
        let (outer, profile) = Guard::install(80);
        {
            let (_inner, _) = Guard::install(20);
            assert_eq!(test_profile().unwrap().input_limit, 20);
        }
        let panic = std::panic::catch_unwind(|| {
            let (_inner, _) = Guard::install(30);
            panic!("synthetic profile unwind");
        });
        assert!(panic.is_err());
        assert!(Rc::ptr_eq(&test_profile().unwrap(), &profile));
        drop(outer);
        assert!(test_profile().is_none());
    }
    #[test]
    fn project_json_discard_stop_prevents_starting_next_sdk_value() {
        let unknown =
            r##"{"unknown":["first","second"],"filament_colour":["#112233"]}"##.to_owned();
        let tail = format!(
            r##"{{"filament_colour":[{},"first","second"]}}"##,
            std::iter::repeat_n(r##""#112233""##, 256)
                .collect::<Vec<_>>()
                .join(",")
        );
        for (text, expected_starts) in [(unknown, 2), (tail, 1)] {
            let (_guard, profile) = Guard::install(TextRole::ProjectSettings.byte_limit());
            profile.state.borrow_mut().after_discard_stop =
                Some(TestStop::Resource(ResourceStop::ReadWorkExceeded));
            let observer = ZipResourceObserver::new();
            assert_eq!(
                parse(&text, &observer),
                Err(StlError::ResourceStopped(ResourceStop::ReadWorkExceeded))
            );
            assert_eq!(profile.state.borrow().factories, 1);
            assert_eq!(profile.state.borrow().discard_starts, expected_starts);
        }
    }

    struct ParserGuard(Option<Rc<super::super::ParserTestProbe>>);
    impl Drop for ParserGuard {
        fn drop(&mut self) {
            super::super::PARSER_TEST_PROBE.with(|slot| slot.replace(self.0.take()));
        }
    }
    fn public_package(project: &str) -> Vec<u8> {
        use std::io::Write;
        let root = r#"<model unit="millimeter"><resources><object id="1" type="model"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/></vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object></resources><build><item objectid="1"/></build></model>"#;
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, value) in [
            (super::super::ROOT_MODEL_FALLBACK, root),
            (super::super::PROJECT_SETTINGS_PATH, project),
        ] {
            writer
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(value.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }
    #[test]
    fn project_json_actual_public_context_never_falls_back_after_cached_project_stop() {
        let bytes = public_package(r##"{"filament_colour":["#112233"]}"##);
        for stop in [
            TestStop::Resource(ResourceStop::InputLimitExceeded),
            TestStop::Resource(ResourceStop::ReadWorkExceeded),
            TestStop::Resource(ResourceStop::EnvelopeChanged),
            TestStop::Allocation,
        ] {
            let (_guard, profile) = Guard::install(TextRole::ProjectSettings.byte_limit());
            profile.state.borrow_mut().final_stop = Some(stop);
            let observer = ZipResourceObserver::new();
            let probe = Rc::new(super::super::ParserTestProbe {
                member: "do-not-inject-source-fault",
                failure: super::super::ParserTestFailure::Allocation,
                state: std::cell::RefCell::new(super::super::ParserTestState::default()),
                guard_observer: Some(observer.clone()),
            });
            let previous =
                super::super::PARSER_TEST_PROBE.with(|slot| slot.replace(Some(probe.clone())));
            let _parser_guard = ParserGuard(previous);
            let mut context = crate::package_bounds::PackageProcessingContext::new(observer);
            let expected = match stop {
                TestStop::Resource(stop) => StlError::ResourceStopped(stop),
                TestStop::Allocation => StlError::AllocationFailed,
            };
            assert_eq!(
                crate::parse_model_with_plates_in_context(&bytes, &mut context),
                Err(expected)
            );
            assert_eq!(profile.state.borrow().factories, 1);
            let state = probe.state.borrow();
            assert!(state.root_cached);
            assert_eq!(state.total_work, 0);
            assert_eq!(state.work_after_stop, 0);
            assert_eq!(state.after_io, 0);
        }
    }
    #[test]
    fn project_json_allocation_first_cause_survives_later_conflicting_stop_in_public_parser() {
        let bytes = public_package(r##"{"filament_colour":["#112233"]}"##);
        let (_guard, profile) = Guard::install(TextRole::ProjectSettings.byte_limit());
        {
            let mut state = profile.state.borrow_mut();
            state.fail_at = Some(1);
            state.final_stop = Some(TestStop::Resource(ResourceStop::EnvelopeChanged));
        }
        let observer = ZipResourceObserver::new();
        let mut context = crate::package_bounds::PackageProcessingContext::new(observer.clone());
        assert_eq!(
            crate::parse_model_with_plates_in_context(&bytes, &mut context),
            Err(StlError::AllocationFailed)
        );
        assert!(matches!(
            observer.check(),
            Err(crate::zip_bounds::ZipOpenError::AllocationFailed)
        ));
        assert_eq!(profile.state.borrow().factories, 1);
        assert_eq!(profile.state.borrow().reserves, 1);
    }
}
