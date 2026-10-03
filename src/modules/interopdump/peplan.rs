//! PE64 carve planning — pure logic, no Win32.
//!
//! Turning the in-memory `GameAssembly.dll` image into a file Cpp2IL/LibCpp2IL
//! can load: the existing (externally converted, Cpp2IL-verified) dump uses the
//! "raw == virtual" convention, which we replicate exactly:
//!
//!   * `FileAlignment`   = `SectionAlignment` (0x1000 for this build)
//!   * `SizeOfHeaders`   = header end aligned up to `FileAlignment`
//!   * per section: `PointerToRawData = VirtualAddress`,
//!     `SizeOfRawData = VirtualSize = align(VirtualSize, FileAlignment)`
//!   * file length       = max(end of last section, align(SizeOfImage))
//!
//! The carve itself (reading process memory, zero-filling unreadable gaps) is
//! the Windows half in `mod.rs`; this module only parses headers and computes
//! the layout so it stays unit-testable off-Windows.
#![forbid(unsafe_code)]

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionPlan {
    pub name: [u8; 8],
    /// RVA in the loaded image; also the file offset in the output.
    pub va: u32,
    /// Aligned virtual size; bytes to emit for this section.
    pub size: u32,
    /// Offset of this section's 40-byte header inside the PE headers (for rewrite).
    pub hdr_off: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarvePlan {
    pub file_alignment: u32,
    pub size_of_headers: u32,
    /// End of the section table inside the source headers — how many bytes of
    /// the original image to copy before rewriting.
    pub header_end: usize,
    pub file_size: u64,
    pub sections: Vec<SectionPlan>,
    /// Offset of the FileAlignment field (optional header + 36).
    pub file_align_field_off: usize,
    /// Offset of the SizeOfHeaders field (optional header + 60).
    pub size_of_headers_field_off: usize,
    /// Offset of the ImageBase field (optional header + 24, u64 in PE32+).
    pub image_base_field_off: usize,
}

fn u16le(buf: &[u8], at: usize) -> Option<u16> {
    buf.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}

fn u32le(buf: &[u8], at: usize) -> Option<u32> {
    buf.get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn align_up(v: u64, a: u32) -> u64 {
    let a = a as u64;
    v.div_ceil(a) * a
}

/// Parse PE64 headers (needs at least the section table; pass the first
/// `SizeOfHeaders` bytes of the image — 64 KiB is always enough).
pub fn plan(headers: &[u8]) -> Option<CarvePlan> {
    if headers.len() < 0x40 || u16le(headers, 0)? != 0x5A4D {
        return None; // "MZ"
    }
    let e_lfanew = u32le(headers, 0x3C)? as usize;
    if u32le(headers, e_lfanew)? != 0x0000_4550 {
        return None; // "PE\0\0"
    }
    let coff = e_lfanew + 4;
    if u16le(headers, coff)? != 0x8664 {
        return None; // AMD64 only
    }
    let nsec = u16le(headers, coff + 2)? as usize;
    if nsec == 0 || nsec > 96 {
        return None;
    }
    let szopt = u16le(headers, coff + 16)? as usize;
    let opt = coff + 20;
    if u16le(headers, opt)? != 0x020B {
        return None; // PE32+
    }
    let section_alignment = u32le(headers, opt + 32)?;
    let size_of_image = u32le(headers, opt + 56)?;
    // The dump convention: file alignment == section alignment. Guard against a
    // degenerate value; fall back to one page.
    let file_alignment = if section_alignment.is_power_of_two() && section_alignment >= 0x200 {
        section_alignment
    } else {
        0x1000
    };

    let sec_table = opt + szopt;
    let header_end = sec_table + nsec * 40;
    if header_end > headers.len() {
        return None;
    }
    let size_of_headers = align_up(header_end as u64, file_alignment) as u32;

    let mut sections = Vec::with_capacity(nsec);
    let mut file_size = align_up(size_of_image as u64, file_alignment);
    for i in 0..nsec {
        let at = sec_table + i * 40;
        let mut name = [0u8; 8];
        name.copy_from_slice(&headers[at..at + 8]);
        let vsz = u32le(headers, at + 8)?;
        let va = u32le(headers, at + 12)?;
        let size = align_up(vsz as u64, file_alignment) as u32;
        if va == 0 || va >= size_of_image {
            return None;
        }
        file_size = file_size.max(align_up(va as u64 + size as u64, file_alignment));
        sections.push(SectionPlan {
            name,
            va,
            size,
            hdr_off: at,
        });
    }
    Some(CarvePlan {
        file_alignment,
        size_of_headers,
        header_end,
        file_size,
        sections,
        file_align_field_off: opt + 36,
        size_of_headers_field_off: opt + 60,
        image_base_field_off: opt + 24,
    })
}

/// Rewrite a copy of the original headers into the raw==virtual dump form.
/// `out` must be at least `plan.size_of_headers` bytes (zero-padded tail).
///
/// `image_base` must be the ACTUAL load address of the image, not the PE's
/// preferred base: every pointer in the dumped data (registration structs,
/// method-pointer tables) was fixed up by the loader to the live base, so
/// consumers computing `rva = pointer - ImageBase` only get correct RVAs when
/// the header's ImageBase matches the base the dump was taken from. (This is
/// why a dump from an ASLR-relocated run crashes Cpp2IL with the preferred
/// base left in place.)
pub fn rewrite_headers(out: &mut [u8], plan: &CarvePlan, image_base: u64) {
    out[plan.image_base_field_off..plan.image_base_field_off + 8]
        .copy_from_slice(&image_base.to_le_bytes());
    out[plan.file_align_field_off..plan.file_align_field_off + 4]
        .copy_from_slice(&plan.file_alignment.to_le_bytes());
    out[plan.size_of_headers_field_off..plan.size_of_headers_field_off + 4]
        .copy_from_slice(&plan.size_of_headers.to_le_bytes());
    for s in &plan.sections {
        out[s.hdr_off + 8..s.hdr_off + 12].copy_from_slice(&s.size.to_le_bytes()); // VirtualSize
        out[s.hdr_off + 12..s.hdr_off + 16].copy_from_slice(&s.va.to_le_bytes()); // VirtualAddress
        out[s.hdr_off + 16..s.hdr_off + 20].copy_from_slice(&s.size.to_le_bytes()); // SizeOfRawData
        out[s.hdr_off + 20..s.hdr_off + 24].copy_from_slice(&s.va.to_le_bytes()); // PointerToRawData
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal PE64: 0x400 header bytes, e_lfanew=0x80, two sections.
    fn synthetic_pe() -> Vec<u8> {
        let mut b = vec![0u8; 0x400];
        b[0..2].copy_from_slice(b"MZ");
        b[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        b[0x80..0x84].copy_from_slice(b"PE\0\0");
        let coff = 0x84;
        b[coff..coff + 2].copy_from_slice(&0x8664u16.to_le_bytes());
        b[coff + 2..coff + 4].copy_from_slice(&2u16.to_le_bytes()); // nsec
        b[coff + 16..coff + 18].copy_from_slice(&0xF0u16.to_le_bytes()); // szopt
        let opt = coff + 20;
        b[opt..opt + 2].copy_from_slice(&0x020Bu16.to_le_bytes());
        b[opt + 32..opt + 36].copy_from_slice(&0x1000u32.to_le_bytes()); // SectionAlignment
        b[opt + 36..opt + 40].copy_from_slice(&0x200u32.to_le_bytes()); // FileAlignment
        b[opt + 56..opt + 60].copy_from_slice(&0x3000u32.to_le_bytes()); // SizeOfImage
        b[opt + 60..opt + 64].copy_from_slice(&0x400u32.to_le_bytes()); // SizeOfHeaders
        let sec = opt + 0xF0;
        // .text VA=0x1000 VSz=0x612
        b[sec..sec + 5].copy_from_slice(b".text");
        b[sec + 8..sec + 12].copy_from_slice(&0x612u32.to_le_bytes());
        b[sec + 12..sec + 16].copy_from_slice(&0x1000u32.to_le_bytes());
        // .data VA=0x2000 VSz=0x834
        let s2 = sec + 40;
        b[s2..s2 + 5].copy_from_slice(b".data");
        b[s2 + 8..s2 + 12].copy_from_slice(&0x834u32.to_le_bytes());
        b[s2 + 12..s2 + 16].copy_from_slice(&0x2000u32.to_le_bytes());
        b
    }

    #[test]
    fn plans_two_section_pe() {
        let p = plan(&synthetic_pe()).unwrap();
        assert_eq!(p.file_alignment, 0x1000);
        assert_eq!(p.sections.len(), 2);
        assert_eq!(p.sections[0].va, 0x1000);
        assert_eq!(p.sections[0].size, 0x1000); // 0x612 aligned up
        assert_eq!(p.sections[1].va, 0x2000);
        assert_eq!(p.sections[1].size, 0x1000);
        // header end = 0x84+20+0xF0+80 = 0x1B8 -> 0x1000
        assert_eq!(p.size_of_headers, 0x1000);
        assert_eq!(p.file_size, 0x3000);
    }

    #[test]
    fn rewrite_patches_layout_fields() {
        let orig = synthetic_pe();
        let p = plan(&orig).unwrap();
        let mut out = vec![0u8; p.size_of_headers as usize];
        out[..orig.len()].copy_from_slice(&orig);
        rewrite_headers(&mut out, &p, 0x00007FFC_9C000000);
        // ImageBase now the live load base (u64 LE).
        assert_eq!(
            out[p.image_base_field_off..p.image_base_field_off + 8],
            0x00007FFC_9C000000u64.to_le_bytes()
        );
        // FileAlignment now 0x1000, SizeOfHeaders 0x1000.
        assert_eq!(u32le(&out, p.file_align_field_off), Some(0x1000));
        assert_eq!(u32le(&out, p.size_of_headers_field_off), Some(0x1000));
        // Section 0: raw ptr == VA, raw size == aligned virtual size.
        let s0 = p.sections[0].hdr_off;
        assert_eq!(u32le(&out, s0 + 8), Some(0x1000)); // VirtualSize aligned
        assert_eq!(u32le(&out, s0 + 16), Some(0x1000)); // SizeOfRawData
        assert_eq!(u32le(&out, s0 + 20), Some(0x1000)); // PointerToRawData == VA
        let s1 = p.sections[1].hdr_off;
        assert_eq!(u32le(&out, s1 + 20), Some(0x2000));
    }

    #[test]
    fn rejects_non_pe() {
        assert!(plan(&[0u8; 64]).is_none());
        let mut b = synthetic_pe();
        b[0x80] = b'X'; // break PE signature
        assert!(plan(&b).is_none());
        let mut b = synthetic_pe();
        b[0x84] = 0x4C; // machine i386
        b[0x85] = 0x01;
        assert!(plan(&b).is_none());
    }

    #[test]
    fn rejects_truncated_section_table() {
        let b = synthetic_pe();
        assert!(plan(&b[..0x100]).is_none());
    }
}
