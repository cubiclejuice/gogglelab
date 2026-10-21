//! Opt-in constructor limits. These bound cumulative admission work, not process RSS.
use crate::result::{ZipError, ZipResult};
use std::cell::Cell;
use std::io::{self, Read, Seek, SeekFrom};

/// Cumulative admission limits for the opt-in ZIP metadata constructor.
///
/// Every footer retry shares these limits. They do not constrain subsequent
/// member payload reads or represent a process-memory limit.
#[derive(Clone, Copy, Debug)]
pub struct MetadataLimits {
    /// Maximum physical central-directory records processed across attempts.
    pub max_entries: u64,
    /// Maximum encoded footer, central-directory and local-header metadata bytes processed.
    pub max_encoded_bytes: u64,
    /// Maximum combined ZIP32 and ZIP64 footer candidates examined.
    pub max_candidates: u64,
    /// Maximum bytes actually read while constructing archive metadata.
    pub max_read_bytes: u64,
    /// Maximum conservative cumulative allocation work admitted before allocation.
    /// This includes capacity and copy estimates rather than measured resident memory.
    pub max_allocation_work: u64,
    /// Maximum combined read and seek calls, including short or empty reads.
    pub max_io_calls: u64,
}
impl Default for MetadataLimits {
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            max_encoded_bytes: 16 * 1024 * 1024,
            max_candidates: 64,
            max_read_bytes: 64 * 1024 * 1024,
            max_allocation_work: 64 * 1024 * 1024,
            max_io_calls: 1_000_000,
        }
    }
}
/// The configured constructor resource limit which stopped admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetadataLimitKind {
    /// Physical central-directory record count.
    Entries,
    /// Encoded footer, central-directory and local-header metadata bytes.
    EncodedBytes,
    /// Combined ZIP32 and ZIP64 footer candidate attempts.
    Candidates,
    /// Bytes actually read during construction.
    ReadBytes,
    /// Conservative cumulative allocation work or representable capacity.
    AllocationWork,
    /// Combined metadata read and seek calls.
    IoCalls,
}
/// A deterministic constructor admission failure, recoverable without parsing text.
///
/// Actual allocator failures use [`MetadataAllocationFailed`] instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MetadataLimitExceeded {
    /// The resource whose admission limit was exceeded.
    pub kind: MetadataLimitKind,
    /// The effective maximum for that resource.
    pub limit: u64,
    /// The attempted cumulative value; overflow is represented by [`u64::MAX`].
    pub attempted: u64,
}
impl std::fmt::Display for MetadataLimitExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ZIP metadata {:?} limit exceeded ({} > {})",
            self.kind, self.attempted, self.limit
        )
    }
}
impl std::error::Error for MetadataLimitExceeded {}

