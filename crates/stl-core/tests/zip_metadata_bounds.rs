//! Opt-in dependency constructor tests. These do not migrate model readers.
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use zip::read::{Config, MetadataLimitKind, MetadataLimits};
use zip::ZipArchive;

fn u16le(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
fn u32le(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
fn u64le(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 != 0 { 0xedb88320 } else { 0 };
        }
    }
    !crc
}
#[derive(Clone)]
struct Entry {
    name: Vec<u8>,
    data: Vec<u8>,
    extra: Vec<u8>,
    flags: u16,
}
fn entry(name: &[u8]) -> Entry {
    Entry {
        name: name.to_vec(),
        data: b"mesh".to_vec(),
        extra: Vec::new(),
        flags: 0,
    }
}
fn package(entries: &[Entry], comment: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut offsets = Vec::new();
    for e in entries {
        offsets.push(bytes.len() as u32);
        u32le(&mut bytes, 0x04034b50);
        u16le(&mut bytes, 20);
        u16le(&mut bytes, e.flags);
        u16le(&mut bytes, 0);
        u16le(&mut bytes, 0);
        u16le(&mut bytes, 0);
        u32le(&mut bytes, crc32(&e.data));
        u32le(&mut bytes, e.data.len() as u32);
        u32le(&mut bytes, e.data.len() as u32);
        u16le(&mut bytes, e.name.len() as u16);
        u16le(&mut bytes, 0);
        bytes.extend_from_slice(&e.name);
        bytes.extend_from_slice(&e.data);
    }
    let start = bytes.len() as u32;
    for (e, offset) in entries.iter().zip(offsets) {
        u32le(&mut bytes, 0x02014b50);
        u16le(&mut bytes, 20);
        u16le(&mut bytes, 20);
        u16le(&mut bytes, e.flags);
        u16le(&mut bytes, 0);
        u16le(&mut bytes, 0);
        u16le(&mut bytes, 0);
        u32le(&mut bytes, crc32(&e.data));
        u32le(&mut bytes, e.data.len() as u32);
        u32le(&mut bytes, e.data.len() as u32);
        u16le(&mut bytes, e.name.len() as u16);
        u16le(&mut bytes, e.extra.len() as u16);
        u16le(&mut bytes, 0);
        u16le(&mut bytes, 0);
        u16le(&mut bytes, 0);
        u32le(&mut bytes, 0);
        u32le(&mut bytes, offset);
        bytes.extend_from_slice(&e.name);
        bytes.extend_from_slice(&e.extra);
    }
    let size = bytes.len() as u32 - start;
    footer(&mut bytes, entries.len() as u16, size, start, comment);
    bytes
}
fn footer(bytes: &mut Vec<u8>, count: u16, size: u32, offset: u32, comment: &[u8]) {
    u32le(bytes, 0x06054b50);
    u16le(bytes, 0);
    u16le(bytes, 0);
    u16le(bytes, count);
    u16le(bytes, count);
    u32le(bytes, size);
    u32le(bytes, offset);
    u16le(bytes, comment.len() as u16);
    bytes.extend_from_slice(comment);
}
fn end(bytes: &[u8]) -> usize {
    bytes.windows(4).rposition(|b| b == b"PK\x05\x06").unwrap()
}
fn get32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn set16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn set32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn bounded(
    bytes: Vec<u8>,
    limits: MetadataLimits,
) -> zip::result::ZipResult<ZipArchive<Cursor<Vec<u8>>>> {
    ZipArchive::with_metadata_limits(Config::default(), Cursor::new(bytes), limits)
}
fn limit(bytes: Vec<u8>, limits: MetadataLimits) -> MetadataLimitKind {
    bounded(bytes, limits)
        .unwrap_err()
        .metadata_limit()
        .expect("typed constructor exhaustion")
        .kind
}
fn zip64(mut bytes: Vec<u8>, extension: &[u8]) -> Vec<u8> {
    let old = end(&bytes);
    let count = u16::from_le_bytes(bytes[old + 10..old + 12].try_into().unwrap());
    let size = get32(&bytes, old + 12);
    let offset = get32(&bytes, old + 16);
    bytes.truncate(old);
    let z64 = bytes.len() as u64;
    u32le(&mut bytes, 0x06064b50);
    u64le(&mut bytes, 44 + extension.len() as u64);
    u16le(&mut bytes, 45);
    u16le(&mut bytes, 45);
    u32le(&mut bytes, 0);
    u32le(&mut bytes, 0);
    u64le(&mut bytes, u64::from(count));
    u64le(&mut bytes, u64::from(count));
    u64le(&mut bytes, u64::from(size));
    u64le(&mut bytes, u64::from(offset));
    bytes.extend_from_slice(extension);
    u32le(&mut bytes, 0x07064b50);
    u32le(&mut bytes, 0);
    u64le(&mut bytes, z64);
    u32le(&mut bytes, 1);
    footer(&mut bytes, u16::MAX, u32::MAX, u32::MAX, b"");
    bytes
}

