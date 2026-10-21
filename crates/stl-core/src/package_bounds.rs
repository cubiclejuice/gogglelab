//! Actual package inflation accounting and fixed-role retained input admission.
//!
//! This bounds delivered bytes, not caller buffer allocation, decompressor work
//! space or process RSS. In particular, `Read::read_to_end` may reserve before
//! calling this reader. The fixed-role collectors below admit their own buffers
//! before growth; arbitrary consumers of an inflation reader must do likewise.

use std::io::{self, Read};

use crate::limits::MAX_3MF_INFLATED_BYTES;
use crate::zip_bounds::ZipResourceObserver;
use crate::ResourceStop;

const RETAINED_INPUT_BYTES: usize = 16 * 1024 * 1024;
const TEXT_CHUNK: usize = 4096;

/// Canonical retained package inputs, with no arbitrary production limit.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextRole {
    Relationships,
    ModelSettings,
    ProjectSettings,
}

impl TextRole {
    pub const fn byte_limit(self) -> usize {
        match self {
            Self::Relationships => 64 * 1024,
            Self::ModelSettings | Self::ProjectSettings => 8 * 1024 * 1024,
        }
    }
}

/// Fixed binary input roles; collected bytes do not spend the text counter.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryRole {
    EmbeddedImage,
    ModelMember,
}

impl BinaryRole {
    pub const fn byte_limit(self) -> usize {
        match self {
            Self::EmbeddedImage => 16 * 1024 * 1024,
            Self::ModelMember => crate::limits::MAX_FILE_BYTES as usize,
        }
    }
}

/// One context reused across sequential processing of an admitted package.
///
/// The retained counter measures cumulative copied INPUT bytes, not all owned
/// metadata strings, Vec capacities/reallocation peaks, an Owned permit or RSS.
#[doc(hidden)]
pub struct PackageProcessingContext {
    inflation: PackageInflationBudget,
    observer: ZipResourceObserver,
    retained_input: usize,
    paths: crate::metadata_owned::PathAllocator,
    #[cfg(test)]
    limits: PackageTestLimits,
    #[cfg(test)]
    collector_test: CollectorTestState,
}

/// Read-only accounting for one package operation; this grants no allowance.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackageUsage {
    /// Bytes successfully charged by the inflation reader, excluding its terminal excess probe.
    pub inflation_bytes: u64,
    /// Cumulative collected text input, excluding binary and normalized metadata copies.
    pub retained_input_bytes: usize,
}

impl PackageProcessingContext {
    pub fn new(observer: ZipResourceObserver) -> Self {
        Self::with_funding(observer, None)
    }

    pub fn new_funded(
        observer: ZipResourceObserver,
        host: std::sync::Arc<dyn crate::metadata_owned::HostOwnedFunding>,
    ) -> Self {
        Self::with_funding(observer, Some(host))
    }

    fn with_funding(
        observer: ZipResourceObserver,
        host: Option<std::sync::Arc<dyn crate::metadata_owned::HostOwnedFunding>>,
    ) -> Self {
        Self {
            inflation: PackageInflationBudget::new(observer.clone()),
            paths: crate::metadata_owned::PathAllocator::new(observer.clone(), host),
            observer,
            retained_input: 0,
            #[cfg(test)]
            limits: PackageTestLimits::default(),
            #[cfg(test)]
            collector_test: CollectorTestState::default(),
        }
    }

    /// Split allocation from the unique inflation-reader borrow without resetting either.
    pub fn path_allocator(&self) -> crate::metadata_owned::PathAllocator {
        self.paths.clone()
    }

    pub fn copy_path(
        &self,
        text: &str,
    ) -> Result<crate::metadata_owned::SharedPath, crate::StlError> {
        self.paths.copy_path(text)
    }

    pub fn observer(&self) -> ZipResourceObserver {
        self.observer.clone()
    }

    /// Inspect accounting without resetting, refunding or changing its policy.
    pub fn usage(&self) -> PackageUsage {
        PackageUsage {
            inflation_bytes: self.inflation.initial - self.inflation.remaining,
            retained_input_bytes: self.retained_input,
        }
    }

    /// The returned unique reader must finish/drop before another context pass.
    /// ```compile_fail
    /// use std::io::{Cursor, Read};
    /// use stl_core::package_bounds::{PackageProcessingContext, TextRole};
    /// use stl_core::zip_bounds::ZipResourceObserver;
    /// let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
    /// let mut first = context.inflated_reader(Cursor::new(b"first"));
    /// let _ = context.collect_text(Cursor::new(b"second"), TextRole::ModelSettings);
    /// let _ = first.read(&mut [0; 1]);
    /// ```
    pub fn inflated_reader<R: Read>(&mut self, raw: R) -> InflatedReader<'_, R> {
        self.inflation.reader(raw)
    }

    pub fn model_reader<R: Read>(&mut self, raw: R) -> ModelPartReader<'_, R> {
        #[cfg(test)]
        let remaining = self.limits.model;
        #[cfg(not(test))]
        let remaining = crate::limits::MAX_FILE_BYTES;
        ModelPartReader {
            observer: self.observer.clone(),
            inner: self.inflation.reader(raw),
            remaining,
            eof: false,
        }
    }

    /// Collect one raw ZIP member under a fixed binary-input role.
    pub fn collect_binary<R: Read>(
        &mut self,
        raw: R,
        role: BinaryRole,
    ) -> Result<Vec<u8>, crate::StlError> {
        self.collect_binary_with_limit(raw, role.byte_limit())
    }

    /// Preserve a caller's narrower model policy without expanding the fixed cap.
    pub fn collect_model_member<R: Read>(
        &mut self,
        raw: R,
        policy_max: u64,
    ) -> Result<Vec<u8>, crate::StlError> {
        let limit = policy_max.min(crate::limits::MAX_FILE_BYTES) as usize;
        self.collect_binary_with_limit(raw, limit)
    }

    fn collect_binary_with_limit<R: Read>(
        &mut self,
        raw: R,
        limit: usize,
    ) -> Result<Vec<u8>, crate::StlError> {
        let observer = self.observer.clone();
        let result = collect_input(
            self.inflation.reader(raw),
            CollectionPolicy {
                limit,
                exact: false,
                text_role: None,
            },
            None,
            &observer,
            #[cfg(test)]
            CollectorTest {
                fail_reserve: self.limits.fail_reserve,
                state: &mut self.collector_test,
            },
        );
        observer.finish(result)
    }

    pub fn collect_text<R: Read>(
        &mut self,
        raw: R,
        role: TextRole,
    ) -> Result<String, crate::StlError> {
        let observer = self.observer.clone();
        #[cfg(test)]
        let role_limit = match role {
            TextRole::Relationships => self.limits.relationships,
            TextRole::ModelSettings => self.limits.model_settings,
            TextRole::ProjectSettings => self.limits.project_settings,
        };
        #[cfg(not(test))]
        let role_limit = role.byte_limit();
        #[cfg(test)]
        let total_limit = self.limits.text_total;
        #[cfg(not(test))]
        let total_limit = RETAINED_INPUT_BYTES;
        let result = collect_input(
            self.inflation.reader(raw),
            CollectionPolicy {
                limit: role_limit,
                exact: false,
                text_role: Some(role),
            },
            Some((&mut self.retained_input, total_limit)),
            &observer,
            #[cfg(test)]
            CollectorTest {
                fail_reserve: self.limits.fail_reserve,
                state: &mut self.collector_test,
            },
        )
        .and_then(|bytes| String::from_utf8(bytes).map_err(|_| invalid_text(role)));
        observer.finish(result)
    }

    #[cfg(test)]
    pub(crate) fn with_test_limits(
        observer: ZipResourceObserver,
        limits: PackageTestLimits,
    ) -> Self {
        Self {
            inflation: PackageInflationBudget::with_limit(observer.clone(), limits.inflation),
            paths: crate::metadata_owned::PathAllocator::new(observer.clone(), None),
            observer,
            retained_input: 0,
            limits,
            collector_test: CollectorTestState::default(),
        }
    }
}

