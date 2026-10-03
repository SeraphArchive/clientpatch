//! IL2CPP `global-metadata.dat` header recognition — pure logic, no Win32.
//!
//! The runtime dump locates the decrypted metadata blob by scanning process
//! memory for the header magic, then validating structurally before trusting a
//! size. A 4-byte magic alone is not enough (it can appear in random data), so
//! acceptance requires the whole table layout to be self-consistent:
//!
//!   * `sanity == 0xFAB11BAF`, `version` in a known IL2CPP range
//!   * table 0 starts at a plausible header size (0xF0..=0x400, 8-aligned)
//!   * every table's offset lies past the header bytes consumed so far
//!   * offsets are non-decreasing (tables are laid out in declaration order;
//!     empty tables reuse the previous end offset, so equality is allowed)
//!   * at least 20 sane consecutive tables (real v29+ headers have ~31)
//!   * total size in a plausible range for a full game metadata (1 MiB..512 MiB)
//!
//! Verified against HBR's real v31 header (Unity 6000.0.x): 31 tables, header
//! size 0x100, last table end == file size exactly.
#![forbid(unsafe_code)]

/// `Il2CppGlobalMetadataHeader.sanity` (LE bytes `AF 1B B1 FA`).
pub const MAGIC: u32 = 0xFAB1_1BAF;
pub const MIN_VERSION: u32 = 24;
pub const MAX_VERSION: u32 = 31;

/// Header byte-offsets of the (offset, size) pairs the handle restore needs.
/// (Pair 19 = typeDefinitions, pair 12 = genericParameters.)
pub const HDR_TYPE_DEFINITIONS: usize = 0xA0;
pub const HDR_GENERIC_PARAMETERS: usize = 0x68;

/// Read one (offset, size) table pair from the metadata header.
pub fn table_range(buf: &[u8], hdr_off: usize) -> Option<(u64, u64)> {
    let off = u32le(buf, hdr_off)? as u64;
    let size = u32le(buf, hdr_off + 4)? as u64;
    Some((off, size))
}

const MIN_TABLES: usize = 20;
const MAX_TABLES: usize = 64;
const MIN_TOTAL_SIZE: u64 = 1024 * 1024;
const MAX_TOTAL_SIZE: u64 = 512 * 1024 * 1024;
/// Header-size plausibility window for table 0's offset.
const MIN_HEADER_SIZE: u32 = 0xF0;
const MAX_HEADER_SIZE: u32 = 0x400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetadataHeader {
    pub version: u32,
    /// Bytes from blob start to the end of the last table == the dump length.
    pub total_size: u64,
    pub table_count: usize,
}

