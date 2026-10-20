//! Bounded ZIP access which preserves terminal processing failures across SDK recovery.
//!
//! This foundation is not wired into model producers yet. Constructor metadata
//! limits do not bound member inflation, parser allocations, source identity or RSS.
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};

use thiserror::Error;
use zip::read::{Config, MetadataLimitKind, MetadataLimits};
use zip::result::{ZipError, ZipResult};
use zip::{CompressionMethod, ZipArchive};

use crate::ResourceStop;

/// Concrete operational allocation marker for IO-only processing boundaries.
/// Neither an ordinary OutOfMemory kind nor matching text identifies this marker.
#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
#[error("Model processing could not allocate memory")]
pub struct ZipAllocationFailed;

impl ZipAllocationFailed {
    /// Preserve this category across an IO-only boundary without input details.
    pub fn into_io_error(self) -> io::Error {
        io::Error::new(io::ErrorKind::OutOfMemory, self)
    }

    /// Recover only this concrete IO payload.
    pub fn from_io_error(error: &io::Error) -> Option<Self> {
        error.get_ref()?.downcast_ref::<Self>().copied()
    }
}

/// A terminal resource/allocator failure or an unchanged ordinary archive error.
#[derive(Debug, Error)]
pub enum ZipOpenError {
    /// A deterministic processing stop; optional metadata must not swallow it.
    #[error(transparent)]
    ResourceStopped(#[from] ResourceStop),
    /// Operational allocator failure, distinct from deterministic input allowances.
    #[error("Model processing could not allocate memory")]
    AllocationFailed,
    /// Ordinary ZIP failures retain their SDK identity for contextual adaptation.
    #[error(transparent)]
    Archive(ZipError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalFailure {
    ResourceStopped(ResourceStop),
    AllocationFailed,
}
impl TerminalFailure {
    fn from_io(error: &io::Error) -> Option<Self> {
        if let Some(stop) = ResourceStop::from_io_error(error) {
            return Some(Self::ResourceStopped(stop));
        }
        if ZipAllocationFailed::from_io_error(error).is_some()
            || error
                .get_ref()
                .is_some_and(|value| value.is::<zip::read::MetadataAllocationFailed>())
        {
            return Some(Self::AllocationFailed);
        }
        None
    }
    fn error(self) -> ZipOpenError {
        match self {
            Self::ResourceStopped(stop) => ZipOpenError::ResourceStopped(stop),
            Self::AllocationFailed => ZipOpenError::AllocationFailed,
        }
    }
    fn io_error(self) -> io::Error {
        match self {
            Self::ResourceStopped(stop) => stop.into_io_error(),
            Self::AllocationFailed => ZipAllocationFailed.into_io_error(),
        }
    }
}

/// Classify only concrete SDK or project markers, never text or a generic IO kind.
/// This function does not create an observer or enforce whole-operation completion.
pub fn classify_zip_error(error: ZipError) -> ZipOpenError {
    if let Some(limit) = error.metadata_limit() {
        let stop = match limit.kind {
            MetadataLimitKind::Entries
            | MetadataLimitKind::EncodedBytes
            | MetadataLimitKind::AllocationWork => ResourceStop::InputLimitExceeded,
            MetadataLimitKind::Candidates
            | MetadataLimitKind::ReadBytes
            | MetadataLimitKind::IoCalls => ResourceStop::ReadWorkExceeded,
        };
        return ZipOpenError::ResourceStopped(stop);
    }
    if error.metadata_allocation_failure().is_some() {
        return ZipOpenError::AllocationFailed;
    }
    if let ZipError::Io(ref io) = error {
        if let Some(failure) = TerminalFailure::from_io(io) {
            return failure.error();
        }
    }
    ZipOpenError::Archive(error)
}

/// First-terminal-cause observer, retained outside optional opens and all early returns.
///
/// Share one observer across every archive opened by an operation, and call
/// [`Self::finish`] on its complete result before formatting or publishing it.
/// A completion check cannot undo side effects performed before a stop.
#[derive(Debug, Clone, Default)]
pub struct ZipResourceObserver {
    terminal: Arc<Mutex<Option<TerminalFailure>>>,
}
impl ZipResourceObserver {
    /// Create an independent whole-operation observer before opening any ZIP.
    pub fn new() -> Self {
        Self::default()
    }
    fn failure(&self) -> Option<TerminalFailure> {
        *self
            .terminal
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
    fn record(&self, failure: TerminalFailure) {
        let mut terminal = self
            .terminal
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if terminal.is_none() {
            *terminal = Some(failure);
        }
    }
    fn observe_io(&self, error: &io::Error) {
        if let Some(failure) = TerminalFailure::from_io(error) {
            self.record(failure);
        }
    }
    /// Check the operation before further work or tentative publication.
    pub fn check(&self) -> Result<(), ZipOpenError> {
        match self.failure() {
            Some(failure) => Err(failure.error()),
            None => Ok(()),
        }
    }
    fn check_io(&self) -> io::Result<()> {
        match self.failure() {
            Some(failure) => Err(failure.io_error()),
            None => Ok(()),
        }
    }
    /// Reject tentative success or an unrelated error when an optional inner
    /// operation swallowed a terminal cause. No early return may bypass this gate.
    pub fn finish<T, E: From<ZipOpenError>>(&self, result: Result<T, E>) -> Result<T, E> {
        self.check().map_err(E::from)?;
        result
    }
    fn zip_result<T>(&self, result: ZipResult<T>) -> Result<T, ZipOpenError> {
        let result = result.map_err(classify_zip_error);
        if let Err(error) = &result {
            match error {
                ZipOpenError::ResourceStopped(stop) => {
                    self.record(TerminalFailure::ResourceStopped(*stop));
                }
                ZipOpenError::AllocationFailed => self.record(TerminalFailure::AllocationFailed),
                ZipOpenError::Archive(_) => {}
            }
        }
        self.finish(result)
    }
    fn read_result(&self, operation: impl FnOnce() -> io::Result<usize>) -> io::Result<usize> {
        self.check_io()?;
        let result = operation();
        if let Err(error) = &result {
            self.observe_io(error);
        }
        self.check_io()?;
        result
    }
    /// Open one bounded archive while retaining this observer even if opening fails.
    /// Uses the reviewed SDK default constructor limits; no source is reopened.
    pub fn open_bounded_zip<R: Read + Seek>(
        &self,
        reader: R,
    ) -> Result<BoundedZipArchive<R>, ZipOpenError> {
        self.open_with_limits(reader, MetadataLimits::default())
    }
    fn open_with_limits<R: Read + Seek>(
        &self,
        reader: R,
        limits: MetadataLimits,
    ) -> Result<BoundedZipArchive<R>, ZipOpenError> {
        self.check()?;
        let tracked = StopTrackingReader {
            inner: reader,
            observer: self.clone(),
        };
        // The observer lives outside SDK fallback and is checked before Ok/Err
        // selection, including when the constructor swallowed a source error.
        let result = ZipArchive::with_metadata_limits(Config::default(), tracked, limits);
        let inner = self.zip_result(result)?;
        Ok(BoundedZipArchive {
            inner,
            observer: self.clone(),
        })
    }
}

struct StopTrackingReader<R> {
    inner: R,
    observer: ZipResourceObserver,
}
impl<R: Read> Read for StopTrackingReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.observer.read_result(|| self.inner.read(output))
    }
}
impl<R: Seek> Seek for StopTrackingReader<R> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.observer.check_io()?;
        let result = self.inner.seek(from);
        if let Err(error) = &result {
            self.observer.observe_io(error);
        }
        self.observer.check_io()?;
        result
    }
}

