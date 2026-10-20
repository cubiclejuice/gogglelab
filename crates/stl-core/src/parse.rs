use crate::error::StlError;
use crate::limits::MAX_TRIANGLES;
use crate::zip_bounds::ZipResourceObserver;
use crate::ResourceStop;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StlFormat {
    Binary,
    Ascii,
    /// 3MF package (see `threemf.rs`). Kept on this enum, despite the name,
    /// because every consumer already switches on it and a parsed 3MF is
    /// otherwise indistinguishable from a parsed STL.
    ThreeMf,
}

/// A parsed STL, before measurement or integrity checks (S3).
///
/// `positions` is the flat `[f32; 9*N]` layout that becomes the IPC geometry
/// buffer (spec §2, buffer v2): 3 vertices × xyz per triangle, non-indexed.
/// `stored_normals` is `[f32; 3*N]`, one normal per triangle exactly as recorded
/// in the file — kept for `integrity()` (S3), which reports what exporters wrote
/// as a fact distinct from mesh topology. Buffer v2 does not ship these over IPC.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedStl {
    pub format: StlFormat,
    pub triangle_count: u32,
    pub positions: Vec<f32>,
    pub stored_normals: Vec<f32>,
}

const BINARY_HEADER_LEN: usize = 80;
const BINARY_COUNT_LEN: usize = 4;
const BINARY_PREFIX_LEN: usize = BINARY_HEADER_LEN + BINARY_COUNT_LEN; // 84
const BINARY_RECORD_LEN: u64 = 50; // 12 (normal) + 36 (3 vertices) + 2 (attribute)

// Production always uses the fixed policy; the scoped test profile exercises
// the actual public discriminator with small fixtures.
#[cfg(test)]
fn triangle_limit() -> u32 {
    if let Some(profile) = test_profile() {
        return profile.limit;
    }
    MAX_TRIANGLES
}

#[cfg(test)]
#[derive(Clone, Copy)]
enum ParseTestStop {
    Resource(ResourceStop),
    Allocation,
}

#[cfg(test)]
#[derive(Default)]
struct ParseTestProbes {
    binary_reserves: usize,
    binary_float_reads: usize,
    ascii_refusal_lengths: Option<(usize, usize)>,
    reserve_calls: usize,
    fail_reserve_at: Option<usize>,
    stop_after_first_success: Option<ParseTestStop>,
    failure_lengths: Option<(usize, usize)>,
    discriminator_entries: usize,
}

#[cfg(test)]
struct ParseTestProfile {
    limit: u32,
    probes: std::cell::RefCell<ParseTestProbes>,
}

#[cfg(test)]
thread_local! {
    static PARSE_TEST_PROFILE: std::cell::RefCell<Option<std::rc::Rc<ParseTestProfile>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn test_profile() -> Option<std::rc::Rc<ParseTestProfile>> {
    PARSE_TEST_PROFILE.with(|slot| slot.borrow().clone())
}

#[cfg(test)]
struct ParseTestProfileGuard {
    previous: Option<std::rc::Rc<ParseTestProfile>>,
}

#[cfg(test)]
impl ParseTestProfileGuard {
    fn install(limit: u32) -> (Self, std::rc::Rc<ParseTestProfile>) {
        let profile = std::rc::Rc::new(ParseTestProfile {
            limit,
            probes: std::cell::RefCell::new(ParseTestProbes::default()),
        });
        let previous = PARSE_TEST_PROFILE.with(|slot| slot.replace(Some(profile.clone())));
        (Self { previous }, profile)
    }
}

#[cfg(test)]
impl Drop for ParseTestProfileGuard {
    fn drop(&mut self) {
        PARSE_TEST_PROFILE.with(|slot| slot.replace(self.previous.take()));
    }
}

struct ParseControl<'a> {
    observer: &'a ZipResourceObserver,
    limit: u32,
    #[cfg(test)]
    profile: Option<std::rc::Rc<ParseTestProfile>>,
}

impl<'a> ParseControl<'a> {
    fn new(observer: &'a ZipResourceObserver) -> Self {
        #[cfg(test)]
        let profile = test_profile();
        Self {
            observer,
            #[cfg(not(test))]
            limit: MAX_TRIANGLES,
            #[cfg(test)]
            limit: profile
                .as_ref()
                .map_or(MAX_TRIANGLES, |profile| profile.limit),
            #[cfg(test)]
            profile,
        }
    }

