//! Unity 6 runtime-handle restore — pure logic, no Win32.
//!
//! In a live v31.1 (Unity 6000.0) process, il2cpp rewrites parts of the static
//! image from metadata *indices* to metadata *handles* (direct pointers into the
//! loaded `global-metadata.dat` blob). Two kinds matter for Cpp2IL/LibCpp2IL,
//! which expects the static index form:
//!
//!   * `Il2CppType.data` for VALUETYPE/CLASS — becomes a pointer into the
//!     typeDefinitions table (stride 0x58). Observed: 64,338 in HBR.
//!   * `Il2CppType.data` for VAR/MVAR (generic parameters) — becomes a pointer
//!     into the genericParameters table (stride 0x10). Observed: 19,344.
//!
//! Both mutations are mechanically invertible: any qword in the dumped image
//! that points into one of those tables IS such a handle (the static image has
//! no legitimate raw pointers into the metadata blob), and its static value is
//! `(ptr - table_base) / stride`. Verified empirically: gcd of all handle
//! differences == stride, min handle == table start, span == table size, and
//! Cpp2IL consumes the restored dump to completion (187 DLLs, zero failures).
#![forbid(unsafe_code)]

/// One metadata table that runtime handles can point into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandleTable {
    /// Table offset within the metadata blob.
    pub off: u64,
    /// Table size in bytes.
    pub size: u64,
    /// Entry stride; a qword is a handle only if `(ptr - base) % stride == 0`.
    pub stride: u64,
}

/// Replace every qword in `image` that points into one of `tables` (relative to
/// `meta_base`, the address the blob was dumped from) with its static table
/// index. Returns the number of qwords rewritten.
///
/// `image` is scanned at 8-byte alignment (pointers in a PE image always are).
pub fn restore(image: &mut [u8], meta_base: u64, tables: &[HandleTable]) -> u64 {
    let mut patched = 0u64;
    let mut off = 0usize;
    while off + 8 <= image.len() {
        let v = u64::from_le_bytes(image[off..off + 8].try_into().unwrap());
        for t in tables {
            let lo = meta_base.saturating_add(t.off);
            let hi = lo.saturating_add(t.size);
            if v >= lo && v < hi {
                let rel = v - lo;
                if rel % t.stride == 0 {
                    image[off..off + 8].copy_from_slice(&(rel / t.stride).to_le_bytes());
                    patched += 1;
                }
                break;
            }
        }
        off += 8;
    }
    patched
}

#[cfg(test)]
mod tests {
    use super::*;

    const META_BASE: u64 = 0x1799_e250000;
    const TYPEDEF: HandleTable = HandleTable {
        off: 0x1b7ab80,
        size: 39746 * 0x58,
        stride: 0x58,
    };
    const GENPARAM: HandleTable = HandleTable {
        off: 0x18ed658,
        size: 15089 * 0x10,
        stride: 0x10,
    };

    fn put(image: &mut [u8], at: usize, v: u64) {
        image[at..at + 8].copy_from_slice(&v.to_le_bytes());
    }
    fn get(image: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(image[at..at + 8].try_into().unwrap())
    }

    #[test]
    fn restores_typedef_and_generic_param_handles() {
        let mut img = vec![0u8; 0x100];
        // typedef handle for index 3, generic-param handle for index 7.
        put(&mut img, 0x00, META_BASE + TYPEDEF.off + 3 * 0x58);
        put(&mut img, 0x08, META_BASE + GENPARAM.off + 7 * 0x10);
        let n = restore(&mut img, META_BASE, &[TYPEDEF, GENPARAM]);
        assert_eq!(n, 2);
        assert_eq!(get(&img, 0x00), 3);
        assert_eq!(get(&img, 0x08), 7);
    }

    #[test]
    fn leaves_unaligned_and_out_of_range_values() {
        let mut img = vec![0u8; 0x100];
        // Mid-entry pointer (not stride-aligned): not a handle, untouched.
        put(&mut img, 0x00, META_BASE + TYPEDEF.off + 0x2C);
        // Pointer past the table end.
        put(&mut img, 0x08, META_BASE + TYPEDEF.off + TYPEDEF.size + 0x58);
        // In-image pointer (below the blob).
        put(&mut img, 0x10, 0x00007ffc_a3bc1000);
        // Small scalar.
        put(&mut img, 0x18, 0x9b42);
        let n = restore(&mut img, META_BASE, &[TYPEDEF, GENPARAM]);
        assert_eq!(n, 0);
        assert_eq!(get(&img, 0x00), META_BASE + TYPEDEF.off + 0x2C);
        assert_eq!(get(&img, 0x18), 0x9b42);
    }

    #[test]
    fn first_and_last_table_entries_restore() {
        let mut img = vec![0u8; 0x10];
        put(&mut img, 0, META_BASE + TYPEDEF.off); // index 0
        put(&mut img, 8, META_BASE + TYPEDEF.off + TYPEDEF.size - 0x58); // last
        let n = restore(&mut img, META_BASE, &[TYPEDEF]);
        assert_eq!(n, 2);
        assert_eq!(get(&img, 0), 0);
        assert_eq!(get(&img, 8), TYPEDEF.size / 0x58 - 1);
    }
}