fn u32le(buf: &[u8], at: usize) -> Option<u32> {
    buf.get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Validate a candidate metadata header at the start of `buf` (needs only the
/// first ~600 bytes; extra bytes are ignored). Returns the header on success.
pub fn parse(buf: &[u8]) -> Option<MetadataHeader> {
    if u32le(buf, 0)? != MAGIC {
        return None;
    }
    let version = u32le(buf, 4)?;
    if !(MIN_VERSION..=MAX_VERSION).contains(&version) {
        return None;
    }

    let mut prev_off = 0u32;
    let mut total: u64 = 0;
    let mut count = 0usize;
    for i in 0..MAX_TABLES {
        let at = 8 + i * 8;
        let (Some(off), Some(size)) = (u32le(buf, at), u32le(buf, at + 4)) else {
            break;
        };
        // Terminator: real headers are followed by non-table data (we observed
        // (0,0) then garbage past the last v31 table).
        if off == 0 && size == 0 {
            break;
        }
        // Table offsets must lie at/past the header consumed through this pair.
        let header_so_far = 8 + (i as u32 + 1) * 8;
        if off < header_so_far || off < prev_off {
            break;
        }
        if i == 0 && (!(MIN_HEADER_SIZE..=MAX_HEADER_SIZE).contains(&off) || off % 8 != 0) {
            return None;
        }
        let end = off as u64 + size as u64;
        if end > MAX_TOTAL_SIZE {
            break;
        }
        total = total.max(end);
        prev_off = off;
        count += 1;
    }
    if count < MIN_TABLES {
        return None;
    }
    if !(MIN_TOTAL_SIZE..=MAX_TOTAL_SIZE).contains(&total) {
        return None;
    }
    Some(MetadataHeader {
        version,
        total_size: total,
        table_count: count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic v31-style header: 31 tables, header size 0x100,
    /// strictly increasing offsets, one empty table mid-way.
    fn synthetic(npairs: usize) -> Vec<u8> {
        let mut buf = vec![0u8; 0x600];
        buf[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        buf[4..8].copy_from_slice(&31u32.to_le_bytes());
        let mut off = 0x100u32;
        for i in 0..npairs {
            // ~4 MiB total so the header clears the 1 MiB plausibility floor.
            let size = if i == 7 { 0 } else { 0x20000 + i as u32 * 0x101 };
            buf[8 + i * 8..12 + i * 8].copy_from_slice(&off.to_le_bytes());
            buf[12 + i * 8..16 + i * 8].copy_from_slice(&size.to_le_bytes());
            off += size;
        }
        // Terminator after the tables.
        buf[8 + npairs * 8..12 + npairs * 8].copy_from_slice(&0u32.to_le_bytes());
        buf[12 + npairs * 8..16 + npairs * 8].copy_from_slice(&0u32.to_le_bytes());
        buf
    }

    #[test]
    fn accepts_realistic_v31_header() {
        let h = parse(&synthetic(31)).expect("31-table header must validate");
        assert_eq!(h.version, 31);
        assert_eq!(h.table_count, 31);
        assert!(h.total_size >= MIN_TOTAL_SIZE);
    }

    #[test]
    fn total_size_is_last_table_end() {
        let buf = synthetic(31);
        let h = parse(&buf).unwrap();
        // Last table: index 30.
        let last_off = u32le(&buf, 8 + 30 * 8).unwrap() as u64;
        let last_sz = u32le(&buf, 12 + 30 * 8).unwrap() as u64;
        assert_eq!(h.total_size, last_off + last_sz);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut buf = synthetic(31);
        buf[0] ^= 0xFF;
        assert!(parse(&buf).is_none());
    }

    #[test]
    fn rejects_unknown_version() {
        let mut buf = synthetic(31);
        buf[4..8].copy_from_slice(&32u32.to_le_bytes());
        assert!(parse(&buf).is_none());
    }

    #[test]
    fn rejects_unaligned_first_table() {
        let mut buf = synthetic(31);
        buf[8..12].copy_from_slice(&0x104u32.to_le_bytes());
        assert!(parse(&buf).is_none());
    }

    #[test]
    fn rejects_too_few_tables() {
        // 10 tables then terminator: below MIN_TABLES.
        assert!(parse(&synthetic(10)).is_none());
    }

    #[test]
    fn rejects_decreasing_offsets() {
        let mut buf = synthetic(31);
        // Table 5 jumps back before table 4.
        buf[8 + 5 * 8..12 + 5 * 8].copy_from_slice(&0x200u32.to_le_bytes());
        assert!(parse(&buf).is_none());
    }

    #[test]
    fn rejects_tiny_total() {
        // All sizes 8 bytes: total stays under 1 MiB.
        let mut buf = synthetic(31);
        let mut off = 0x100u32;
        for i in 0..31 {
            buf[8 + i * 8..12 + i * 8].copy_from_slice(&off.to_le_bytes());
            buf[12 + i * 8..16 + i * 8].copy_from_slice(&8u32.to_le_bytes());
            off += 8;
        }
        assert!(parse(&buf).is_none());
    }

    #[test]
    fn empty_table_with_reused_offset_is_ok() {
        // synthetic() already puts a zero-size table at index 7 reusing the
        // previous end offset — acceptance above proves the rule.
        let h = parse(&synthetic(31)).unwrap();
        assert_eq!(h.table_count, 31);
    }

    #[test]
    fn short_buffer_is_rejected() {
        assert!(parse(&[0xAF, 0x1B, 0xB1, 0xFA]).is_none());
    }

    #[test]
    fn table_range_reads_pairs() {
        let buf = synthetic(31);
        let (off, size) = table_range(&buf, HDR_TYPE_DEFINITIONS).unwrap();
        assert_eq!(off, u32le(&buf, HDR_TYPE_DEFINITIONS).unwrap() as u64);
        assert_eq!(size, u32le(&buf, HDR_TYPE_DEFINITIONS + 4).unwrap() as u64);
        assert!(table_range(&buf[..4], HDR_TYPE_DEFINITIONS).is_none());
    }
}