#[test]
fn zip_metadata_bounds_encoded_exact_and_one_over() {
    let bytes = package(&[entry(b"a")], b"abc");
    // Footer, central record, then eagerly admitted local fixed header/name.
    let exact = 22 + 3 + 46 + 1 + 30 + 1;
    assert!(bounded(
        bytes.clone(),
        MetadataLimits {
            max_encoded_bytes: exact,
            ..MetadataLimits::default()
        }
    )
    .is_ok());
    assert_eq!(
        limit(
            bytes,
            MetadataLimits {
                max_encoded_bytes: exact - 1,
                ..MetadataLimits::default()
            }
        ),
        MetadataLimitKind::EncodedBytes
    );
}
#[test]
fn zip_metadata_bounds_counts_physical_records_before_duplicate_dedup() {
    let bytes = package(&[entry(b"same"), entry(b"same")], b"");
    assert_eq!(
        ZipArchive::new(Cursor::new(bytes.clone())).unwrap().len(),
        1
    );
    assert_eq!(
        limit(
            bytes.clone(),
            MetadataLimits {
                max_entries: 1,
                ..MetadataLimits::default()
            }
        ),
        MetadataLimitKind::Entries
    );
    assert_eq!(
        bounded(
            bytes,
            MetadataLimits {
                max_entries: 2,
                ..MetadataLimits::default()
            }
        )
        .unwrap()
        .len(),
        1
    );
}
#[test]
fn zip_metadata_bounds_capacity_zero_fallback_cannot_bypass_count() {
    let original = package(&[entry(b"a"), entry(b"b")], b"");
    let old = end(&original);
    let start = get32(&original, old + 16) as usize;
    let mut central_only = original[start..old].to_vec();
    let size = central_only.len() as u32;
    footer(&mut central_only, 2, size, 0, b"");
    assert_eq!(
        limit(
            central_only,
            MetadataLimits {
                max_entries: 1,
                ..MetadataLimits::default()
            }
        ),
        MetadataLimitKind::Entries
    );
}
#[test]
fn zip_metadata_bounds_rejects_declared_count_and_directory_range_mismatch() {
    let bytes = package(&[entry(b"a"), entry(b"b")], b"");
    let mut under = bytes.clone();
    let e = end(&under);
    set16(&mut under, e + 8, 1);
    set16(&mut under, e + 10, 1);
    assert!(bounded(under, MetadataLimits::default()).is_err());
    let mut small = bytes;
    let e = end(&small);
    set32(&mut small, e + 12, 46);
    assert!(bounded(small, MetadataLimits::default()).is_err());
}
#[test]
fn zip_metadata_bounds_comments_and_zip64_extension_admit_before_allocation() {
    assert_eq!(
        limit(
            package(&[], &[0; 128]),
            MetadataLimits {
                max_encoded_bytes: 22,
                ..MetadataLimits::default()
            }
        ),
        MetadataLimitKind::EncodedBytes
    );
    let large = zip64(package(&[entry(b"a")], b""), &[0; 256]);
    assert_eq!(
        limit(
            large,
            MetadataLimits {
                max_encoded_bytes: 100,
                ..MetadataLimits::default()
            }
        ),
        MetadataLimitKind::EncodedBytes
    );
    assert_eq!(
        limit(
            package(&[], b""),
            MetadataLimits {
                max_allocation_work: 16 * 1024,
                ..MetadataLimits::default()
            }
        ),
        MetadataLimitKind::AllocationWork
    );
}
#[test]
fn zip_metadata_bounds_fatal_budget_survives_older_footer_retry_and_last_error() {
    let mut bytes = package(&[entry(b"a")], &[0; 100]);
    footer(&mut bytes, 1, 0, 0, b"");
    assert_eq!(
        limit(
            bytes.clone(),
            MetadataLimits {
                max_candidates: 1,
                ..MetadataLimits::default()
            }
        ),
        MetadataLimitKind::Candidates
    );
    assert_eq!(
        limit(
            bytes,
            MetadataLimits {
                max_encoded_bytes: 44,
                ..MetadataLimits::default()
            }
        ),
        MetadataLimitKind::EncodedBytes
    );
}
#[test]
fn zip_metadata_bounds_read_and_io_call_caps_are_typed() {
    let bytes = package(&[entry(b"a")], b"");
    assert_eq!(
        limit(
            bytes.clone(),
            MetadataLimits {
                max_read_bytes: 1,
                ..MetadataLimits::default()
            }
        ),
        MetadataLimitKind::ReadBytes
    );
    assert_eq!(
        limit(
            bytes,
            MetadataLimits {
                max_io_calls: 1,
                ..MetadataLimits::default()
            }
        ),
        MetadataLimitKind::IoCalls
    );
}
#[derive(Debug)]
struct ShortReads(Cursor<Vec<u8>>);
impl Read for ShortReads {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        let length = b.len().min(1);
        self.0.read(&mut b[..length])
    }
}
impl Seek for ShortReads {
    fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
        self.0.seek(p)
    }
}
#[test]
fn zip_metadata_bounds_short_reads_cannot_bypass_operation_budget() {
    let error = ZipArchive::with_metadata_limits(
        Config::default(),
        ShortReads(Cursor::new(package(&[entry(b"a")], b""))),
        MetadataLimits {
            max_io_calls: 8,
            ..MetadataLimits::default()
        },
    )
    .unwrap_err();
    assert_eq!(
        error.metadata_limit().unwrap().kind,
        MetadataLimitKind::IoCalls
    );
}
struct MutableFooter {
    inner: Cursor<Vec<u8>>,
    footer: usize,
    changed: bool,
}
impl Read for MutableFooter {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        if self.inner.position() == self.footer as u64 && !self.changed {
            set16(self.inner.get_mut(), self.footer + 8, 3);
            set16(self.inner.get_mut(), self.footer + 10, 3);
            self.changed = true;
        }
        self.inner.read(b)
    }
}
impl Seek for MutableFooter {
    fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
        self.inner.seek(p)
    }
}
#[test]
fn zip_metadata_bounds_checks_mutated_fields_at_actual_use_not_prior_preflight() {
    let bytes = package(&[entry(b"a")], b"");
    assert_eq!(
        ZipArchive::new(Cursor::new(bytes.clone())).unwrap().len(),
        1
    );
    let footer = end(&bytes);
    let source = MutableFooter {
        inner: Cursor::new(bytes),
        footer,
        changed: false,
    };
    let error = ZipArchive::with_metadata_limits(
        Config::default(),
        source,
        MetadataLimits {
            max_entries: 2,
            ..MetadataLimits::default()
        },
    )
    .err()
    .unwrap();
    assert_eq!(
        error.metadata_limit().unwrap().kind,
        MetadataLimitKind::Entries
    );
}
#[test]
fn zip_metadata_bounds_preserves_stored_deflate_unicode_cp437_and_small_zip64() {
    let mut cp = entry(&[0x82, b'.', b's', b't', b'l']);
    cp.data = b"cp437".to_vec();
    let mut utf = entry("café.stl".as_bytes());
    utf.flags = 0x0800;
    for bytes in [
        package(&[cp], b"comment"),
        package(&[utf], b""),
        zip64(package(&[entry(b"a")], b""), &[0; 8]),
    ] {
        let mut prefixed = b"leading junk".to_vec();
        prefixed.extend_from_slice(&bytes);
        let mut archive = bounded(prefixed, MetadataLimits::default()).unwrap();
        let mut value = Vec::new();
        archive
            .by_index(0)
            .unwrap()
            .read_to_end(&mut value)
            .unwrap();
        assert!(!value.is_empty());
    }
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(
            "a",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
    writer.write_all(b"deflated model").unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    let mut archive = bounded(bytes, MetadataLimits::default()).unwrap();
    let mut text = String::new();
    archive
        .by_name("a")
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "deflated model");
}
#[test]
fn zip_metadata_bounds_unicode_extra_and_truncated_nested_lengths() {
    let mut e = entry(b"ascii");
    let replacement = "modèle.stl".as_bytes();
    u16le(&mut e.extra, 0x7075);
    u16le(&mut e.extra, (5 + replacement.len()) as u16);
    e.extra.push(1);
    u32le(&mut e.extra, crc32(&e.name));
    e.extra.extend_from_slice(replacement);
    let mut archive = bounded(package(&[e], b""), MetadataLimits::default()).unwrap();
    assert!(archive.by_name("modèle.stl").is_ok());
    let mut e = entry(b"a");
    e.extra = vec![0x75, 0x70, 0xff, 0xff];
    assert!(bounded(package(&[e], b""), MetadataLimits::default()).is_err());
}
#[test]
fn zip_metadata_bounds_do_not_limit_member_payload_after_constructor() {
    let mut e = entry(b"a");
    e.data = vec![7; 8192];
    let mut archive = bounded(
        package(&[e], b""),
        MetadataLimits {
            max_read_bytes: 4096,
            ..MetadataLimits::default()
        },
    )
    .unwrap();
    let mut bytes = Vec::new();
    archive
        .by_index(0)
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, vec![7; 8192]);
}

