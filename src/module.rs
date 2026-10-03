//! Module trait + compile-time registry.
use crate::config::Config;
use std::path::{Path, PathBuf};

/// Subdirectory of the game dir that holds everything clientpatch itself
/// creates or owns: config, log, langpacks, interop dumps, keys, the version
/// sidecar. `version.dll`, `BepInEx/`, and the manager exe stay in the game dir.
pub const DATA_DIR_NAME: &str = "clientpatch";

#[cfg(windows)]
use crate::il2cpp::Il2Cpp;

/// `<game_dir>/clientpatch`, creating it. This is where every file clientpatch
/// owns lives; `game_dir` itself only holds `version.dll`, `BepInEx/`, and the
/// manager exe.
pub fn data_dir(game_dir: &Path) -> PathBuf {
    let dir = game_dir.join(DATA_DIR_NAME);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// A file clientpatch owns: always `<game_dir>/clientpatch/<relative>`.
pub fn resolve_data_path(game_dir: &Path, relative: &Path) -> PathBuf {
    data_dir(game_dir).join(relative)
}

/// Everything a module needs at init: the resolved runtime (Windows), config, and
/// the directory containing `version.dll` (for resolving sidecar files).
///
/// `il2cpp` is `&'static` because module hooks (detours) keep running long after
/// `init` returns and dereference it; the bootstrap leaks the `Il2Cpp` so the
/// reference is genuinely process-lived.
pub struct LoaderCtx<'a> {
    #[cfg(windows)]
    pub il2cpp: &'static Il2Cpp,
    pub config: &'a Config,
    /// Directory of the loaded `version.dll`; sidecar paths (e.g. langpacks)
    /// resolve against this.
    pub dll_dir: &'a Path,
}

pub trait Module {
    fn name(&self) -> &str;

    /// Pre-IL2CPP pass: install native hooks that must beat the game's first
    /// use of a subsystem (e.g. the registry redirect must hook before Unity's
    /// first PlayerPrefs access). Runs right after config load, before the
    /// runtime is bound. `dll_dir` is the directory of the loaded `version.dll`
    /// (the game root). Default: no-op.
    fn init_early(&self, _config: &Config, _dll_dir: &Path) -> anyhow::Result<()> {
        Ok(())
    }

    /// Mid pass: IL2CPP is enumeratable and `Domain.dll` is registered, but the
    /// game's `InitializeFirst` coroutine may already be running. Used for
    /// hooks that must beat `InitializeLanguage` (which is gated by `isInit`
    /// and never rebuilds `_languages` if we miss the first call). Default: no-op.
    fn init_il2cpp_early(&self, _ctx: &LoaderCtx) -> anyhow::Result<()> {
        Ok(())
    }

    /// Main pass: runs after IL2CPP is initialized and a UI window exists.
    /// Default: no-op.
    fn init(&self, _ctx: &LoaderCtx) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Compile-time module list. To add a module later, push it here — no dynamic
/// loading (YAGNI).
pub fn registry() -> Vec<Box<dyn Module>> {
    vec![
        Box::new(crate::modules::steam::Steam),
        Box::new(crate::modules::lilypad::Lilypad),
        Box::new(crate::modules::regredirect::RegRedirect),
        Box::new(crate::modules::titlebar::Titlebar),
        Box::new(crate::modules::interopdump::InteropDump),
        // Last in the main pass: bepinex may block on dump+generate and restart
        // the game. (Its EARLY pass runs first — see bootstrap.)
        Box::new(crate::modules::bepinex::BepInEx),
    ]
}
