//! Per-launch append logger -> one `<stem>_<timestamp>.log` file per game launch
//! inside `<game dir>/clientpatch/logs` (10 newest kept; the stem comes from the
//! `loader.log` config key), ALSO teed to an allocated console window (always on)
//! and a small in-memory ring buffer that the crash guard shows in its crash
//! window. Each line is `[+<ms since init>] TAG msg`.
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// Subdirectory of the data dir holding the per-launch log files.
pub const LOGS_DIR_NAME: &str = "logs";
/// Per-launch log files kept (newest) before pruning.
const KEEP: usize = 10;

struct Sink {
    file: Option<File>,
    start: Instant,
}

static SINK: Mutex<Option<Sink>> = Mutex::new(None);
static CONSOLE: AtomicPtr<core::ffi::c_void> = AtomicPtr::new(core::ptr::null_mut());
static RECENT: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
const RECENT_MAX: usize = 18;

/// Start a fresh per-launch log: `<dir>/logs/<stem>_<timestamp>.log`, then prune
/// the directory to the newest KEEP files. `file` only supplies the stem (the
/// `loader.log` config key); any directory part is ignored, so logs always land
/// in `<dir>/logs/`. Errors are swallowed (logging must never crash the host);
/// a failed init simply means subsequent `line()` calls write no file.
pub fn init(dir: &Path, file: &str) {
    let stem = Path::new(file)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("clientpatch")
        .to_string();
    let logs_dir = dir.join(LOGS_DIR_NAME);
    let _ = std::fs::create_dir_all(&logs_dir);
    let stamp = timestamp();
    let mut file = None;
    let mut suffix = 0;
    while file.is_none() && suffix < 10 {
        // A relaunch within the same second (bepinex setup restarts the game)
        // would hit an existing name; the -N suffix keeps create_new from
        // clobbering a previous launch's log.
        let name = if suffix == 0 {
            format!("{stem}_{stamp}.log")
        } else {
            format!("{stem}_{stamp}-{suffix}.log")
        };
        match OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(logs_dir.join(name))
        {
            Ok(f) => file = Some(f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => suffix += 1,
            Err(_) => break,
        }
    }
    if let Ok(mut guard) = SINK.lock() {
        *guard = Some(Sink {
            file,
            start: Instant::now(),
        });
    }
    prune(&logs_dir);
}

/// Local wall-clock `YYYYMMDD-HHMMSS` for the per-launch file name. Falls back
/// to Unix seconds if the clock reads as unusable.
fn timestamp() -> String {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::SYSTEMTIME;
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;
        let mut st = SYSTEMTIME {
            wYear: 0,
            wMonth: 0,
            wDayOfWeek: 0,
            wDay: 0,
            wHour: 0,
            wMinute: 0,
            wSecond: 0,
            wMilliseconds: 0,
        };
        unsafe { GetLocalTime(&mut st) };
        if st.wYear >= 1601 {
            return format!(
                "{:04}{:02}{:02}-{:02}{:02}{:02}",
                st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
            );
        }
    }
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

/// Keep only the newest KEEP `*.log` files in the per-launch logs directory.
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<(PathBuf, std::time::SystemTime)> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
        .filter_map(|e| {
            let modified = e.metadata().ok()?.modified().ok()?;
            Some((e.path(), modified))
        })
        .collect();
    let excess = logs.len().saturating_sub(KEEP);
    if excess == 0 {
        return;
    }
    logs.sort_by_key(|(_, t)| *t);
    for (path, _) in logs.into_iter().take(excess) {
        let _ = std::fs::remove_file(path);
    }
}