#[test]
fn zip_metadata_bounds_zip64_inner_candidates_share_the_outer_budget() {
    let bytes = zip64(package(&[entry(b"a")], b""), b"");
    assert_eq!(
        limit(
            bytes.clone(),
            MetadataLimits {
                max_candidates: 1,
                ..MetadataLimits::default()
            }
        ),
        MetadataLimitKind::Candidates
    );
    assert!(bounded(
        bytes,
        MetadataLimits {
            max_candidates: 2,
            ..MetadataLimits::default()
        }
    )
    .is_ok());
}
#[test]
fn zip_metadata_bounds_zip64_locator_cannot_swallow_read_exhaustion() {
    let bytes = zip64(package(&[entry(b"a")], b""), b"");
    // One backwards window covers this small fixture; the next 22 bytes parse
    // the ZIP32 footer. Exhaustion then happens while reading its ZIP64 locator.
    let work = bytes.len() as u64 + 22;
    assert_eq!(
        limit(
            bytes,
            MetadataLimits {
                max_read_bytes: work,
                ..MetadataLimits::default()
            }
        ),
        MetadataLimitKind::ReadBytes
    );
}
#[test]
fn zip_metadata_bounds_decoded_names_and_extra_copies_have_separate_allocation_admission() {
    let e = entry(&vec![0x82; 4096]);
    assert_eq!(
        limit(
            package(&[e], b""),
            MetadataLimits {
                max_allocation_work: 64 * 1024,
                ..MetadataLimits::default()
            }
        ),
        MetadataLimitKind::AllocationWork
    );
    let mut e = entry(b"a");
    u16le(&mut e.extra, 0xffff);
    u16le(&mut e.extra, 600);
    e.extra.extend_from_slice(&[0; 600]);
    assert_eq!(
        limit(
            package(&[e], b""),
            MetadataLimits {
                max_allocation_work: 64 * 1024,
                ..MetadataLimits::default()
            }
        ),
        MetadataLimitKind::AllocationWork
    );
}