/// Bounded, tracked ZIP archive with no mutable raw archive or reader escape.
pub struct BoundedZipArchive<R: Read + Seek> {
    inner: ZipArchive<StopTrackingReader<R>>,
    observer: ZipResourceObserver,
}
impl<R: Read + Seek> BoundedZipArchive<R> {
    /// Number of entries after the SDK's existing filename deduplication.
    pub fn len(&self) -> usize {
        self.inner.len()
    }
    /// Whether the archive has no entries.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
    /// Immutable interpreted entry names; performs no input IO.
    pub fn file_names(&self) -> impl Iterator<Item = &str> {
        self.inner.file_names()
    }
    /// Open an interpreted-name member through the tracked terminal boundary.
    pub fn by_name(&mut self, name: &str) -> Result<BoundedZipMember<'_, R>, ZipOpenError> {
        let observer = self.observer.clone();
        observer.check()?;
        let inner = observer.zip_result(self.inner.by_name(name))?;
        Ok(BoundedZipMember { inner, observer })
    }
    /// Open a member by deduplicated index through the tracked terminal boundary.
    pub fn by_index(&mut self, index: usize) -> Result<BoundedZipMember<'_, R>, ZipOpenError> {
        let observer = self.observer.clone();
        observer.check()?;
        let inner = observer.zip_result(self.inner.by_index(index))?;
        Ok(BoundedZipMember { inner, observer })
    }
    /// Open compressed member bytes without bypassing tracked terminal failures.
    pub fn by_index_raw(&mut self, index: usize) -> Result<BoundedZipMember<'_, R>, ZipOpenError> {
        let observer = self.observer.clone();
        observer.check()?;
        let inner = observer.zip_result(self.inner.by_index_raw(index))?;
        Ok(BoundedZipMember { inner, observer })
    }
}