    fn input_limit(&self) -> StlError {
        let _ = self.observer.stop_io(ResourceStop::InputLimitExceeded);
        self.observer
            .check()
            .expect_err("input stop is latched")
            .into()
    }

    fn allocation_failed(&self) -> StlError {
        let _ = self.observer.allocation_failed_io();
        self.observer
            .check()
            .expect_err("allocation stop is latched")
            .into()
    }

    fn elements(&self, triangles: usize, per_triangle: usize) -> Result<usize, StlError> {
        triangles
            .checked_mul(per_triangle)
            .filter(|elements| *elements <= isize::MAX as usize / std::mem::size_of::<f32>())
            .ok_or_else(|| self.input_limit())
    }

    fn reserve(
        &self,
        output: &mut Vec<f32>,
        needed: usize,
        ceiling: usize,
        exact: bool,
    ) -> Result<(), StlError> {
        if needed > ceiling {
            return Err(self.input_limit());
        }
        if output.capacity() >= needed {
            return Ok(());
        }
        let target = if exact {
            needed
        } else {
            needed
                .max(output.capacity().checked_mul(2).unwrap_or(ceiling))
                .min(ceiling)
        };
        let additional = target
            .checked_sub(output.len())
            .ok_or_else(|| self.input_limit())?;
        self.observer.check()?;
        #[cfg(test)]
        if let Some(profile) = &self.profile {
            let fail = {
                let mut probes = profile.probes.borrow_mut();
                probes.reserve_calls += 1;
                if exact {
                    probes.binary_reserves += 1;
                }
                probes.fail_reserve_at == Some(probes.reserve_calls)
            };
            if fail {
                return Err(self.allocation_failed());
            }
        }
        output
            .try_reserve_exact(additional)
            .map_err(|_| self.allocation_failed())?;
        #[cfg(test)]
        if let Some(profile) = &self.profile {
            let injected = {
                let probes = profile.probes.borrow();
                if probes.reserve_calls == 1 {
                    probes.stop_after_first_success
                } else {
                    None
                }
            };
            match injected {
                Some(ParseTestStop::Resource(stop)) => {
                    let _ = self.observer.stop_io(stop);
                }
                Some(ParseTestStop::Allocation) => {
                    let _ = self.observer.allocation_failed_io();
                }
                None => {}
            }
        }
        self.observer.check()?;
        Ok(())
    }

    fn reserve_pair(
        &self,
        positions: &mut Vec<f32>,
        normals: &mut Vec<f32>,
        triangles: u32,
        exact: bool,
    ) -> Result<(), StlError> {
        let needed_positions = self.elements(triangles as usize, 9)?;
        let needed_normals = self.elements(triangles as usize, 3)?;
        let max_positions = self.elements(self.limit as usize, 9)?;
        let max_normals = self.elements(self.limit as usize, 3)?;
        let result = (|| {
            self.reserve(positions, needed_positions, max_positions, exact)?;
            self.reserve(normals, needed_normals, max_normals, exact)
        })();
        #[cfg(test)]
        if result.is_err() {
            if let Some(profile) = &self.profile {
                profile.probes.borrow_mut().failure_lengths =
                    Some((positions.len(), normals.len()));
            }
        }
        result
    }
}

/// Parse STL bytes already read into memory. The `MAX_FILE_BYTES` cap (spec §3)
/// is enforced by the caller via `fs::metadata` before the read that produces
/// `bytes` — see module docs in `limits.rs`.
pub fn parse(bytes: &[u8]) -> Result<ParsedStl, StlError> {
    let observer = ZipResourceObserver::new();
    parse_observed(bytes, &observer)
}

pub(crate) fn parse_observed(
    bytes: &[u8],
    observer: &ZipResourceObserver,
) -> Result<ParsedStl, StlError> {
    observer.check()?;
    let control = ParseControl::new(observer);
    let result = parse_inner(bytes, &control);
    observer.finish(result)
}

