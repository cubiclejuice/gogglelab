//! Eager header and compressed-range admission for the opt-in constructor only.
use super::CentralDirectoryInfo;
use super::metadata_limits::ConstructorBudget;
use crate::result::{ZipResult, invalid};
use crate::spec::{FixedSizeBlock, ZipLocalEntryBlock};
use crate::types::ZipFileData;
use std::io::{Read, Seek, SeekFrom};

// Consume bounded metadata without allocating from any source-provided length.
fn discard(reader: &mut impl Read, mut length: usize) -> ZipResult<()> {
    let mut scratch = [0u8; 512];
    while length != 0 {
        let count = length.min(scratch.len());
        reader.read_exact(&mut scratch[..count])?;
        length -= count;
    }
    Ok(())
}

fn size_matches(local: Option<u64>, central: u64, descriptor: bool) -> bool {
    match local {
        Some(value) if descriptor && (value == 0 || value == u64::MAX) => true,
        Some(value) => value == central,
        None => descriptor,
    }
}

pub(super) fn admit_local_header<R: Read + Seek>(
    reader: &mut R,
    file: &ZipFileData,
    wire_name: &[u8],
    wire_method: u16,
    directory: &CentralDirectoryInfo,
    budget: &ConstructorBudget,
) -> ZipResult<()> {
    let fixed_end = file
        .header_start
        .checked_add(30)
        .filter(|end| {
            file.header_start >= directory.archive_offset && *end <= directory.directory_start
        })
        .ok_or_else(|| invalid!("Local header range is invalid"))?;
    reader.seek(SeekFrom::Start(file.header_start))?;
    budget.encoded(30)?;
    let block = ZipLocalEntryBlock::parse(reader)?;
    let data_start = fixed_end
        .checked_add(u64::from(block.file_name_length))
        .and_then(|end| end.checked_add(u64::from(block.extra_field_length)))
        .filter(|end| *end <= directory.directory_start)
        .ok_or_else(|| invalid!("Local variable fields exceed their bounded range"))?;
    data_start
        .checked_add(file.compressed_size)
        .filter(|end| *end <= directory.directory_start)
        .ok_or_else(|| invalid!("Compressed member range is invalid"))?;
    if block.flags != file.flags || block.compression_method != wire_method {
        return Err(invalid!(
            "Local flags or compression method disagree with central metadata"
        ));
    }
    if usize::from(block.file_name_length) != wire_name.len() {
        return Err(invalid!("Local filename disagrees with central wire name"));
    }
    budget.encoded(u64::from(block.file_name_length) + u64::from(block.extra_field_length))?;
    let mut scratch = [0u8; 512];
    for chunk in wire_name.chunks(scratch.len()) {
        reader.read_exact(&mut scratch[..chunk.len()])?;
        if chunk != &scratch[..chunk.len()] {
            return Err(invalid!("Local filename disagrees with central wire name"));
        }
    }

    let mut uncompressed =
        (block.uncompressed_size != u32::MAX).then_some(u64::from(block.uncompressed_size));
    let mut compressed =
        (block.compressed_size != u32::MAX).then_some(u64::from(block.compressed_size));
    let mut remaining = usize::from(block.extra_field_length);
    let mut zip64_seen = false;
    while remaining >= 4 {
        let mut header = [0u8; 4];
        reader.read_exact(&mut header)?;
        remaining -= 4;
        let tag = u16::from_le_bytes([header[0], header[1]]);
        let length = usize::from(u16::from_le_bytes([header[2], header[3]]));
        if length > remaining {
            return Err(invalid!("Local extra field exceeds its bounded range"));
        }
        let mut unread = length;
        if tag == 1 {
            if zip64_seen {
                return Err(invalid!("Ambiguous local ZIP64 metadata"));
            }
            zip64_seen = true;
            // Only sentinel fields have ZIP64 replacements, in specification order.
            for value in [&mut uncompressed, &mut compressed] {
                if value.is_none() && unread >= 8 {
                    let mut bytes = [0u8; 8];
                    reader.read_exact(&mut bytes)?;
                    *value = Some(u64::from_le_bytes(bytes));
                    unread -= 8;
                } else if value.is_none() && unread != 0 {
                    return Err(invalid!("Local ZIP64 size is truncated"));
                }
            }
        }
        discard(reader, unread)?;
        remaining -= length;
    }
    // Retain upstream compatibility with padding shorter than a TLV header.
    discard(reader, remaining)?;
    let descriptor = file.using_data_descriptor;
    if (!descriptor || block.crc32 != 0) && block.crc32 != file.crc32 {
        return Err(invalid!("Local CRC disagrees with central metadata"));
    }
    if !size_matches(uncompressed, file.uncompressed_size, descriptor)
        || !size_matches(compressed, file.compressed_size, descriptor)
    {
        return Err(invalid!("Local sizes disagree with central metadata"));
    }
    file.data_start
        .set(data_start)
        .map_err(|_| invalid!("Local data start was already admitted"))?;
    Ok(())
}