/// Operational allocation failure, distinct from configured admission limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MetadataAllocationFailed;
impl std::fmt::Display for MetadataAllocationFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ZIP metadata allocation failed")
    }
}
impl std::error::Error for MetadataAllocationFailed {}
#[derive(Clone, Copy)]
enum ConstructorFailure {
    Limit(MetadataLimitExceeded),
    AllocationFailed,
    InvalidEnvelope(&'static str),
}
impl ConstructorFailure {
    fn error(self) -> ZipError {
        match self {
            Self::Limit(error) => ZipError::Io(io::Error::other(error)),
            Self::AllocationFailed => ZipError::Io(io::Error::new(
                io::ErrorKind::OutOfMemory,
                MetadataAllocationFailed,
            )),
            Self::InvalidEnvelope(message) => crate::result::invalid_archive(message),
        }
    }
}

pub(crate) struct ConstructorBudget {
    limits: Option<MetadataLimits>,
    encoded: Cell<u64>,
    candidates: Cell<u64>,
    reads: Cell<u64>,
    allocations: Cell<u64>,
    entries: Cell<u64>,
    io_calls: Cell<u64>,
    fatal: Cell<Option<ConstructorFailure>>,
}
impl ConstructorBudget {
    pub(crate) fn new(limits: MetadataLimits) -> Self {
        Self {
            limits: Some(limits),
            ..Self::unlimited()
        }
    }
    pub(crate) fn unlimited() -> Self {
        Self {
            limits: None,
            encoded: Cell::new(0),
            candidates: Cell::new(0),
            reads: Cell::new(0),
            allocations: Cell::new(0),
            entries: Cell::new(0),
            io_calls: Cell::new(0),
            fatal: Cell::new(None),
        }
    }
    pub(crate) fn bounded(&self) -> bool {
        self.limits.is_some()
    }
    pub(crate) fn check(&self) -> ZipResult<()> {
        match self.fatal.get() {
            Some(failure) => Err(failure.error()),
            None => Ok(()),
        }
    }
    fn fail(&self, kind: MetadataLimitKind, limit: u64, attempted: u64) -> ZipError {
        let failure =
            self.fatal
                .get()
                .unwrap_or(ConstructorFailure::Limit(MetadataLimitExceeded {
                    kind,
                    limit,
                    attempted,
                }));
        self.fatal.set(Some(failure));
        failure.error()
    }
    pub(crate) fn allocation_failure(&self) -> ZipError {
        let failure = self
            .fatal
            .get()
            .unwrap_or(ConstructorFailure::AllocationFailed);
        self.fatal.set(Some(failure));
        failure.error()
    }
    pub(crate) fn invalid_envelope(&self, message: &'static str) -> ZipError {
        let failure = self
            .fatal
            .get()
            .unwrap_or(ConstructorFailure::InvalidEnvelope(message));
        self.fatal.set(Some(failure));
        failure.error()
    }
    fn io_failure(&self, error: io::Error) -> io::Error {
        if error.kind() == io::ErrorKind::OutOfMemory {
            match self.allocation_failure() {
                ZipError::Io(error) => error,
                other => io::Error::from(other),
            }
        } else {
            error
        }
    }
    fn charge(
        &self,
        counter: &Cell<u64>,
        amount: u64,
        limit: u64,
        kind: MetadataLimitKind,
    ) -> ZipResult<()> {
        self.check()?;
        if self.limits.is_none() {
            return Ok(());
        }
        let next = counter
            .get()
            .checked_add(amount)
            .ok_or_else(|| self.fail(kind, limit, u64::MAX))?;
        if next > limit {
            return Err(self.fail(kind, limit, next));
        }
        counter.set(next);
        Ok(())
    }
    pub(crate) fn encoded(&self, amount: u64) -> ZipResult<()> {
        self.charge(
            &self.encoded,
            amount,
            self.limits.map_or(u64::MAX, |l| l.max_encoded_bytes),
            MetadataLimitKind::EncodedBytes,
        )
    }
    pub(crate) fn allocate(&self, amount: u64) -> ZipResult<()> {
        self.charge(
            &self.allocations,
            amount,
            self.limits.map_or(u64::MAX, |l| l.max_allocation_work),
            MetadataLimitKind::AllocationWork,
        )
    }
    pub(crate) fn candidate(&self) -> ZipResult<()> {
        self.charge(
            &self.candidates,
            1,
            self.limits.map_or(u64::MAX, |l| l.max_candidates),
            MetadataLimitKind::Candidates,
        )?;
        self.allocate(4096)
    }
    pub(crate) fn io_call(&self) -> ZipResult<()> {
        self.charge(
            &self.io_calls,
            1,
            self.limits.map_or(u64::MAX, |l| l.max_io_calls),
            MetadataLimitKind::IoCalls,
        )
    }
    pub(crate) fn declared_entries(&self, amount: u64) -> ZipResult<()> {
        self.check()?;
        if let Some(l) = self.limits {
            let limit = l.max_entries.min(usize::MAX as u64);
            if amount > limit {
                return Err(self.fail(MetadataLimitKind::Entries, limit, amount));
            }
        }
        Ok(())
    }
    pub(crate) fn entry(&self) -> ZipResult<()> {
        self.charge(
            &self.entries,
            1,
            self.limits.map_or(u64::MAX, |l| l.max_entries),
            MetadataLimitKind::Entries,
        )
    }
    pub(crate) fn capacity_limit_error(&self) -> ZipError {
        self.fail(
            MetadataLimitKind::AllocationWork,
            self.limits.map_or(u64::MAX, |l| l.max_allocation_work),
            u64::MAX,
        )
    }
    pub(crate) fn cost(&self, amount: usize, multiple: u64) -> ZipResult<u64> {
        (amount as u64)
            .checked_mul(multiple)
            .filter(|cost| *cost <= isize::MAX as u64)
            .ok_or_else(|| self.capacity_limit_error())
    }
    pub(crate) fn zeroed(&self, len: usize) -> ZipResult<Box<[u8]>> {
        if !self.bounded() {
            return Ok(vec![0; len].into_boxed_slice());
        }
        self.allocate(
            self.cost(len, 2)?
                .checked_add(64)
                .ok_or_else(|| self.capacity_limit_error())?,
        )?;
        let mut data = Vec::new();
        data.try_reserve_exact(len)
            .map_err(|_| self.allocation_failure())?;
        data.resize(len, 0);
        Ok(data.into_boxed_slice())
    }
    pub(crate) fn copied(&self, bytes: &[u8]) -> ZipResult<Box<[u8]>> {
        let mut value = self.zeroed(bytes.len())?;
        value.copy_from_slice(bytes);
        Ok(value)
    }
    /// Resolve the optional ZIP64 disk-start sentinel without trusting a field
    /// which the upstream reader otherwise ignores. The extra data is already
    /// captured and range-validated; no nested allocation occurs here.
    pub(crate) fn entry_disk(
        &self,
        disk: u16,
        raw: &[u8],
        uncompressed: u32,
        compressed: u32,
        header: u32,
    ) -> ZipResult<()> {
        if !self.bounded() {
            return Ok(());
        }
        let mut offset = 0usize;
        let mut found = false;
        while raw.len() - offset >= 4 {
            let kind = u16::from_le_bytes([raw[offset], raw[offset + 1]]);
            let length = usize::from(u16::from_le_bytes([raw[offset + 2], raw[offset + 3]]));
            offset += 4;
            let end = offset
                .checked_add(length)
                .filter(|end| *end <= raw.len())
                .ok_or_else(|| crate::result::invalid_archive("Invalid extra field range"))?;
            if kind == 1 {
                if found {
                    return Err(self.invalid_envelope("Ambiguous ZIP64 entry metadata"));
                }
                found = true;
                let prefix = if length >= 24 {
                    24
                } else {
                    8 * usize::from(uncompressed == u32::MAX)
                        + 8 * usize::from(compressed == u32::MAX)
                        + 8 * usize::from(header == u32::MAX)
                };
                if disk == u16::MAX || length >= 28 {
                    if length < prefix + 4 {
                        return Err(self.invalid_envelope("ZIP64 disk-start field is missing"));
                    }
                    let position = offset + prefix;
                    let start = u32::from_le_bytes(
                        raw[position..position + 4]
                            .try_into()
                            .expect("checked disk-start range"),
                    );
                    if start != 0 {
                        return Err(self.invalid_envelope("ZIP64 entry requires disk zero"));
                    }
                }
            }
            offset = end;
        }
        if disk == u16::MAX && !found {
            return Err(self.invalid_envelope("ZIP64 disk-start field is missing"));
        }
        Ok(())
    }

