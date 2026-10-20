//! Opaque, bounded XML event acquisition. This foundation does not bound model
//! collections, retained strings, decompression work, or SDK allocator failures.
use std::io::{self, BufRead, Read};

use quick_xml::{events::Event, Reader};

use crate::{zip_bounds::ZipResourceObserver, ResourceStop, StlError};

const CHUNK: usize = 4096;
const DEPTH: usize = 128;

#[derive(Clone, Copy)]
struct Limits {
    token: usize,
    attribute: usize,
    attributes: usize,
    depth: usize,
    names: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            token: 65536,
            attribute: 4096,
            attributes: 128,
            depth: DEPTH,
            names: 262144,
        }
    }
}
#[derive(Clone, Copy)]
enum Mode {
    Text,
    Prefix,
    Bang,
    Tag,
    Comment,
    Cdata,
    Pi,
    Dtd,
}
#[derive(Clone)]
struct Lex {
    mode: Mode,
    token: usize,
    tail: [u8; 3],
    prefix: [u8; 9],
    prefix_len: usize,
    quote: u8,
    attr_bytes: usize,
    name: usize,
    name_done: bool,
    end: bool,
    previous: u8,
    names: [usize; DEPTH],
    depth: usize,
    live_names: usize,
    subset: bool,
    after_subset: bool,
    declaration: bool,
    dtd_special: u8,
    dtd_prefix: [u8; 4],
    dtd_prefix_len: usize,
}
impl Default for Lex {
    fn default() -> Self {
        Self {
            mode: Mode::Text,
            token: 0,
            tail: [0; 3],
            prefix: [0; 9],
            prefix_len: 0,
            quote: 0,
            attr_bytes: 0,
            name: 0,
            name_done: false,
            end: false,
            previous: 0,
            names: [0; DEPTH],
            depth: 0,
            live_names: 0,
            subset: false,
            after_subset: false,
            declaration: false,
            dtd_special: 0,
            dtd_prefix: [0; 4],
            dtd_prefix_len: 0,
        }
    }
}
fn space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n')
}
impl Lex {
    fn finish(&mut self) {
        self.mode = Mode::Text;
        self.token = 0;
        self.tail = [0; 3];
    }
    fn step(&mut self, b: u8, limits: Limits) -> Result<(), ()> {
        if matches!(self.mode, Mode::Text) && b == b'<' {
            self.mode = Mode::Prefix;
            self.token = 0;
            self.prefix_len = 0;
            self.tail = [0; 3];
        }
        self.token = self.token.checked_add(1).ok_or(())?;
        if self.token > limits.token {
            return Err(());
        }
        self.tail = [self.tail[1], self.tail[2], b];
        match self.mode {
            Mode::Text => {}
            Mode::Prefix => {
                if self.token == 1 {
                    return Ok(());
                }
                match b {
                    b'!' => {
                        self.mode = Mode::Bang;
                        self.prefix_len = 0;
                    }
                    b'?' => self.mode = Mode::Pi,
                    _ => {
                        self.mode = Mode::Tag;
                        self.end = b == b'/';
                        self.name = 0;
                        self.name_done = false;
                        self.quote = 0;
                        self.previous = 0;
                        if !self.end {
                            self.tag(b, limits)?;
                        }
                    }
                }
            }
            Mode::Bang => {
                if self.prefix_len < self.prefix.len() {
                    self.prefix[self.prefix_len] = b;
                    self.prefix_len += 1;
                }
                let p = &self.prefix[..self.prefix_len];
                if p == b"--" {
                    self.mode = Mode::Comment;
                } else if p == b"[CDATA[" {
                    self.mode = Mode::Cdata;
                } else if p == b"D" || p == b"d" {
                    self.mode = Mode::Dtd;
                    self.subset = false;
                    self.after_subset = false;
                    self.declaration = false;
                    self.dtd_special = 0;
                    self.dtd_prefix_len = 0;
                    self.quote = 0;
                } else if !b"--".starts_with(p) && !b"[CDATA[".starts_with(p) {
                    // Unknown bang syntax is left to quick-xml. Never reset the
                    // counter on an unrecognized byte sequence.
                }
            }
            Mode::Tag => self.tag(b, limits)?,
            Mode::Comment if self.tail == *b"-->" => self.finish(),
            Mode::Cdata if self.tail == *b"]]>" => self.finish(),
            Mode::Pi if self.tail[1..] == *b"?>" => self.finish(),
            Mode::Dtd => self.dtd(b),
            _ => {}
        }
        Ok(())
    }
    fn tag(&mut self, b: u8, limits: Limits) -> Result<(), ()> {
        // Match quick-xml's permissive raw name_len: only XML whitespace
        // ends the name, including slash/question mark/quoted malformed names.
        // Element delimiter quotes are tracked independently of name syntax.
        if !(self.name_done || b == b'>' && self.quote == 0) {
            if space(b) {
                self.name_done = true;
            } else {
                self.name = self.name.checked_add(1).ok_or(())?;
            }
        }
        if self.quote != 0 {
            if b == self.quote {
                self.quote = 0;
            } else {
                self.attr_bytes = self.attr_bytes.checked_add(1).ok_or(())?;
                if self.attr_bytes > limits.attribute {
                    return Err(());
                }
            }
            self.previous = b;
            return Ok(());
        }
        if b == b'>' {
            if self.end {
                if self.depth > 0 {
                    self.depth -= 1;
                    self.live_names -= self.names[self.depth];
                }
            } else if self.previous != b'/' {
                if self.depth >= limits.depth || self.depth >= DEPTH {
                    return Err(());
                }
                let total = self.live_names.checked_add(self.name).ok_or(())?;
                if total > limits.names {
                    return Err(());
                }
                self.names[self.depth] = self.name;
                self.depth += 1;
                self.live_names = total;
            }
            self.finish();
            return Ok(());
        }
        if matches!(b, b'\'' | b'"') {
            self.quote = b;
            self.attr_bytes = 0;
        }
        self.previous = b;
        Ok(())
    }
    fn dtd(&mut self, b: u8) {
        if self.after_subset {
            if b == b'>' {
                self.finish();
            }
            return;
        }
        if self.dtd_special != 0 {
            if (self.dtd_special == 1 && self.tail == *b"-->")
                || (self.dtd_special == 2 && self.tail[1..] == *b"?>")
            {
                self.dtd_special = 0;
                self.declaration = false;
                self.dtd_prefix_len = 0;
            }
            return;
        }
        if self.quote != 0 {
            if b == self.quote {
                self.quote = 0;
            }
            return;
        }
        if matches!(b, b'\'' | b'"') {
            self.quote = b;
            return;
        }
        if !self.subset {
            if b == b'[' {
                self.subset = true;
            } else if b == b'>' {
                self.finish();
            }
            return;
        }
        if self.declaration {
            if self.dtd_prefix_len < 4 {
                self.dtd_prefix[self.dtd_prefix_len] = b;
                self.dtd_prefix_len += 1;
                let prefix = &self.dtd_prefix[..self.dtd_prefix_len];
                if prefix == b"?" {
                    self.dtd_special = 2;
                    return;
                }
                if prefix == b"!--" {
                    self.dtd_special = 1;
                    return;
                }
            }
            if b == b'>' {
                self.declaration = false;
                self.dtd_prefix_len = 0;
            }
        } else if b == b'<' {
            self.declaration = true;
            self.dtd_prefix_len = 0;
        } else if b == b']' {
            self.subset = false;
            self.after_subset = true;
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum FixedXmlReason {
    Lexical,
    Attributes,
}
#[derive(Clone, Copy)]
enum FixedDisposition {
    Immediate,
    Deferred,
}

pub(crate) struct FixedXmlViolation {
    reason: FixedXmlReason,
    parent: ZipResourceObserver,
}
impl FixedXmlViolation {
    pub(crate) fn activate(self) -> StlError {
        let _reason = self.reason;
        if let Err(error) = self.parent.check() {
            return error.into();
        }
        let _ = self.parent.stop_io(ResourceStop::InputLimitExceeded);
        self.parent
            .check()
            .expect_err("fixed validation activated")
            .into()
    }
}
pub(crate) enum DeferredXmlReadError {
    Fixed(FixedXmlViolation),
    Xml(quick_xml::Error),
    Terminal(StlError),
}

struct LexicalReader<R> {
    inner: R,
    observer: ZipResourceObserver,
    limits: Limits,
    lex: Lex,
    buffer: [u8; CHUNK],
    start: usize,
    end: usize,
    event_bytes: usize,
    initial: bool,
    bom_remaining: usize,
    fixed_disposition: FixedDisposition,
    fixed_failure: Option<FixedXmlReason>,
}
impl<R: Read> LexicalReader<R> {
    fn new(inner: R, observer: ZipResourceObserver, limits: Limits) -> Self {
        Self {
            inner,
            observer,
            limits,
            lex: Lex::default(),
            buffer: [0; CHUNK],
            start: 0,
            end: 0,
            event_bytes: 0,
            initial: true,
            bom_remaining: 0,
            fixed_disposition: FixedDisposition::Immediate,
            fixed_failure: None,
        }
    }
}
impl<R: Read> LexicalReader<R> {
    fn reject_fixed(&mut self, reason: FixedXmlReason) -> io::Error {
        self.fixed_failure.get_or_insert(reason);
        match self.fixed_disposition {
            FixedDisposition::Immediate => self.observer.stop_io(ResourceStop::InputLimitExceeded),
            FixedDisposition::Deferred => io::ErrorKind::InvalidData.into(),
        }
    }
}
impl<R: Read> BufRead for LexicalReader<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.observer.check_io()?;
        if self.start == self.end {
            self.start = 0;
            self.end = 0;
            // Accumulate the first three bytes so BOM recognition remains
            // correct even when the original Read returns one byte at a time.
            loop {
                let read = self.inner.read(&mut self.buffer[self.end..]);
                if let Err(error) = &read {
                    self.observer.observe_io(error);
                }
                self.observer.check_io()?;
                let n = match read {
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    other => other?,
                };
                self.end += n;
                if !self.initial || self.end >= 3 || n == 0 {
                    break;
                }
            }
            if self.initial && self.buffer[..self.end].starts_with(&[0xef, 0xbb, 0xbf]) {
                self.bom_remaining = 3;
            }
            self.initial = false;
        }
        let mut candidate = self.lex.clone();
        let mut allowed = 0;
        for &b in &self.buffer[self.start..self.end] {
            if allowed < self.bom_remaining {
                allowed += 1;
                continue;
            }
            // read_text needs to *see* the following '<' to return an exact
            // length Text event; it does not append that byte to the event.
            // Permit this single boundary lookahead, never arbitrary suffix.
            let text_boundary = matches!(candidate.mode, Mode::Text) && b == b'<';
            if (self.event_bytes + allowed.saturating_sub(self.bom_remaining) >= self.limits.token
                && !text_boundary)
                || candidate.step(b, self.limits).is_err()
            {
                break;
            }
            allowed += 1;
        }
        if allowed == 0 && self.start != self.end {
            return Err(self.reject_fixed(FixedXmlReason::Lexical));
        }
        self.observer.check_io()?;
        Ok(&self.buffer[self.start..self.start + allowed])
    }
    fn consume(&mut self, amount: usize) {
        assert!(
            amount <= self.end - self.start,
            "consume exceeds available XML bytes"
        );
        let exempt = amount.min(self.bom_remaining);
        for &b in &self.buffer[self.start + exempt..self.start + amount] {
            // Only a previously admitted prefix is consumed by quick-xml.
            let result = self.lex.step(b, self.limits);
            debug_assert!(result.is_ok());
        }
        self.start += amount;
        self.bom_remaining -= exempt;
        self.event_bytes += amount - exempt;
    }
}
impl<R: Read> Read for LexicalReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.observer.check_io()?;
        if output.is_empty() {
            return Ok(0);
        }
        let available = self.fill_buf()?;
        let n = output.len().min(available.len());
        output[..n].copy_from_slice(&available[..n]);
        self.consume(n);
        Ok(n)
    }
}

