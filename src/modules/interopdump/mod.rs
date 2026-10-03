//! Module #3 — `interopdump`: runtime IL2CPP dump for local interop generation.
//!
//! Why this exists: the on-disk `GameAssembly.dll` / `global-metadata.dat` are
//! protected, but both exist decrypted in the live process. Once per game build
//! this module carves them out:
//!
//!   1. scans own-process memory for the metadata header magic and validates it
//!      structurally (`metahdr`), then dumps the blob;
//!   2. hashes the blob — the build id (metadata content is identical across
//!      runs of the same build, unlike the relocated image);
//!   3. carves the loaded GameAssembly image into a raw==virtual PE
//!      (`peplan` — the convention Cpp2IL already accepts on this game);
//!   4. writes `<out_dir>/<build_id>/{GameAssembly.dll,global-metadata.dat,
//!      manifest.json,DONE}` plus `<out_dir>/latest.json`, ready for a
//!      Cpp2IL/Il2CppInterop generator to consume.
//!
//! Runs on its own thread from the main init pass (IL2CPP fully loaded by
//! then); a completed `DONE` marker makes subsequent launches free.
use crate::module::{LoaderCtx, Module};
use std::path::{Path, PathBuf};

#[cfg(any(windows, test))]
pub(crate) mod cleanup;
pub mod handles;
pub mod metahdr;
pub mod peplan;

// The background dump can still be running when all hook passes finish.
// Cleanup waits on its own thread so it cannot remove a dump being published.
#[cfg(any(windows, test))]
static CACHE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Default)]
pub struct InteropDump;

impl Module for InteropDump {
    fn name(&self) -> &str {
        "interopdump"
    }

    /// Main pass: IL2CPP is initialized, so the decrypted metadata blob and the
    /// full GameAssembly image are resident. Spawn and return — the dump
    /// (a multi-GB scan + ~170 MB of writes) must not delay other modules.
    #[cfg(windows)]
    fn init(&self, ctx: &LoaderCtx) -> anyhow::Result<()> {
        let cfg = ctx.config.interopdump.clone().unwrap_or_default();
        let out_root = resolve_out_dir(ctx.dll_dir, &cfg.out_dir);
        let dll_dir = ctx.dll_dir.to_path_buf();
        crate::logging::line(
            "INFO",
            &format!("interopdump: worker starting (out={})", out_root.display()),
        );
        std::thread::Builder::new()
            .name("interopdump".to_string())
            .spawn(move || worker::run(out_root, dll_dir, cfg.force, cfg.restore_handles))
            .map_err(|e| anyhow::anyhow!("spawn interopdump worker: {e}"))?;
        Ok(())
    }
}

/// `out_dir` resolves inside `<dll_dir>/clientpatch`, keeping subdirectories.
/// An absolute value keeps only its last component, so a dump cannot be pointed
/// outside that directory.
pub(crate) fn resolve_out_dir(dll_dir: &Path, out_dir: &str) -> PathBuf {
    let p = Path::new(out_dir);
    let name = if p.is_absolute() {
        p.file_name().map(PathBuf::from).unwrap_or_default()
    } else {
        p.to_path_buf()
    };
    crate::module::resolve_data_path(dll_dir, &name)
}

/// Bytes of the on-disk GameAssembly used for the build fingerprint.
const FINGERPRINT_PREFIX: usize = 4 * 1024 * 1024;

/// Pure core of [`ga_fingerprint`]: sha256 over `size_le64 ++ prefix`.
pub fn fingerprint_bytes(prefix: &[u8], size: u64) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(size.to_le_bytes());
    h.update(prefix);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Cheap build fingerprint for the on-disk (protected) GameAssembly.dll:
/// sha256(file_size ++ first 4 MiB). ~10 ms instead of ~1.5 s for a full-file
/// hash — it runs on the init thread in the doorstop race window. Sufficient:
/// the protected file is repacked wholesale every game update.
pub fn ga_fingerprint(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let size = f.metadata().ok()?.len();
    let mut prefix = vec![0u8; FINGERPRINT_PREFIX.min(size as usize)];
    f.read_exact(&mut prefix).ok()?;
    Some(fingerprint_bytes(&prefix, size))
}

/// Run a dump synchronously on the calling thread (used by the bepinex module
/// when it drives the dump itself). Blocks for a few seconds.
#[cfg(windows)]
pub fn dump_now(out_root: PathBuf, dll_dir: PathBuf, force: bool, restore_handles: bool) {
    worker::run(out_root, dll_dir, force, restore_handles)
}