#[derive(Debug)]
struct OomAt {
    inner: Cursor<Vec<u8>>,
    position: u64,
}
impl Read for OomAt {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.inner.position() == self.position {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "private injected allocator detail",
            ));
        }
        self.inner.read(output)
    }
}
impl Seek for OomAt {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.inner.seek(from)
    }
}
#[test]
fn zip_metadata_bounds_allocator_failure_stays_operational_through_locator_and_retry() {
    let z64 = zip64(package(&[entry(b"a")], b""), b"");
    let locator = end(&z64) - 20;
    let mut retried = package(&[entry(b"a")], b"");
    let older = end(&retried);
    footer(&mut retried, 1, 0, 0, b"");
    for (bytes, position) in [(z64, locator), (retried, older)] {
        let error = ZipArchive::with_metadata_limits(
            Config::default(),
            OomAt {
                inner: Cursor::new(bytes),
                position: position as u64,
            },
            MetadataLimits::default(),
        )
        .err()
        .unwrap();
        assert!(error.metadata_limit().is_none());
        assert!(error.metadata_allocation_failure().is_some());
        assert!(!error.to_string().contains("private injected"));
        assert!(
            matches!(error,zip::result::ZipError::Io(ref error) if error.kind()==io::ErrorKind::OutOfMemory)
        );
    }
}
#[test]
fn zip_metadata_bounds_single_volume_counts_and_disk_numbers_are_consistent() {
    let original = package(&[entry(b"a"), entry(b"b")], b"");
    let e = end(&original);
    let mut unequal = original.clone();
    set16(&mut unequal, e + 10, 1);
    assert_eq!(
        ZipArchive::new(Cursor::new(unequal.clone())).unwrap().len(),
        2
    );
    assert!(bounded(unequal, MetadataLimits::default()).is_err());
    let mut split = original.clone();
    set16(&mut split, e + 4, 1);
    set16(&mut split, e + 6, 1);
    assert!(bounded(split, MetadataLimits::default()).is_err());
    let mut split = original;
    let cd = get32(&split, e + 16) as usize;
    set16(&mut split, cd + 34, 1);
    assert!(bounded(split, MetadataLimits::default()).is_err());
    let original = zip64(package(&[entry(b"a")], b""), b"");
    let z = original
        .windows(4)
        .position(|p| p == b"PK\x06\x06")
        .unwrap();
    let l = end(&original) - 20;
    let mut unequal = original.clone();
    unequal[z + 24..z + 32].copy_from_slice(&0u64.to_le_bytes());
    assert!(bounded(unequal, MetadataLimits::default()).is_err());
    for (offset, value) in [(l + 4, 1), (l + 16, 2)] {
        let mut split = original.clone();
        set32(&mut split, offset, value);
        assert!(bounded(split, MetadataLimits::default()).is_err());
    }
}
#[test]
fn zip_metadata_bounds_zip64_entry_disk_sentinel_requires_explicit_zero() {
    for disk in [0, 1] {
        let mut e = entry(b"a");
        u16le(&mut e.extra, 1);
        u16le(&mut e.extra, 4);
        u32le(&mut e.extra, disk);
        let mut bytes = package(&[e], b"");
        let footer = end(&bytes);
        let cd = get32(&bytes, footer + 16) as usize;
        set16(&mut bytes, cd + 34, u16::MAX);
        assert_eq!(bounded(bytes, MetadataLimits::default()).is_ok(), disk == 0);
    }
}
struct MutableCentral {
    inner: Cursor<Vec<u8>>,
    central: usize,
    footer: usize,
    field: usize,
    footer_read: bool,
    variable_read: std::rc::Rc<std::cell::Cell<bool>>,
}
impl Read for MutableCentral {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let position = self.inner.position() as usize;
        if position == self.footer {
            self.footer_read = true;
        }
        if self.footer_read && position == self.central {
            if self.field == 30 {
                set16(self.inner.get_mut(), self.central + 28, 1);
                set16(self.inner.get_mut(), self.central + 30, 256);
            } else {
                set16(self.inner.get_mut(), self.central + self.field, u16::MAX);
            }
        }
        if self.footer_read && position == self.central + 46 {
            self.variable_read.set(true);
        }
        self.inner.read(output)
    }
}
impl Seek for MutableCentral {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.inner.seek(from)
    }
}
#[test]
fn zip_metadata_bounds_mutated_central_lengths_fail_before_variable_buffer_reads() {
    let bytes = package(&[entry(&vec![b'a'; 257])], b"");
    let footer = end(&bytes);
    let central = get32(&bytes, footer + 16) as usize;
    assert!(bounded(
        bytes.clone(),
        MetadataLimits {
            max_allocation_work: 40 * 1024,
            ..MetadataLimits::default()
        }
    )
    .is_ok());
    for field in [28, 30, 32] {
        let variable_read = std::rc::Rc::new(std::cell::Cell::new(false));
        let source = MutableCentral {
            inner: Cursor::new(bytes.clone()),
            central,
            footer,
            field,
            footer_read: false,
            variable_read: variable_read.clone(),
        };
        let error = ZipArchive::with_metadata_limits(
            Config::default(),
            source,
            MetadataLimits {
                max_allocation_work: 40 * 1024,
                ..MetadataLimits::default()
            },
        )
        .err()
        .unwrap();
        assert!(!variable_read.get(), "field {field}");
        if field == 30 {
            assert_eq!(
                error.metadata_limit().unwrap().kind,
                MetadataLimitKind::AllocationWork
            );
        }
    }
}