/// Collect an already-admitted model source against its exact version envelope.
///
/// This does not charge ZIP inflation. When reading a ZIP member, the caller
/// supplies exactly one context inflation reader around that member.
#[doc(hidden)]
pub fn collect_verified_model_source<R: Read>(
    raw: R,
    expected_len: u64,
    observer: &ZipResourceObserver,
) -> Result<Vec<u8>, crate::StlError> {
    observer.check()?;
    if expected_len > crate::limits::MAX_FILE_BYTES {
        return Err(input_limit(observer));
    }
    let limit = usize::try_from(expected_len).map_err(|_| input_limit(observer))?;
    let result = collect_input(
        raw,
        CollectionPolicy {
            limit,
            exact: true,
            text_role: None,
        },
        None,
        observer,
        #[cfg(test)]
        CollectorTest {
            fail_reserve: false,
            state: &mut CollectorTestState::default(),
        },
    );
    observer.finish(result)
}

struct CollectionPolicy {
    limit: usize,
    exact: bool,
    text_role: Option<TextRole>,
}

#[cfg(test)]
struct CollectorTest<'a> {
    fail_reserve: bool,
    state: &'a mut CollectorTestState,
}

fn collect_input<R: Read>(
    mut reader: R,
    policy: CollectionPolicy,
    mut copied: Option<(&mut usize, usize)>,
    observer: &ZipResourceObserver,
    #[cfg(test)] instrumentation: CollectorTest<'_>,
) -> Result<Vec<u8>, crate::StlError> {
    observer.check()?;
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; TEXT_CHUNK];
    loop {
        observer.check()?;
        let role_remaining = policy
            .limit
            .checked_sub(bytes.len())
            .ok_or_else(|| input_limit(observer))?;
        let aggregate_remaining = match &copied {
            Some((count, limit)) => limit
                .checked_sub(**count)
                .ok_or_else(|| input_limit(observer))?,
            None => role_remaining,
        };
        let available = role_remaining.min(aggregate_remaining);
        if available == 0 {
            let mut probe = [0_u8; 1];
            #[cfg(test)]
            let before = (bytes.len(), bytes.capacity());
            let result = read_retained_chunk(&mut reader, &mut probe, observer);
            #[cfg(test)]
            instrumentation
                .state
                .probes
                .push((before, (bytes.len(), bytes.capacity())));
            let read = result.map_err(|e| collection_io_error(observer, policy.text_role, e))?;
            observer.check()?;
            if read == 0 {
                break;
            }
            return Err(if policy.exact {
                envelope_changed(observer)
            } else {
                input_limit(observer)
            });
        }
        let length = available.min(chunk.len());
        let read = read_retained_chunk(&mut reader, &mut chunk[..length], observer)
            .map_err(|e| collection_io_error(observer, policy.text_role, e))?;
        observer.check()?;
        if read > length {
            return Err(collection_io_error(
                observer,
                policy.text_role,
                invalid_read_length(),
            ));
        }
        if read == 0 {
            if policy.exact {
                return Err(envelope_changed(observer));
            }
            break;
        }
        let prospective = bytes
            .len()
            .checked_add(read)
            .filter(|value| *value <= policy.limit)
            .ok_or_else(|| input_limit(observer))?;
        let next_copied = match &copied {
            Some((count, limit)) => Some(
                count
                    .checked_add(read)
                    .filter(|value| *value <= *limit)
                    .ok_or_else(|| input_limit(observer))?,
            ),
            None => None,
        };
        if prospective > bytes.capacity() {
            let target = retained_capacity_target(
                bytes.len(),
                bytes.capacity(),
                prospective,
                policy.limit,
                aggregate_remaining,
            )
            .ok_or_else(|| input_limit(observer))?;
            let additional = target
                .checked_sub(bytes.len())
                .ok_or_else(|| input_limit(observer))?;
            #[cfg(test)]
            instrumentation.state.targets.push(target);
            #[cfg(test)]
            if instrumentation.fail_reserve {
                return Err(allocation_failure(observer));
            }
            #[cfg(test)]
            let old_capacity = bytes.capacity();
            bytes
                .try_reserve_exact(additional)
                .map_err(|_| allocation_failure(observer))?;
            #[cfg(test)]
            instrumentation
                .state
                .capacities
                .push((old_capacity, target, bytes.capacity()));
        }
        observer.check()?;
        bytes.extend_from_slice(&chunk[..read]);
        if let (Some((count, _)), Some(next)) = (&mut copied, next_copied) {
            **count = next;
        }
    }
    observer.check()?;
    Ok(bytes)
}

fn envelope_changed(observer: &ZipResourceObserver) -> crate::StlError {
    let _ = observer.stop_io(ResourceStop::EnvelopeChanged);
    observer
        .check()
        .expect_err("terminal envelope stop was recorded")
        .into()
}

fn collection_io_error(
    observer: &ZipResourceObserver,
    role: Option<TextRole>,
    error: io::Error,
) -> crate::StlError {
    observer.observe_io(&error);
    match observer.check() {
        Err(error) => error.into(),
        Ok(()) => role
            .map(invalid_text)
            .unwrap_or(crate::StlError::Malformed3mf {
                detail: "Package input could not be read".into(),
            }),
    }
}

fn read_retained_chunk<R: Read>(
    reader: &mut R,
    output: &mut [u8],
    observer: &ZipResourceObserver,
) -> io::Result<usize> {
    loop {
        observer.check_io()?;
        let result = reader.read(output);
        if let Err(error) = &result {
            observer.observe_io(error);
        }
        // A concrete terminal payload or stop during IO wins even when its
        // error kind is Interrupted. Retry only an ordinary EINTR afterward.
        observer.check_io()?;
        match result {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => return result,
        }
    }
}