/// Allocate a console window and tee log output to it (UTF-8). Always on.
#[cfg(windows)]
pub fn enable_console() {
    use windows_sys::Win32::Storage::FileSystem::CreateFileW;
    use windows_sys::Win32::System::Console::{
        AllocConsole, GetConsoleMode, SetConsoleMode, SetConsoleOutputCP, SetConsoleTitleW,
        ENABLE_EXTENDED_FLAGS, ENABLE_QUICK_EDIT_MODE,
    };
    unsafe {
        AllocConsole();
        SetConsoleOutputCP(65001); // CP_UTF8 so CJK log text renders
        // Quick-edit mode is on by default, and selecting text in it blocks every
        // console write in the process until the selection is cleared. The game and
        // BepInEx share this console, so one accidental click freezes both. Turning
        // it off costs text selection; the log file keeps the full output.
        let input = CreateFileW(
            "CONIN$\0".encode_utf16().collect::<Vec<_>>().as_ptr(),
            0xC000_0000,
            3,
            core::ptr::null(),
            3,
            0,
            core::ptr::null_mut(),
        );
        if !input.is_null() && (input as isize) != -1 {
            let mut mode = 0u32;
            if GetConsoleMode(input, &mut mode) != 0 {
                SetConsoleMode(
                    input,
                    (mode & !ENABLE_QUICK_EDIT_MODE) | ENABLE_EXTENDED_FLAGS,
                );
            }
        }
        let title: Vec<u16> = "clientpatch — runtime log\0".encode_utf16().collect();
        SetConsoleTitleW(title.as_ptr());
        let name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
        // GENERIC_WRITE=0x40000000, share R|W=3, OPEN_EXISTING=3
        let h = CreateFileW(name.as_ptr(), 0x4000_0000, 3, core::ptr::null(), 3, 0, core::ptr::null_mut());
        if !h.is_null() && (h as isize) != -1 {
            CONSOLE.store(h, Ordering::SeqCst);
        }
    }
}

#[cfg(not(windows))]
pub fn enable_console() {}

/// Append one timestamped line to the file, the console, and the ring buffer.
pub fn line(tag: &str, msg: &str) {
    line_inner(tag, msg, false);
}

/// Crash-path logger: never blocks on the sink mutex (same thread may already
/// hold it). Drops the line if the lock is busy.
pub fn line_try(tag: &str, msg: &str) {
    line_inner(tag, msg, true);
}

fn line_inner(tag: &str, msg: &str, from_crash: bool) {
    // Format before taking the lock, so a poisoned or busy sink still reaches
    // the console and the crash dialog's ring buffer.
    let text = format!("{tag:<5} {msg}");
    let sink_guard = if from_crash {
        SINK.try_lock().ok()
    } else {
        SINK.lock().ok()
    };
    let text = if let Some(mut guard) = sink_guard {
        if let Some(sink) = guard.as_mut() {
            let ms = sink.start.elapsed().as_millis();
            let stamped = format!("[+{ms:>7}ms] {text}");
            if let Some(f) = sink.file.as_mut() {
                let _ = writeln!(f, "{stamped}");
                let _ = f.flush();
            }
            stamped
        } else {
            text
        }
    } else {
        text
    };
    #[cfg(windows)]
    {
        let h = CONSOLE.load(Ordering::SeqCst);
        if !h.is_null() {
            let mut buf = text.clone();
            buf.push_str("\r\n");
            let wide: Vec<u16> = buf.encode_utf16().collect();
            let mut written = 0u32;
            unsafe {
                windows_sys::Win32::System::Console::WriteConsoleW(
                    h,
                    wide.as_ptr(),
                    wide.len() as u32,
                    &mut written,
                    core::ptr::null(),
                );
            }
        }
    }
    let recent_guard = if from_crash {
        RECENT.try_lock().ok()
    } else {
        RECENT.lock().ok()
    };
    if let Some(mut r) = recent_guard {
        r.push_back(text);
        while r.len() > RECENT_MAX {
            r.pop_front();
        }
    }
}

/// The last (up to RECENT_MAX) log lines, for the crash window. Uses `try_lock` so
/// it can never deadlock when called from the crashing thread's exception filter.
pub fn recent() -> String {
    match RECENT.try_lock() {
        Ok(r) => r.iter().cloned().collect::<Vec<_>>().join("\n"),
        Err(_) => String::new(),
    }
}
