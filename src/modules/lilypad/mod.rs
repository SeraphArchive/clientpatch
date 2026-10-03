//! Module #1 — redirect the client to a LilyPad private server.
pub mod keyconv;
pub mod report;
pub mod url;

// `platform` is host-compiled for its pure origin-rewrite helper + tests; its
// native PAYMENT_* hooks are gated inside the file. `api_redirect`/`signing` are
// entirely IL2CPP/retour and only exist on Windows.
pub mod platform;
#[cfg(windows)]
mod api_redirect;
#[cfg(windows)]
mod migcapture;
#[cfg(windows)]
mod signing;

use crate::module::{LoaderCtx, Module};

#[derive(Default)]
pub struct Lilypad;

impl Module for Lilypad {
    fn name(&self) -> &str {
        "lilypad"
    }

    #[cfg(windows)]
    fn init_early(&self, config: &crate::config::Config, dll_dir: &std::path::Path) -> anyhow::Result<()> {
        // GameLib PAYMENT_* must be hooked BEFORE GGLInitialize / PAYMENT_Initialize
        // (prologue, well before the IL2CPP settle). Missing this is the
        // 第三方服务器设置不正常 dialog. Arm the native hooks here, not in init().
        if let Some(lp) = config.lilypad.as_ref() {
            if let Err(e) = platform::apply(lp) {
                crate::logging::line("ERR", &format!("platform early hook failed: {e}"));
                crate::status::mark_failed(&format!("platform early hook: {e}"));
            }
        }
        report::init_early(config, dll_dir)
    }

    #[cfg(windows)]
    fn init(&self, ctx: &LoaderCtx) -> anyhow::Result<()> {
        let cfg = ctx
            .config
            .lilypad
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("[lilypad] config table missing"))?;
        crate::logging::line("INFO", &format!("lilypad init: api_base={}", cfg.api_base));

        api_redirect::apply(ctx.il2cpp, cfg)?;
        signing::apply(ctx.il2cpp, cfg, ctx.dll_dir)?;
        // platform::apply is armed in init_early (must beat PAYMENT_Initialize).
        // Re-run is idempotent and will attach if the wrapper loaded in the
        // meantime.
        platform::apply(cfg)?;

        if cfg.neutralize_debugger_flag {
            api_redirect::neutralize_debugger(ctx.il2cpp);
        }
        if cfg.capture_md5 {
            migcapture::apply(ctx.il2cpp)?;
        }
        Ok(())
    }

    #[cfg(not(windows))]
    fn init(&self, _ctx: &LoaderCtx) -> anyhow::Result<()> {
        Ok(()) // no-op on non-Windows hosts (keeps the registry host-compilable)
    }
}