fn retained_capacity_target(
    length: usize,
    capacity: usize,
    prospective: usize,
    role_limit: usize,
    aggregate_remaining: usize,
) -> Option<usize> {
    let ceiling = length.checked_add(aggregate_remaining)?.min(role_limit);
    if prospective > ceiling {
        return None;
    }
    let doubled = capacity.checked_mul(2).unwrap_or(ceiling);
    Some(prospective.max(doubled).min(ceiling))
}

fn invalid_text(role: TextRole) -> crate::StlError {
    let detail = match role {
        TextRole::Relationships => "_rels/.rels is not valid text",
        TextRole::ModelSettings => "Metadata/model_settings.config is not valid UTF-8 text",
        TextRole::ProjectSettings => "Metadata/project_settings.config is not valid UTF-8 text",
    };
    crate::StlError::Malformed3mf {
        detail: detail.into(),
    }
}

fn input_limit(observer: &ZipResourceObserver) -> crate::StlError {
    let _ = observer.stop_io(ResourceStop::InputLimitExceeded);
    observer
        .check()
        .expect_err("terminal input stop was recorded")
        .into()
}

fn allocation_failure(observer: &ZipResourceObserver) -> crate::StlError {
    let _ = observer.allocation_failed_io();
    observer
        .check()
        .expect_err("terminal allocation failure was recorded")
        .into()
}

/// Fixed model-part input admission around the shared actual inflation reader.
#[doc(hidden)]
pub struct ModelPartReader<'a, R> {
    inner: InflatedReader<'a, R>,
    observer: ZipResourceObserver,
    remaining: u64,
    eof: bool,
}

impl<R: Read> Read for ModelPartReader<'_, R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.observer.check_io()?;
        if output.is_empty() || self.eof {
            return Ok(0);
        }
        if self.remaining == 0 {
            let mut probe = [0_u8; 1];
            let result = self.inner.read(&mut probe);
            if let Err(error) = &result {
                self.observer.observe_io(error);
            }
            self.observer.check_io()?;
            return match result? {
                0 => {
                    self.eof = true;
                    Ok(0)
                }
                1 => Err(self.observer.stop_io(ResourceStop::InputLimitExceeded)),
                _ => Err(invalid_read_length()),
            };
        }
        let length = output
            .len()
            .min(usize::try_from(self.remaining).unwrap_or(usize::MAX));
        let result = self.inner.read(&mut output[..length]);
        match &result {
            Ok(read) if *read <= length => self.remaining -= *read as u64,
            Err(error) => self.observer.observe_io(error),
            _ => {}
        }
        self.observer.check_io()?;
        let read = result?;
        if read > length {
            return Err(invalid_read_length());
        }
        if read == 0 {
            self.eof = true;
        }
        Ok(read)
    }
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) struct PackageTestLimits {
    pub inflation: u64,
    pub model: u64,
    pub relationships: usize,
    pub model_settings: usize,
    pub project_settings: usize,
    pub text_total: usize,
    pub fail_reserve: bool,
}

#[cfg(test)]
impl Default for PackageTestLimits {
    fn default() -> Self {
        Self {
            inflation: MAX_3MF_INFLATED_BYTES,
            model: crate::limits::MAX_FILE_BYTES,
            relationships: TextRole::Relationships.byte_limit(),
            model_settings: TextRole::ModelSettings.byte_limit(),
            project_settings: TextRole::ProjectSettings.byte_limit(),
            text_total: RETAINED_INPUT_BYTES,
            fail_reserve: false,
        }
    }
}

#[cfg(test)]
#[derive(Default)]
struct CollectorTestState {
    targets: Vec<usize>,
    capacities: Vec<(usize, usize, usize)>,
    probes: Vec<((usize, usize), (usize, usize))>,
}

/// One non-cloneable, single-owner actual-read allowance for a package operation.
///
/// Every repeated member read spends this same allowance. Declared entry lengths
/// are not charged here; cached parsing without input reads spends nothing.
pub struct PackageInflationBudget {
    initial: u64,
    remaining: u64,
    observer: ZipResourceObserver,
}

impl PackageInflationBudget {
    /// Use the fixed 512 MiB allowance and the operation's existing observer.
    pub fn new(observer: ZipResourceObserver) -> Self {
        Self {
            initial: MAX_3MF_INFLATED_BYTES,
            remaining: MAX_3MF_INFLATED_BYTES,
            observer,
        }
    }

    /// Read one member without permitting simultaneous spending of this budget.
    ///
    /// The mutable borrow prevents concurrent/reentrant budget readers. No
    /// accounting mutex or RefCell borrow is held across the underlying IO.
    ///
    /// ```compile_fail
    /// use std::io::{Cursor, Read};
    /// use stl_core::package_bounds::PackageInflationBudget;
    /// use stl_core::zip_bounds::ZipResourceObserver;
    /// let mut budget = PackageInflationBudget::new(ZipResourceObserver::new());
    /// let mut first = budget.reader(Cursor::new(b"first"));
    /// let _second = budget.reader(Cursor::new(b"second"));
    /// let _ = first.read(&mut [0; 1]);
    /// ```
    pub fn reader<R: Read>(&mut self, input: R) -> InflatedReader<'_, R> {
        InflatedReader {
            budget: self,
            input,
            eof: false,
        }
    }

    #[cfg(test)]
    fn with_limit(observer: ZipResourceObserver, limit: u64) -> Self {
        Self {
            initial: limit,
            remaining: limit,
            observer,
        }
    }
}

/// A member reader retaining the unique mutable package allowance borrow.
///
/// The input cannot be recovered or reset through this wrapper. Once EOF was
/// observed it is cached for this reader only; a newly opened member charges its
/// own actual reads against the same package allowance.
pub struct InflatedReader<'a, R> {
    budget: &'a mut PackageInflationBudget,
    input: R,
    eof: bool,
}

impl<R: Read> Read for InflatedReader<'_, R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.budget.observer.check_io()?;
        if output.is_empty() || self.eof {
            return Ok(0);
        }

        if self.budget.remaining == 0 {
            // Do not invent EOF at the cap or append an excess byte to a caller
            // buffer. One stack byte distinguishes real EOF/CRC from excess.
            let mut probe = [0_u8; 1];
            let result = self.input.read(&mut probe);
            if let Err(error) = &result {
                self.budget.observer.observe_io(error);
            }
            self.budget.observer.check_io()?;
            return match result? {
                0 => {
                    self.eof = true;
                    Ok(0)
                }
                1 => Err(self.budget.observer.stop_io(ResourceStop::ReadWorkExceeded)),
                _ => Err(invalid_read_length()),
            };
        }

        let length = output
            .len()
            .min(usize::try_from(self.budget.remaining).unwrap_or(usize::MAX));
        let result = self.input.read(&mut output[..length]);
        match &result {
            Ok(read) if *read <= length => {
                // Charge positive bytes actually obtained, including short
                // reads. The slice bound makes this subtraction exact and safe.
                self.budget.remaining -= *read as u64;
            }
            Err(error) => self.budget.observer.observe_io(error),
            _ => {}
        }
        // A stop observed during IO wins over tentative bytes or ordinary errors.
        self.budget.observer.check_io()?;
        let read = result?;
        if read > length {
            return Err(invalid_read_length());
        }
        if read == 0 {
            self.eof = true;
        }
        Ok(read)
    }
}

