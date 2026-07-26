//! Minimal ZIP reader for downloaded bill archives.
//!
//! Alipay serves its daily bill as a ZIP of CSV members, so reconciliation
//! needs to open one. This is deliberately the smallest reader that can do
//! that job — central directory, local headers, stored and DEFLATE members —
//! rather than a general-purpose ZIP implementation: no encryption, no ZIP64,
//! no multi-disk archives, no directory extraction.
//!
//! Everything a bill archive can contain is attacker-adjacent (the bytes come
//! off the network), so every length is bounds-checked against the buffer and
//! the total inflated size is capped up front — a bill that claims to expand
//! to a terabyte is refused rather than attempted.

use std::io::Read as _;

use crate::PayError;

/// Central directory file header signature.
const CENTRAL_HEADER: [u8; 4] = [0x50, 0x4B, 0x01, 0x02];
/// Local file header signature.
const LOCAL_HEADER: [u8; 4] = [0x50, 0x4B, 0x03, 0x04];
/// End-of-central-directory signature.
const END_OF_DIRECTORY: [u8; 4] = [0x50, 0x4B, 0x05, 0x06];
/// Fixed size of the end-of-central-directory record, comment excluded.
const END_OF_DIRECTORY_LEN: usize = 22;
/// Fixed size of a central directory file header, name/extra/comment excluded.
const CENTRAL_HEADER_LEN: usize = 46;
/// Fixed size of a local file header, name/extra excluded.
const LOCAL_HEADER_LEN: usize = 30;

/// Compression method: the member is stored verbatim.
const METHOD_STORED: u16 = 0;
/// Compression method: the member is DEFLATE-compressed.
const METHOD_DEFLATE: u16 = 8;

/// One member of an archive.
#[derive(Debug)]
pub(crate) struct ZipEntry {
    /// Member name as stored. Bill archives name members in the same legacy
    /// encoding as their contents, so this is decoded leniently and is only
    /// ever used for diagnostics — never to choose a member.
    pub name: String,
    /// Inflated bytes.
    pub data: Vec<u8>,
}

/// Read every member of `archive`, refusing to inflate more than `max_total`
/// bytes in total.
///
/// # Errors
///
/// Returns [`PayError::Reconcile`] for a truncated or malformed archive, an
/// unsupported compression method, or an archive that would exceed
/// `max_total`.
pub(crate) fn read_entries(archive: &[u8], max_total: usize) -> Result<Vec<ZipEntry>, PayError> {
    let end = find_end_of_directory(archive)?;
    let entry_count = usize::from(read_u16(archive, end + 10)?);
    let directory_offset = read_u32(archive, end + 16)? as usize;

    let mut entries = Vec::with_capacity(entry_count.min(64));
    let mut cursor = directory_offset;
    let mut inflated = 0_usize;
    for _ in 0..entry_count {
        if archive.get(cursor..cursor + 4) != Some(&CENTRAL_HEADER) {
            return Err(malformed("central directory header"));
        }
        let method = read_u16(archive, cursor + 10)?;
        let compressed_size = read_u32(archive, cursor + 20)? as usize;
        let uncompressed_size = read_u32(archive, cursor + 24)? as usize;
        let name_len = usize::from(read_u16(archive, cursor + 28)?);
        let extra_len = usize::from(read_u16(archive, cursor + 30)?);
        let comment_len = usize::from(read_u16(archive, cursor + 32)?);
        let local_offset = read_u32(archive, cursor + 42)? as usize;
        let name = archive
            .get(cursor + CENTRAL_HEADER_LEN..cursor + CENTRAL_HEADER_LEN + name_len)
            .ok_or_else(|| malformed("member name"))?;

        inflated = inflated
            .checked_add(uncompressed_size)
            .ok_or_else(|| malformed("member size overflow"))?;
        if inflated > max_total {
            return Err(PayError::Reconcile(format!(
                "bill archive expands to more than {max_total} bytes"
            )));
        }

        let data = read_member(
            archive,
            local_offset,
            method,
            compressed_size,
            uncompressed_size,
        )?;
        entries.push(ZipEntry {
            name: String::from_utf8_lossy(name).into_owned(),
            data,
        });
        cursor = cursor + CENTRAL_HEADER_LEN + name_len + extra_len + comment_len;
    }
    Ok(entries)
}

/// Read and decompress one member, starting from its local file header.
fn read_member(
    archive: &[u8],
    local_offset: usize,
    method: u16,
    compressed_size: usize,
    uncompressed_size: usize,
) -> Result<Vec<u8>, PayError> {
    if archive.get(local_offset..local_offset + 4) != Some(&LOCAL_HEADER) {
        return Err(malformed("local file header"));
    }
    // The local header repeats the name and extra lengths, and they may differ
    // from the central directory's, so the data offset is taken from here.
    let name_len = usize::from(read_u16(archive, local_offset + 26)?);
    let extra_len = usize::from(read_u16(archive, local_offset + 28)?);
    let start = local_offset + LOCAL_HEADER_LEN + name_len + extra_len;
    let compressed = archive
        .get(start..start + compressed_size)
        .ok_or_else(|| malformed("member data"))?;

    match method {
        METHOD_STORED => {
            if compressed.len() != uncompressed_size {
                return Err(malformed("stored member size mismatch"));
            }
            Ok(compressed.to_vec())
        }
        METHOD_DEFLATE => {
            let mut data = Vec::with_capacity(uncompressed_size);
            flate2::read::DeflateDecoder::new(compressed)
                // Bound the reader too: the declared size is attacker-supplied
                // and a stream can claim less than it produces.
                .take(uncompressed_size as u64)
                .read_to_end(&mut data)
                .map_err(|error| PayError::Reconcile(format!("inflate bill member: {error}")))?;
            Ok(data)
        }
        other => Err(PayError::Reconcile(format!(
            "unsupported bill archive compression method {other}"
        ))),
    }
}