/// A tracked ZIP member. Member inflation/processing budgets remain separate gates.
pub struct BoundedZipMember<'a, R: Read + Seek> {
    inner: zip::read::ZipFile<'a, StopTrackingReader<R>>,
    observer: ZipResourceObserver,
}
impl<R: Read + Seek> Read for BoundedZipMember<'_, R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.observer.read_result(|| self.inner.read(output))
    }
}
impl<R: Read + Seek> BoundedZipMember<'_, R> {
    /// Interpreted name, after the SDK's Unicode metadata handling.
    pub fn name(&self) -> &str {
        self.inner.name()
    }
    /// Raw name exposed by the SDK, which may include a UnicodePath override.
    pub fn name_raw(&self) -> &[u8] {
        self.inner.name_raw()
    }
    /// Declared uncompressed member size.
    pub fn size(&self) -> u64 {
        self.inner.size()
    }
    /// Admitted compressed payload length.
    pub fn compressed_size(&self) -> u64 {
        self.inner.compressed_size()
    }
    /// Declared member CRC32; this accessor does not verify payload identity.
    pub fn crc32(&self) -> u32 {
        self.inner.crc32()
    }
    /// Effective interpreted compression method.
    pub fn compression(&self) -> CompressionMethod {
        self.inner.compression()
    }
    /// Whether the member is encrypted.
    pub fn encrypted(&self) -> bool {
        self.inner.encrypted()
    }
    /// Whether the member is a file according to the SDK.
    pub fn is_file(&self) -> bool {
        self.inner.is_file()
    }
    /// Whether the member is a directory according to the SDK.
    pub fn is_dir(&self) -> bool {
        self.inner.is_dir()
    }
    /// Whether the SDK's interpreted Unix mode identifies a symbolic link.
    pub fn is_symlink(&self) -> bool {
        self.inner.is_symlink()
    }
    /// The SDK-validated relative member path, without changing path semantics.
    pub fn enclosed_name(&self) -> Option<std::path::PathBuf> {
        self.inner.enclosed_name()
    }
    /// Optional interpreted Unix permission bits.
    pub fn unix_mode(&self) -> Option<u32> {
        self.inner.unix_mode()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::io::{Cursor, Write};
    use std::rc::Rc;

    const FAILURES: [TerminalFailure; 4] = [
        TerminalFailure::ResourceStopped(ResourceStop::InputLimitExceeded),
        TerminalFailure::ResourceStopped(ResourceStop::ReadWorkExceeded),
        TerminalFailure::ResourceStopped(ResourceStop::EnvelopeChanged),
        TerminalFailure::AllocationFailed,
    ];
    fn assert_failure(error: ZipOpenError, expected: TerminalFailure) {
        match (error, expected) {
            (ZipOpenError::ResourceStopped(actual), TerminalFailure::ResourceStopped(expected)) => {
                assert_eq!(actual, expected)
            }
            (ZipOpenError::AllocationFailed, TerminalFailure::AllocationFailed) => {}
            (actual, expected) => panic!("expected {expected:?}, got {actual:?}"),
        }
    }
    fn fixture(method: CompressionMethod) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(
                "model.stl",
                zip::write::SimpleFileOptions::default().compression_method(method),
            )
            .unwrap();
        writer.write_all(b"model payload model payload").unwrap();
        writer.finish().unwrap().into_inner()
    }
    fn end(bytes: &[u8]) -> usize {
        bytes
            .windows(4)
            .rposition(|value| value == b"PK\x05\x06")
            .unwrap()
    }
    fn value32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }
    fn footer(bytes: &mut Vec<u8>, count: u16, size: u32, offset: u32) {
        bytes.extend_from_slice(b"PK\x05\x06\0\0\0\0");
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&size.to_le_bytes());
        bytes.extend_from_slice(&offset.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
    }
    fn zip64(mut bytes: Vec<u8>) -> Vec<u8> {
        let old = end(&bytes);
        let size = value32(&bytes, old + 12);
        let offset = value32(&bytes, old + 16);
        // Keep the original valid ZIP32 footer as an older recovery candidate.
        // The newest footer must contain a sentinel or may_be_zip64() skips the
        // locator entirely. Use size rather than count so a swallowed locator
        // error reaches an ordinary range failure, not the sticky entry limit.
        let z64 = bytes.len() as u64;
        bytes.extend_from_slice(b"PK\x06\x06");
        bytes.extend_from_slice(&44u64.to_le_bytes());
        bytes.extend_from_slice(&45u16.to_le_bytes());
        bytes.extend_from_slice(&45u16.to_le_bytes());
        bytes.extend_from_slice(&[0; 8]);
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.extend_from_slice(&u64::from(size).to_le_bytes());
        bytes.extend_from_slice(&u64::from(offset).to_le_bytes());
        bytes.extend_from_slice(b"PK\x06\x07");
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&z64.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        footer(&mut bytes, 1, u32::MAX, offset);
        bytes
    }

    #[test]
    fn zip_facade_classifies_concrete_sdk_limits_and_latches_optional_failed_opens() {
        let profiles = [
            (
                MetadataLimits {
                    max_entries: 0,
                    ..MetadataLimits::default()
                },
                ResourceStop::InputLimitExceeded,
            ),
            (
                MetadataLimits {
                    max_encoded_bytes: 0,
                    ..MetadataLimits::default()
                },
                ResourceStop::InputLimitExceeded,
            ),
            (
                MetadataLimits {
                    max_allocation_work: 0,
                    ..MetadataLimits::default()
                },
                ResourceStop::InputLimitExceeded,
            ),
            (
                MetadataLimits {
                    max_candidates: 0,
                    ..MetadataLimits::default()
                },
                ResourceStop::ReadWorkExceeded,
            ),
            (
                MetadataLimits {
                    max_read_bytes: 0,
                    ..MetadataLimits::default()
                },
                ResourceStop::ReadWorkExceeded,
            ),
            (
                MetadataLimits {
                    max_io_calls: 0,
                    ..MetadataLimits::default()
                },
                ResourceStop::ReadWorkExceeded,
            ),
        ];
        for (limits, expected) in profiles {
            let observer = ZipResourceObserver::new();
            let _optional = observer
                .open_with_limits(Cursor::new(fixture(CompressionMethod::Stored)), limits)
                .ok();
            let result: Result<Option<()>, ZipOpenError> = observer.finish(Ok(None));
            assert_failure(
                result.unwrap_err(),
                TerminalFailure::ResourceStopped(expected),
            );
        }
    }
    #[test]
    fn zip_facade_preserves_ordinary_errors_without_text_or_kind_inference() {
        for failure in FAILURES {
            let original = failure.error();
            let fake = ZipError::Io(io::Error::new(
                io::ErrorKind::OutOfMemory,
                original.to_string(),
            ));
            let observer = ZipResourceObserver::new();
            assert!(matches!(
                observer.zip_result::<()>(Err(fake)),
                Err(ZipOpenError::Archive(_))
            ));
            let result: Result<_, ZipOpenError> = observer.finish(Ok(42));
            assert_eq!(result.unwrap(), 42);
        }
        assert!(matches!(
            classify_zip_error(ZipError::FileNotFound),
            ZipOpenError::Archive(ZipError::FileNotFound)
        ));
        let ordinary = ZipError::InvalidArchive("Model source changed during processing".into());
        assert!(matches!(
            classify_zip_error(ordinary),
            ZipOpenError::Archive(ZipError::InvalidArchive(_))
        ));
        let marker = ZipError::Io(io::Error::new(
            io::ErrorKind::OutOfMemory,
            zip::read::MetadataAllocationFailed,
        ));
        assert!(matches!(
            classify_zip_error(marker),
            ZipOpenError::AllocationFailed
        ));
    }

    #[derive(Clone, Copy)]
    enum FaultOn {
        Read,
        Seek,
    }
    struct FaultReader {
        inner: Cursor<Vec<u8>>,
        target: u64,
        on: FaultOn,
        failure: TerminalFailure,
        stopped: bool,
        after: Rc<Cell<usize>>,
        fired: Rc<Cell<bool>>,
        armed: Rc<Cell<bool>>,
    }
    impl FaultReader {
        fn before(&mut self, trigger: bool) -> io::Result<()> {
            if self.stopped {
                self.after.set(self.after.get() + 1);
            }
            if !self.stopped && self.armed.get() && trigger {
                self.stopped = true;
                self.fired.set(true);
                return Err(self.failure.io_error());
            }
            Ok(())
        }
    }
    impl Read for FaultReader {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            self.before(matches!(self.on, FaultOn::Read) && self.inner.position() == self.target)?;
            self.inner.read(output)
        }
    }
    impl Seek for FaultReader {
        fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
            self.before(
                matches!(self.on, FaultOn::Seek)
                    && matches!(from,SeekFrom::Start(value) if value==self.target),
            )?;
            self.inner.seek(from)
        }
    }
    #[test]
    fn zip_facade_constructor_recovery_cannot_swallow_external_read_or_seek_stops() {
        let plain = fixture(CompressionMethod::Stored);
        let mut repeated = plain.clone();
        let original_footer = end(&plain);
        footer(&mut repeated, 1, 0, 0); // Earlier ordinary central-range error becomes last_err.
        let central = u64::from(value32(&plain, original_footer + 16));
        let z64 = zip64(plain.clone());
        let locator = (end(&z64) - 20) as u64;
        for failure in FAILURES {
            for on in [FaultOn::Read, FaultOn::Seek] {
                for (bytes, target) in [
                    (z64.clone(), locator),
                    (plain.clone(), central),
                    (repeated.clone(), original_footer as u64),
                ] {
                    let after = Rc::new(Cell::new(0));
                    let fired = Rc::new(Cell::new(false));
                    let observer = ZipResourceObserver::new();
                    let source = FaultReader {
                        inner: Cursor::new(bytes),
                        target,
                        on,
                        failure,
                        stopped: false,
                        after: after.clone(),
                        fired: fired.clone(),
                        armed: Rc::new(Cell::new(true)),
                    };
                    let error = observer
                        .open_bounded_zip(source)
                        .err()
                        .expect("fault must reject the archive");
                    assert!(fired.get(), "intended injection site was reached");
                    assert_failure(error, failure);
                    assert_eq!(after.get(), 0, "no underlying IO after first stop");
                    let result: Result<_, ZipOpenError> = observer.finish(Ok("cached fallback"));
                    assert_failure(result.unwrap_err(), failure);
                }
            }
        }
    }
    #[test]
    fn zip_facade_first_terminal_wins_even_over_sdk_limits_and_success_results() {
        for failure in FAILURES {
            let observer = ZipResourceObserver::new();
            observer.record(failure);
            let sdk = ZipError::Io(io::Error::other(zip::read::MetadataLimitExceeded {
                kind: MetadataLimitKind::ReadBytes,
                limit: 1,
                attempted: 2,
            }));
            assert_failure(observer.zip_result::<()>(Err(sdk)).unwrap_err(), failure);
            assert_failure(observer.zip_result(Ok(())).unwrap_err(), failure);
            assert_failure(
                observer
                    .finish::<(), ZipOpenError>(Err(ZipOpenError::Archive(ZipError::FileNotFound)))
                    .unwrap_err(),
                failure,
            );
        }
    }
    #[test]
    fn zip_facade_whole_operation_finish_rejects_optional_cached_success_and_error() {
        for failure in FAILURES {
            let observer = ZipResourceObserver::new();
            let finish = observer.clone();
            let operation = || {
                let optional: Option<()> = {
                    observer.record(failure);
                    Err::<(), _>(failure.error())
                }
                .ok();
                assert!(optional.is_none());
                Ok::<_, ZipOpenError>("cached geometry without further IO")
            };
            let result = operation();
            assert_failure(finish.finish(result).unwrap_err(), failure);
            assert_failure(
                finish
                    .finish::<(), ZipOpenError>(Err(ZipOpenError::Archive(ZipError::FileNotFound)))
                    .unwrap_err(),
                failure,
            );
        }
    }
    #[test]
    fn zip_facade_observer_is_shared_across_opens_and_survives_failed_optional_open() {
        let observer = ZipResourceObserver::new();
        let mut first = observer
            .open_bounded_zip(Cursor::new(fixture(CompressionMethod::Stored)))
            .unwrap();
        assert_eq!(first.len(), 1);
        assert!(!first.is_empty());
        assert_eq!(first.file_names().collect::<Vec<_>>(), vec!["model.stl"]);
        let optional = observer
            .open_with_limits(
                Cursor::new(fixture(CompressionMethod::Stored)),
                MetadataLimits {
                    max_entries: 0,
                    ..MetadataLimits::default()
                },
            )
            .ok();
        assert!(optional.is_none());
        assert_failure(
            first.by_index(0).err().unwrap(),
            TerminalFailure::ResourceStopped(ResourceStop::InputLimitExceeded),
        );
        let after = Rc::new(Cell::new(0));
        let source = FaultReader {
            inner: Cursor::new(fixture(CompressionMethod::Stored)),
            target: 0,
            on: FaultOn::Read,
            failure: TerminalFailure::AllocationFailed,
            stopped: true,
            after: after.clone(),
            fired: Rc::new(Cell::new(false)),
            armed: Rc::new(Cell::new(true)),
        };
        assert_failure(
            observer.open_bounded_zip(source).err().unwrap(),
            TerminalFailure::ResourceStopped(ResourceStop::InputLimitExceeded),
        );
        assert_eq!(after.get(), 0);
    }

    #[test]
    fn zip_facade_control_sdk_really_recovers_a_swallowed_locator_stop() {
        let bytes = zip64(fixture(CompressionMethod::Stored));
        let target = (end(&bytes) - 20) as u64;
        for on in [FaultOn::Read, FaultOn::Seek] {
            let fired = Rc::new(Cell::new(false));
            let after = Rc::new(Cell::new(0));
            let source = FaultReader {
                inner: Cursor::new(bytes.clone()),
                target,
                on,
                failure: TerminalFailure::ResourceStopped(ResourceStop::EnvelopeChanged),
                stopped: false,
                after: after.clone(),
                fired: fired.clone(),
                armed: Rc::new(Cell::new(true)),
            };
            let recovered = ZipArchive::with_metadata_limits(
                Config::default(),
                source,
                MetadataLimits::default(),
            );
            assert!(fired.get());
            assert!(
                recovered.is_ok(),
                "unwrapped SDK control must recover through ZIP32"
            );
            assert!(
                after.get() > 0,
                "control continues IO after swallowing the marker"
            );
        }
    }
    #[test]
    fn zip_facade_members_preserve_each_stop_and_perform_no_io_afterward() {
        for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
            let bytes = fixture(method);
            let data_start = 30
                + u16::from_le_bytes(bytes[26..28].try_into().unwrap()) as u64
                + u16::from_le_bytes(bytes[28..30].try_into().unwrap()) as u64;
            for failure in FAILURES {
                for raw in [false, true] {
                    let observer = ZipResourceObserver::new();
                    let after = Rc::new(Cell::new(0));
                    let fired = Rc::new(Cell::new(false));
                    let armed = Rc::new(Cell::new(false));
                    let source = FaultReader {
                        inner: Cursor::new(bytes.clone()),
                        target: data_start,
                        on: FaultOn::Read,
                        failure,
                        stopped: false,
                        after: after.clone(),
                        fired: fired.clone(),
                        armed: armed.clone(),
                    };
                    let mut archive = observer.open_bounded_zip(source).unwrap();
                    armed.set(true);
                    {
                        let mut member = if raw {
                            archive.by_index_raw(0).unwrap()
                        } else {
                            archive.by_name("model.stl").unwrap()
                        };
                        assert_eq!(member.name(), "model.stl");
                        assert_eq!(member.name_raw(), b"model.stl");
                        assert!(member.is_file());
                        assert!(!member.is_dir());
                        assert!(!member.encrypted());
                        assert_eq!(member.size(), 27);
                        assert!(member.compressed_size() > 0);
                        assert_eq!(member.compression(), method);
                        let _ = (member.crc32(), member.unix_mode());
                        let error = member.read(&mut [0; 1]).unwrap_err();
                        assert!(fired.get());
                        assert_eq!(TerminalFailure::from_io(&error), Some(failure));
                        let error = member.read(&mut []).unwrap_err();
                        assert_eq!(TerminalFailure::from_io(&error), Some(failure));
                    }
                    assert_failure(archive.by_index(0).err().unwrap(), failure);
                    drop(archive);
                    assert_eq!(after.get(), 0);
                    let result: Result<Option<()>, ZipOpenError> = observer.finish(Ok(None));
                    assert_failure(result.unwrap_err(), failure);
                }
            }
        }
    }

    struct StopDuringSuccess {
        observer: ZipResourceObserver,
        calls: Rc<Cell<usize>>,
        failure: TerminalFailure,
    }
    impl Read for StopDuringSuccess {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            self.calls.set(self.calls.get() + 1);
            assert!(
                self.observer.terminal.try_lock().is_ok(),
                "no latch held across delegated Read"
            );
            output[0] = 7;
            self.observer.record(self.failure);
            Ok(1)
        }
    }
    impl Seek for StopDuringSuccess {
        fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
            self.calls.set(self.calls.get() + 1);
            assert!(
                self.observer.terminal.try_lock().is_ok(),
                "no latch held across delegated Seek"
            );
            self.observer.record(self.failure);
            Ok(123)
        }
    }
    #[test]
    fn zip_facade_post_ok_read_seek_and_member_read_gate_override_buffered_success() {
        for failure in FAILURES {
            for seek in [false, true] {
                let observer = ZipResourceObserver::new();
                let calls = Rc::new(Cell::new(0));
                let source = StopDuringSuccess {
                    observer: observer.clone(),
                    calls: calls.clone(),
                    failure,
                };
                let mut reader = StopTrackingReader {
                    inner: source,
                    observer: observer.clone(),
                };
                let error = if seek {
                    reader.stream_position().unwrap_err()
                } else {
                    reader.read(&mut [0; 1]).unwrap_err()
                };
                assert_eq!(TerminalFailure::from_io(&error), Some(failure));
                assert_eq!(calls.get(), 1);
                assert!(reader.read(&mut []).is_err());
                assert!(reader.stream_position().is_err());
                assert_eq!(calls.get(), 1);
            }
            // The same read_result gate surrounds actual ZipFile::read, including
            // a decoder which may return buffered Ok despite an inner source error.
            let observer = ZipResourceObserver::new();
            let mut bytes = [0; 2];
            let error = observer
                .read_result(|| {
                    bytes.copy_from_slice(b"ok");
                    observer.record(failure);
                    Ok(2)
                })
                .unwrap_err();
            assert_eq!(&bytes, b"ok");
            assert_eq!(TerminalFailure::from_io(&error), Some(failure));
        }
    }
    #[test]
    fn zip_facade_finish_rejects_outer_bufreader_cached_bytes_without_further_member_io() {
        for failure in FAILURES {
            let observer = ZipResourceObserver::new();
            let mut archive = observer
                .open_bounded_zip(Cursor::new(fixture(CompressionMethod::Deflated)))
                .unwrap();
            let member = archive.by_index(0).unwrap();
            let mut buffered = io::BufReader::with_capacity(8, member);
            let mut byte = [0];
            buffered.read_exact(&mut byte).unwrap();
            assert!(buffered.buffer().len() > 1);
            observer.record(failure);
            // BufReader supplies cached data without invoking the member wrapper.
            buffered.read_exact(&mut byte).unwrap();
            let result: Result<_, ZipOpenError> = observer.finish(Ok(byte));
            assert_failure(result.unwrap_err(), failure);
        }
    }
    struct DropProbe(Rc<Cell<usize>>);
    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    #[test]
    fn zip_facade_finish_drops_tentative_result_exactly_once() {
        for failure in FAILURES {
            let observer = ZipResourceObserver::new();
            let drops = Rc::new(Cell::new(0));
            observer.record(failure);
            let result: Result<_, ZipOpenError> = observer.finish(Ok(DropProbe(drops.clone())));
            assert_eq!(drops.get(), 1);
            assert_failure(result.err().unwrap(), failure);
            assert_eq!(drops.get(), 1);
        }
    }
    #[test]
    fn zip_facade_ordinary_failed_optional_open_does_not_poison_valid_followup() {
        let observer = ZipResourceObserver::new();
        assert!(observer
            .open_bounded_zip(Cursor::new(b"not a zip"))
            .is_err());
        observer.check().unwrap();
        let mut archive = observer
            .open_bounded_zip(Cursor::new(fixture(CompressionMethod::Stored)))
            .unwrap();
        let mut payload = String::new();
        archive
            .by_index(0)
            .unwrap()
            .read_to_string(&mut payload)
            .unwrap();
        assert_eq!(payload, "model payload model payload");
        let result: Result<_, ZipOpenError> = observer.finish(Ok(payload));
        assert!(result.is_ok());
    }
    struct OrdinaryLocatorError {
        inner: Cursor<Vec<u8>>,
        target: u64,
        fired: Rc<Cell<bool>>,
    }
    impl Read for OrdinaryLocatorError {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            if self.inner.position() == self.target && !self.fired.replace(true) {
                return Err(io::Error::other(ResourceStop::EnvelopeChanged.to_string()));
            }
            self.inner.read(output)
        }
    }
    impl Seek for OrdinaryLocatorError {
        fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
            self.inner.seek(from)
        }
    }
    #[test]
    fn zip_facade_matching_text_locator_error_keeps_ordinary_sdk_recovery() {
        let bytes = zip64(fixture(CompressionMethod::Stored));
        let target = (end(&bytes) - 20) as u64;
        let fired = Rc::new(Cell::new(false));
        let observer = ZipResourceObserver::new();
        let source = OrdinaryLocatorError {
            inner: Cursor::new(bytes),
            target,
            fired: fired.clone(),
        };
        let mut archive = observer.open_bounded_zip(source).unwrap();
        assert!(fired.get());
        let mut bytes = Vec::new();
        archive
            .by_index(0)
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, b"model payload model payload");
        let result: Result<_, ZipOpenError> = observer.finish(Ok(bytes));
        assert!(result.is_ok());
    }
}