// Use the pinned SDK's actual recovery behavior instead of duplicating its
// malformed-attribute grammar. Each next() handles at most one key and borrows
// its raw value. The possible 129th inspection is included in the documented
// 16 KiB temporary SDK workspace allowance; this is not allocation-free and
// does not provide fallible allocation inside the SDK's Vec/HashSet.
fn attributes_fit(start: &quick_xml::events::BytesStart<'_>, limits: Limits) -> bool {
    let mut attributes = start.attributes();
    for index in 0..=limits.attributes {
        match attributes.next() {
            None => return true,
            Some(_) if index == limits.attributes => return false,
            Some(Ok(attribute)) => {
                if !matches!(&attribute.value, std::borrow::Cow::Borrowed(_))
                    || attribute.value.len() > limits.attribute
                {
                    return false;
                }
            }
            // Count errors too, but preserve the Event and ordinary SDK error
            // semantics below the cap. Do not format or normalize anything.
            Some(Err(_)) => {}
        }
    }
    false
}

/// XML event reader with fixed production limits and no raw parser escape.
///
/// Attribute normalization remains quick-xml's predefined-entity operation:
/// bounded raw values bound its allocation size, not allocator failure recovery.
/// Attribute admission uses up to 129 SDK iterator calls and requires a
/// conservative 16 KiB temporary workspace estimate for the reviewed toolchain.
/// Baseline: rustc 1.94.0 locally and Linux/Windows CI; macOS CI uses rolling
/// stable, so any actual version change requires re-review of this estimate.
/// This is a memory-profile allowance, not an implemented producer-ledger permit.
#[doc(hidden)]
pub struct XmlEventReader<R: Read> {
    xml: Reader<LexicalReader<R>>,
    scratch: Vec<u8>,
    observer: ZipResourceObserver,
    failed: bool,
}
impl<R: Read> XmlEventReader<R> {
    pub fn new(reader: R, observer: ZipResourceObserver) -> Result<Self, StlError> {
        Self::with_limits(reader, observer, Limits::default(), false)
    }
    fn with_limits(
        reader: R,
        observer: ZipResourceObserver,
        limits: Limits,
        fail_reserve: bool,
    ) -> Result<Self, StlError> {
        Self::with_fixed_disposition(
            reader,
            observer,
            limits,
            fail_reserve,
            FixedDisposition::Immediate,
        )
    }
    fn with_fixed_disposition(
        reader: R,
        observer: ZipResourceObserver,
        limits: Limits,
        fail_reserve: bool,
        fixed_disposition: FixedDisposition,
    ) -> Result<Self, StlError> {
        Self::with_workspace_hooks(
            reader,
            observer,
            limits,
            fail_reserve,
            fixed_disposition,
            |_| {},
            |_| {},
        )
    }
    fn with_workspace_hooks(
        reader: R,
        observer: ZipResourceObserver,
        limits: Limits,
        fail_reserve: bool,
        fixed_disposition: FixedDisposition,
        before_reserve: impl FnOnce(&ZipResourceObserver),
        after_reserve: impl FnOnce(&ZipResourceObserver),
    ) -> Result<Self, StlError> {
        observer.check()?;
        let mut scratch = Vec::new();
        let capacity = limits
            .token
            .checked_add(2)
            .ok_or(ResourceStop::InputLimitExceeded)?;
        before_reserve(&observer);
        observer.check()?;
        if fail_reserve || scratch.try_reserve_exact(capacity).is_err() {
            let error = observer.allocation_failed_io();
            observer.observe_io(&error);
            observer.check()?;
            return Err(StlError::AllocationFailed);
        }
        after_reserve(&observer);
        observer.check()?;
        let mut lexical = LexicalReader::new(reader, observer.clone(), limits);
        lexical.fixed_disposition = fixed_disposition;
        Ok(Self {
            xml: Reader::from_reader(lexical),
            scratch,
            observer,
            failed: false,
        })
    }
    /// Configure only the existing caller's whitespace behavior.
    pub fn set_trim_text(&mut self, trim: bool) {
        self.xml.config_mut().trim_text(trim);
    }
    /// Acquire a borrowed event; errors do not permit this reader to resume.
    pub fn next_event(&mut self) -> quick_xml::Result<Event<'_>> {
        self.next_event_after_inspection(|_| {})
    }
    // Private deterministic seam: no production caller can install a callback
    // or obtain an inner parser/observer through the public event API.
    fn next_event_after_inspection(
        &mut self,
        after_inspection: impl FnOnce(&ZipResourceObserver),
    ) -> quick_xml::Result<Event<'_>> {
        Self::read_event_parts(
            &mut self.xml,
            &mut self.scratch,
            &self.observer,
            &mut self.failed,
            after_inspection,
        )
    }
    fn read_event_parts<'buffer>(
        xml: &mut Reader<LexicalReader<R>>,
        scratch: &'buffer mut Vec<u8>,
        observer: &ZipResourceObserver,
        failed: &mut bool,
        after_inspection: impl FnOnce(&ZipResourceObserver),
    ) -> quick_xml::Result<Event<'buffer>> {
        observer.check_io()?;
        if *failed {
            return Err(io::Error::other("XML reader stopped after an error").into());
        }
        scratch.clear();
        xml.get_mut().event_bytes = 0;
        let event = xml.read_event_into(scratch);
        if let Err(error) = &event {
            if let quick_xml::Error::Io(error) = error {
                observer.observe_io(error);
            }
            *failed = true;
        }
        observer.check_io()?;
        if xml.get_ref().fixed_failure.is_some() {
            *failed = true;
            return Err(io::Error::from(io::ErrorKind::InvalidData).into());
        }
        if let Ok(Event::Start(start) | Event::Empty(start)) = &event {
            if !attributes_fit(start, xml.get_ref().limits) {
                *failed = true;
                return Err(xml
                    .get_mut()
                    .reject_fixed(FixedXmlReason::Attributes)
                    .into());
            }
        }
        if matches!(
            &event,
            Ok(Event::Start(_)
                | Event::Empty(_)
                | Event::End(_)
                | Event::Comment(_)
                | Event::CData(_)
                | Event::Decl(_)
                | Event::PI(_)
                | Event::DocType(_))
        ) && !matches!(xml.get_ref().lex.mode, Mode::Text)
        {
            *failed = true;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "XML lexical boundary does not match parser boundary",
            )
            .into());
        }
        after_inspection(observer);
        // Another observer owner may latch while bounded metadata inspection
        // runs. Never return the tentative borrowed Event after that stop.
        observer.check_io()?;
        event
    }
}

