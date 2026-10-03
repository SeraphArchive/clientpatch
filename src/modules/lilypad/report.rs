//! Module #1a — report the installed build's version to LilyPad at game launch.
//!
//! The client's `/api/app/start` response must echo the asset version/hash of the
//! *installed* build, or the client fetches a CDN catalog and fails to connect
//! (the launch "black screen"). LilyPad keeps these in config, which must be
//! hand-edited every game patch. Instead, clientpatch reads the install's own
//! version (from the Addressables settings.json), reports it once at launch via
//! a small WinHTTP POST, and tags the game's own requests with `x-clientpatch-id`
//! so the server can echo the right version back per client.
//!
//! Best-effort and non-fatal: a failed report only means the server falls back
//! to its config/global version. Runs on the init thread (never the game's main
//! thread), before the IL2CPP runtime is bound.
use std::path::Path;

use crate::config::Config;
use crate::logging;

#[cfg(windows)]
use crate::wide;
#[cfg(windows)]
use std::sync::OnceLock;

/// The installation ID generated/loaded this launch; injected into game requests as
/// `x-clientpatch-id` by the api_redirect hook.
#[cfg(windows)]
static CLIENT_ID: OnceLock<String> = OnceLock::new();

/// A process identity, deliberately never persisted across launches.
#[cfg(windows)]
pub fn session_id() -> &'static str {
    static SESSION_ID: OnceLock<String> = OnceLock::new();
    SESSION_ID.get_or_init(generate_client_id).as_str()
}

/// The installed build's version triple, parsed from `aa/settings.json`.
struct AssetVersion {
    program_version: String,
    asset_version: String,
    asset_hash: String,
}

/// The client ID injected into game requests, when the report module is active.
#[cfg(windows)]
pub fn client_id() -> Option<&'static str> {
    CLIENT_ID.get().map(String::as_str)
}

/// Report the installed build at launch. Returns Ok even when the report fails
/// (it is best-effort); the failure is logged.
#[cfg(windows)]
pub fn init_early(cfg: &Config, dll_dir: &Path) -> anyhow::Result<()> {
    if !cfg.version_report_enabled() {
        return Ok(());
    }
    let Some(lilypad) = cfg.lilypad.as_ref() else {
        return Ok(()); // no [lilypad] -> nothing to report against
    };

    // Always establish the client ID (header injection still works even if the
    // report itself fails — the server just falls back to global/config).
    let id = load_or_create_client_id(dll_dir);
    let _ = CLIENT_ID.set(id.clone());

    let version = match read_asset_version(dll_dir) {
        Ok(v) => v,
        Err(e) => {
            logging::line("WARN", &format!("report: cannot read installed version: {e}"));
            return Ok(());
        }
    };

    let url = if lilypad.report.url.trim().is_empty() {
        format!("{}/api/clientpatch/version", super::url::origin(&lilypad.api_base))
    } else {
        lilypad.report.url.trim().to_string()
    };

    // POST on a detached thread: the early pass must return fast so the other
    // early hooks (regredirect, titlebar) install before the game first touches
    // their subsystems. The report only needs to land before /api/app/start,
    // which is many seconds later. logging::line is thread-safe.
    std::thread::spawn(move || {
        match post_report(&url, &id, &version) {
            Ok(result) => logging::line(
                "INFO",
                &format!(
                    "reported version {} (hash {}) to {url} -> {result}",
                    version.asset_version, version.asset_hash
                ),
            ),
            Err(e) => logging::line("WARN", &format!("report POST to {url} failed: {e}")),
        }
    });
    Ok(())
}

/// Read the install's `aa/settings.json` and extract program/asset version +
/// asset hash from the remote catalog location.
fn read_asset_version(dll_dir: &Path) -> anyhow::Result<AssetVersion> {
    let path = dll_dir.join("HeavenBurnsRed_Data/StreamingAssets/aa/settings.json");
    let s = std::fs::read_to_string(&path)?;
    let sj: SettingsJson = serde_json::from_str(&s)?;
    let internal = sj
        .catalog_locations
        .iter()
        .find(|c| c.keys.iter().any(|k| k == "AddressablesMainContentCatalogRemoteHash"))
        .map(|c| c.internal_id.as_str())
        .ok_or_else(|| anyhow::anyhow!("settings.json has no remote catalog location"))?;
    let version = extract_version(internal)?;
    let hash = extract_hash(internal)?;
    Ok(AssetVersion {
        program_version: version.clone(),
        asset_version: version,
        asset_hash: hash,
    })
}

#[derive(serde::Deserialize)]
struct SettingsJson {
    #[serde(rename = "m_CatalogLocations")]
    catalog_locations: Vec<CatalogLocation>,
}

#[derive(serde::Deserialize)]
struct CatalogLocation {
    #[serde(rename = "m_Keys")]
    keys: Vec<String>,
    #[serde(rename = "m_InternalId")]
    internal_id: String,
}