/// Locate the end-of-central-directory record, scanning back over a trailing
/// comment.
fn find_end_of_directory(archive: &[u8]) -> Result<usize, PayError> {
    if archive.len() < END_OF_DIRECTORY_LEN {
        return Err(malformed("archive is too small"));
    }
    let earliest = archive.len().saturating_sub(END_OF_DIRECTORY_LEN + 0xFFFF);
    for candidate in (earliest..=archive.len() - END_OF_DIRECTORY_LEN).rev() {
        if archive[candidate..candidate + 4] == END_OF_DIRECTORY {
            return Ok(candidate);
        }
    }
    Err(malformed("no end-of-central-directory record"))
}

fn read_u16(bytes: &[u8], at: usize) -> Result<u16, PayError> {
    bytes
        .get(at..at + 2)
        .and_then(|slice| slice.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or_else(|| malformed("truncated field"))
}

fn read_u32(bytes: &[u8], at: usize) -> Result<u32, PayError> {
    bytes
        .get(at..at + 4)
        .and_then(|slice| slice.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| malformed("truncated field"))
}

fn malformed(what: &str) -> PayError {
    PayError::Reconcile(format!("malformed bill archive: {what}"))
}

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    reason = "fixture sizes are a handful of bytes"
)]
mod tests {
    use std::io::Write as _;

    use super::*;

    /// Build a real archive with the given members, so the reader is tested
    /// against bytes rather than against its own assumptions.
    fn archive(members: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut directory = Vec::new();
        for (name, data, deflate) in members {
            let local_offset = out.len() as u32;
            let payload = if *deflate {
                let mut encoder =
                    flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(data).unwrap();
                encoder.finish().unwrap()
            } else {
                (*data).to_vec()
            };
            let method: u16 = if *deflate {
                METHOD_DEFLATE
            } else {
                METHOD_STORED
            };

            out.extend_from_slice(&LOCAL_HEADER);
            out.extend_from_slice(&[20, 0, 0, 0]); // version, flags
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0; 8]); // time, date, crc placeholder
            out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0_u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&payload);

            directory.extend_from_slice(&CENTRAL_HEADER);
            directory.extend_from_slice(&[20, 0, 20, 0, 0, 0]); // versions, flags
            directory.extend_from_slice(&method.to_le_bytes());
            directory.extend_from_slice(&[0; 8]); // time, date, crc placeholder
            directory.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            directory.extend_from_slice(&(data.len() as u32).to_le_bytes());
            directory.extend_from_slice(&(name.len() as u16).to_le_bytes());
            directory.extend_from_slice(&[0; 8]); // extra, comment, disk, attrs
            directory.extend_from_slice(&[0; 4]); // external attributes
            directory.extend_from_slice(&local_offset.to_le_bytes());
            directory.extend_from_slice(name.as_bytes());
        }

        let directory_offset = out.len() as u32;
        let directory_len = directory.len() as u32;
        out.extend_from_slice(&directory);
        out.extend_from_slice(&END_OF_DIRECTORY);
        out.extend_from_slice(&[0; 4]); // disk numbers
        out.extend_from_slice(&(members.len() as u16).to_le_bytes());
        out.extend_from_slice(&(members.len() as u16).to_le_bytes());
        out.extend_from_slice(&directory_len.to_le_bytes());
        out.extend_from_slice(&directory_offset.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes()); // comment length
        out
    }

    #[test]
    fn reads_stored_and_deflated_members() {
        let long = "order,amount\n".repeat(200);
        let bytes = archive(&[
            ("summary.csv", b"a,b\n1,2\n", false),
            ("detail.csv", long.as_bytes(), true),
        ]);
        let entries = read_entries(&bytes, 1 << 20).expect("read");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "summary.csv");
        assert_eq!(entries[0].data, b"a,b\n1,2\n");
        assert_eq!(entries[1].name, "detail.csv");
        assert_eq!(entries[1].data, long.as_bytes());
    }

    #[test]
    fn refuses_an_archive_that_would_expand_past_the_cap() {
        let big = "x".repeat(4096);
        let bytes = archive(&[("big.csv", big.as_bytes(), true)]);
        // Compresses to almost nothing, but declares 4 KiB uncompressed.
        assert!(bytes.len() < 1024, "the archive itself is small");
        let error = read_entries(&bytes, 1024).expect_err("cap enforced");
        assert!(
            matches!(&error, PayError::Reconcile(message) if message.contains("expands")),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn rejects_malformed_and_unsupported_archives() {
        assert!(read_entries(b"", 1 << 20).is_err());
        assert!(read_entries(b"not a zip at all, really", 1 << 20).is_err());

        let mut truncated = archive(&[("a.csv", b"data", false)]);
        truncated.truncate(truncated.len() / 2);
        assert!(read_entries(&truncated, 1 << 20).is_err());

        // An unknown compression method must fail rather than yield garbage.
        let mut unsupported = archive(&[("a.csv", b"data", false)]);
        let end = find_end_of_directory(&unsupported).unwrap();
        let directory = read_u32(&unsupported, end + 16).unwrap() as usize;
        unsupported[directory + 10] = 99;
        let error = read_entries(&unsupported, 1 << 20).expect_err("unsupported method");
        assert!(
            matches!(&error, PayError::Reconcile(message) if message.contains("compression")),
            "unexpected error: {error:?}"
        );
    }
}
