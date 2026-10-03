//! Best-effort "is the Steam client running?" without loading steam_api64.
#![cfg(windows)]

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY_CURRENT_USER, KEY_READ, REG_DWORD,
};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn exe_is_steam(name: &[u16]) -> bool {
    let nul = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    let s = String::from_utf16_lossy(&name[..nul]).to_ascii_lowercase();
    s == "steam.exe"
}

fn process_named_steam() -> bool {
    unsafe {
        let snap: HANDLE = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap == !0usize as HANDLE {
            return false;
        }
        let mut pe: PROCESSENTRY32W = std::mem::zeroed();
        pe.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        if Process32FirstW(snap, &mut pe) != 0 {
            loop {
                if exe_is_steam(&pe.szExeFile) {
                    found = true;
                    break;
                }
                if Process32NextW(snap, &mut pe) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
        found
    }
}

fn registry_pid_alive() -> bool {
    unsafe {
        let sub = wide("Software\\Valve\\Steam\\ActiveProcess");
        let mut key = core::ptr::null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_READ, &mut key) != 0 {
            return false;
        }
        let value = wide("pid");
        let mut ty = 0u32;
        let mut pid = 0u32;
        let mut n = 4u32;
        let st = RegQueryValueExW(
            key,
            value.as_ptr(),
            core::ptr::null_mut(),
            &mut ty,
            &mut pid as *mut u32 as *mut u8,
            &mut n,
        );
        RegCloseKey(key);
        if st != 0 || ty != REG_DWORD || pid == 0 {
            return false;
        }
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return false;
        }
        CloseHandle(h);
        true
    }
}

fn steam_window() -> bool {
    unsafe {
        let class = wide("vguiPopupWindow");
        if !FindWindowW(class.as_ptr(), core::ptr::null()).is_null() {
            return true;
        }
        let class2 = wide("Valve001");
        if !FindWindowW(class2.as_ptr(), core::ptr::null()).is_null() {
            return true;
        }
        false
    }
}

/// True when a Steam client looks present on this machine (process, registry
/// ActiveProcess pid, or the vgui window). Used to resolve `mode = "auto"`.
pub fn client_is_running() -> bool {
    process_named_steam() || registry_pid_alive() || steam_window()
}

#[cfg(test)]
mod tests {
    use super::exe_is_steam;

    #[test]
    fn steam_exe_name_match() {
        let mut n = [0u16; 16];
        for (i, c) in "steam.exe".encode_utf16().enumerate() {
            n[i] = c;
        }
        assert!(exe_is_steam(&n));
        for (i, c) in "STEAM.EXE".encode_utf16().enumerate() {
            n[i] = c;
        }
        assert!(exe_is_steam(&n));
        for (i, c) in "notepad.exe".encode_utf16().enumerate() {
            n[i] = c;
        }
        assert!(!exe_is_steam(&n));
    }
}