/// Extract the `<version>` segment from an `m_InternalId` of the form
/// `https://assets/<version>/StandaloneWindows64/catalog_<sha1>.hash`.
fn extract_version(internal: &str) -> anyhow::Result<String> {
    let rest = internal
        .split("assets/")
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("m_InternalId has no assets/ segment"))?;
    let v = rest
        .split('/')
        .next()
        .ok_or_else(|| anyhow::anyhow!("m_InternalId has empty version"))?;
    if v.is_empty() {
        anyhow::bail!("m_InternalId has empty version");
    }
    Ok(v.to_string())
}

/// Extract the 40-hex `catalog_<sha1>` digest from an `m_InternalId`.
fn extract_hash(internal: &str) -> anyhow::Result<String> {
    let after = internal
        .split("catalog_")
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("m_InternalId has no catalog_ segment"))?;
    let h = after.split(".hash").next().unwrap_or(after);
    if h.len() != 40 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!("m_InternalId has malformed catalog hash {h:?}");
    }
    Ok(h.to_string())
}

/// Load the persisted client ID, or generate one and persist it (best-effort).
#[cfg(windows)]
fn load_or_create_client_id(dll_dir: &Path) -> String {
    let path = crate::module::resolve_data_path(dll_dir, Path::new("clientpatch.id"));
    if let Ok(s) = std::fs::read_to_string(&path) {
        let s = s.trim();
        if !s.is_empty() {
            return s.to_string();
        }
    }
    let id = generate_client_id();
    let _ = std::fs::write(&path, &id); // non-fatal if the dir is read-only
    id
}

/// Generate a stable-enough per-install ID (32 hex chars). Not cryptographic —
/// it is only a correlation key for a private server, not a secret.
#[cfg(windows)]
fn generate_client_id() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let ctr = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:016x}{:08x}{:08x}", nanos & 0xffff_ffff_ffff_ffff, pid, ctr)
}

// --- the report POST (WinHTTP, schannel handles HTTPS) ---

#[cfg(windows)]
fn post_report(url: &str, client_id: &str, version: &AssetVersion) -> anyhow::Result<String> {
    use std::ffi::c_void;
    use windows_sys::Win32::Networking::WinHttp::{
        WinHttpAddRequestHeaders, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen,
        WinHttpOpenRequest, WinHttpQueryDataAvailable, WinHttpQueryHeaders, WinHttpReadData,
        WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
        WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, WINHTTP_ADDREQ_FLAG_ADD, WINHTTP_FLAG_SECURE,
        WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
    };

    let parsed = parse_url(url)?;
    let body = serde_json::json!({
        "clientId": client_id,
        "programVersion": version.program_version,
        "assetVersion": version.asset_version,
        "assetHash": version.asset_hash,
    })
    .to_string();

    let agent = wide::to_wide_nul("clientpatch/0.1");
    let host = wide::to_wide_nul(&parsed.host);
    let path = wide::to_wide_nul(&parsed.path);
    let verb = wide::to_wide_nul("POST");
    let content_type = wide::to_wide_nul("Content-Type: application/json\r\n");

    unsafe {
        let session = WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_DEFAULT_PROXY,
            std::ptr::null(),
            std::ptr::null(),
            0,
        );
        if session.is_null() {
            anyhow::bail!("WinHttpOpen failed");
        }
        let conn = WinHttpConnect(session, host.as_ptr(), parsed.port, 0);
        if conn.is_null() {
            WinHttpCloseHandle(session);
            anyhow::bail!("WinHttpConnect failed");
        }
        let flags = if parsed.secure { WINHTTP_FLAG_SECURE } else { 0 };
        let req = WinHttpOpenRequest(
            conn,
            verb.as_ptr(),
            path.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            flags,
        );
        if req.is_null() {
            WinHttpCloseHandle(conn);
            WinHttpCloseHandle(session);
            anyhow::bail!("WinHttpOpenRequest failed");
        }

        // RAII so any later `bail!` still closes the three handles. The pointers
        // are Copy, so `req`/`conn`/`session` remain usable after this.
        struct Guard {
            req: *mut c_void,
            conn: *mut c_void,
            session: *mut c_void,
        }
        impl Drop for Guard {
            fn drop(&mut self) {
                unsafe {
                    WinHttpCloseHandle(self.req);
                    WinHttpCloseHandle(self.conn);
                    WinHttpCloseHandle(self.session);
                }
            }
        }
        let _guard = Guard { req, conn, session };

        // Bound the whole exchange; this runs on the init thread, not the game's.
        let _ = WinHttpSetTimeouts(req, 3000, 3000, 3000, 3000);
        let _ = WinHttpAddRequestHeaders(
            req,
            content_type.as_ptr(),
            0xFFFF_FFFF,
            WINHTTP_ADDREQ_FLAG_ADD,
        );

        let body_bytes = body.as_bytes();
        if WinHttpSendRequest(
            req,
            std::ptr::null(),
            0,
            body_bytes.as_ptr() as *const c_void,
            body_bytes.len() as u32,
            body_bytes.len() as u32,
            0,
        ) == 0
        {
            anyhow::bail!("WinHttpSendRequest failed");
        }
        if WinHttpReceiveResponse(req, std::ptr::null_mut()) == 0 {
            anyhow::bail!("WinHttpReceiveResponse failed");
        }

        let mut status: u32 = 0;
        let mut status_len = std::mem::size_of::<u32>() as u32;
        if WinHttpQueryHeaders(
            req,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            std::ptr::null(),
            &mut status as *mut u32 as *mut c_void,
            &mut status_len,
            std::ptr::null_mut(),
        ) == 0
        {
            anyhow::bail!("WinHttpQueryHeaders failed");
        }

        // Drain the (tiny) response body best-effort, bounded by 64 KiB.
        let mut buf = Vec::new();
        loop {
            let mut avail: u32 = 0;
            if WinHttpQueryDataAvailable(req, &mut avail) == 0 || avail == 0 {
                break;
            }
            let mut chunk = vec![0u8; avail as usize];
            let mut read: u32 = 0;
            if WinHttpReadData(req, chunk.as_mut_ptr() as *mut c_void, avail, &mut read) == 0
                || read == 0
            {
                break;
            }
            buf.extend_from_slice(&chunk[..read as usize]);
            if buf.len() >= 64 * 1024 {
                break;
            }
        }

        let resp = String::from_utf8_lossy(&buf).trim().to_string();
        Ok(if resp.is_empty() {
            format!("HTTP {status}")
        } else {
            format!("HTTP {status} {resp}")
        })
    }
}