fn add_local_extra(mut bytes: Vec<u8>, extra: &[u8]) -> Vec<u8> {
    let name = usize::from(u16::from_le_bytes(bytes[26..28].try_into().unwrap()));
    bytes.splice(30 + name..30 + name, extra.iter().copied());
    set16(&mut bytes, 28, extra.len() as u16);
    let footer = end(&bytes);
    let central = get32(&bytes, footer + 16);
    set32(&mut bytes, footer + 16, central + extra.len() as u32);
    bytes
}
#[test]
fn zip_metadata_bounds_local_identity_fields_and_ranges_are_checked() {
    let original = package(&[entry(b"a")], b"");
    for (offset, value) in [(6, 8), (8, 8), (26, 2), (28, u16::MAX)] {
        let mut bytes = original.clone();
        set16(&mut bytes, offset, value);
        assert!(
            bounded(bytes, MetadataLimits::default()).is_err(),
            "field {offset}"
        );
    }
    for offset in [14, 18, 22] {
        let mut bytes = original.clone();
        set32(&mut bytes, offset, 9);
        assert!(
            bounded(bytes, MetadataLimits::default()).is_err(),
            "field {offset}"
        );
    }
    let mut bytes = original.clone();
    bytes[30] = b'b';
    assert!(bounded(bytes, MetadataLimits::default()).is_err());
    let footer = end(&original);
    let central = get32(&original, footer + 16) as usize;
    for header in [central as u32, u32::MAX - 10] {
        let mut bytes = original.clone();
        set32(&mut bytes, central + 42, header);
        assert!(bounded(bytes, MetadataLimits::default()).is_err());
    }
    let mut bytes = original;
    set32(&mut bytes, central + 20, u32::MAX - 1);
    assert!(bounded(bytes, MetadataLimits::default()).is_err());
}
#[test]
fn zip_metadata_bounds_local_zip64_descriptor_placeholders_and_empty_members() {
    let mut empty = entry(b"folder/");
    empty.data.clear();
    assert!(bounded(package(&[empty], b""), MetadataLimits::default()).is_ok());
    for descriptor in [false, true] {
        let mut e = entry(b"a");
        e.flags = if descriptor { 8 } else { 0 };
        let base = package(&[e], b"");
        for value in [0, 4, u64::MAX] {
            let mut extra = vec![1, 0, 16, 0];
            u64le(&mut extra, value);
            u64le(&mut extra, value);
            let mut bytes = add_local_extra(base.clone(), &extra);
            set32(&mut bytes, 18, u32::MAX);
            set32(&mut bytes, 22, u32::MAX);
            if descriptor {
                set32(&mut bytes, 14, 0);
            }
            assert_eq!(
                bounded(bytes, MetadataLimits::default()).is_ok(),
                descriptor || value == 4
            );
        }
        let mut bytes = base.clone();
        set32(&mut bytes, 18, u32::MAX);
        set32(&mut bytes, 22, u32::MAX);
        assert_eq!(
            bounded(bytes, MetadataLimits::default()).is_ok(),
            descriptor
        );
        let mut bytes = base.clone();
        set32(&mut bytes, 18, 0);
        set32(&mut bytes, 22, 0);
        assert_eq!(
            bounded(bytes, MetadataLimits::default()).is_ok(),
            descriptor
        );
        let mut bytes = base;
        set32(&mut bytes, 18, 3);
        assert!(bounded(bytes, MetadataLimits::default()).is_err());
    }
    for extra in [
        vec![1, 0, 7, 0, 0, 0, 0, 0, 0, 0, 0],
        vec![2, 0, 255, 255],
        vec![1, 0, 0, 0, 1, 0, 0, 0],
    ] {
        let mut bytes = add_local_extra(package(&[entry(b"a")], b""), &extra);
        set32(&mut bytes, 18, u32::MAX);
        assert!(bounded(bytes, MetadataLimits::default()).is_err());
    }
}
#[test]
fn zip_metadata_bounds_local_ranges_are_admitted_before_duplicate_dedup() {
    let original = package(&[entry(b"a"), entry(b"a")], b"");
    assert_eq!(
        bounded(original.clone(), MetadataLimits::default())
            .unwrap()
            .len(),
        1
    );
    let footer = end(&original);
    let central = get32(&original, footer + 16) as usize;
    let second = central + 47;
    let mut alias = original.clone();
    set32(&mut alias, second + 42, 0);
    assert!(bounded(alias.clone(), MetadataLimits::default()).is_err());
    assert_eq!(ZipArchive::new(Cursor::new(alias)).unwrap().len(), 1);
    // A concrete central/local size can overlap the next physical header even
    // when each individual member still ends before the central directory.
    let mut overlapping = original;
    set32(&mut overlapping, 18, 10);
    set32(&mut overlapping, 22, 10);
    set32(&mut overlapping, central + 20, 10);
    set32(&mut overlapping, central + 24, 10);
    assert!(bounded(overlapping, MetadataLimits::default()).is_err());
}