// Only immutable, already collected text may defer fixed validation. Parent
// checks and actual workspace failure remain immediate on the original observer.
pub(crate) struct DeferredXmlEventReader<'a> {
    reader: XmlEventReader<&'a [u8]>,
}
impl<'a> DeferredXmlEventReader<'a> {
    pub(crate) fn new(bytes: &'a [u8], parent: ZipResourceObserver) -> Result<Self, StlError> {
        Ok(Self {
            reader: XmlEventReader::with_fixed_disposition(
                bytes,
                parent,
                Limits::default(),
                false,
                FixedDisposition::Deferred,
            )?,
        })
    }
    #[cfg(test)]
    pub(crate) fn new_with_workspace_hooks(
        bytes: &'a [u8],
        parent: ZipResourceObserver,
        before: impl FnOnce(&ZipResourceObserver),
        after: impl FnOnce(&ZipResourceObserver),
    ) -> Result<Self, StlError> {
        Ok(Self {
            reader: XmlEventReader::with_workspace_hooks(
                bytes,
                parent,
                Limits::default(),
                false,
                FixedDisposition::Deferred,
                before,
                after,
            )?,
        })
    }
    pub(crate) fn set_trim_text(&mut self, trim: bool) {
        self.reader.set_trim_text(trim);
    }
    pub(crate) fn next_event(&mut self) -> Result<Event<'_>, DeferredXmlReadError> {
        self.acquire_event(|_| {})
    }
    fn acquire_event(
        &mut self,
        after_inspection: impl FnOnce(&ZipResourceObserver),
    ) -> Result<Event<'_>, DeferredXmlReadError> {
        let parent = self.reader.observer.clone();
        parent
            .check()
            .map_err(|error| DeferredXmlReadError::Terminal(error.into()))?;
        let result = XmlEventReader::read_event_parts(
            &mut self.reader.xml,
            &mut self.reader.scratch,
            &self.reader.observer,
            &mut self.reader.failed,
            after_inspection,
        );
        parent
            .check()
            .map_err(|error| DeferredXmlReadError::Terminal(error.into()))?;
        match result {
            Ok(event) => Ok(event),
            Err(error) => match self.reader.xml.get_mut().fixed_failure.take() {
                Some(reason) => Err(DeferredXmlReadError::Fixed(FixedXmlViolation {
                    reason,
                    parent,
                })),
                None => Err(DeferredXmlReadError::Xml(error)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    struct Fragmented<'a> {
        bytes: &'a [u8],
        chunk: usize,
        calls: Rc<Cell<usize>>,
    }
    impl Read for Fragmented<'_> {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            self.calls.set(self.calls.get() + 1);
            let n = self.chunk.min(self.bytes.len()).min(out.len());
            out[..n].copy_from_slice(&self.bytes[..n]);
            self.bytes = &self.bytes[n..];
            Ok(n)
        }
    }
    fn limits() -> Limits {
        Limits {
            token: 128,
            attribute: 16,
            attributes: 4,
            depth: 4,
            names: 16,
        }
    }
    type RunResult = (
        Result<Vec<String>, quick_xml::Error>,
        ZipResourceObserver,
        Rc<Cell<usize>>,
        usize,
        usize,
    );

    fn run(bytes: &[u8], limits: Limits, chunk: usize) -> RunResult {
        let calls = Rc::new(Cell::new(0));
        let observer = ZipResourceObserver::new();
        let source = Fragmented {
            bytes,
            chunk,
            calls: calls.clone(),
        };
        let mut reader =
            XmlEventReader::with_limits(source, observer.clone(), limits, false).unwrap();
        let capacity = reader.scratch.capacity();
        let mut events = Vec::new();
        let result = loop {
            match reader.next_event() {
                Ok(Event::Eof) => break Ok(events),
                Ok(event) => events.push(format!("{event:?}")),
                Err(error) => break Err(error),
            }
        };
        (result, observer, calls, capacity, reader.scratch.capacity())
    }
    fn stopped(bytes: &[u8], limits: Limits) {
        let (result, observer, _, before, after) = run(bytes, limits, 1);
        assert!(result.is_err());
        assert!(matches!(
            observer.check(),
            Err(crate::zip_bounds::ZipOpenError::ResourceStopped(
                ResourceStop::InputLimitExceeded
            ))
        ));
        assert_eq!(before, after);
    }
    #[test]
    fn xml_guard_wire_token_exact_and_one_over() {
        let l = Limits {
            token: 8,
            ..limits()
        };
        assert!(run(b"<a>12345678</a>", l, 1).0.is_ok());
        stopped(b"<a>123456789</a>", l);
        assert!(run(b"<!--x-->", l, 1).0.is_ok());
        stopped(b"<!--xx-->", l);
        stopped(b"123456789", l);
        stopped(b"<a abcdefghijklmnop", l);
    }
    #[test]
    fn xml_guard_attribute_exact_one_over_and_ignored_values() {
        let l = Limits {
            attribute: 3,
            ..limits()
        };
        assert!(run(b"<a ignored='123'/>", l, 1).0.is_ok());
        stopped(b"<a ignored='1234'/>", l);
        assert!(run(
            b"<a x='&amp;'/>",
            Limits {
                attribute: 5,
                ..limits()
            },
            1
        )
        .0
        .is_ok());
        stopped(
            b"<a x='&amp;'/>",
            Limits {
                attribute: 4,
                ..limits()
            },
        );
    }
    #[test]
    fn xml_guard_attribute_count_before_iteration() {
        let l = Limits {
            attributes: 2,
            ..limits()
        };
        assert!(run(b"<a x='' y=''/>", l, 1).0.is_ok());
        stopped(b"<a x='' y='' z=''/>", l);
    }
    #[test]
    fn xml_guard_depth_and_live_name_limits() {
        let l = Limits {
            depth: 2,
            names: 4,
            ..limits()
        };
        assert!(run(b"<aa><bb/></aa>", l, 1).0.is_ok());
        assert!(run(b"<aa><bb></bb></aa>", l, 1).0.is_ok());
        stopped(b"<aa><bbb></bbb></aa>", l);
        stopped(b"<a><b><c/></b></a>", Limits { depth: 1, ..l });
        assert!(run(b"<a/><a/><a/>", Limits { depth: 1, ..l }, 1).0.is_ok());
    }
    #[test]
    fn xml_guard_all_fragment_sizes_preserve_delimiters_bom_and_utf8() {
        let xml = "\u{feff}<?xml version='1.0'?><a x='a > é'><!-- a > --><![CDATA[x ] > é]]><?p '> ?'?>é&amp;</a>";
        let expected = run(xml.as_bytes(), limits(), CHUNK).0.unwrap();
        for chunk in 1..=xml.len() {
            let (result, observer, _, before, after) = run(xml.as_bytes(), limits(), chunk);
            assert_eq!(result.unwrap(), expected, "chunk {chunk}");
            assert!(observer.check().is_ok());
            assert_eq!(before, after);
        }
    }
    #[test]
    fn xml_guard_doctype_internal_quotes_comments_and_pi() {
        let xml = b"<!DOCTYPE a SYSTEM 'x>[' [<!ENTITY x ']>x'><!-- ]> --><?p ]> ?><!ELEMENT a ANY>]><a/>";
        let expected = run(xml, limits(), CHUNK).0.unwrap();
        for chunk in 1..=xml.len() {
            assert_eq!(run(xml, limits(), chunk).0.unwrap(), expected);
        }
        stopped(
            xml,
            Limits {
                token: 32,
                ..limits()
            },
        );
    }
    #[test]
    fn xml_guard_fill_buf_is_idempotent_partial_consume_commits_only_prefix() {
        let observer = ZipResourceObserver::new();
        let mut reader = LexicalReader::new(&b"abc<a/>"[..], observer.clone(), limits());
        let first = reader.fill_buf().unwrap().to_vec();
        assert_eq!(reader.fill_buf().unwrap(), first);
        assert_eq!(reader.lex.token, 0);
        reader.consume(0);
        reader.consume(2);
        assert_eq!(reader.lex.token, 2);
        assert_eq!(reader.fill_buf().unwrap(), &first[2..]);
        let mut tail = Vec::new();
        reader.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, b"c<a/>");
        assert!(observer.check().is_ok());
    }
    #[test]
    fn xml_guard_valid_event_before_bad_unread_suffix_is_not_prelatched() {
        let observer = ZipResourceObserver::new();
        let mut reader = XmlEventReader::with_limits(
            &b"<a/>123456789"[..],
            observer.clone(),
            Limits {
                token: 8,
                ..limits()
            },
            false,
        )
        .unwrap();
        assert!(matches!(reader.next_event().unwrap(), Event::Empty(_)));
        assert!(observer.check().is_ok());
        assert!(reader.next_event().is_err());
        assert!(observer.check().is_err());
    }
    #[test]
    fn xml_guard_terminal_stop_blocks_buffered_and_underlying_io() {
        let calls = Rc::new(Cell::new(0));
        let observer = ZipResourceObserver::new();
        let mut reader = XmlEventReader::with_limits(
            Fragmented {
                bytes: b"123456789abc",
                chunk: 1,
                calls: calls.clone(),
            },
            observer.clone(),
            Limits {
                token: 8,
                ..limits()
            },
            false,
        )
        .unwrap();
        assert!(reader.next_event().is_err());
        let count = calls.get();
        for _ in 0..3 {
            assert!(reader.next_event().is_err());
        }
        assert_eq!(calls.get(), count);
        assert!(observer.finish::<(), StlError>(Ok(())).is_err());
    }
    #[test]
    fn xml_guard_allocation_bridge_preserves_first_cause_without_reading() {
        let observer = ZipResourceObserver::new();
        assert!(matches!(
            XmlEventReader::with_limits(&b""[..], observer.clone(), limits(), true),
            Err(StlError::AllocationFailed)
        ));
        assert!(matches!(
            observer.check(),
            Err(crate::zip_bounds::ZipOpenError::AllocationFailed)
        ));
        let _ = observer.stop_io(ResourceStop::EnvelopeChanged);
        assert!(matches!(
            observer.check(),
            Err(crate::zip_bounds::ZipOpenError::AllocationFailed)
        ));
    }
    #[test]
    fn xml_guard_ordinary_malformed_errors_are_not_terminal_limits() {
        for xml in [
            &b"<a></b>"[..],
            &b"<a x='x"[..],
            &b"<!--"[..],
            &b"<![CDATA["[..],
            &b"<?p"[..],
        ] {
            let (result, observer, _, before, after) = run(xml, limits(), 1);
            assert!(result.is_err());
            assert!(observer.check().is_ok());
            assert_eq!(before, after);
        }
    }
    #[test]
    fn xml_guard_text_entities_do_not_reset_lexical_run_allowance() {
        stopped(
            b"&amp;&amp;&amp;",
            Limits {
                token: 12,
                ..limits()
            },
        );
        stopped(
            b"         ",
            Limits {
                token: 8,
                ..limits()
            },
        );
    }
    #[test]
    fn xml_guard_raw_name_stack_matches_permissive_sdk_byte_lengths() {
        for name in ["12", "a/b", "a?b", "ns:item", "é:α"] {
            let xml = format!("<{name}></{name}>");
            let exact = Limits {
                names: name.len(),
                ..limits()
            };
            assert!(run(xml.as_bytes(), exact, 1).0.is_ok(), "{name}");
            stopped(
                xml.as_bytes(),
                Limits {
                    names: name.len() - 1,
                    ..exact
                },
            );
        }
    }
    #[test]
    fn xml_guard_all_markup_forms_stop_with_unchanged_scratch() {
        for xml in [
            &b"<!--123456789-->"[..],
            &b"<![CDATA[123456789]]>"[..],
            &b"<?p 123456789?>"[..],
            &b"<!DOCTYPE a SYSTEM '123456789'>"[..],
        ] {
            stopped(
                xml,
                Limits {
                    token: 12,
                    ..limits()
                },
            );
        }
    }
    #[test]
    fn xml_guard_duplicate_attribute_semantics_and_normalization_remain_sdk_owned() {
        let observer = ZipResourceObserver::new();
        let mut reader =
            XmlEventReader::new(&b"<a x='&amp;' x='2'/>"[..], observer.clone()).unwrap();
        let event = reader.next_event().unwrap();
        let Event::Empty(start) = event else {
            panic!("expected empty element");
        };
        let mut attrs = start.attributes();
        assert_eq!(
            attrs
                .next()
                .unwrap()
                .unwrap()
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .unwrap(),
            "&"
        );
        assert!(attrs.next().unwrap().is_err());
        assert!(observer.check().is_ok());
    }
    #[test]
    fn xml_guard_malformed_unquoted_recovery_is_capped_before_event() {
        for whitespace in [" ", "\t", "\r", "\n"] {
            for attributes in [
                format!("x=bad{whitespace}y=bad"),
                format!("=bad{whitespace}x=bad"),
                format!("bad{whitespace}next=bad{whitespace}third=bad"),
            ] {
                let xml = format!("<a {attributes}/>");
                stopped(
                    xml.as_bytes(),
                    Limits {
                        attributes: 1,
                        ..limits()
                    },
                );
            }
        }
        let mut xml = String::from("<a");
        for i in 0..129 {
            xml.push_str(&format!(" x{i}=bad"));
        }
        xml.push_str("/>");
        let observer = ZipResourceObserver::new();
        let mut reader = XmlEventReader::new(xml.as_bytes(), observer.clone()).unwrap();
        assert!(
            reader.next_event().is_err(),
            "must not expose Event/SDK attribute iterator"
        );
        assert!(observer.check().is_err());
    }
    #[test]
    fn xml_guard_duplicate_recovery_inside_quoted_value_is_capped_before_event() {
        let mut xml = String::from("<a x='ok' x='bad");
        for i in 0..129 {
            xml.push_str(&format!(" y{i}=bad"));
        }
        xml.push_str("'/>");
        // Only two XML attributes, but SDK flatten() recovery after duplicate x
        // would discover many keys inside the second quoted value.
        let observer = ZipResourceObserver::new();
        let mut reader = XmlEventReader::new(xml.as_bytes(), observer.clone()).unwrap();
        assert!(reader.next_event().is_err());
        assert!(observer.check().is_err());
    }
    #[test]
    fn xml_guard_below_cap_malformed_attribute_iteration_matches_sdk() {
        fn summaries(start: &quick_xml::events::BytesStart<'_>) -> Vec<String> {
            start
                .attributes()
                .map(|attribute| format!("{attribute:?}"))
                .collect()
        }
        for xml in [
            "<a x=bad y=bad/>",
            "<a =bad x=bad/>",
            "<a bad next=bad third=bad/>",
            "<a x = 'a b' x='bad y=bad z=bad'/>",
            "<a x=''y='' z = 'q'/>",
            "<a x= '>' y =bad/>",
            "<a x'foo bar=bad' y=bad/>",
        ] {
            let mut plain = Reader::from_str(xml);
            let expected = match plain.read_event().unwrap() {
                Event::Empty(start) | Event::Start(start) => summaries(&start),
                _ => panic!("expected start"),
            };
            let mut guarded = XmlEventReader::with_limits(
                xml.as_bytes(),
                ZipResourceObserver::new(),
                Limits {
                    attributes: 8,
                    ..limits()
                },
                false,
            )
            .unwrap();
            let actual = match guarded.next_event().unwrap() {
                Event::Empty(start) | Event::Start(start) => summaries(&start),
                _ => panic!("expected start"),
            };
            assert_eq!(actual, expected, "{xml}");
        }
    }
    #[test]
    fn xml_guard_initial_bom_does_not_reduce_exact_markup_allowance() {
        for markup in ["<!--x-->", "<abcde/>"] {
            let l = Limits {
                token: markup.len(),
                ..limits()
            };
            let expected = run(markup.as_bytes(), l, 1).0.unwrap();
            let xml = format!("\u{feff}{markup}");
            for chunk in 1..=xml.len() {
                assert_eq!(
                    run(xml.as_bytes(), l, chunk).0.unwrap(),
                    expected,
                    "chunk {chunk}"
                );
            }
        }
        for prefix in [&b"\xef\xbb"[..], &b"\xef\xbb\xbe"[..]] {
            let mut bytes = prefix.to_vec();
            bytes.extend_from_slice(b"12345678");
            stopped(
                &bytes,
                Limits {
                    token: 8,
                    ..limits()
                },
            );
        }
        // A BOM anywhere except the recognized beginning remains wire text.
        stopped(
            "x\u{feff}12345".as_bytes(),
            Limits {
                token: 8,
                ..limits()
            },
        );
    }

    #[test]
    fn xml_guard_duplicate_recovery_value_span_premise_is_rejected_before_event() {
        let xml = r#"<a x='0' x=' y="abc' z='defghi"'/>"#;
        let attribute_limit = 12;
        for original in [b"0".as_slice(), b" y=\"abc", b"defghi\""] {
            assert!(original.len() <= attribute_limit);
        }
        let mut plain = Reader::from_str(xml);
        let Event::Empty(start) = plain.read_event().unwrap() else {
            panic!("the ordinary SDK must return this malformed-attribute event");
        };
        let mut attributes = start.attributes();
        assert!(attributes.next().unwrap().is_ok());
        assert!(
            attributes.next().unwrap().is_err(),
            "duplicate x is recoverable"
        );
        let recovered = attributes.next().unwrap().unwrap();
        assert_eq!(recovered.key.as_ref(), b"y");
        assert_eq!(recovered.value.as_ref(), b"abc' z='defghi");
        assert_eq!(recovered.value.len(), 14);
        assert!(recovered.value.len() > attribute_limit);

        let observer = ZipResourceObserver::new();
        let mut guarded = XmlEventReader::with_limits(
            xml.as_bytes(),
            observer.clone(),
            Limits {
                token: 128,
                attribute: attribute_limit,
                attributes: 128,
                ..limits()
            },
            false,
        )
        .unwrap();
        assert!(
            guarded.next_event().is_err(),
            "recovered oversized value must be refused before Event exposure"
        );
        assert!(observer.check().is_err());
    }
    #[test]
    fn xml_guard_sdk_admission_allows_128_and_rejects_129_items() {
        for count in [31, 32, 33, 128, 129] {
            for malformed in [false, true] {
                let mut xml = String::from("<a");
                for i in 0..count {
                    if malformed {
                        xml.push_str(&format!(" x{i}=bad"));
                    } else {
                        xml.push_str(&format!(" x{i}='v'"));
                    }
                }
                xml.push_str("/>");
                let calls = Rc::new(Cell::new(0));
                let observer = ZipResourceObserver::new();
                let mut guarded = XmlEventReader::new(
                    Fragmented {
                        bytes: xml.as_bytes(),
                        chunk: CHUNK,
                        calls: calls.clone(),
                    },
                    observer.clone(),
                )
                .unwrap();
                if count <= 128 {
                    let Event::Empty(start) = guarded.next_event().unwrap() else {
                        panic!("expected bounded event");
                    };
                    assert_eq!(start.attributes().count(), count);
                    assert!(observer.check().is_ok());
                } else {
                    assert!(guarded.next_event().is_err());
                    let reads = calls.get();
                    assert!(guarded.next_event().is_err());
                    assert_eq!(calls.get(), reads);
                    assert!(observer.check().is_err());
                }
            }
        }
    }
    #[test]
    fn xml_guard_sdk_admission_rejects_production_recovered_value_over_4096() {
        let first = "a".repeat(2040);
        let second = "b".repeat(2060);
        let xml = format!("<a x='0' x=' y=\"{first}' z='{second}\"'/>");
        assert!(first.len() + 4 <= 4096 && second.len() + 1 <= 4096);
        let mut plain = Reader::from_str(&xml);
        let Event::Empty(start) = plain.read_event().unwrap() else {
            panic!("expected SDK event");
        };
        let mut attributes = start.attributes();
        assert!(attributes.next().unwrap().is_ok());
        assert!(attributes.next().unwrap().is_err());
        let recovered = attributes.next().unwrap().unwrap();
        assert_eq!(recovered.key.as_ref(), b"y");
        assert!(matches!(&recovered.value, std::borrow::Cow::Borrowed(_)));
        assert!(recovered.value.len() > 4096);
        let calls = Rc::new(Cell::new(0));
        let observer = ZipResourceObserver::new();
        let mut guarded = XmlEventReader::new(
            Fragmented {
                bytes: xml.as_bytes(),
                chunk: 1,
                calls: calls.clone(),
            },
            observer.clone(),
        )
        .unwrap();
        let capacity = guarded.scratch.capacity();
        assert!(
            guarded.next_event().is_err(),
            "must reject before public Event"
        );
        assert_eq!(guarded.scratch.capacity(), capacity);
        let reads = calls.get();
        assert!(guarded.next_event().is_err());
        assert_eq!(calls.get(), reads);
        assert!(observer.check().is_err());
    }
    #[test]
    fn xml_guard_after_inspection_terminal_gate_preserves_first_cause_and_blocks_io() {
        use crate::zip_bounds::ZipOpenError;
        for stop in [
            Some(ResourceStop::InputLimitExceeded),
            Some(ResourceStop::ReadWorkExceeded),
            Some(ResourceStop::EnvelopeChanged),
            None,
        ] {
            let calls = Rc::new(Cell::new(0));
            let observer = ZipResourceObserver::new();
            let mut guarded = XmlEventReader::new(
                Fragmented {
                    bytes: b"<a x='v'/><b/>",
                    chunk: 1,
                    calls: calls.clone(),
                },
                observer.clone(),
            )
            .unwrap();
            let reached = Cell::new(false);
            let result = guarded.next_event_after_inspection(|observer| {
                reached.set(true);
                match stop {
                    Some(reason) => {
                        let _ = observer.stop_io(reason);
                    }
                    None => {
                        let _ = observer.allocation_failed_io();
                    }
                }
                // A later competing failure must never replace the first.
                let _ = observer.stop_io(ResourceStop::InputLimitExceeded);
                let _ = observer.allocation_failed_io();
            });
            assert!(reached.get());
            let Err(quick_xml::Error::Io(error)) = result else {
                panic!("no Event or unrelated error may escape after inspection stop");
            };
            match stop {
                Some(expected) => assert_eq!(ResourceStop::from_io_error(&error), Some(expected)),
                None => {
                    assert!(crate::zip_bounds::ZipAllocationFailed::from_io_error(&error).is_some())
                }
            }
            match (stop, observer.check()) {
                (Some(expected), Err(ZipOpenError::ResourceStopped(actual))) => {
                    assert_eq!(actual, expected)
                }
                (None, Err(ZipOpenError::AllocationFailed)) => {}
                _ => panic!("first terminal cause changed"),
            }
            let reads = calls.get();
            for _ in 0..2 {
                assert!(guarded.next_event().is_err());
            }
            assert_eq!(calls.get(), reads, "no source IO after final-gate stop");
        }
    }
    fn deferred_fixed(
        bytes: &[u8],
        limits: Limits,
        parent: &ZipResourceObserver,
    ) -> FixedXmlViolation {
        let mut reader = DeferredXmlEventReader {
            reader: XmlEventReader::with_fixed_disposition(
                bytes,
                parent.clone(),
                limits,
                false,
                FixedDisposition::Deferred,
            )
            .unwrap(),
        };
        loop {
            match reader.next_event() {
                Ok(Event::Eof) => panic!("fixed predicate not reached"),
                Ok(_) => {}
                Err(DeferredXmlReadError::Fixed(fixed)) => {
                    assert!(parent.check().is_ok());
                    return fixed;
                }
                Err(DeferredXmlReadError::Xml(error)) => panic!("unexpected ordinary XML {error}"),
                Err(DeferredXmlReadError::Terminal(error)) => {
                    panic!("unexpected parent stop {error}")
                }
            }
        }
    }
    #[test]
    fn deferred_fixed_predicates_are_local_until_consumed_activation() {
        let mut token = limits();
        token.token = 8;
        let mut attribute = limits();
        attribute.attribute = 2;
        let mut attributes = limits();
        attributes.attributes = 1;
        let mut depth = limits();
        depth.depth = 1;
        let mut names = limits();
        names.names = 2;
        for (bytes, limits) in [
            (&b"<a>123456789</a>"[..], token),
            (&b"<a x='abc'/>"[..], attribute),
            (&b"<a x='1' y='2'/>"[..], attributes),
            (&b"<aa><b></b></aa>"[..], names),
            (&b"<a><b></b></a>"[..], depth),
        ] {
            let parent = ZipResourceObserver::new();
            let fixed = deferred_fixed(bytes, limits, &parent);
            assert!(matches!(
                fixed.activate(),
                StlError::ResourceStopped(ResourceStop::InputLimitExceeded)
            ));
            assert!(matches!(
                parent.check(),
                Err(crate::zip_bounds::ZipOpenError::ResourceStopped(
                    ResourceStop::InputLimitExceeded
                ))
            ));
        }
    }
    #[test]
    fn deferred_fixed_error_observation_has_no_concrete_resource_payload() {
        let parent = ZipResourceObserver::new();
        let mut limit = limits();
        limit.attribute = 2;
        let mut reader = XmlEventReader::with_fixed_disposition(
            &b"<a x='abc'/>"[..],
            parent.clone(),
            limit,
            false,
            FixedDisposition::Deferred,
        )
        .unwrap();
        let error = reader.next_event().unwrap_err();
        let quick_xml::Error::Io(error) = error else {
            panic!("expected lexical IO boundary");
        };
        assert_eq!(ResourceStop::from_io_error(&error), None);
        assert_eq!(
            crate::zip_bounds::ZipAllocationFailed::from_io_error(&error),
            None
        );
        parent.observe_io(&error);
        assert!(parent.check().is_ok());
        assert!(reader.xml.get_ref().fixed_failure.is_some());
    }
    #[test]
    fn deferred_activation_never_overwrites_real_parent_first_cause() {
        for stop in [
            Some(ResourceStop::InputLimitExceeded),
            Some(ResourceStop::ReadWorkExceeded),
            Some(ResourceStop::EnvelopeChanged),
            None,
        ] {
            let parent = ZipResourceObserver::new();
            let mut limit = limits();
            limit.attribute = 2;
            let fixed = deferred_fixed(b"<a x='abc'/>", limit, &parent);
            let expected = match stop {
                Some(stop) => {
                    let _ = parent.stop_io(stop);
                    StlError::ResourceStopped(stop)
                }
                None => {
                    let _ = parent.allocation_failed_io();
                    StlError::AllocationFailed
                }
            };
            assert_eq!(fixed.activate(), expected);
        }
    }
    #[test]
    fn deferred_workspace_failure_and_real_inspection_stop_remain_immediate() {
        let parent = ZipResourceObserver::new();
        let result = XmlEventReader::with_fixed_disposition(
            &b"<a/>"[..],
            parent.clone(),
            limits(),
            true,
            FixedDisposition::Deferred,
        );
        assert!(matches!(result, Err(StlError::AllocationFailed)));
        assert!(matches!(
            parent.check(),
            Err(crate::zip_bounds::ZipOpenError::AllocationFailed)
        ));
        for stop in [
            Some(ResourceStop::InputLimitExceeded),
            Some(ResourceStop::ReadWorkExceeded),
            Some(ResourceStop::EnvelopeChanged),
            None,
        ] {
            let parent = ZipResourceObserver::new();
            let mut reader = DeferredXmlEventReader::new(b"<a/>", parent.clone()).unwrap();
            let expected = match stop {
                Some(stop) => StlError::ResourceStopped(stop),
                None => StlError::AllocationFailed,
            };
            let result = reader.acquire_event(|observer| {
                if let Some(stop) = stop {
                    let _ = observer.stop_io(stop);
                } else {
                    let _ = observer.allocation_failed_io();
                }
            });
            match result {
                Err(DeferredXmlReadError::Terminal(error)) => assert_eq!(error, expected),
                _ => panic!("tentative event escaped real stop"),
            }
            let remaining = reader.reader.xml.get_ref().inner.len();
            assert!(matches!(
                reader.next_event(),
                Err(DeferredXmlReadError::Terminal(_))
            ));
            assert_eq!(reader.reader.xml.get_ref().inner.len(), remaining);
        }
    }
}
