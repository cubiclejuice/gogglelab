use crate::error::StlError;
use crate::limits::MAX_TRIANGLES;

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

/// Parse STL bytes already read into memory. The `MAX_FILE_BYTES` cap (spec §3)
/// is enforced by the caller via `fs::metadata` before the read that produces
/// `bytes` — see module docs in `limits.rs`.
pub fn parse(bytes: &[u8]) -> Result<ParsedStl, StlError> {
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
            return parse_binary(bytes, count);
        }
        if looks_like_ascii(bytes) {
            return parse_ascii(bytes);
        }
        // Binary-shaped (doesn't look like ASCII) but the size doesn't match.
        // A declared count this large can't be a mere truncation — reject it
        // for what it is (R-007) rather than reporting a truncation the
        // reader could never sanely act on.
        if count > MAX_TRIANGLES {
            return Err(StlError::TooManyTriangles {
                count,
                limit: MAX_TRIANGLES,
            });
        }
        return Err(StlError::TruncatedFile {
            expected,
            actual: bytes.len() as u64,
        });
    }

    if looks_like_ascii(bytes) {
        return parse_ascii(bytes);
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

fn read_f32_le(bytes: &[u8], offset: usize) -> Result<f32, StlError> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or(StlError::MalformedHeader)?;
    let arr: [u8; 4] = slice.try_into().map_err(|_| StlError::MalformedHeader)?;
    Ok(f32::from_le_bytes(arr))
}

fn parse_binary(bytes: &[u8], count: u32) -> Result<ParsedStl, StlError> {
    // count > MAX_TRIANGLES is already rejected by the caller before the size
    // check that leads here; parse_binary can assume it holds.
    let mut positions = Vec::with_capacity(count as usize * 9);
    let mut stored_normals = Vec::with_capacity(count as usize * 3);

    for i in 0..count {
        let record_start = BINARY_PREFIX_LEN + i as usize * BINARY_RECORD_LEN as usize;

        for c in 0..3 {
            let v = read_f32_le(bytes, record_start + c * 4)?;
            stored_normals.push(v);
        }

        let vertex_start = record_start + 12;
        for v in 0..3 {
            for c in 0..3 {
                let value = read_f32_le(bytes, vertex_start + v * 12 + c * 4)?;
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
fn parse_ascii(bytes: &[u8]) -> Result<ParsedStl, StlError> {
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
                for _ in 0..3 {
                    let v: f32 = next_float(&mut tokens)?;
                    stored_normals.push(v);
                }
                expect_keyword(&mut tokens, "outer")?;
                expect_keyword(&mut tokens, "loop")?;
                for _ in 0..3 {
                    expect_keyword(&mut tokens, "vertex")?;
                    for _ in 0..3 {
                        let v: f32 = next_float(&mut tokens)?;
                        if !v.is_finite() {
                            return Err(StlError::NonFiniteCoordinate { triangle_index });
                        }
                        positions.push(v);
                    }
                }
                expect_keyword(&mut tokens, "endloop")?;
                expect_keyword(&mut tokens, "endfacet")?;

                if triangle_index == MAX_TRIANGLES {
                    return Err(StlError::TooManyTriangles {
                        count: triangle_index + 1,
                        limit: MAX_TRIANGLES,
                    });
                }
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
