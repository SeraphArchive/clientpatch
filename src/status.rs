//! Process-wide redirect status.
//!
//! The titlebar used to advertise a successful LilyPad redirect as soon as the
//! native `SetWindowTextW` hook was installed — long before GameLib / API hooks
//! had proven they actually rewrote a host. That is a false positive: the user
//! sees "clientpatch → host" while the game is still talking to official
//! servers (and pops 第三方服务器设置不正常).
//!
//! States:
//!   * `Pending` — hooks armed, no rewrite observed yet (title says pending).
//!   * `Live`    — at least one GameLib/API rewrite fired (title shows the host).
//!   * `Failed`  — GameLib init ran against the official host (sticky; title
//!     says HOOK FAIL). A later API rewrite cannot hide this.
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

const PENDING: u8 = 0;
const LIVE: u8 = 1;
const FAILED: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedirectState {
    Pending,
    Live,
    Failed,
}

impl RedirectState {
    fn from_u8(v: u8) -> Self {
        match v {
            LIVE => RedirectState::Live,
            FAILED => RedirectState::Failed,
            _ => RedirectState::Pending,
        }
    }
}

static STATE: AtomicU8 = AtomicU8::new(PENDING);
static REASON: Mutex<String> = Mutex::new(String::new());
static ON_CHANGE: OnceLock<fn()> = OnceLock::new();

/// Register a listener (the titlebar) for state changes. First caller wins.
pub fn set_on_change(f: fn()) {
    let _ = ON_CHANGE.set(f);
}

pub fn state() -> RedirectState {
    RedirectState::from_u8(STATE.load(Ordering::SeqCst))
}

pub fn reason() -> String {
    REASON
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default()
}

/// Proven rewrite of an official host onto LilyPad. Does not clear `Failed`
/// (a GameLib miss is the 第三方-dialog signal and must stay visible).
pub fn mark_live(why: &str) {
    if let Ok(mut g) = REASON.lock() {
        if g.is_empty() {
            *g = why.to_string();
        }
    }
    let prev = STATE.compare_exchange(PENDING, LIVE, Ordering::SeqCst, Ordering::SeqCst);
    match prev {
        Ok(_) => {
            crate::logging::line("STATUS", &format!("redirect LIVE ({why})"));
            notify_titlebar();
        }
        Err(FAILED) => {
            crate::logging::line(
                "STATUS",
                &format!("rewrite observed after FAIL ({why}); title stays HOOK FAIL"),
            );
        }
        Err(_) => {}
    }
}

/// GameLib went official, or the platform hook could not be armed.
/// Sticky: once failed, the title stays HOOK FAIL.
pub fn mark_failed(why: &str) {
    // Release REASON before logging. handle_crash logs first and then reads the
    // reason; holding both locks here is the opposite order and deadlocks the
    // crash dialog.
    if let Ok(mut g) = REASON.lock() {
        *g = why.to_string();
    }
    let prev = STATE.swap(FAILED, Ordering::SeqCst);
    if prev != FAILED {
        crate::logging::line("STATUS", &format!("redirect FAIL ({why})"));
        notify_titlebar();
    }
}

fn notify_titlebar() {
    if let Some(f) = ON_CHANGE.get() {
        f();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_pending() {
        // Fresh process: no mark_* has run. Other unit tests in this crate
        // don't call mark_*, so this is stable under `cargo test`.
        assert_eq!(state(), RedirectState::Pending);
    }
}
