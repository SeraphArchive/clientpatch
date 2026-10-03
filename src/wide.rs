//! UTF-16 <-> Rust String helpers shared by the IL2CPP binding and native hooks.

/// Decode a UTF-16 unit slice (no NUL handling) into a String, lossily.
pub fn utf16_to_string(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

/// Encode a &str as a NUL-terminated UTF-16 buffer, for passing to native
/// wide-string (`const wchar_t*`) parameters.
pub fn to_wide_nul(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Number of UTF-16 units before the NUL terminator.
///
/// # Safety
/// `ptr` must be non-null and point to a valid NUL-terminated UTF-16 buffer.
#[cfg(windows)]
pub unsafe fn wide_len(ptr: *const u16) -> usize {
    if ptr.is_null() {
        return 0;
    }
    // Cap so a non-terminated pointer cannot scan until AV.
    const MAX: usize = 32768;
    let mut n = 0usize;
    while n < MAX && *ptr.add(n) != 0 {
        n += 1;
    }
    n
}

/// Read a NUL-terminated UTF-16 string into a Rust String.
///
/// # Safety
/// `ptr` must be null or a valid NUL-terminated UTF-16 buffer.
#[cfg(windows)]
pub unsafe fn from_wide_nul(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let len = wide_len(ptr);
    let slice = core::slice::from_raw_parts(ptr, len);
    utf16_to_string(slice)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_round_trips_ascii_and_unicode() {
        for s in [
            "",
            "/api/app/start",
            "https://gl-payment.gree-apps.net",
            "引き継ぎ",
        ] {
            let units: Vec<u16> = s.encode_utf16().collect();
            assert_eq!(utf16_to_string(&units), s);
        }
    }

    #[test]
    fn to_wide_nul_is_terminated() {
        let w = to_wide_nul("ok");
        assert_eq!(w, vec![b'o' as u16, b'k' as u16, 0]);
        assert_eq!(*w.last().unwrap(), 0);
    }
}