fn invalid_read_length() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Inflated input returned an invalid read length",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zip_bounds::{ZipAllocationFailed, ZipOpenError};
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};
    use zip::{write::SimpleFileOptions, CompressionMethod};

    #[derive(Default)]
    struct Stats {
        requests: Vec<usize>,
        bytes: usize,
    }

    struct CountedReader {
        cursor: Cursor<Vec<u8>>,
        stats: Arc<Mutex<Stats>>,
        max_chunk: usize,
    }

    impl Read for CountedReader {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            self.stats.lock().unwrap().requests.push(output.len());
            let limit = output.len().min(self.max_chunk);
            let read = self.cursor.read(&mut output[..limit])?;
            self.stats.lock().unwrap().bytes += read;
            Ok(read)
        }
    }

    fn counted(bytes: &[u8], max_chunk: usize) -> (CountedReader, Arc<Mutex<Stats>>) {
        let stats = Arc::new(Mutex::new(Stats::default()));
        (
            CountedReader {
                cursor: Cursor::new(bytes.to_vec()),
                stats: stats.clone(),
                max_chunk,
            },
            stats,
        )
    }

    #[test]
    fn exact_limit_probes_eof_once_and_caches_it_without_touching_output() {
        let observer = ZipResourceObserver::new();
        let mut budget = PackageInflationBudget::with_limit(observer.clone(), 3);
        let (input, stats) = counted(b"abc", usize::MAX);
        let mut reader = budget.reader(input);
        let mut output = [99; 8];
        assert_eq!(reader.read(&mut []).unwrap(), 0);
        assert!(stats.lock().unwrap().requests.is_empty());
        assert_eq!(reader.read(&mut output).unwrap(), 3);
        assert_eq!(&output[..3], b"abc");
        assert_eq!(&output[3..], &[99; 5]);
        output.fill(99);
        assert_eq!(reader.read(&mut output).unwrap(), 0);
        assert_eq!(reader.read(&mut output).unwrap(), 0);
        assert_eq!(output, [99; 8]);
        let snapshot = stats.lock().unwrap();
        assert_eq!(snapshot.requests, [3, 1]);
        assert_eq!(snapshot.bytes, 3);
        drop(snapshot);
        observer.check().unwrap();
        let _ = observer.stop_io(ResourceStop::EnvelopeChanged);
        assert_eq!(
            ResourceStop::from_io_error(&reader.read(&mut output).unwrap_err()),
            Some(ResourceStop::EnvelopeChanged)
        );
        assert_eq!(stats.lock().unwrap().requests, [3, 1]);
    }

    #[test]
    fn one_extra_byte_is_stack_only_and_stops_all_later_io_including_empty_reads() {
        let observer = ZipResourceObserver::new();
        let mut budget = PackageInflationBudget::with_limit(observer.clone(), 3);
        let (input, stats) = counted(b"abcd-more", usize::MAX);
        let mut reader = budget.reader(input);
        reader.read_exact(&mut [0; 3]).unwrap();
        let mut output = [99; 8];
        let error = reader.read(&mut output).unwrap_err();
        assert_eq!(
            ResourceStop::from_io_error(&error),
            Some(ResourceStop::ReadWorkExceeded)
        );
        assert_eq!(output, [99; 8]);
        assert!(reader.read(&mut output).is_err());
        assert!(reader.read(&mut []).is_err());
        let stats = stats.lock().unwrap();
        assert_eq!(stats.requests, [3, 1]);
        assert_eq!(stats.bytes, 4);
        assert!(matches!(
            observer.finish(Ok::<_, ZipOpenError>("tentative success")),
            Err(ZipOpenError::ResourceStopped(
                ResourceStop::ReadWorkExceeded
            ))
        ));
    }

    #[test]
    fn short_and_repeated_member_reads_spend_actual_bytes_only() {
        let observer = ZipResourceObserver::new();
        let mut budget = PackageInflationBudget::with_limit(observer, 5);
        let (input, _) = counted(b"ab", 1);
        {
            let mut reader = budget.reader(input);
            reader.read_exact(&mut [0; 2]).unwrap();
            assert_eq!(reader.read(&mut [0; 4]).unwrap(), 0);
        }
        assert_eq!(budget.remaining, 3);
        {
            let mut reader = budget.reader(Cursor::new(b"cde"));
            reader.read_exact(&mut [0; 3]).unwrap();
            assert_eq!(reader.read(&mut [0; 1]).unwrap(), 0);
        }
        assert_eq!(budget.remaining, 0);

        let mut budget = PackageInflationBudget::with_limit(ZipResourceObserver::new(), 3);
        budget
            .reader(Cursor::new(b"ab"))
            .read_exact(&mut [0; 2])
            .unwrap();
        let mut repeated = budget.reader(Cursor::new(b"ab"));
        assert_eq!(repeated.read(&mut [0; 2]).unwrap(), 1);
        assert!(repeated.read(&mut [0; 1]).is_err());
    }

    #[test]
    fn zero_allowance_still_distinguishes_empty_input_from_one_byte() {
        let mut budget = PackageInflationBudget::with_limit(ZipResourceObserver::new(), 0);
        let (input, stats) = counted(b"", usize::MAX);
        {
            let mut reader = budget.reader(input);
            assert_eq!(reader.read(&mut []).unwrap(), 0);
            assert_eq!(reader.read(&mut [0; 1]).unwrap(), 0);
            assert_eq!(reader.read(&mut [0; 1]).unwrap(), 0);
        }
        assert_eq!(stats.lock().unwrap().requests, [1]);
        let error = budget
            .reader(Cursor::new(b"x"))
            .read(&mut [99; 1])
            .unwrap_err();
        assert_eq!(
            ResourceStop::from_io_error(&error),
            Some(ResourceStop::ReadWorkExceeded)
        );
    }

    struct ErrorOnce {
        error: Option<io::Error>,
        calls: Arc<Mutex<usize>>,
    }

    impl Read for ErrorOnce {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            *self.calls.lock().unwrap() += 1;
            match self.error.take() {
                Some(error) => Err(error),
                None => Ok(0),
            }
        }
    }

    #[test]
    fn ordinary_io_kind_and_matching_text_do_not_become_terminal_markers() {
        for failure in [
            io::Error::other(ResourceStop::ReadWorkExceeded.to_string()),
            io::Error::new(io::ErrorKind::OutOfMemory, "allocation failed"),
        ] {
            let observer = ZipResourceObserver::new();
            let mut budget = PackageInflationBudget::with_limit(observer.clone(), 3);
            let calls = Arc::new(Mutex::new(0));
            {
                let mut reader = budget.reader(ErrorOnce {
                    error: Some(failure),
                    calls: calls.clone(),
                });
                let error = reader.read(&mut [0; 1]).unwrap_err();
                assert_eq!(ResourceStop::from_io_error(&error), None);
                assert!(ZipAllocationFailed::from_io_error(&error).is_none());
                observer.check().unwrap();
                assert_eq!(reader.read(&mut [0; 1]).unwrap(), 0);
            }
            assert_eq!(budget.remaining, 3);
            assert_eq!(*calls.lock().unwrap(), 2);
        }
    }

    #[test]
    fn concrete_input_failures_latch_the_same_observer_and_preserve_first_cause() {
        for failure in [
            ResourceStop::InputLimitExceeded.into_io_error(),
            ResourceStop::ReadWorkExceeded.into_io_error(),
            ResourceStop::EnvelopeChanged.into_io_error(),
            ZipAllocationFailed.into_io_error(),
        ] {
            let expected = failure.to_string();
            let observer = ZipResourceObserver::new();
            let mut budget = PackageInflationBudget::with_limit(observer.clone(), 3);
            let calls = Arc::new(Mutex::new(0));
            let mut reader = budget.reader(ErrorOnce {
                error: Some(failure),
                calls: calls.clone(),
            });
            assert_eq!(reader.read(&mut [0; 3]).unwrap_err().to_string(), expected);
            let _ = observer.stop_io(ResourceStop::ReadWorkExceeded);
            assert_eq!(reader.read(&mut []).unwrap_err().to_string(), expected);
            assert_eq!(reader.read(&mut [0; 3]).unwrap_err().to_string(), expected);
            assert_eq!(*calls.lock().unwrap(), 1);
            assert_eq!(
                observer
                    .finish(Ok::<_, ZipOpenError>(()))
                    .unwrap_err()
                    .to_string(),
                expected
            );
        }
    }

    #[test]
    fn ordinary_crc_error_at_exact_limit_is_not_fake_eof_or_a_resource_stop() {
        struct CrcReader(Cursor<&'static [u8]>);
        impl Read for CrcReader {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                if self.0.position() == 3 {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "CRC mismatch"));
                }
                self.0.read(output)
            }
        }
        let observer = ZipResourceObserver::new();
        let mut budget = PackageInflationBudget::with_limit(observer.clone(), 3);
        let mut reader = budget.reader(CrcReader(Cursor::new(b"abc")));
        reader.read_exact(&mut [0; 3]).unwrap();
        let error = reader.read(&mut [99; 1]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(ResourceStop::from_io_error(&error), None);
        observer.check().unwrap();
    }

    #[test]
    fn preexisting_stop_wins_over_empty_reads_without_touching_input() {
        let observer = ZipResourceObserver::new();
        let _ = observer.allocation_failed_io();
        let mut budget = PackageInflationBudget::with_limit(observer, 3);
        let (input, stats) = counted(b"abc", usize::MAX);
        let mut reader = budget.reader(input);
        let error = reader.read(&mut []).unwrap_err();
        assert!(ZipAllocationFailed::from_io_error(&error).is_some());
        assert!(stats.lock().unwrap().requests.is_empty());
    }

    #[test]
    fn post_io_observer_stop_overrides_tentative_bytes_and_ordinary_errors() {
        struct StopsDuringRead {
            observer: ZipResourceObserver,
            ordinary_error: bool,
        }
        impl Read for StopsDuringRead {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                let _ = self.observer.stop_io(ResourceStop::EnvelopeChanged);
                if self.ordinary_error {
                    Err(io::Error::other("ordinary failure"))
                } else {
                    output[0] = 7;
                    Ok(1)
                }
            }
        }
        for ordinary_error in [false, true] {
            let observer = ZipResourceObserver::new();
            let mut budget = PackageInflationBudget::with_limit(observer.clone(), 3);
            let mut reader = budget.reader(StopsDuringRead {
                observer,
                ordinary_error,
            });
            let error = reader.read(&mut [0; 1]).unwrap_err();
            assert_eq!(
                ResourceStop::from_io_error(&error),
                Some(ResourceStop::EnvelopeChanged)
            );
        }
    }

    fn zip_fixture(method: CompressionMethod, corrupt_crc: bool) -> Vec<u8> {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(
            "part",
            SimpleFileOptions::default().compression_method(method),
        )
        .unwrap();
        zip.write_all(b"abc").unwrap();
        let mut bytes = zip.finish().unwrap().into_inner();
        if corrupt_crc {
            let central = bytes
                .windows(4)
                .position(|window| window == b"PK\x01\x02")
                .unwrap();
            bytes[14..18].fill(0);
            bytes[central + 16..central + 20].fill(0);
        }
        bytes
    }

    #[test]
    fn real_stored_and_deflated_members_verify_eof_and_reject_bad_crc_at_cap() {
        for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
            for corrupt in [false, true] {
                let observer = ZipResourceObserver::new();
                let mut archive = observer
                    .open_bounded_zip(Cursor::new(zip_fixture(method, corrupt)))
                    .unwrap();
                let member = archive.by_name("part").unwrap();
                let mut budget = PackageInflationBudget::with_limit(observer.clone(), 3);
                let mut reader = budget.reader(member);
                let result = (|| {
                    let mut output = [0; 3];
                    reader.read_exact(&mut output)?;
                    assert_eq!(&output, b"abc");
                    assert_eq!(reader.read(&mut [99; 1])?, 0);
                    Ok::<_, io::Error>(())
                })();
                if corrupt {
                    let error = result.unwrap_err();
                    assert_eq!(ResourceStop::from_io_error(&error), None);
                } else {
                    result.unwrap();
                }
                observer.check().unwrap();
            }
        }
    }

    #[test]
    fn budget_and_sendable_input_reader_remain_send_without_shared_spending() {
        fn assert_send<T: Send>() {}
        assert_send::<PackageInflationBudget>();
        assert_send::<InflatedReader<'static, Cursor<Vec<u8>>>>();
        let budget = PackageInflationBudget::new(ZipResourceObserver::new());
        assert_eq!(budget.remaining, 512 * 1024 * 1024);
    }

    fn tiny_context(limits: PackageTestLimits) -> PackageProcessingContext {
        PackageProcessingContext::with_test_limits(ZipResourceObserver::new(), limits)
    }

    fn tiny_limits() -> PackageTestLimits {
        PackageTestLimits {
            inflation: 32,
            model: 3,
            relationships: 3,
            model_settings: 3,
            project_settings: 3,
            text_total: 16,
            fail_reserve: false,
        }
    }

    #[test]
    fn binary_and_plain_collectors_preserve_lower_policy_and_exact_envelopes() {
        assert_eq!(BinaryRole::EmbeddedImage.byte_limit(), 16 * 1024 * 1024);
        assert_eq!(BinaryRole::ModelMember.byte_limit(), 100 * 1024 * 1024);
        for (input, expected, stop) in [
            (&b"abc"[..], 3, None),
            (&b"ab"[..], 3, Some(ResourceStop::EnvelopeChanged)),
            (&b"abcd"[..], 3, Some(ResourceStop::EnvelopeChanged)),
            (&b""[..], 0, None),
            (&b"x"[..], 0, Some(ResourceStop::EnvelopeChanged)),
        ] {
            let observer = ZipResourceObserver::new();
            let (raw, stats) = counted(input, 1);
            let result = collect_verified_model_source(raw, expected, &observer);
            if let Some(stop) = stop {
                assert_eq!(result.unwrap_err(), crate::StlError::ResourceStopped(stop));
                let (later, later_stats) = counted(b"later", 1);
                assert!(collect_verified_model_source(later, 5, &observer).is_err());
                assert!(later_stats.lock().unwrap().requests.is_empty());
            } else {
                assert_eq!(result.unwrap(), input);
            }
            assert!(stats.lock().unwrap().requests.iter().all(|n| *n <= 3));
        }
        let observer = ZipResourceObserver::new();
        let (raw, stats) = counted(b"ignored", 1);
        assert_eq!(
            collect_verified_model_source(raw, crate::limits::MAX_FILE_BYTES + 1, &observer)
                .unwrap_err(),
            crate::StlError::ResourceStopped(ResourceStop::InputLimitExceeded)
        );
        assert!(stats.lock().unwrap().requests.is_empty());
        let mut context = tiny_context(tiny_limits());
        assert_eq!(
            context
                .collect_model_member(Cursor::new(b"abc"), 3)
                .unwrap(),
            b"abc"
        );
        assert_eq!(
            context.usage(),
            PackageUsage {
                inflation_bytes: 3,
                retained_input_bytes: 0
            }
        );
        assert_eq!(
            context
                .collect_model_member(Cursor::new(b"abcd"), 3)
                .unwrap_err(),
            crate::StlError::ResourceStopped(ResourceStop::InputLimitExceeded)
        );
        assert!(context.collector_test.probes.iter().all(|(a, b)| a == b));
        assert!(context.collector_test.targets.iter().all(|n| *n <= 3));
    }

    #[test]
    fn plain_collection_spends_no_package_allowance_and_inflated_exact_composition_spends_once() {
        let mut context = tiny_context(tiny_limits());
        let observer = context.observer();
        let before = context.usage();
        assert_eq!(
            collect_verified_model_source(Cursor::new(b"abc"), 3, &observer).unwrap(),
            b"abc"
        );
        assert_eq!(context.usage(), before);
        assert_eq!(
            collect_verified_model_source(
                context.inflated_reader(Cursor::new(b"abc")),
                3,
                &observer
            )
            .unwrap(),
            b"abc"
        );
        assert_eq!(
            context.usage(),
            PackageUsage {
                inflation_bytes: 3,
                retained_input_bytes: 0
            }
        );
        assert_eq!(
            context
                .collect_binary(Cursor::new(b"png"), BinaryRole::EmbeddedImage)
                .unwrap(),
            b"png"
        );
        assert_eq!(
            context.usage(),
            PackageUsage {
                inflation_bytes: 6,
                retained_input_bytes: 0
            }
        );
        let mut context = tiny_context(PackageTestLimits {
            fail_reserve: true,
            ..tiny_limits()
        });
        assert_eq!(
            context
                .collect_binary(Cursor::new(b"png"), BinaryRole::EmbeddedImage)
                .unwrap_err(),
            crate::StlError::AllocationFailed
        );
        assert!(context.collector_test.capacities.is_empty());
        assert_eq!(context.retained_input, 0);
    }

    #[test]
    fn real_binary_and_exact_zip_collection_verify_crc_and_first_cause() {
        for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
            for corrupt in [false, true] {
                for exact in [false, true] {
                    let observer = ZipResourceObserver::new();
                    let mut zip = observer
                        .open_bounded_zip(Cursor::new(zip_fixture(method, corrupt)))
                        .unwrap();
                    let mut context =
                        PackageProcessingContext::with_test_limits(observer.clone(), tiny_limits());
                    let raw = zip.by_name("part").unwrap();
                    let result = if exact {
                        collect_verified_model_source(context.inflated_reader(raw), 3, &observer)
                    } else {
                        context.collect_model_member(raw, 3)
                    };
                    if corrupt {
                        assert!(matches!(result, Err(crate::StlError::Malformed3mf { .. })));
                    } else {
                        assert_eq!(result.unwrap(), b"abc");
                    }
                    observer.check().unwrap();
                }
            }
        }
        let mut context = tiny_context(PackageTestLimits {
            inflation: 3,
            ..tiny_limits()
        });
        assert_eq!(
            context
                .collect_model_member(Cursor::new(b"abcd"), 3)
                .unwrap_err(),
            crate::StlError::ResourceStopped(ResourceStop::ReadWorkExceeded)
        );
        let mut context = tiny_context(PackageTestLimits {
            inflation: 3,
            ..tiny_limits()
        });
        let observer = context.observer();
        assert_eq!(
            collect_verified_model_source(
                context.inflated_reader(Cursor::new(b"abcd")),
                3,
                &observer
            )
            .unwrap_err(),
            crate::StlError::ResourceStopped(ResourceStop::ReadWorkExceeded)
        );
    }

    #[test]
    fn processing_context_roles_have_canonical_fixed_production_limits() {
        assert_eq!(TextRole::Relationships.byte_limit(), 64 * 1024);
        assert_eq!(TextRole::ModelSettings.byte_limit(), 8 * 1024 * 1024);
        assert_eq!(TextRole::ProjectSettings.byte_limit(), 8 * 1024 * 1024);
        let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
        assert_eq!(context.retained_input, 0);
        assert_eq!(
            context.model_reader(Cursor::new(b"")).remaining,
            crate::limits::MAX_FILE_BYTES
        );
    }

    #[test]
    fn processing_context_collects_exact_roles_and_stops_one_over_without_probe_growth() {
        for role in [
            TextRole::Relationships,
            TextRole::ModelSettings,
            TextRole::ProjectSettings,
        ] {
            let mut context = tiny_context(tiny_limits());
            assert_eq!(
                context.collect_text(Cursor::new(b"abc"), role).unwrap(),
                "abc"
            );
            assert_eq!(context.retained_input, 3);
            assert!(context
                .collector_test
                .probes
                .iter()
                .all(|(before, after)| before == after));
            let mut context = tiny_context(tiny_limits());
            assert!(matches!(
                context.collect_text(Cursor::new(b"abcd"), role),
                Err(crate::StlError::ResourceStopped(
                    ResourceStop::InputLimitExceeded
                ))
            ));
            assert_eq!(context.retained_input, 3);
            assert!(context
                .collector_test
                .probes
                .iter()
                .all(|(before, after)| before == after));
            let (input, calls) = counted(b"later", usize::MAX);
            assert!(context.collect_text(input, role).is_err());
            assert!(calls.lock().unwrap().requests.is_empty());
            assert!(matches!(
                context
                    .observer()
                    .finish(Ok::<_, ZipOpenError>("cached result")),
                Err(ZipOpenError::ResourceStopped(
                    ResourceStop::InputLimitExceeded
                ))
            ));
        }
    }

    #[test]
    fn processing_context_text_counter_is_cumulative_across_roles_and_dropped_buffers() {
        let mut context = tiny_context(PackageTestLimits {
            relationships: 8,
            model_settings: 8,
            project_settings: 8,
            text_total: 5,
            ..tiny_limits()
        });
        let first = context
            .collect_text(Cursor::new(b"abc"), TextRole::Relationships)
            .unwrap();
        let second = context
            .collect_text(Cursor::new(b"de"), TextRole::ModelSettings)
            .unwrap();
        assert_eq!(first, "abc");
        assert_eq!(second, "de");
        drop((first, second));
        assert_eq!(context.retained_input, 5);
        assert!(matches!(
            context.collect_text(Cursor::new(b"x"), TextRole::ProjectSettings),
            Err(crate::StlError::ResourceStopped(
                ResourceStop::InputLimitExceeded
            ))
        ));
        assert_eq!(context.retained_input, 5);
    }

    #[test]
    fn processing_context_geometric_growth_uses_current_length_plus_aggregate_remaining() {
        let mut context = tiny_context(PackageTestLimits {
            model_settings: 10,
            text_total: 10,
            ..tiny_limits()
        });
        assert_eq!(
            context
                .collect_text(Cursor::new(b"abc"), TextRole::ModelSettings)
                .unwrap(),
            "abc"
        );
        let prior = context.collector_test.targets.len();
        let (input, _) = counted(b"defghij", 1);
        assert_eq!(
            context
                .collect_text(input, TextRole::ModelSettings)
                .unwrap(),
            "defghij"
        );
        assert_eq!(context.retained_input, 10);
        assert!(context.collector_test.targets[prior..]
            .iter()
            .all(|target| *target <= 7));
        // Pure target assertions avoid relying on allocator capacity rounding.
        assert_eq!(retained_capacity_target(4, 4, 5, 10, 3), Some(7));
        assert_eq!(retained_capacity_target(4, 4, 5, 10, 0), None);
        assert_eq!(
            retained_capacity_target(usize::MAX, 0, 1, usize::MAX, 1),
            None
        );

        let bytes = vec![b'a'; 128 * 1024];
        let mut context = tiny_context(PackageTestLimits {
            inflation: (bytes.len() * 2) as u64,
            model_settings: bytes.len(),
            text_total: bytes.len() * 2,
            ..tiny_limits()
        });
        assert_eq!(
            context
                .collect_text(Cursor::new(&bytes), TextRole::ModelSettings)
                .unwrap()
                .len(),
            bytes.len()
        );
        assert!(
            context.collector_test.targets.len() <= 8,
            "growth must not reserve once per 4 KiB chunk"
        );
        assert!(context
            .collector_test
            .targets
            .iter()
            .all(|target| *target <= bytes.len()));
        assert!(context
            .collector_test
            .capacities
            .iter()
            .all(|(_, target, actual)| actual >= target));
    }

    #[test]
    fn processing_context_utf8_validation_moves_buffer_and_keeps_ordinary_failures_charged() {
        let limits = PackageTestLimits {
            model_settings: 8,
            ..tiny_limits()
        };
        let mut context = tiny_context(limits);
        let (input, _) = counted("a€".as_bytes(), 1);
        assert_eq!(
            context
                .collect_text(input, TextRole::ModelSettings)
                .unwrap(),
            "a€"
        );
        assert_eq!(context.retained_input, 4);
        let error = context
            .collect_text(Cursor::new([0xff]), TextRole::ModelSettings)
            .unwrap_err();
        assert!(matches!(error, crate::StlError::Malformed3mf { .. }));
        context.observer().check().unwrap();
        assert_eq!(context.retained_input, 5);
        assert_eq!(
            context
                .collect_text(Cursor::new(b"ok"), TextRole::ModelSettings)
                .unwrap(),
            "ok"
        );
        assert_eq!(context.retained_input, 7);
    }

    #[test]
    fn processing_context_failed_reservation_does_not_append_or_commit_input_copy_count() {
        let mut context = tiny_context(PackageTestLimits {
            fail_reserve: true,
            ..tiny_limits()
        });
        let (input, stats) = counted(b"abc", usize::MAX);
        assert!(matches!(
            context.collect_text(input, TextRole::ModelSettings),
            Err(crate::StlError::AllocationFailed)
        ));
        assert_eq!(context.retained_input, 0);
        assert_eq!(context.collector_test.targets, [3]);
        assert!(context.collector_test.capacities.is_empty());
        assert_eq!(stats.lock().unwrap().bytes, 3);
        let _ = context.observer().stop_io(ResourceStop::InputLimitExceeded);
        assert!(matches!(
            context.observer().check(),
            Err(ZipOpenError::AllocationFailed)
        ));
    }

    #[test]
    fn processing_context_model_limit_probes_real_eof_and_preserves_output_on_excess() {
        let mut context = tiny_context(tiny_limits());
        {
            let mut reader = context.model_reader(Cursor::new(b"abc"));
            reader.read_exact(&mut [0; 3]).unwrap();
            assert_eq!(reader.read(&mut [99; 1]).unwrap(), 0);
        }
        assert_eq!(context.inflation.remaining, 29);
        let mut context = tiny_context(tiny_limits());
        let (input, stats) = counted(b"abcd", usize::MAX);
        let mut reader = context.model_reader(input);
        reader.read_exact(&mut [0; 3]).unwrap();
        let mut output = [99; 1];
        let error = reader.read(&mut output).unwrap_err();
        assert_eq!(
            ResourceStop::from_io_error(&error),
            Some(ResourceStop::InputLimitExceeded)
        );
        assert_eq!(output, [99; 1]);
        assert!(reader.read(&mut []).is_err());
        assert_eq!(stats.lock().unwrap().bytes, 4);
    }

    #[test]
    fn processing_context_overlapping_caps_keep_inner_inflation_as_first_cause() {
        let mut context = tiny_context(PackageTestLimits {
            inflation: 3,
            ..tiny_limits()
        });
        let mut reader = context.model_reader(Cursor::new(b"abcd"));
        reader.read_exact(&mut [0; 3]).unwrap();
        let error = reader.read(&mut [0; 1]).unwrap_err();
        assert_eq!(
            ResourceStop::from_io_error(&error),
            Some(ResourceStop::ReadWorkExceeded)
        );
        for (role, total) in [(3, 16), (8, 3)] {
            let mut context = tiny_context(PackageTestLimits {
                inflation: 3,
                model_settings: role,
                text_total: total,
                ..tiny_limits()
            });
            assert!(matches!(
                context.collect_text(Cursor::new(b"abcd"), TextRole::ModelSettings),
                Err(crate::StlError::ResourceStopped(
                    ResourceStop::ReadWorkExceeded
                ))
            ));
        }
    }

    #[test]
    fn processing_context_model_text_and_raw_passes_use_exactly_one_inflation_counter() {
        let mut context = tiny_context(PackageTestLimits {
            inflation: 5,
            ..tiny_limits()
        });
        context
            .model_reader(Cursor::new(b"ab"))
            .read_exact(&mut [0; 2])
            .unwrap();
        assert_eq!(
            context
                .collect_text(Cursor::new(b"cd"), TextRole::ModelSettings)
                .unwrap(),
            "cd"
        );
        assert_eq!(context.retained_input, 2);
        assert_eq!(context.inflation.remaining, 1);
        let mut reader = context.inflated_reader(Cursor::new(b"ef"));
        assert_eq!(reader.read(&mut [0; 2]).unwrap(), 1);
        assert_eq!(
            ResourceStop::from_io_error(&reader.read(&mut [0; 1]).unwrap_err()),
            Some(ResourceStop::ReadWorkExceeded)
        );
    }

    #[test]
    fn processing_context_real_member_crc_remains_ordinary_for_both_fixed_read_paths() {
        for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
            for corrupt in [false, true] {
                let observer = ZipResourceObserver::new();
                let mut archive = observer
                    .open_bounded_zip(Cursor::new(zip_fixture(method, corrupt)))
                    .unwrap();
                let mut context =
                    PackageProcessingContext::with_test_limits(observer.clone(), tiny_limits());
                let text =
                    context.collect_text(archive.by_name("part").unwrap(), TextRole::ModelSettings);
                if corrupt {
                    assert!(matches!(text, Err(crate::StlError::Malformed3mf { .. })));
                } else {
                    assert_eq!(text.unwrap(), "abc");
                }
                observer.check().unwrap();

                let observer = ZipResourceObserver::new();
                let mut archive = observer
                    .open_bounded_zip(Cursor::new(zip_fixture(method, corrupt)))
                    .unwrap();
                let mut context =
                    PackageProcessingContext::with_test_limits(observer.clone(), tiny_limits());
                let mut reader = context.model_reader(archive.by_name("part").unwrap());
                let result = (|| {
                    reader.read_exact(&mut [0; 3])?;
                    assert_eq!(reader.read(&mut [0; 1])?, 0);
                    Ok::<_, io::Error>(())
                })();
                if corrupt {
                    assert!(result.is_err());
                } else {
                    result.unwrap();
                }
                observer.check().unwrap();
            }
        }
    }

    struct InterruptedReader {
        inner: CountedReader,
        interrupt_call: usize,
        attempts: usize,
        error: Option<io::Error>,
    }

    impl Read for InterruptedReader {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            self.attempts += 1;
            if self.attempts == self.interrupt_call {
                self.inner.stats.lock().unwrap().requests.push(output.len());
                return Err(self.error.take().unwrap());
            }
            self.inner.read(output)
        }
    }

    #[test]
    fn processing_context_usage_tracks_shared_reads_without_resetting() {
        let observer = ZipResourceObserver::new();
        let mut context = PackageProcessingContext::with_test_limits(
            observer.clone(),
            PackageTestLimits {
                inflation: 7,
                ..PackageTestLimits::default()
            },
        );
        assert_eq!(
            context.usage(),
            PackageUsage {
                inflation_bytes: 0,
                retained_input_bytes: 0,
            }
        );
        let mut first = Vec::new();
        context
            .inflated_reader(Cursor::new(b"abc"))
            .read_to_end(&mut first)
            .unwrap();
        assert_eq!(first, b"abc");
        assert_eq!(
            context
                .collect_text(Cursor::new(b"de"), TextRole::ModelSettings)
                .unwrap(),
            "de"
        );
        assert_eq!(
            context.usage(),
            PackageUsage {
                inflation_bytes: 5,
                retained_input_bytes: 2,
            }
        );
        let mut last = Vec::new();
        assert!(context
            .inflated_reader(Cursor::new(b"fgh"))
            .read_to_end(&mut last)
            .is_err());
        assert_eq!(last, b"fg");
        assert_eq!(context.usage().inflation_bytes, 7);
        assert!(context
            .collect_text(Cursor::new(b"ignored"), TextRole::ModelSettings)
            .is_err());
        assert_eq!(
            context.usage(),
            PackageUsage {
                inflation_bytes: 7,
                retained_input_bytes: 2,
            }
        );
        assert!(observer.check().is_err());
    }

    #[test]
    fn processing_context_retries_plain_interrupted_chunk_and_exact_eof_probe() {
        for interrupt_call in [1, 2] {
            let mut context = tiny_context(tiny_limits());
            let (inner, stats) = counted(b"abc", usize::MAX);
            let input = InterruptedReader {
                inner,
                interrupt_call,
                attempts: 0,
                error: Some(io::Error::from(io::ErrorKind::Interrupted)),
            };
            assert_eq!(
                context
                    .collect_text(input, TextRole::ModelSettings)
                    .unwrap(),
                "abc"
            );
            assert_eq!(context.retained_input, 3);
            assert_eq!(context.inflation.remaining, 29);
            let stats = stats.lock().unwrap();
            assert_eq!(stats.bytes, 3);
            assert_eq!(
                stats.requests,
                if interrupt_call == 1 {
                    vec![3, 3, 1]
                } else {
                    vec![3, 1, 1]
                }
            );
            context.observer().check().unwrap();
        }
    }

    #[test]
    fn processing_context_interrupted_terminal_payload_never_retries_input() {
        for interrupt_call in [1, 2] {
            for payload in [
                io::Error::new(io::ErrorKind::Interrupted, ResourceStop::InputLimitExceeded),
                io::Error::new(io::ErrorKind::Interrupted, ResourceStop::ReadWorkExceeded),
                io::Error::new(io::ErrorKind::Interrupted, ResourceStop::EnvelopeChanged),
                io::Error::new(io::ErrorKind::Interrupted, ZipAllocationFailed),
                io::Error::new(
                    io::ErrorKind::Interrupted,
                    crate::zip_bounds::ZipFundingBusy,
                ),
            ] {
                let expected = payload.to_string();
                let mut context = tiny_context(tiny_limits());
                let (inner, stats) = counted(b"abc", usize::MAX);
                let input = InterruptedReader {
                    inner,
                    interrupt_call,
                    attempts: 0,
                    error: Some(payload),
                };
                let error = context
                    .collect_text(input, TextRole::ModelSettings)
                    .unwrap_err();
                assert_eq!(error.to_string(), expected);
                assert!(matches!(
                    error,
                    crate::StlError::ResourceStopped(_)
                        | crate::StlError::AllocationFailed
                        | crate::StlError::FundingBusy
                ));
                assert_eq!(stats.lock().unwrap().requests.len(), interrupt_call);
                let requests = stats.lock().unwrap().requests.len();
                assert!(context
                    .collect_text(Cursor::new(b"later"), TextRole::ModelSettings)
                    .is_err());
                assert_eq!(stats.lock().unwrap().requests.len(), requests);
                assert_eq!(
                    context.retained_input,
                    if interrupt_call == 1 { 0 } else { 3 }
                );
                assert_eq!(
                    context.inflation.remaining,
                    if interrupt_call == 1 { 32 } else { 29 }
                );
            }
        }
    }
}