    /// Before nested parsers use subrecord lengths. Padding below four bytes
    /// cannot allocate and retains upstream's padding behavior.
    pub(crate) fn extras(&self, raw: &[u8], name_len: usize, comment_len: usize) -> ZipResult<()> {
        if !self.bounded() {
            return Ok(());
        }
        let mut offset = 0usize;
        let mut name_bound = self.cost(name_len, 3)?;
        let mut comment_bound = self.cost(comment_len, 3)?;
        while raw.len() - offset >= 4 {
            let kind = u16::from_le_bytes([raw[offset], raw[offset + 1]]);
            let length = usize::from(u16::from_le_bytes([raw[offset + 2], raw[offset + 3]]));
            offset += 4;
            let end = offset
                .checked_add(length)
                .filter(|end| *end <= raw.len())
                .ok_or_else(|| {
                    crate::result::invalid_archive(
                        "Extra field length exceeds its bounded central record",
                    )
                })?;
            if kind == 0x7075 || kind == 0x6375 {
                let previous = if kind == 0x7075 {
                    name_bound
                } else {
                    comment_bound
                };
                self.allocate(
                    previous
                        .checked_mul(4)
                        .ok_or_else(|| self.capacity_limit_error())?,
                )?;
                if kind == 0x7075 {
                    name_bound = length as u64;
                } else {
                    comment_bound = length as u64;
                }
            }
            offset = end;
        }
        Ok(())
    }
}
/// Exists only for construction, and is dropped before the original R is stored
/// in ZipArchive. Member payload reads never inherit the metadata-read cap.
pub(crate) struct MetadataReader<'a, R> {
    pub(crate) inner: &'a mut R,
    pub(crate) budget: &'a ConstructorBudget,
}
impl<R: Read> Read for MetadataReader<'_, R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.budget.io_call().map_err(io::Error::from)?;
        if output.is_empty() {
            return Ok(0);
        }
        let limits = self.budget.limits.expect("metadata reader is opt-in only");
        let remaining = limits
            .max_read_bytes
            .saturating_sub(self.budget.reads.get());
        if remaining == 0 {
            return Err(io::Error::from(self.budget.fail(
                MetadataLimitKind::ReadBytes,
                limits.max_read_bytes,
                limits.max_read_bytes.saturating_add(1),
            )));
        }
        let length = output
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let read = self
            .inner
            .read(&mut output[..length])
            .map_err(|error| self.budget.io_failure(error))?;
        if read > length {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ZIP reader returned an invalid length",
            ));
        }
        self.budget
            .charge(
                &self.budget.reads,
                read as u64,
                limits.max_read_bytes,
                MetadataLimitKind::ReadBytes,
            )
            .map_err(io::Error::from)?;
        Ok(read)
    }
}
impl<R: Seek> Seek for MetadataReader<'_, R> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.budget.io_call().map_err(io::Error::from)?;
        self.inner
            .seek(from)
            .map_err(|error| self.budget.io_failure(error))
    }
}

#[cfg(test)]
mod allocation_failure_tests {
    use super::*;
    #[test]
    fn allocator_failure_is_sticky_operational_and_not_limit_exhaustion() {
        let budget = ConstructorBudget::new(MetadataLimits::default());
        let failure = budget.allocation_failure();
        assert!(failure.metadata_allocation_failure().is_some());
        assert!(failure.metadata_limit().is_none());
        let repeated = budget.allocate(1).unwrap_err();
        assert!(repeated.metadata_allocation_failure().is_some());
        assert!(repeated.metadata_limit().is_none());
    }
}