struct LocalMutation {
    inner: Cursor<Vec<u8>>,
    local_seen: bool,
    footer_seen: bool,
    field: usize,
    variable_read: std::rc::Rc<std::cell::Cell<bool>>,
}
impl Read for LocalMutation {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let footer = end(self.inner.get_ref()) as u64;
        if self.inner.position() == footer {
            self.footer_seen = true;
        }
        if self.footer_seen && self.inner.position() == 0 {
            self.local_seen = true;
            set16(self.inner.get_mut(), self.field, u16::MAX);
        }
        if self.local_seen && self.inner.position() == 30 {
            self.variable_read.set(true);
        }
        self.inner.read(output)
    }
}
impl Seek for LocalMutation {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.inner.seek(from)
    }
}
#[test]
fn zip_metadata_bounds_actual_local_lengths_are_checked_before_variable_reads() {
    for field in [26, 28] {
        let variable_read = std::rc::Rc::new(std::cell::Cell::new(false));
        let source = LocalMutation {
            inner: Cursor::new(package(&[entry(b"a")], b"")),
            local_seen: false,
            footer_seen: false,
            field,
            variable_read: variable_read.clone(),
        };
        assert!(ZipArchive::with_metadata_limits(
            Config::default(),
            source,
            MetadataLimits::default()
        )
        .is_err());
        assert!(!variable_read.get());
    }
}