#[cfg(windows)]
mod worker {
    use super::{handles, metahdr, peplan};
    use crate::logging;
    use core::ffi::c_void;
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Instant;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleA;
    use windows_sys::Win32::System::Memory::{
        VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE_READ,
        PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOACCESS, PAGE_READONLY,
        PAGE_READWRITE, PAGE_WRITECOPY,
    };

    const DONE_MARKER: &str = "DONE";
    /// Bytes read at a candidate for header validation (needs 8 + 64*8).
    const HEADER_PROBE: usize = 1024;
    /// Bytes read at the image base for PE parsing (covers any section table).
    const PE_HEADER_READ: usize = 0x10000;
    /// Scan chunk size (HEADER_PROBE bytes of overlap for boundary candidates).
    const SCAN_CHUNK: usize = 4 * 1024 * 1024;

    pub fn run(out_root: PathBuf, dll_dir: PathBuf, force: bool, restore_handles: bool) {
        let t0 = Instant::now();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = super::CACHE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            run_inner(&out_root, &dll_dir, force, restore_handles)
        }));
        match r {
            Ok(Ok(())) => logging::line(
                "INFO",
                &format!("interopdump: done in {:.1}s", t0.elapsed().as_secs_f32()),
            ),
            Ok(Err(e)) => logging::line("ERR", &format!("interopdump: {e:#}")),
            Err(_) => logging::line("ERR", "interopdump: worker panicked (swallowed)"),
        }
    }

    fn run_inner(
        out_root: &Path,
        dll_dir: &Path,
        force: bool,
        restore_handles: bool,
    ) -> anyhow::Result<()> {
        // Fingerprint of the on-disk (protected) GameAssembly: lets the bepinex
        // module detect a game update before IL2CPP is up, without decrypting.
        let ga_file_hash = super::ga_fingerprint(&dll_dir.join("GameAssembly.dll"));

        // 1. Locate + read the decrypted metadata blob.
        let (meta_addr, hdr) = find_metadata().ok_or_else(|| {
            anyhow::anyhow!("metadata blob not found in process memory (magic scan exhausted)")
        })?;
        logging::line(
            "INFO",
            &format!(
                "interopdump: metadata v{} at {meta_addr:#x}, {} tables, {} bytes",
                hdr.version, hdr.table_count, hdr.total_size
            ),
        );
        let blob = unsafe { read_slice(meta_addr, hdr.total_size as usize) }
            .ok_or_else(|| anyhow::anyhow!("metadata range became unreadable during read"))?;

        // 2. Build id = hash of the metadata (deterministic across runs of one
        //    build; the relocated image is not).
        let digest = Sha256::digest(&blob);
        let build_id: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
        let target = out_root.join(&build_id);

        if target.join(DONE_MARKER).is_file() && !force {
            logging::line(
                "INFO",
                &format!("interopdump: build {build_id} already dumped (cached)"),
            );
            write_latest(out_root, &build_id, hdr.version, ga_file_hash.as_deref())?;
            return Ok(());
        }

        // 3. Dump into a temp dir, then move into place atomically-ish.
        let tmp = out_root.join(format!("{build_id}.tmp"));
        if tmp.exists() {
            fs::remove_dir_all(&tmp)?;
        }
        fs::create_dir_all(&tmp)?;
        fs::write(tmp.join("global-metadata.dat"), &blob)?;
        logging::line(
            "INFO",
            &format!("interopdump: metadata written ({} bytes)", blob.len()),
        );

        let (ga_size, ga_base, restored) =
            carve_game_assembly(&tmp, meta_addr as u64, &blob, restore_handles)?;
        logging::line(
            "INFO",
            &format!(
                "interopdump: GameAssembly.dll carved ({ga_size} bytes, {restored} runtime handles restored)"
            ),
        );

        let manifest = serde_json::json!({
            "build_id": build_id,
            "clientpatch": crate::VERSION,
            "metadata_version": hdr.version,
            "metadata_size": blob.len(),
            "game_assembly_size": ga_size,
            "image_base": format!("{ga_base:#x}"),
            "handles_restored": restored,
            "game_assembly_file_fingerprint": ga_file_hash,
            "dumped_unix": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        });
        fs::write(
            tmp.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        // The DONE marker goes last: its presence means the whole set is complete.
        fs::write(tmp.join(DONE_MARKER), build_id.as_bytes())?;

        if target.exists() {
            // Interrupted earlier dump (no DONE) or a forced re-dump.
            fs::remove_dir_all(&target)?;
        }
        fs::rename(&tmp, &target)?;
        write_latest(out_root, &build_id, hdr.version, ga_file_hash.as_deref())?;
        logging::line(
            "INFO",
            &format!(
                "interopdump: build {build_id} ready at {}",
                target.display()
            ),
        );
        Ok(())
    }

    fn write_latest(
        out_root: &Path,
        build_id: &str,
        metadata_version: u32,
        ga_file_hash: Option<&str>,
    ) -> anyhow::Result<()> {
        fs::create_dir_all(out_root)?;
        let latest = serde_json::json!({
            "build_id": build_id,
            "dir": build_id,
            "metadata_version": metadata_version,
            "game_assembly_file_fingerprint": ga_file_hash,
        });
        let tmp = out_root.join("latest.json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&latest)?)?;
        let dst = out_root.join("latest.json");
        if dst.exists() {
            fs::remove_file(&dst)?;
        }
        fs::rename(&tmp, &dst)?;
        Ok(())
    }

    // ---- GameAssembly carve ------------------------------------------------

    /// sizeof(Il2CppTypeDefinition) at v31 — typedef handle stride.
    const TYPEDEF_STRIDE: u64 = 0x58;
    /// Generic-parameter handle-table entry stride at v31.1 (derived
    /// empirically: gcd of all runtime handle differences; see handles.rs).
    const GENPARAM_STRIDE: u64 = 0x10;

    fn carve_game_assembly(
        out_dir: &Path,
        meta_base: u64,
        meta_hdr: &[u8],
        restore_handles: bool,
    ) -> anyhow::Result<(u64, u64, u64)> {
        let base = unsafe { GetModuleHandleA(c"GameAssembly.dll".as_ptr() as *const u8) };
        if base.is_null() {
            anyhow::bail!("GameAssembly.dll module handle is null");
        }
        let base = base as usize;
        let hdr_buf = unsafe { read_slice(base, PE_HEADER_READ) }
            .ok_or_else(|| anyhow::anyhow!("GameAssembly headers unreadable at {base:#x}"))?;
        let plan = peplan::plan(&hdr_buf)
            .ok_or_else(|| anyhow::anyhow!("GameAssembly PE headers did not parse"))?;
        logging::line(
            "DBG",
            &format!("interopdump: GameAssembly live base {base:#x}"),
        );

        // Assemble the whole image in memory so the handle restore can run over
        // it before a single write.
        let mut image = vec![0u8; plan.file_size as usize];

        // Rewritten headers (zero-padded to the new SizeOfHeaders). The live
        // base goes into ImageBase so the loader-fixed-up pointers in the
        // dumped data resolve to correct RVAs (ASLR-safe; the NT loader usually
        // already patches the in-memory ImageBase, but don't rely on it).
        let mut out_hdrs = vec![0u8; plan.size_of_headers as usize];
        out_hdrs[..plan.header_end].copy_from_slice(&hdr_buf[..plan.header_end]);
        peplan::rewrite_headers(&mut out_hdrs, &plan, base as u64);
        image[..plan.size_of_headers as usize].copy_from_slice(&out_hdrs);

        for s in &plan.sections {
            let va = s.va as usize;
            let end = (s.va as u64).saturating_add(s.size as u64);
            if end > plan.file_size {
                logging::line(
                    "WARN",
                    &format!(
                        "interopdump: section at {va:#x} extends past image ({} > {}); skipped",
                        end, plan.file_size
                    ),
                );
                continue;
            }
            copy_memory_region(&mut image[va..va + s.size as usize], base + va);
        }

        // Unity 6 runtime mutation undo: type/generic-param handles -> indices.
        // Default OFF: Il2CppDumper wants the raw handle form (its
        // IsDumped mode derives the blob base from the handles); restore only
        // for Cpp2IL-compatible output.
        let restored = if restore_handles {
            let mut tables = Vec::with_capacity(2);
            if let Some((off, size)) = metahdr::table_range(meta_hdr, metahdr::HDR_TYPE_DEFINITIONS)
            {
                tables.push(handles::HandleTable {
                    off,
                    size,
                    stride: TYPEDEF_STRIDE,
                });
            }
            if let Some((off, size)) =
                metahdr::table_range(meta_hdr, metahdr::HDR_GENERIC_PARAMETERS)
            {
                tables.push(handles::HandleTable {
                    off,
                    size,
                    stride: GENPARAM_STRIDE,
                });
            }
            handles::restore(&mut image, meta_base, &tables)
        } else {
            0
        };

        fs::write(out_dir.join("GameAssembly.dll"), &image)?;
        Ok((plan.file_size, base as u64, restored))
    }

    /// Copy own-process memory into `dst`, leaving zeros for any sub-page that
    /// is not readable (guard pages / discardable sections).
    fn copy_memory_region(dst: &mut [u8], src: usize) {
        const PAGE: usize = 0x1000;
        let mut done = 0usize;
        while done < dst.len() {
            let n = PAGE.min(dst.len() - done);
            let at = src + done;
            if let Some(bytes) = unsafe { read_slice(at, n) } {
                dst[done..done + n].copy_from_slice(&bytes);
            }
            done += n;
        }
    }

    // ---- metadata scan -----------------------------------------------------

    /// Walk our own address space for the metadata header magic; return the
    /// first candidate whose header validates AND whose whole blob is readable.
    fn find_metadata() -> Option<(usize, metahdr::MetadataHeader)> {
        let magic = metahdr::MAGIC.to_le_bytes();
        let mut addr = 0x10000usize;
        let mut hits = 0u32;
        let mut scanned_mb = 0u64;
        loop {
            let mbi = query(addr)?;
            let end = (mbi.BaseAddress as usize).saturating_add(mbi.RegionSize);
            if end <= addr {
                break;
            }
            if region_is_readable(&mbi) {
                scanned_mb += (mbi.RegionSize / 0x100000) as u64;
                // Chunk the scan with a per-chunk re-query: a multi-GB region
                // (or a CoreCLR GC segment, now that BepInEx shares the process)
                // can be partially unmapped between the region walk and the
                // read. One big slice read here was an AV in version.dll.
                let base = mbi.BaseAddress as usize;
                let mut off = 0usize;
                while off < mbi.RegionSize {
                    // Never read past the region this walk validated: the page
                    // right after it can be a guard page, and reading into it AVs.
                    // The chunk overlaps the next one by HEADER_PROBE so a magic
                    // near a boundary still has its validation bytes in-buffer.
                    let n = SCAN_CHUNK.min(mbi.RegionSize - off);
                    if let Some(data) = unsafe { read_slice(base + off, n) } {
                        let mut co = 0usize;
                        while let Some(p) = data[co..].iter().position(|&b| b == magic[0]) {
                            let i = co + p;
                            if i + 4 > data.len() {
                                break;
                            }
                            if data[i + 1] == magic[1]
                                && data[i + 2] == magic[2]
                                && data[i + 3] == magic[3]
                            {
                                hits += 1;
                                let cand = base + off + i;
                                if let Some(h) = probe_candidate(cand, data.len() - i) {
                                    if hits > 1 {
                                        logging::line(
                                            "WARN",
                                            &format!(
                                                "interopdump: {hits} magic hits; using first valid"
                                            ),
                                        );
                                    }
                                    return Some((cand, h));
                                }
                            }
                            co = i + 1;
                        }
                    }
                    let step = n.saturating_sub(HEADER_PROBE).max(1);
                    off += step;
                }
            }
            addr = end;
        }
        logging::line(
            "DBG",
            &format!("interopdump: scan exhausted ({scanned_mb} MiB committed, {hits} magic hits)"),
        );
        None
    }

    /// Validate a magic hit: parse the header (bounded by the current region),
    /// then require the entire computed blob range to be readable.
    fn probe_candidate(cand: usize, in_region: usize) -> Option<metahdr::MetadataHeader> {
        if in_region < HEADER_PROBE {
            return None;
        }
        let probe = unsafe { read_slice(cand, HEADER_PROBE) }?;
        let hdr = metahdr::parse(&probe)?;
        if !range_readable(cand, hdr.total_size as usize) {
            return None;
        }
        Some(hdr)
    }

    // ---- memory helpers ----------------------------------------------------

    /// Read own-process memory into a Vec; None if any part is unreadable.
    unsafe fn read_slice(addr: usize, len: usize) -> Option<Vec<u8>> {
        if !range_readable(addr, len) {
            return None;
        }
        use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        // VirtualQuery is only a preflight. The OS reports a concurrent unmap as
        // a failed/partial copy instead of allowing an AV in this Rust frame.
        let mut bytes = vec![0u8; len];
        let mut read = 0usize;
        if ReadProcessMemory(
            GetCurrentProcess(),
            addr as *const c_void,
            bytes.as_mut_ptr() as *mut c_void,
            len,
            &mut read,
        ) == 0
            || read != len
        {
            return None;
        }
        Some(bytes)
    }

    fn query(addr: usize) -> Option<MEMORY_BASIC_INFORMATION> {
        let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
        let n = unsafe {
            VirtualQuery(
                addr as *const c_void,
                &mut mbi,
                core::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if n == 0 {
            None
        } else {
            Some(mbi)
        }
    }

    fn region_is_readable(mbi: &MEMORY_BASIC_INFORMATION) -> bool {
        if mbi.State != MEM_COMMIT {
            return false;
        }
        if mbi.Protect & (PAGE_GUARD | PAGE_NOACCESS) != 0 {
            return false;
        }
        mbi.Protect
            & (PAGE_READONLY
                | PAGE_READWRITE
                | PAGE_WRITECOPY
                | PAGE_EXECUTE_READ
                | PAGE_EXECUTE_READWRITE
                | PAGE_EXECUTE_WRITECOPY)
            != 0
    }

    /// Every page in `[addr, addr+len)` committed and readable. Region-walks via
    /// VirtualQuery. This is a preflight only; ReadProcessMemory handles a
    /// mapping becoming unreadable between this check and the actual copy.
    fn range_readable(mut addr: usize, len: usize) -> bool {
        let end = match addr.checked_add(len) {
            Some(e) => e,
            None => return false,
        };
        while addr < end {
            let Some(mbi) = query(addr) else { return false };
            let rend = (mbi.BaseAddress as usize).saturating_add(mbi.RegionSize);
            if rend <= addr || !region_is_readable(&mbi) {
                return false;
            }
            addr = rend;
        }
        true
    }

    #[cfg(test)]
    mod memory_tests {
        #[test]
        fn checked_memory_copy_accepts_owned_bytes_and_rejects_invalid_pages() {
            let bytes = vec![0xa5u8; 8192];
            let copied = unsafe { super::read_slice(bytes.as_ptr() as usize, bytes.len()) };
            assert_eq!(copied.as_deref(), Some(bytes.as_slice()));
            assert!(unsafe { super::read_slice(1, 1024) }.is_none());
            assert!(unsafe { super::read_slice(usize::MAX - 2, 1024) }.is_none());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_out_dir_resolves_inside_data_dir() {
        let got = resolve_out_dir(Path::new("C:\\game"), "interop");
        assert_eq!(got, PathBuf::from("C:\\game\\clientpatch\\interop"));
    }

    #[test]
    fn absolute_out_dir_keeps_only_its_name() {
        let got = resolve_out_dir(Path::new("C:\\game"), "D:\\cache");
        assert_eq!(got, PathBuf::from("C:\\game\\clientpatch\\cache"));
    }

    #[test]
    fn fingerprint_is_size_and_prefix_sensitive() {
        let a = fingerprint_bytes(&[1, 2, 3, 4], 4);
        // Same prefix, different size -> different fingerprint.
        assert_ne!(a, fingerprint_bytes(&[1, 2, 3, 4], 5));
        // Same size, different prefix -> different fingerprint.
        assert_ne!(a, fingerprint_bytes(&[1, 2, 3, 5], 4));
        // Deterministic.
        assert_eq!(a, fingerprint_bytes(&[1, 2, 3, 4], 4));
        assert_eq!(a.len(), 64);
    }

    /// Opt-in verification against a real IL2CPP dump directory (containing
    /// `global-metadata.dat` + `GameAssembly.dll`). Ships no game data; run
    /// manually after each game update:
    ///   `HBR_IL2CPP_DIR=<dir> cargo test -- --ignored real_hbr_dump`
    #[test]
    #[ignore = "requires HBR_IL2CPP_DIR pointing at a real dump"]
    fn real_hbr_dump_validates() {
        let dir = std::env::var("HBR_IL2CPP_DIR")
            .expect("set HBR_IL2CPP_DIR when explicitly running the real dump test");
        let dir = PathBuf::from(dir);

        let meta = std::fs::read(dir.join("global-metadata.dat")).unwrap();
        let h = metahdr::parse(&meta).expect("real metadata header must validate");
        assert_eq!(
            h.total_size,
            meta.len() as u64,
            "last table end == file size"
        );
        eprintln!(
            "metadata: v{} tables={} size={}",
            h.version, h.table_count, h.total_size
        );

        let pe = std::fs::read(dir.join("GameAssembly.dll")).unwrap();
        let p = peplan::plan(&pe[..PE_HEADER_READ_MAX.min(pe.len())])
            .expect("real GameAssembly headers must parse");
        assert_eq!(p.file_size, pe.len() as u64, "carve plan size == file size");
        eprintln!(
            "image: {} sections, file_size={}",
            p.sections.len(),
            p.file_size
        );
    }

    /// Mirror of the worker's header-read size for the ignored real-file test.
    const PE_HEADER_READ_MAX: usize = 0x10000;
}