fn parse_inner(bytes: &[u8], control: &ParseControl<'_>) -> Result<ParsedStl, StlError> {
    #[cfg(test)]
    if let Some(profile) = &control.profile {
        profile.probes.borrow_mut().discriminator_entries += 1;
    }
    if bytes.is_empty() {
        return Err(StlError::EmptyFile);
    }

    if bytes.len() >= BINARY_PREFIX_LEN {
        // NOTE: `count` is only meaningful once we know this is binary-shaped.
        // For an ASCII file, bytes[80..84] are arbitrary text bytes reinterpreted
        // as an LE u32 — routinely a huge, meaningless number. It must never be
        // compared against MAX_TRIANGLES before the ASCII possibility is ruled
        // out, or every sufficiently long ASCII file gets misdiagnosed.
        let count = read_u32_le(bytes, BINARY_HEADER_LEN)?;
        let expected = BINARY_PREFIX_LEN as u64 + count as u64 * BINARY_RECORD_LEN;

        if expected == bytes.len() as u64 {
            // Reliable discriminator (R-021): the size arithmetic matches, so
            // this is binary even if it happens to start with "solid ". Only
            // now is `count` known to be the real, intended triangle count.
            if count > control.limit {
                return Err(StlError::TooManyTriangles {
                    count,
                    limit: control.limit,
                });
            }
            return parse_binary(bytes, count, control);
        }
        if looks_like_ascii(bytes) {
            return parse_ascii(bytes, control);
        }
        // Binary-shaped (doesn't look like ASCII) but the size doesn't match.
        // A declared count this large can't be a mere truncation — reject it
        // for what it is (R-007) rather than reporting a truncation the
        // reader could never sanely act on.
        if count > control.limit {
            return Err(StlError::TooManyTriangles {
                count,
                limit: control.limit,
            });
        }
        return Err(StlError::TruncatedFile {
            expected,
            actual: bytes.len() as u64,
        });
    }

    if looks_like_ascii(bytes) {
        return parse_ascii(bytes, control);
    }

    Err(StlError::MalformedHeader)
}

fn looks_like_ascii(bytes: &[u8]) -> bool {
    let trimmed = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .map(|start| &bytes[start..])
        .unwrap_or(&[]);
    trimmed.len() >= 5 && trimmed[..5].eq_ignore_ascii_case(b"solid")
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Result<u32, StlError> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or(StlError::MalformedHeader)?;
    let arr: [u8; 4] = slice.try_into().map_err(|_| StlError::MalformedHeader)?;
    Ok(u32::from_le_bytes(arr))
}

fn read_f32_le(bytes: &[u8], offset: usize, _control: &ParseControl<'_>) -> Result<f32, StlError> {
    #[cfg(test)]
    if let Some(profile) = &_control.profile {
        profile.probes.borrow_mut().binary_float_reads += 1;
    }
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or(StlError::MalformedHeader)?;
    let arr: [u8; 4] = slice.try_into().map_err(|_| StlError::MalformedHeader)?;
    Ok(f32::from_le_bytes(arr))
}

fn parse_binary(
    bytes: &[u8],
    count: u32,
    control: &ParseControl<'_>,
) -> Result<ParsedStl, StlError> {
    // The size discriminator and count admission precede both reservations.
    let mut positions = Vec::new();
    let mut stored_normals = Vec::new();
    control.reserve_pair(&mut positions, &mut stored_normals, count, true)?;

    for i in 0..count {
        let record_start = BINARY_PREFIX_LEN + i as usize * BINARY_RECORD_LEN as usize;

        for c in 0..3 {
            let v = read_f32_le(bytes, record_start + c * 4, control)?;
            stored_normals.push(v);
        }

        let vertex_start = record_start + 12;
        for v in 0..3 {
            for c in 0..3 {
                let value = read_f32_le(bytes, vertex_start + v * 12 + c * 4, control)?;
                if !value.is_finite() {
                    return Err(StlError::NonFiniteCoordinate { triangle_index: i });
                }
                positions.push(value);
            }
        }
        // 2-byte attribute field at record_start + 48 is intentionally ignored.
    }

    Ok(ParsedStl {
        format: StlFormat::Binary,
        triangle_count: count,
        positions,
        stored_normals,
    })
}

