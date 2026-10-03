//! Process-identity helpers. Host-compilable so the gate can be unit-tested.
//!
//! `version.dll` is picked up by any child that searches the game directory
//! (UnityCrashHandler, CEF helpers, job workers). Native detours + AllocConsole
//! in those processes are wasted at best and crashy at worst.

/// True when this process is the HBR player we actually want to patch.
///
/// `exe_base` is the file name (not a path). Unity/CEF helpers often re-exec
/// the same `HeavenBurnsRed.exe` with `--type=renderer` / `--type=gpu`.
pub fn is_target_process(exe_base: &str, cmdline: &str) -> bool {
    if !exe_base.eq_ignore_ascii_case("HeavenBurnsRed.exe") {
        return false;
    }
    let cl = cmdline.to_ascii_lowercase();
    if cl.contains("--type=") || cl.contains("--type ") {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::is_target_process;

    #[test]
    fn accepts_bare_game_exe() {
        assert!(is_target_process("HeavenBurnsRed.exe", r"E:\game\HeavenBurnsRed.exe"));
        assert!(is_target_process("heavenburnsred.exe", "HeavenBurnsRed.exe"));
    }

    #[test]
    fn rejects_other_exes() {
        assert!(!is_target_process("UnityCrashHandler64.exe", "UnityCrashHandler64.exe"));
        assert!(!is_target_process("version.dll", ""));
        assert!(!is_target_process("", ""));
    }

    #[test]
    fn rejects_cef_helpers_of_the_same_exe() {
        assert!(!is_target_process(
            "HeavenBurnsRed.exe",
            r#""E:\game\HeavenBurnsRed.exe" --type=renderer --log-level=1"#
        ));
        assert!(!is_target_process(
            "HeavenBurnsRed.exe",
            "HeavenBurnsRed.exe --type=gpu-process"
        ));
        assert!(!is_target_process(
            "HeavenBurnsRed.exe",
            "HeavenBurnsRed.exe --type renderer"
        ));
    }
}