#[derive(Clone, Debug)]
struct SharedChangingSource {
    bytes: std::rc::Rc<std::cell::RefCell<Vec<u8>>>,
    position: u64,
    local_reads: std::rc::Rc<std::cell::Cell<usize>>,
}
impl Read for SharedChangingSource {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.position == 0 {
            self.local_reads.set(self.local_reads.get() + 1);
        }
        let bytes = self.bytes.borrow();
        let start = usize::try_from(self.position)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        let count = output.len().min(bytes.len() - start);
        output[..count].copy_from_slice(&bytes[start..start + count]);
        self.position += count as u64;
        Ok(count)
    }
}
impl Seek for SharedChangingSource {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let next = match from {
            SeekFrom::Start(value) => i128::from(value),
            SeekFrom::End(value) => self.bytes.borrow().len() as i128 + i128::from(value),
            SeekFrom::Current(value) => i128::from(self.position) + i128::from(value),
        };
        self.position =
            u64::try_from(next).map_err(|_| io::Error::other("invalid synthetic seek"))?;
        Ok(self.position)
    }
}
#[test]
fn zip_metadata_bounds_cached_local_ranges_do_not_grow_after_source_mutation() {
    let bytes = std::rc::Rc::new(std::cell::RefCell::new(package(&[entry(b"a")], b"")));
    let local_reads = std::rc::Rc::new(std::cell::Cell::new(0));
    let source = SharedChangingSource {
        bytes: bytes.clone(),
        position: 0,
        local_reads: local_reads.clone(),
    };
    let mut archive =
        ZipArchive::with_metadata_limits(Config::default(), source, MetadataLimits::default())
            .unwrap();
    let admitted_reads = local_reads.get();
    set16(&mut bytes.borrow_mut(), 26, u16::MAX);
    set16(&mut bytes.borrow_mut(), 28, u16::MAX);
    for _ in 0..2 {
        let mut normal = Vec::new();
        archive
            .by_index(0)
            .unwrap()
            .read_to_end(&mut normal)
            .unwrap();
        assert_eq!(normal, b"mesh");
        let mut raw = Vec::new();
        archive
            .by_index_raw(0)
            .unwrap()
            .read_to_end(&mut raw)
            .unwrap();
        assert_eq!(raw, b"mesh");
        let mut seek = Vec::new();
        archive
            .by_index_seek(0)
            .unwrap()
            .read_to_end(&mut seek)
            .unwrap();
        assert_eq!(seek, b"mesh");
    }
    assert_eq!(local_reads.get(), admitted_reads);
}

