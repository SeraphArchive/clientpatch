//! Component-aware native object-name mapping. Keep counted UTF-16 names intact.
#[derive(Clone, Debug)]
pub(super) struct Mapping {
    pub source: Vec<u16>,
    pub target: Vec<u16>,
}

fn equal(a: &[u16], b: &[u16]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::Globalization::CompareStringOrdinal(
            a.as_ptr(),
            a.len() as i32,
            b.as_ptr(),
            b.len() as i32,
            1,
        ) == 2
    }
    #[cfg(not(windows))]
    {
        String::from_utf16_lossy(a).to_lowercase() == String::from_utf16_lossy(b).to_lowercase()
    }
}

pub(super) fn contains(root: &[u16], name: &[u16]) -> bool {
    name.len() >= root.len()
        && equal(root, &name[..root.len()])
        && (name.len() == root.len() || name[root.len()] == b'\\' as u16)
}

impl Mapping {
    pub fn new(source: &str, target: &str) -> Self {
        Self {
            source: source.encode_utf16().collect(),
            target: target.encode_utf16().collect(),
        }
    }

    pub fn rewrite(&self, name: &[u16]) -> Option<Vec<u16>> {
        if !contains(&self.source, name) {
            return None;
        }
        let mut result = self.target.clone();
        result.extend_from_slice(&name[self.source.len()..]);
        Some(result)
    }
}

pub(super) fn join(root: &[u16], name: &[u16]) -> Vec<u16> {
    let mut result = root.to_vec();
    if result.last() != Some(&(b'\\' as u16)) {
        result.push(b'\\' as u16);
    }
    result.extend_from_slice(name);
    result
}

/// NT handle-relative names can contain dot components even when Win32 has
/// already normalized ordinary absolute names. Registry dots are literal keys
/// and never pass through this function.
pub(super) fn normalize_file(name: &[u16]) -> Option<Vec<u16>> {
    if name.first() != Some(&(b'\\' as u16)) || name.contains(&0) {
        return None;
    }
    let mut parts: Vec<&[u16]> = Vec::new();
    for part in name.split(|c| *c == b'\\' as u16) {
        if part.is_empty() || part == [46] {
            continue;
        }
        if part == [46, 46] {
            // Never let relative traversal consume the native device prefix.
            if parts.len() <= 2 {
                return None;
            }
            parts.pop();
        } else {
            parts.push(part);
        }
    }
    let mut result = Vec::new();
    for part in parts {
        result.push(b'\\' as u16);
        result.extend_from_slice(part);
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn mapping_is_bounded_case_insensitive_and_idempotent() {
        let m = Mapping::new(
            r"\Device\Disk\Local\GameLib",
            r"\Device\Disk\Local\GameLib.test.isolated",
        );
        assert_eq!(
            m.rewrite(&w(r"\device\disk\local\gamelib\auth.db-wal")),
            Some(w(r"\Device\Disk\Local\GameLib.test.isolated\auth.db-wal"))
        );
        assert!(m.rewrite(&w(r"\Device\Disk\Local\GameLibrary\x")).is_none());
        assert!(m.rewrite(&m.target).is_none());
    }

    #[test]
    fn relative_traversal_is_resolved_before_matching() {
        let path = normalize_file(&join(
            &w(r"\Device\Disk\Local\Other"),
            &w(r"..\GameLib\auth.db"),
        ))
        .unwrap();
        assert_eq!(path, w(r"\Device\Disk\Local\GameLib\auth.db"));
        assert!(normalize_file(&w(r"\Device\Disk\..\..\x")).is_none());
    }

    #[test]
    fn suffix_cannot_escape_or_alias_a_profile() {
        for s in [
            "", ".", "..", "a..b", "a.", ".a", "a\\b", "a/b", "a:b", "a\0b", "a ",
        ] {
            assert!(!super::super::valid_suffix(s), "{s:?}");
        }
        for s in ["clientpatch", "dev-2", "private.server", "test_1"] {
            assert!(super::super::valid_suffix(s));
        }
    }
}