/// Minimal whitespace-token ASCII STL parser. STL ASCII is not indentation- or
/// newline-sensitive, so tokenizing on whitespace and walking the keyword
/// sequence (`facet normal .. outer loop vertex .. vertex .. vertex ..
/// endloop endfacet`) is both correct and simple. Malformed structure or a
/// non-numeric token where a float is expected is `MalformedHeader`.
fn parse_ascii(bytes: &[u8], control: &ParseControl<'_>) -> Result<ParsedStl, StlError> {
    let text = std::str::from_utf8(bytes).map_err(|_| StlError::MalformedHeader)?;
    let mut tokens = text.split_ascii_whitespace().peekable();

    // "solid" [name...] — name may be absent or multiple words; skip everything
    // up to the first "facet" or "endsolid".
    match tokens.next() {
        Some(t) if t.eq_ignore_ascii_case("solid") => {}
        _ => return Err(StlError::MalformedHeader),
    }
    while let Some(&t) = tokens.peek() {
        if t.eq_ignore_ascii_case("facet") || t.eq_ignore_ascii_case("endsolid") {
            break;
        }
        tokens.next();
    }

    let mut positions = Vec::new();
    let mut stored_normals = Vec::new();
    let mut triangle_index: u32 = 0;

    loop {
        match tokens.next() {
            Some(t) if t.eq_ignore_ascii_case("endsolid") => break,
            Some(t) if t.eq_ignore_ascii_case("facet") => {
                expect_keyword(&mut tokens, "normal")?;
                let mut facet_normal = [0.0_f32; 3];
                for value in &mut facet_normal {
                    *value = next_float(&mut tokens)?;
                }
                expect_keyword(&mut tokens, "outer")?;
                expect_keyword(&mut tokens, "loop")?;
                let mut facet_positions = [0.0_f32; 9];
                for vertex in facet_positions.chunks_exact_mut(3) {
                    expect_keyword(&mut tokens, "vertex")?;
                    for value in vertex {
                        *value = next_float(&mut tokens)?;
                        if !value.is_finite() {
                            return Err(StlError::NonFiniteCoordinate { triangle_index });
                        }
                    }
                }
                expect_keyword(&mut tokens, "endloop")?;
                expect_keyword(&mut tokens, "endfacet")?;

                if triangle_index == control.limit {
                    #[cfg(test)]
                    if let Some(profile) = &control.profile {
                        profile.probes.borrow_mut().ascii_refusal_lengths =
                            Some((positions.len(), stored_normals.len()));
                    }
                    return Err(StlError::TooManyTriangles {
                        count: triangle_index + 1,
                        limit: control.limit,
                    });
                }
                control.reserve_pair(
                    &mut positions,
                    &mut stored_normals,
                    triangle_index + 1,
                    false,
                )?;
                positions.extend_from_slice(&facet_positions);
                stored_normals.extend_from_slice(&facet_normal);
                triangle_index += 1;
            }
            _ => return Err(StlError::MalformedHeader),
        }
    }

    Ok(ParsedStl {
        format: StlFormat::Ascii,
        triangle_count: triangle_index,
        positions,
        stored_normals,
    })
}

fn expect_keyword<'a>(
    tokens: &mut impl Iterator<Item = &'a str>,
    keyword: &str,
) -> Result<(), StlError> {
    match tokens.next() {
        Some(t) if t.eq_ignore_ascii_case(keyword) => Ok(()),
        _ => Err(StlError::MalformedHeader),
    }
}