#[test]
fn zip_metadata_bounds_descriptor_gaps_are_not_required_or_interpreted() {
    for gap in [
        vec![],
        vec![0; 12],
        vec![0x50, 0x4b, 0x07, 0x08, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    ] {
        let mut e = entry(b"a");
        e.flags = 8;
        let mut bytes = package(&[e], b"");
        set32(&mut bytes, 14, 0);
        set32(&mut bytes, 18, 0);
        set32(&mut bytes, 22, 0);
        let footer = end(&bytes);
        let central = get32(&bytes, footer + 16) as usize;
        bytes.splice(central..central, gap.iter().copied());
        let footer = end(&bytes);
        set32(&mut bytes, footer + 16, (central + gap.len()) as u32);
        let mut archive = bounded(bytes, MetadataLimits::default()).unwrap();
        let mut payload = Vec::new();
        archive
            .by_index(0)
            .unwrap()
            .read_to_end(&mut payload)
            .unwrap();
        assert_eq!(payload, b"mesh");
    }
}
#[derive(Clone, Copy)]
enum LocalFault {
    None,
    ReadOom,
    RestoreOom,
    RestoreIo,
}
struct LocalWork {
    inner: Cursor<Vec<u8>>,
    footer_seen: bool,
    local_started: bool,
    read_bytes: u64,
    local_prefix: std::rc::Rc<std::cell::Cell<u64>>,
    fault: LocalFault,
}
impl Read for LocalWork {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.inner.position() == end(self.inner.get_ref()) as u64 {
            self.footer_seen = true;
        }
        if self.footer_seen && self.inner.position() == 0 {
            self.local_started = true;
            self.local_prefix.set(self.read_bytes);
            if matches!(self.fault, LocalFault::ReadOom) {
                return Err(io::Error::new(
                    io::ErrorKind::OutOfMemory,
                    "private local read detail",
                ));
            }
        }
        let count = self.inner.read(output)?;
        self.read_bytes += count as u64;
        Ok(count)
    }
}
impl Seek for LocalWork {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let footer = end(self.inner.get_ref()) as u64;
        if self.local_started && matches!(from, SeekFrom::Start(value) if value==footer) {
            match self.fault {
                LocalFault::RestoreOom => {
                    return Err(io::Error::new(
                        io::ErrorKind::OutOfMemory,
                        "private restore detail",
                    ))
                }
                LocalFault::RestoreIo => {
                    return Err(io::Error::other("synthetic central restore failed"))
                }
                _ => {}
            }
        }
        self.inner.seek(from)
    }
}
fn local_work(
    bytes: Vec<u8>,
    fault: LocalFault,
    prefix: std::rc::Rc<std::cell::Cell<u64>>,
) -> LocalWork {
    LocalWork {
        inner: Cursor::new(bytes),
        footer_seen: false,
        local_started: false,
        read_bytes: 0,
        local_prefix: prefix,
        fault,
    }
}
#[test]
fn zip_metadata_bounds_local_work_uses_shared_limits_and_restoration_failures_are_fatal() {
    let bytes = package(&[entry(b"a")], b"");
    let prefix = std::rc::Rc::new(std::cell::Cell::new(0));
    assert!(ZipArchive::with_metadata_limits(
        Config::default(),
        local_work(bytes.clone(), LocalFault::None, prefix.clone()),
        MetadataLimits::default()
    )
    .is_ok());
    assert!(prefix.get() > 0);
    let error = ZipArchive::with_metadata_limits(
        Config::default(),
        local_work(bytes.clone(), LocalFault::None, prefix.clone()),
        MetadataLimits {
            max_read_bytes: prefix.get() + 30,
            ..MetadataLimits::default()
        },
    )
    .err()
    .unwrap();
    assert_eq!(
        error.metadata_limit().unwrap().kind,
        MetadataLimitKind::ReadBytes
    );
    for fault in [LocalFault::ReadOom, LocalFault::RestoreOom] {
        let error = ZipArchive::with_metadata_limits(
            Config::default(),
            local_work(bytes.clone(), fault, prefix.clone()),
            MetadataLimits::default(),
        )
        .err()
        .unwrap();
        assert!(error.metadata_allocation_failure().is_some());
        assert!(error.metadata_limit().is_none());
        assert!(!error.to_string().contains("private"));
    }
    let error = ZipArchive::with_metadata_limits(
        Config::default(),
        local_work(bytes, LocalFault::RestoreIo, prefix),
        MetadataLimits::default(),
    )
    .err()
    .unwrap();
    assert!(error
        .to_string()
        .contains("synthetic central restore failed"));
}

#[test]
fn zip_metadata_bounds_zip64_physical_arithmetic_is_checked_without_large_fixture() {
    for (compressed, offset) in [(4, u64::MAX - 10), (u64::MAX, 0)] {
        let mut e = entry(b"a");
        e.extra = vec![1, 0, 24, 0];
        u64le(&mut e.extra, 4);
        u64le(&mut e.extra, compressed);
        u64le(&mut e.extra, offset);
        let mut bytes = package(&[e], b"");
        let footer = end(&bytes);
        let central = get32(&bytes, footer + 16) as usize;
        set32(&mut bytes, central + 20, u32::MAX);
        set32(&mut bytes, central + 24, u32::MAX);
        set32(&mut bytes, central + 42, u32::MAX);
        assert!(bounded(bytes, MetadataLimits::default()).is_err());
    }
}
#[test]
fn zip_metadata_bounds_aes_wire_method_is_compared_before_effective_method_override() {
    let mut e = entry(b"a");
    e.flags = 1;
    // AE-2 AES extra field: vendor AE, 128-bit strength, underlying Stored.
    e.extra = vec![1, 0x99, 7, 0, 2, 0, b'A', b'E', 1, 0, 0];
    let mut bytes = package(&[e], b"");
    let footer = end(&bytes);
    let central = get32(&bytes, footer + 16) as usize;
    set16(&mut bytes, 8, 99);
    set16(&mut bytes, central + 10, 99);
    assert!(bounded(bytes.clone(), MetadataLimits::default()).is_ok());
    set16(&mut bytes, 8, 0);
    assert!(bounded(bytes, MetadataLimits::default()).is_err());
}