/// A split-out HTTP URL (host, port, path), independent of any HTTP library so
/// it stays unit-testable.
struct ParsedUrl {
    secure: bool,
    host: String,
    port: u16,
    path: String,
}

fn parse_url(url: &str) -> anyhow::Result<ParsedUrl> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| anyhow::anyhow!("url has no scheme"))?;
    let secure = match scheme {
        "https" => true,
        "http" => false,
        other => anyhow::bail!("unsupported scheme {other:?}"),
    };
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (
            h,
            p.parse::<u16>().map_err(|_| anyhow::anyhow!("bad port"))?,
        ),
        None => (hostport, if secure { 443 } else { 80 }),
    };
    if host.is_empty() {
        anyhow::bail!("url has empty host");
    }
    Ok(ParsedUrl {
        secure,
        host: host.to_string(),
        port,
        path: path.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn process_session_is_stable_and_distinct_from_new_ids() {
        let first = session_id();
        assert_eq!(first, session_id());
        assert_eq!(first.len(), 32);
        assert_ne!(first, generate_client_id());
    }

    #[test]
    fn extracts_version_and_hash_from_internal_id() {
        let id = "https://assets/6.9.0/StandaloneWindows64/catalog_ee37066d49cb385a73f2dea8e39069e3c55219a6.hash";
        assert_eq!(extract_version(id).unwrap(), "6.9.0");
        assert_eq!(
            extract_hash(id).unwrap(),
            "ee37066d49cb385a73f2dea8e39069e3c55219a6"
        );
    }

    #[test]
    fn rejects_malformed_internal_id() {
        assert!(extract_version("https://assets//x").is_err());
        assert!(extract_hash("https://assets/6.9.0/catalog_short.hash").is_err());
        assert!(extract_hash("https://assets/6.9.0/no_catalog_segment").is_err());
    }

    #[test]
    fn parses_http_and_https_urls() {
        let p = parse_url("http://127.0.0.1:8443/api/clientpatch/version").unwrap();
        assert!(!p.secure);
        assert_eq!(p.host, "127.0.0.1");
        assert_eq!(p.port, 8443);
        assert_eq!(p.path, "/api/clientpatch/version");

        let p = parse_url("https://lily.example.com/api/x").unwrap();
        assert!(p.secure);
        assert_eq!(p.port, 443);
        assert_eq!(p.host, "lily.example.com");
    }

    #[test]
    fn rejects_bad_scheme() {
        assert!(parse_url("ftp://host/x").is_err());
        assert!(parse_url("no-scheme").is_err());
    }

    #[test]
    fn parses_settings_fixture() {
        let sj: SettingsJson = serde_json::from_str(
            r#"{"m_CatalogLocations":[
                {"m_Keys":["AddressablesMainContentCatalogRemoteHash"],
                 "m_InternalId":"https://assets/6.9.0/StandaloneWindows64/catalog_ee37066d49cb385a73f2dea8e39069e3c55219a6.hash"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(sj.catalog_locations.len(), 1);
        assert_eq!(
            sj.catalog_locations[0].keys[0],
            "AddressablesMainContentCatalogRemoteHash"
        );
    }
}