fn next_float<'a>(tokens: &mut impl Iterator<Item = &'a str>) -> Result<f32, StlError> {
    tokens
        .next()
        .and_then(|t| t.parse::<f32>().ok())
        .ok_or(StlError::MalformedHeader)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binary(count: u32, solid_header: bool) -> Vec<u8> {
        let mut bytes = vec![0_u8; BINARY_PREFIX_LEN + count as usize * BINARY_RECORD_LEN as usize];
        if solid_header {
            bytes[..6].copy_from_slice(b"solid ");
        }
        bytes[BINARY_HEADER_LEN..BINARY_PREFIX_LEN].copy_from_slice(&count.to_le_bytes());
        bytes
    }

    const FACET: &str =
        "facet normal 0 0 1 outer loop vertex 0 0 0 vertex 1 0 0 vertex 0 1 0 endloop endfacet\n";
    fn ascii(facets: &[&str]) -> Vec<u8> {
        format!("solid test\n{}endsolid test\n", facets.concat()).into_bytes()
    }

    #[test]
    fn before_growth_size_correct_binary_rejects_before_reservation() {
        let (_guard, profile) = ParseTestProfileGuard::install(1);
        assert_eq!(
            parse(&binary(2, false)),
            Err(StlError::TooManyTriangles { count: 2, limit: 1 })
        );
        let probes = profile.probes.borrow();
        assert_eq!(probes.binary_reserves, 0);
        assert_eq!(probes.binary_float_reads, 0);
    }

    #[test]
    fn before_growth_size_correct_solid_binary_still_uses_binary_limit() {
        let (_guard, profile) = ParseTestProfileGuard::install(1);
        assert_eq!(
            parse(&binary(2, true)),
            Err(StlError::TooManyTriangles { count: 2, limit: 1 })
        );
        let probes = profile.probes.borrow();
        assert_eq!(probes.binary_reserves, 0);
        assert_eq!(probes.binary_float_reads, 0);
    }

    #[test]
    fn before_growth_ascii_rejects_facet_without_appending_extra_values() {
        let (_guard, profile) = ParseTestProfileGuard::install(1);
        assert_eq!(
            parse(&ascii(&[FACET, FACET])),
            Err(StlError::TooManyTriangles { count: 2, limit: 1 })
        );
        assert_eq!(profile.probes.borrow().ascii_refusal_lengths, Some((9, 3)));
    }

    #[test]
    fn before_growth_profile_restores_nested_scope_and_panic() {
        assert_eq!(triangle_limit(), MAX_TRIANGLES);
        let (outer, outer_profile) = ParseTestProfileGuard::install(3);
        assert_eq!(triangle_limit(), 3);
        {
            let (_inner, _) = ParseTestProfileGuard::install(1);
            assert_eq!(triangle_limit(), 1);
        }
        assert!(std::rc::Rc::ptr_eq(
            &test_profile().unwrap(),
            &outer_profile
        ));
        let caught = std::panic::catch_unwind(|| {
            let (_inner, _) = ParseTestProfileGuard::install(2);
            assert_eq!(triangle_limit(), 2);
            panic!("synthetic scoped profile unwind");
        });
        assert!(caught.is_err());
        assert!(std::rc::Rc::ptr_eq(
            &test_profile().unwrap(),
            &outer_profile
        ));
        drop(outer);
        assert_eq!(triangle_limit(), MAX_TRIANGLES);
        assert!(test_profile().is_none());
    }

    #[test]
    fn before_growth_controls_keep_discriminator_and_exact_limit_behavior() {
        let (_guard, _) = ParseTestProfileGuard::install(1);
        let parsed = parse(&binary(1, true)).unwrap();
        assert_eq!(parsed.format, StlFormat::Binary);
        assert_eq!(parsed.triangle_count, 1);
        let source = ascii(&[FACET]);
        assert!(source.len() > BINARY_PREFIX_LEN);
        let parsed = parse(&source).unwrap();
        assert_eq!(parsed.format, StlFormat::Ascii);
        assert_eq!(parsed.triangle_count, 1);
        let mut truncated = binary(2, false);
        truncated.truncate(BINARY_PREFIX_LEN);
        assert_eq!(
            parse(&truncated),
            Err(StlError::TooManyTriangles { count: 2, limit: 1 })
        );
        let mut truncated = binary(1, false);
        truncated.pop();
        assert_eq!(
            parse(&truncated),
            Err(StlError::TruncatedFile {
                expected: 134,
                actual: 133
            })
        );
    }

    #[test]
    fn before_growth_over_cap_ascii_preserves_malformed_and_nonfinite_priority() {
        let (_guard, _) = ParseTestProfileGuard::install(1);
        let malformed = FACET.replace("endfacet", "wrong");
        assert_eq!(
            parse(&ascii(&[FACET, &malformed])),
            Err(StlError::MalformedHeader)
        );
        for value in ["NaN", "inf", "-inf"] {
            let nonfinite = FACET.replacen("vertex 0 0 0", &format!("vertex {value} 0 0"), 1);
            assert_eq!(
                parse(&ascii(&[FACET, &nonfinite])),
                Err(StlError::NonFiniteCoordinate { triangle_index: 1 })
            );
            let mixed = nonfinite.replace("endfacet", "wrong");
            assert_eq!(
                parse(&ascii(&[FACET, &mixed])),
                Err(StlError::NonFiniteCoordinate { triangle_index: 1 })
            );
        }
    }

    #[test]
    fn before_growth_nonfinite_normals_remain_recorded_facts() {
        let (_guard, _) = ParseTestProfileGuard::install(1);
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut source = binary(1, false);
            source[BINARY_PREFIX_LEN..BINARY_PREFIX_LEN + 4].copy_from_slice(&value.to_le_bytes());
            let parsed = parse(&source).unwrap();
            assert!(if value.is_nan() {
                parsed.stored_normals[0].is_nan()
            } else {
                parsed.stored_normals[0] == value
            });
        }
        for value in ["NaN", "inf", "-inf"] {
            let facet = FACET.replacen("normal 0 0 1", &format!("normal {value} 0 1"), 1);
            let normal = parse(&ascii(&[&facet])).unwrap().stored_normals[0];
            assert!(match value {
                "NaN" => normal.is_nan(),
                "inf" => normal == f32::INFINITY,
                "-inf" => normal == f32::NEG_INFINITY,
                _ => unreachable!(),
            });
        }
    }
    #[test]
    fn before_growth_first_and_second_reserve_faults_do_not_append() {
        for source in [binary(1, false), ascii(&[FACET])] {
            for fail_at in [1, 2] {
                let (_guard, profile) = ParseTestProfileGuard::install(1);
                profile.probes.borrow_mut().fail_reserve_at = Some(fail_at);
                assert_eq!(parse(&source), Err(StlError::AllocationFailed));
                let probes = profile.probes.borrow();
                assert_eq!(probes.reserve_calls, fail_at);
                assert_eq!(probes.failure_lengths, Some((0, 0)));
                assert_eq!(probes.binary_float_reads, 0);
            }
        }
    }

    #[test]
    fn before_growth_later_ascii_reserve_fault_preserves_admitted_facets_only() {
        for fail_at in [3, 4] {
            let (_guard, profile) = ParseTestProfileGuard::install(2);
            profile.probes.borrow_mut().fail_reserve_at = Some(fail_at);
            assert_eq!(
                parse(&ascii(&[FACET, FACET])),
                Err(StlError::AllocationFailed)
            );
            let probes = profile.probes.borrow();
            assert_eq!(probes.reserve_calls, fail_at);
            assert_eq!(probes.failure_lengths, Some((9, 3)));
        }
    }

    #[test]
    fn before_growth_model_dispatch_latches_same_observer_allocation_and_spends_no_zip_bytes() {
        use crate::package_bounds::{PackageProcessingContext, PackageUsage};
        use crate::zip_bounds::ZipOpenError;
        for source in [binary(1, false), ascii(&[FACET])] {
            for fail_at in [1, 2] {
                let (_guard, profile) = ParseTestProfileGuard::install(1);
                profile.probes.borrow_mut().fail_reserve_at = Some(fail_at);
                let observer = ZipResourceObserver::new();
                let mut context = PackageProcessingContext::new(observer.clone());
                assert_eq!(
                    crate::parse_model_with_plates_in_context(&source, &mut context),
                    Err(StlError::AllocationFailed)
                );
                assert!(matches!(
                    observer.check(),
                    Err(ZipOpenError::AllocationFailed)
                ));
                assert_eq!(
                    context.usage(),
                    PackageUsage {
                        inflation_bytes: 0,
                        retained_input_bytes: 0
                    }
                );
                assert_eq!(profile.probes.borrow().reserve_calls, fail_at);
                assert_eq!(
                    crate::parse_model_with_plates_in_context(&[], &mut context),
                    Err(StlError::AllocationFailed)
                );
                assert_eq!(profile.probes.borrow().reserve_calls, fail_at);
            }
        }
    }

    #[test]
    fn before_growth_prior_terminal_cause_prevents_discriminator_and_reservation() {
        use crate::package_bounds::PackageProcessingContext;
        for reason in [
            Some(ResourceStop::InputLimitExceeded),
            Some(ResourceStop::ReadWorkExceeded),
            Some(ResourceStop::EnvelopeChanged),
            None,
        ] {
            let (_guard, profile) = ParseTestProfileGuard::install(1);
            let observer = ZipResourceObserver::new();
            let expected = if let Some(stop) = reason {
                let _ = observer.stop_io(stop);
                StlError::ResourceStopped(stop)
            } else {
                let _ = observer.allocation_failed_io();
                StlError::AllocationFailed
            };
            let mut context = PackageProcessingContext::new(observer.clone());
            assert_eq!(
                crate::parse_model_with_plates_in_context(&binary(1, false), &mut context),
                Err(expected)
            );
            let probes = profile.probes.borrow();
            assert_eq!(probes.discriminator_entries, 0);
            assert_eq!(probes.reserve_calls, 0);
            assert_eq!(probes.binary_float_reads, 0);
        }
    }

    #[test]
    fn before_growth_legacy_count_error_is_not_replaced_by_a_resource_stop() {
        let (_guard, profile) = ParseTestProfileGuard::install(1);
        for source in [binary(2, false), ascii(&[FACET, FACET])] {
            let observer = ZipResourceObserver::new();
            assert_eq!(
                parse_observed(&source, &observer),
                Err(StlError::TooManyTriangles { count: 2, limit: 1 })
            );
            assert!(observer.check().is_ok());
        }
        assert_eq!(profile.probes.borrow().binary_reserves, 0);
    }

    #[test]
    fn before_growth_successful_model_stl_dispatch_spends_no_package_usage() {
        use crate::package_bounds::{PackageProcessingContext, PackageUsage};
        let (_guard, _) = ParseTestProfileGuard::install(1);
        for source in [binary(1, false), ascii(&[FACET])] {
            let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
            let parsed = crate::parse_model_with_plates_in_context(&source, &mut context).unwrap();
            assert_eq!(parsed.model.triangle_count, 1);
            assert_eq!(
                context.usage(),
                PackageUsage {
                    inflation_bytes: 0,
                    retained_input_bytes: 0
                }
            );
        }
    }

    #[test]
    fn before_growth_checked_sizing_stops_before_reservation() {
        let (_guard, profile) = ParseTestProfileGuard::install(1);
        let observer = ZipResourceObserver::new();
        let control = ParseControl::new(&observer);
        assert_eq!(
            control.elements(usize::MAX, 9),
            Err(StlError::ResourceStopped(ResourceStop::InputLimitExceeded))
        );
        assert_eq!(profile.probes.borrow().reserve_calls, 0);
        assert!(observer.check().is_err());
    }
    #[test]
    fn before_growth_stop_after_first_reserve_prevents_second_reserve_and_append() {
        use crate::package_bounds::PackageProcessingContext;
        for source in [binary(1, false), ascii(&[FACET])] {
            for injected in [
                ParseTestStop::Resource(ResourceStop::InputLimitExceeded),
                ParseTestStop::Resource(ResourceStop::ReadWorkExceeded),
                ParseTestStop::Resource(ResourceStop::EnvelopeChanged),
                ParseTestStop::Allocation,
            ] {
                let (_guard, profile) = ParseTestProfileGuard::install(1);
                profile.probes.borrow_mut().stop_after_first_success = Some(injected);
                let observer = ZipResourceObserver::new();
                let mut context = PackageProcessingContext::new(observer.clone());
                let expected = match injected {
                    ParseTestStop::Resource(stop) => StlError::ResourceStopped(stop),
                    ParseTestStop::Allocation => StlError::AllocationFailed,
                };
                assert_eq!(
                    crate::parse_model_with_plates_in_context(&source, &mut context),
                    Err(expected)
                );
                let actual = StlError::from(observer.check().unwrap_err());
                assert_eq!(
                    actual,
                    match injected {
                        ParseTestStop::Resource(stop) => StlError::ResourceStopped(stop),
                        ParseTestStop::Allocation => StlError::AllocationFailed,
                    }
                );
                let probes = profile.probes.borrow();
                assert_eq!(probes.reserve_calls, 1);
                assert_eq!(probes.binary_float_reads, 0);
                assert_eq!(probes.failure_lengths, Some((0, 0)));
            }
        }
    }
}
