//! Keep the installed successful build's dump cache on every launch. A newer
//! latest.json may describe a dump awaiting generation, so protect that input too.
use std::fs;
use std::path::Path;

use crate::modules::bepinex::{parse_latest, parse_marker, MARKER_NAME};

fn is_build_id(name: &str) -> bool {
    name.len() == 16
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_link(meta: &fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Includes junctions and other reparse points, not just symbolic links.
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn check_tree(path: &Path) -> anyhow::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        !is_link(&meta),
        "cache contains a linked path: {}",
        path.display()
    );
    if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            check_tree(&entry?.path())?;
        }
    }
    Ok(())
}

pub(crate) fn prune(data: &Path, root: &Path, interop: &Path) -> anyhow::Result<usize> {
    let _guard = super::CACHE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if !root.exists() || !interop.join(MARKER_NAME).exists() {
        return Ok(0);
    }
    // Destructive operations stay under clientpatch, including custom out_dir.
    // Reject traversal and reparse points rather than following them out of it.
    let relative = root.strip_prefix(data)?;
    anyhow::ensure!(
        relative
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_))),
        "dump root must stay inside clientpatch"
    );
    for path in root.ancestors().take_while(|p| p.starts_with(data)) {
        anyhow::ensure!(
            !is_link(&fs::symlink_metadata(path)?),
            "linked dump root: {}",
            path.display()
        );
    }
    let resolved_data = data.canonicalize()?;
    let resolved_root = root.canonicalize()?;
    anyhow::ensure!(
        resolved_root != resolved_data && resolved_root.starts_with(&resolved_data),
        "dump root must stay inside clientpatch"
    );

    let Some(successful) =
        parse_marker(&fs::read_to_string(interop.join(MARKER_NAME))?).filter(|id| is_build_id(id))
    else {
        return Ok(0);
    };
    // The installation marker is committed last by both generation paths. A
    // dump DONE alone does not mean generation or installation succeeded.
    for name in ["Assembly-CSharp.dll", "UnityEngine.CoreModule.dll"] {
        let Ok(meta) = fs::metadata(interop.join(name)) else {
            return Ok(0);
        };
        if !meta.is_file() || meta.len() == 0 {
            return Ok(0);
        }
    }
    if fs::read_to_string(root.join(&successful).join("DONE"))
        .ok()
        .as_deref()
        != Some(&successful)
    {
        return Ok(0);
    }
    let pending = match fs::read_to_string(root.join("latest.json")) {
        Ok(json) => {
            let Some(latest) = parse_latest(&json).filter(|l| is_build_id(&l.build_id)) else {
                return Ok(0);
            };
            Some(latest.build_id)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let mut removed = 0;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        // Only version directories: latest.json, unrelated files/directories,
        // and active .tmp dumps never belong to the deletion set.
        if !is_build_id(name) || name == successful || pending.as_deref() == Some(name) {
            continue;
        }
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)?;
        if !meta.is_dir() || is_link(&meta) {
            continue;
        }
        let result = check_tree(&path).and_then(|()| {
            let resolved = path.canonicalize()?;
            anyhow::ensure!(
                resolved.parent() == Some(resolved_root.as_path()),
                "version escaped dump root"
            );
            fs::remove_dir_all(&path)?;
            Ok(())
        });
        match result {
            Ok(()) => removed += 1,
            Err(e) => crate::logging::line(
                "WARN",
                &format!(
                    "interopdump: cannot remove {}: {e:#}; will retry next launch",
                    path.display()
                ),
            ),
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    const CURRENT: &str = "1234567890abcdef";
    const OLD: &str = "0123456789abcdef";
    const NEXT: &str = "abcdef0123456789";
    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    struct Cache {
        base: PathBuf,
        data: PathBuf,
        root: PathBuf,
        interop: PathBuf,
    }
    impl Cache {
        fn new() -> Self {
            let base = std::env::temp_dir().join(format!(
                "clientpatch-cleanup-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            let data = base.join("clientpatch");
            let root = data.join("interop");
            let interop = base.join("BepInEx/interop");
            fs::create_dir_all(&interop).unwrap();
            let cache = Self {
                base,
                data,
                root,
                interop,
            };
            for id in [CURRENT, OLD] {
                cache.dump(id);
            }
            cache.latest(CURRENT);
            fs::write(
                cache.interop.join(MARKER_NAME),
                format!(r#"{{"build_id":"{CURRENT}"}}"#),
            )
            .unwrap();
            for name in ["Assembly-CSharp.dll", "UnityEngine.CoreModule.dll"] {
                fs::write(cache.interop.join(name), b"installed assembly").unwrap();
            }
            cache
        }
        fn dump(&self, id: &str) {
            let dir = self.root.join(id);
            fs::create_dir_all(dir.join("dumper_out/DummyDll")).unwrap();
            fs::write(dir.join("DONE"), id).unwrap();
            fs::write(dir.join("dumper_out/DummyDll/test.dll"), b"cache").unwrap();
        }
        fn latest(&self, id: &str) {
            fs::write(
                self.root.join("latest.json"),
                format!(r#"{{"build_id":"{id}"}}"#),
            )
            .unwrap();
        }
        fn prune(&self) -> usize {
            prune(&self.data, &self.root, &self.interop).unwrap()
        }
    }
    impl Drop for Cache {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    #[test]
    fn every_launch_keeps_success_and_removes_old_cache_recursively() {
        let c = Cache::new();
        fs::create_dir(c.root.join("notes")).unwrap();
        fs::create_dir(c.root.join(format!("{NEXT}.tmp"))).unwrap();
        fs::write(c.root.join(NEXT), b"unrelated file").unwrap();
        assert_eq!(c.prune(), 1);
        assert!(!c.root.join(OLD).exists());
        assert!(c.root.join(CURRENT).join("DONE").exists());
        assert!(c.root.join("latest.json").exists());
        assert!(c.root.join("notes").exists());
        assert!(c.root.join(format!("{NEXT}.tmp")).exists());
        assert!(c.root.join(NEXT).is_file());
        c.dump(OLD); // No new generation: the next launch still checks.
        assert_eq!(c.prune(), 1);
        assert_eq!(c.prune(), 0);
    }

    #[test]
    fn pending_generation_keeps_input_and_last_success() {
        let c = Cache::new();
        c.dump(NEXT);
        c.latest(NEXT);
        assert_eq!(c.prune(), 1);
        assert!(c.root.join(CURRENT).exists());
        assert!(c.root.join(NEXT).exists());
        fs::write(
            c.interop.join(MARKER_NAME),
            format!(r#"{{"build_id":"{NEXT}"}}"#),
        )
        .unwrap();
        assert_eq!(c.prune(), 1);
        assert!(!c.root.join(CURRENT).exists());
        assert!(c.root.join(NEXT).exists());
    }

    #[test]
    fn incomplete_or_failed_install_does_not_clear_cache() {
        for missing in [
            MARKER_NAME,
            "Assembly-CSharp.dll",
            "UnityEngine.CoreModule.dll",
        ] {
            let c = Cache::new();
            fs::remove_file(c.interop.join(missing)).unwrap();
            assert_eq!(c.prune(), 0);
            assert!(c.root.join(OLD).exists());
        }
        let c = Cache::new();
        fs::remove_file(c.root.join(CURRENT).join("DONE")).unwrap();
        assert_eq!(c.prune(), 0);
        assert!(c.root.join(OLD).exists());
    }

    #[test]
    fn invalid_markers_and_escaping_root_do_not_delete() {
        let c = Cache::new();
        for id in ["../outside", "", "ABCDEF0123456789"] {
            c.latest(id);
            assert_eq!(c.prune(), 0);
            assert!(c.root.join(OLD).exists());
        }
        c.latest(CURRENT);
        assert!(prune(&c.data, &c.data.join("../clientpatch/interop"), &c.interop).is_err());
        assert!(c.root.join(OLD).exists());
    }

    #[test]
    fn custom_dump_root_is_pruned_inside_clientpatch() {
        let mut c = Cache::new();
        let custom = c.data.join("cache/custom-interop");
        fs::create_dir_all(custom.parent().unwrap()).unwrap();
        fs::rename(&c.root, &custom).unwrap();
        c.root = custom;
        assert_eq!(c.prune(), 1);
        assert!(c.root.join(CURRENT).exists());
    }

    #[cfg(windows)]
    #[test]
    fn locked_old_files_are_retained_and_retried() {
        use std::os::windows::fs::OpenOptionsExt;
        let c = Cache::new();
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(c.root.join(OLD).join("dumper_out/DummyDll/test.dll"))
            .unwrap();
        assert_eq!(c.prune(), 0);
        assert!(c.root.join(OLD).exists());
        drop(held);
        assert_eq!(c.prune(), 1);
        assert!(!c.root.join(OLD).exists());
    }

    #[cfg(windows)]
    #[test]
    fn junction_inside_old_version_preserves_external_files() {
        let c = Cache::new();
        let outside = c.base.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep.txt"), b"keep").unwrap();
        let link = c.root.join(OLD).join("linked");
        assert!(std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .output()
            .unwrap()
            .status
            .success());
        assert_eq!(c.prune(), 0);
        assert_eq!(fs::read(outside.join("keep.txt")).unwrap(), b"keep");
        assert!(c.root.join(OLD).exists());
        fs::remove_dir(link).unwrap();
        assert_eq!(c.prune(), 1);
    }
}
