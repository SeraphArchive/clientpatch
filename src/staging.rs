//! Recoverable file-set replacement for the no-manager setup fallback.
use std::path::{Path, PathBuf};

#[derive(serde::Serialize, serde::Deserialize)]
struct RecoveryJournal {
    files: Vec<(PathBuf, bool)>,
    gate: Option<PathBuf>,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum StoredJournal {
    Current(RecoveryJournal),
    Legacy(Vec<(PathBuf, bool)>),
}

pub(crate) fn checked_path(root: &Path, path: &Path) -> anyhow::Result<()> {
    let relative = path.strip_prefix(root)?;
    anyhow::ensure!(!relative.as_os_str().is_empty(), "destination must be below game directory");
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            anyhow::bail!("unsafe destination: {}", path.display());
        };
        anyhow::ensure!(!name.to_string_lossy().contains(':'), "unsafe destination stream");
        current.push(name);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) => {
                #[cfg(windows)]
                let linked = {
                    use std::os::windows::fs::MetadataExt;
                    meta.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let linked = meta.file_type().is_symlink();
                anyhow::ensure!(!linked, "linked installation path: {}", current.display());
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

pub(crate) fn pending(game_dir: &Path) -> bool {
    crate::module::data_dir(game_dir)
        .join(".native-install-backup/journal.json")
        .is_file()
}

pub(crate) fn recover_pending(game_dir: &Path) -> anyhow::Result<()> {
    recover(
        game_dir,
        &crate::module::data_dir(game_dir).join(".native-install-backup"),
    )
}

pub(crate) fn commit(
    game_dir: &Path,
    replacements: &[(PathBuf, Option<PathBuf>)],
    gate: Option<&Path>,
) -> anyhow::Result<()> {
    let backup = crate::module::data_dir(game_dir).join(".native-install-backup");
    checked_path(game_dir, &backup)?;
    // A prior interrupted commit must be recovered before another starts.
    recover(game_dir, &backup)?;
    std::fs::create_dir_all(&backup)?;
    if let Some(marker) = gate {
        anyhow::ensure!(replacements.iter().any(|(target, _)| target == marker), "completion marker is missing from replacements");
    }
    let mut saved = Vec::new();
    for (index, (target, _)) in replacements.iter().enumerate() {
        checked_path(game_dir, target)?;
        if target.is_dir() {
            anyhow::bail!("expected a file at {}", target.display());
        }
        let relative = target.strip_prefix(game_dir)?;
        if relative
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            anyhow::bail!("unsafe destination");
        }
        let existed = target.is_file();
        if existed {
            checked_path(game_dir, &backup.join(index.to_string()))?;
            std::fs::copy(target, backup.join(index.to_string()))?;
        }
        saved.push((relative.to_path_buf(), existed));
    }
    // Snapshot is complete before publishing the recovery journal.
    let journal_temp = backup.join("journal.pending");
    checked_path(game_dir, &journal_temp)?;
    match std::fs::remove_file(&journal_temp) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let mut journal = std::fs::OpenOptions::new().write(true).create_new(true).open(&journal_temp)?;
    use std::io::Write;
    let saved = RecoveryJournal {
        files: saved,
        gate: gate.map(|path| path.strip_prefix(game_dir).map(Path::to_path_buf)).transpose()?,
    };
    journal.write_all(&serde_json::to_vec(&saved)?)?;
    journal.sync_all()?;
    drop(journal);
    std::fs::rename(&journal_temp, backup.join("journal.json"))?;
    let result = (|| -> anyhow::Result<()> {
        if let Some(marker) = gate {
            if marker.exists() {
                std::fs::remove_file(marker)?;
            }
        }
        for (target, source) in replacements {
            checked_path(game_dir, target)?;
            if gate == Some(target.as_path()) {
                continue;
            }
            replace(target, source.as_deref())?;
        }
        if let Some(marker) = gate {
            if let Some((_, source)) = replacements.iter().find(|(target, _)| target == marker) {
                replace(marker, source.as_deref())?;
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        recover(game_dir, &backup)?;
        return Err(error);
    }
    std::fs::remove_file(backup.join("journal.json"))?;
    let _ = std::fs::remove_dir_all(&backup);
    Ok(())
}

fn recover(game_dir: &Path, backup: &Path) -> anyhow::Result<()> {
    let journal = backup.join("journal.json");
    checked_path(game_dir, &journal)?;
    if !journal.is_file() {
        return Ok(());
    }
    let stored: StoredJournal = serde_json::from_slice(&std::fs::read(&journal)?)?;
    let saved = match stored {
        StoredJournal::Current(saved) => saved,
        StoredJournal::Legacy(files) => {
            // The legacy native generation path only used this completion marker.
            let gate = files.iter().find(|(path, _)| path.file_name().is_some_and(|name|
                name == crate::modules::bepinex::MARKER_NAME)).map(|(path, _)| path.clone());
            RecoveryJournal { files, gate }
        }
    };
    if let Some(gate) = saved.gate.as_ref() {
        anyhow::ensure!(saved.files.iter().any(|(path, _)| path == gate), "recovery gate is missing from snapshot");
        let target = game_dir.join(gate);
        checked_path(game_dir, &target)?;
        // A failed/interrupted recovery must never advertise a mixed file set.
        match std::fs::remove_file(&target) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    // Validate the entire journal before restoring any payload file.
    for (index, (relative, existed)) in saved.files.iter().enumerate() {
        if relative.is_absolute()
            || relative
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            anyhow::bail!("unsafe recovery journal");
        }
        checked_path(game_dir, &game_dir.join(relative))?;
        if *existed {
            checked_path(game_dir, &backup.join(index.to_string()))?;
            anyhow::ensure!(backup.join(index.to_string()).is_file(), "recovery backup is missing");
        }
    }
    let restore_order = saved.files.iter().enumerate().filter(|(_, (path, _))| Some(path) != saved.gate.as_ref())
        .chain(saved.files.iter().enumerate().filter(|(_, (path, _))| Some(path) == saved.gate.as_ref()));
    for (index, (relative, existed)) in restore_order {
        replace(
            &game_dir.join(relative),
            if *existed {
                Some(backup.join(index.to_string()))
            } else {
                None
            }
            .as_deref(),
        )?;
    }
    std::fs::remove_file(&journal)?;
    std::fs::remove_dir_all(backup)?;
    Ok(())
}

fn replace(target: &Path, source: Option<&Path>) -> anyhow::Result<()> {
    let Some(source) = source else {
        if target.is_file() {
            std::fs::remove_file(target)?;
        }
        return Ok(());
    };
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    static NEXT_TEMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let serial = NEXT_TEMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = target.with_file_name(format!(".cpm-new-{}-{serial}", std::process::id()));
    let mut output = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;
    let copied = (|| -> std::io::Result<()> {
        std::io::copy(&mut std::fs::File::open(source)?, &mut output)?;
        output.sync_all()
    })();
    drop(output);
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&temp);
        return Err(e.into());
    }
    #[cfg(windows)]
    let result = {
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };
        let from = crate::wide::to_wide_nul(&temp.to_string_lossy());
        let to = crate::wide::to_wide_nul(&target.to_string_lossy());
        if unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    };
    #[cfg(not(windows))]
    let result = std::fs::rename(&temp, target);
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result?;
    Ok(())
}

#[cfg(test)]
mod tests {
    fn test_root(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("cpm-native-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn failed_commit_restores_bundle_and_marker_without_touching_other_temp_files() {
        let root = test_root("rollback");
        let loader = root.join("version.dll");
        let manager = root.join("manager.exe");
        let marker = root.join("bundle.version");
        let source = root.join("source");
        std::fs::write(&loader, "old loader").unwrap();
        std::fs::write(&manager, "old manager").unwrap();
        std::fs::write(&marker, "old version").unwrap();
        std::fs::write(&source, "new").unwrap();
        let unrelated = loader.with_extension("cpm-new");
        std::fs::write(&unrelated, "keep").unwrap();
        assert!(super::commit(&root, &[
            (marker.clone(), Some(source.clone())),
            (loader.clone(), Some(source)),
            (manager.clone(), Some(root.join("missing"))),
        ], Some(&marker)).is_err());
        assert_eq!(std::fs::read_to_string(loader).unwrap(), "old loader");
        assert_eq!(std::fs::read_to_string(manager).unwrap(), "old manager");
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "old version");
        assert_eq!(std::fs::read_to_string(unrelated).unwrap(), "keep");
        assert!(!super::pending(&root));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn interrupted_recovery_keeps_gate_absent_until_all_files_restore() {
        use std::os::windows::fs::OpenOptionsExt;
        for legacy in [false, true] {
            let root = test_root(if legacy { "legacy-gate" } else { "recovery-gate" });
            let backup = crate::module::data_dir(&root).join(".native-install-backup");
            std::fs::create_dir_all(&backup).unwrap();
            let marker = std::path::PathBuf::from(crate::modules::bepinex::MARKER_NAME);
            let files = vec![(marker.clone(), true), ("a.dll".into(), true), ("b.dll".into(), true)];
            for (index, (relative, _)) in files.iter().enumerate() {
                std::fs::write(root.join(relative), "new").unwrap();
                std::fs::write(backup.join(index.to_string()), "old").unwrap();
            }
            let json = if legacy { serde_json::to_vec(&files).unwrap() } else {
                serde_json::to_vec(&super::RecoveryJournal { files, gate: Some(marker.clone()) }).unwrap()
            };
            std::fs::write(backup.join("journal.json"), json).unwrap();
            let held = std::fs::OpenOptions::new().read(true).share_mode(1)
                .open(root.join("b.dll")).unwrap();
            assert!(super::recover_pending(&root).is_err());
            assert!(!root.join(&marker).exists());
            assert_eq!(std::fs::read_to_string(root.join("a.dll")).unwrap(), "old");
            assert!(super::pending(&root));
            drop(held);
            super::recover_pending(&root).unwrap();
            assert_eq!(std::fs::read_to_string(root.join(&marker)).unwrap(), "old");
            assert_eq!(std::fs::read_to_string(root.join("b.dll")).unwrap(), "old");
            assert!(!super::pending(&root));
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    fn refuses_junction_destination_before_any_replacement() {
        let root = test_root("junction");
        let outside = test_root("junction-external");
        let link = root.join("BepInEx");
        assert!(std::process::Command::new("cmd").args(["/c", "mklink", "/J"])
            .arg(&link).arg(&outside).output().unwrap().status.success());
        let target = outside.join("interop.dll");
        let source = root.join("source");
        std::fs::write(&target, "external").unwrap();
        std::fs::write(&source, "new").unwrap();
        assert!(super::commit(&root, &[(link.join("interop.dll"), Some(source))], None).is_err());
        assert_eq!(std::fs::read_to_string(target).unwrap(), "external");
        std::fs::remove_dir(link).unwrap();
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn preflight_failure_keeps_earlier_files() {
        let root = std::env::temp_dir().join(format!("cpm-native-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let a = root.join("a.dll");
        let b = root.join("b.dll");
        let source = root.join("source");
        std::fs::write(&a, "old").unwrap();
        std::fs::write(&source, "new").unwrap();
        std::fs::create_dir_all(&b).unwrap();
        assert!(super::commit(
            &root,
            &[(a.clone(), Some(source.clone())), (b, Some(source))],
            None
        )
        .is_err());
        assert_eq!(std::fs::read_to_string(a).unwrap(), "old");
        std::fs::remove_dir_all(root).unwrap();
    }
}
