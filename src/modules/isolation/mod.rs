//! One profile for PlayerPrefs, Unity persistent data, and native GameLib storage.
//! The native layer redirects object names before Windows opens a handle. Reads,
//! writes, SQLite journals, directory enumeration and handle-relative opens then
//! operate on the same isolated objects. This is not a sandbox for hostile code.
use crate::module::Module;

#[cfg(windows)]
mod native;
mod paths;

pub struct Isolation;

impl Module for Isolation {
    fn name(&self) -> &str {
        "isolation"
    }

    #[cfg(windows)]
    fn init_early(
        &self,
        config: &crate::config::Config,
        _: &std::path::Path,
    ) -> anyhow::Result<()> {
        let cfg = config
            .isolation
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("isolation config missing"))?;
        native::install(&cfg.suffix)
    }
}

/// Also used by configuration validation; the suffix becomes part of both a
/// Windows directory component and a registry key. Keep the manager in sync.
pub fn valid_suffix(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && !value.starts_with('.')
        && !value.ends_with('.')
        && !value.contains("..")
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
}

#[cfg(windows)]
pub(crate) fn stop(error: &str) -> ! {
    crate::logging::line_try(
        "ERR",
        &format!("isolation unavailable; stopping to protect profiles: {error}"),
    );
    unsafe {
        windows_sys::Win32::System::Threading::TerminateProcess(
            windows_sys::Win32::System::Threading::GetCurrentProcess(),
            1,
        );
    }
    std::process::abort()
}
