//! Actual package inflation accounting, unwired until the model-reader gate.
//!
//! This bounds delivered bytes, not caller buffer allocation, decompressor work
//! space or process RSS. In particular, `Read::read_to_end` may reserve before
//! calling this reader; retained-buffer admission remains a separate prerequisite.

use std::io::{self, Read};

use crate::limits::MAX_3MF_INFLATED_BYTES;
use crate::zip_bounds::ZipResourceObserver;
use crate::ResourceStop;

/// One non-cloneable, single-owner actual-read allowance for a package operation.
///
/// Every repeated member read spends this same allowance. Declared entry lengths
/// are not charged here; cached parsing without input reads spends nothing.
pub struct PackageInflationBudget {
    remaining: u64,
    observer: ZipResourceObserver,
}

impl PackageInflationBudget {
    /// Use the fixed 512 MiB allowance and the operation's existing observer.
    pub fn new(observer: ZipResourceObserver) -> Self {
        Self {
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
}
