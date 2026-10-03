//! In-process steam_api64 stand-in. Covers the exports + vtable slots HBR
//! actually uses (GameLib ticket/init + Steamworks.NET CSteamAPIContext).
//! Never talks to steamclient.
#![cfg(windows)]
#![allow(non_snake_case)] // C SteamAPI_* names, held as fn pointers for GetProcAddress

use crate::logging;
use std::ffi::CStr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

const ERESULT_OK: i32 = 1;
const TICKET_HANDLE: u32 = 1;
const HUSER: i32 = 1;
const HPIPE: i32 = 1;

static INITED: AtomicBool = AtomicBool::new(false);
static INIT_COOKIE: AtomicU32 = AtomicU32::new(1);
static COUNTRY: Mutex<Option<&'static [u8]>> = Mutex::new(None);
static LANGUAGE: Mutex<Option<&'static [u8]>> = Mutex::new(None);

#[derive(Clone, Copy)]
struct Cb(*mut u8);
unsafe impl Send for Cb {}
unsafe impl Sync for Cb {}

static CALLBACKS: Mutex<Vec<(Cb, i32)>> = Mutex::new(Vec::new());
static UNKNOWN: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());

const TICKET: [u8; 64] = [0xA5; 64];

pub fn configure(ip_country: &str, ui_language: &str) {
    *COUNTRY.lock().unwrap_or_else(|e| e.into_inner()) =
        Some(leak_cstr(ip_country));
    *LANGUAGE.lock().unwrap_or_else(|e| e.into_inner()) =
        Some(leak_cstr(ui_language));
}

fn leak_cstr(s: &str) -> &'static [u8] {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    Box::leak(v.into_boxed_slice())
}

fn country() -> *const i8 {
    COUNTRY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .unwrap_or(b"JP\0")
        .as_ptr() as *const i8
}

fn language() -> *const i8 {
    LANGUAGE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .unwrap_or(b"japanese\0")
        .as_ptr() as *const i8
}

fn cstr<'a>(p: *const i8) -> &'a str {
    if p.is_null() {
        return "";
    }
    unsafe { CStr::from_ptr(p) }.to_str().unwrap_or("")
}

// --- lifecycle -----------------------------------------------------------

pub unsafe extern "C" fn SteamAPI_Init() -> u8 {
    INITED.store(true, Ordering::SeqCst);
    INIT_COOKIE.fetch_add(1, Ordering::SeqCst);
    logging::line("STEAM", "stub: SteamAPI_Init -> true");
    1
}

pub unsafe extern "C" fn SteamAPI_InitSafe() -> u8 {
    SteamAPI_Init()
}

pub unsafe extern "C" fn SteamAPI_Shutdown() {
    INITED.store(false, Ordering::SeqCst);
}

pub unsafe extern "C" fn SteamAPI_RestartAppIfNecessary(_appid: u32) -> u8 {
    0
}

pub unsafe extern "C" fn SteamAPI_IsSteamRunning() -> u8 {
    1
}

pub unsafe extern "C" fn SteamAPI_GetHSteamUser() -> i32 {
    HUSER
}

pub unsafe extern "C" fn SteamAPI_GetHSteamPipe() -> i32 {
    HPIPE
}

pub unsafe extern "C" fn SteamAPI_GetHSteamUserCurrent() -> i32 {
    HUSER
}

pub unsafe extern "C" fn SteamAPI_ReleaseCurrentThreadMemory() {}

pub unsafe extern "C" fn SteamAPI_SetWarningMessageHook(_f: *mut core::ffi::c_void) {}

pub unsafe extern "C" fn SteamAPI_RunCallbacks() {
    // Ticket callback is delivered synchronously from GetAuthSessionTicket.
}

// --- callbacks -----------------------------------------------------------

// CCallbackBase is an IL2CPP object here (dump.cs): object header, then
// m_vfptr at 0x10, m_nCallbackFlags at 0x18, m_iCallback at 0x1C. The native
// C layout (flags at +8, id at +12) lands inside the object header.
const CB_VFPTR: usize = 0x10;
const CB_FLAGS: usize = 0x18;
const CB_ID: usize = 0x1C;

pub unsafe extern "C" fn SteamAPI_RegisterCallback(cb: *mut u8, i_callback: i32) {
    if cb.is_null() {
        return;
    }
    *cb.add(CB_FLAGS) |= 1;
    (cb.add(CB_ID) as *mut i32).write_unaligned(i_callback);
    let mut g = CALLBACKS.lock().unwrap_or_else(|e| e.into_inner());
    if !g.iter().any(|(p, _)| p.0 == cb) {
        g.push((Cb(cb), i_callback));
    }
}

pub unsafe extern "C" fn SteamAPI_UnregisterCallback(cb: *mut u8) {
    if cb.is_null() {
        return;
    }
    *cb.add(CB_FLAGS) &= !1;
    CALLBACKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|(p, _)| p.0 != cb);
}

pub unsafe extern "C" fn SteamAPI_RegisterCallResult(_cb: *mut u8, _call: u64) {}

pub unsafe extern "C" fn SteamAPI_UnregisterCallResult(_cb: *mut u8, _call: u64) {}

unsafe fn fire(i_callback: i32, param: *mut core::ffi::c_void) {
    let list: Vec<(Cb, i32)> = CALLBACKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    for (p, id) in list {
        if id != i_callback || p.0.is_null() {
            continue;
        }
        // m_vfptr points at CCallbackBaseVTable, whose first field is
        // m_RunCallResult (4 args). The callback dispatcher is m_RunCallback,
        // the second slot. Slot 0 would call it with two garbage arguments.
        let vtable = *(p.0.add(CB_VFPTR) as *const *const usize);
        if vtable.is_null() {
            continue;
        }
        let run: unsafe extern "C" fn(*mut u8, *mut core::ffi::c_void) =
            core::mem::transmute(*vtable.add(1));
        run(p.0, param);
    }
}

// --- SteamInternal -------------------------------------------------------

pub unsafe extern "C" fn SteamInternal_CreateInterface(ver: *const i8) -> *mut core::ffi::c_void {
    iface_for_version(cstr(ver))
}

pub unsafe extern "C" fn SteamInternal_FindOrCreateUserInterface(
    _huser: i32,
    ver: *const i8,
) -> *mut core::ffi::c_void {
    iface_for_version(cstr(ver))
}

pub unsafe extern "C" fn SteamInternal_FindOrCreateGameServerInterface(
    _huser: i32,
    ver: *const i8,
) -> *mut core::ffi::c_void {
    iface_for_version(cstr(ver))
}

/// SDK cookie: `[init_fn, counter, cached_ptr]`. Returns `&cached_ptr`.
pub unsafe extern "C" fn SteamInternal_ContextInit(
    cookie: *mut usize,
) -> *mut core::ffi::c_void {
    if cookie.is_null() {
        return core::ptr::null_mut();
    }
    let want = INIT_COOKIE.load(Ordering::SeqCst) as usize;
    if *cookie.add(1) != want {
        let fn_ptr = *cookie;
        if fn_ptr != 0 {
            let f: unsafe extern "C" fn(*mut usize) = core::mem::transmute(fn_ptr);
            f(cookie.add(2));
        }
        *cookie.add(1) = want;
    }
    cookie.add(2) as *mut core::ffi::c_void
}

// --- interfaces ----------------------------------------------------------

fn leak_iface(methods: &[*const core::ffi::c_void]) -> *mut core::ffi::c_void {
    let vt = Box::leak(methods.to_vec().into_boxed_slice()).as_ptr();
    Box::leak(Box::new(vt)) as *mut _ as *mut core::ffi::c_void
}

unsafe extern "C" fn nop0() -> usize {
    0
}

fn nops(n: usize) -> Vec<*const core::ffi::c_void> {
    vec![nop0 as *const core::ffi::c_void; n]
}

fn iface_user() -> *mut core::ffi::c_void {
    static ONCE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *ONCE.get_or_init(|| {
        let mut m = nops(48);
        m[1] = user_blogged_on as *const core::ffi::c_void; // BLoggedOn
        m[2] = user_get_steamid as *const core::ffi::c_void;
        m[13] = user_get_auth_session_ticket as *const core::ffi::c_void;
        leak_iface(&m) as usize
    }) as *mut core::ffi::c_void
}

fn iface_apps() -> *mut core::ffi::c_void {
    static ONCE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *ONCE.get_or_init(|| {
        let mut m = nops(32);
        m[0] = ret_true as *const core::ffi::c_void; // BIsSubscribed
        m[4] = apps_language as *const core::ffi::c_void; // GetCurrentGameLanguage
        m[6] = ret_true as *const core::ffi::c_void; // BIsSubscribedApp
        leak_iface(&m) as usize
    }) as *mut core::ffi::c_void
}

fn iface_utils() -> *mut core::ffi::c_void {
    static ONCE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *ONCE.get_or_init(|| {
        let mut m = nops(40);
        m[4] = utils_ip_country as *const core::ffi::c_void;
        m[9] = utils_appid as *const core::ffi::c_void;
        m[16] = nop0 as *const core::ffi::c_void; // SetWarningMessageHook
        m[22] = utils_ui_language as *const core::ffi::c_void;
        leak_iface(&m) as usize
    }) as *mut core::ffi::c_void
}

fn iface_stats() -> *mut core::ffi::c_void {
    static ONCE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *ONCE.get_or_init(|| {
        let mut m = nops(32);
        m[0] = ret_true as *const core::ffi::c_void; // RequestCurrentStats
        m[6] = stats_get_achievement as *const core::ffi::c_void;
        m[7] = stats_set_achievement as *const core::ffi::c_void;
        m[10] = ret_true as *const core::ffi::c_void; // StoreStats
        leak_iface(&m) as usize
    }) as *mut core::ffi::c_void
}

fn iface_client() -> *mut core::ffi::c_void {
    static ONCE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *ONCE.get_or_init(|| {
        let mut m = nops(48);
        m[2] = client_connect_user as *const core::ffi::c_void;
        m[5] = client_get_user as *const core::ffi::c_void;
        m[8] = client_get_friends as *const core::ffi::c_void;
        m[9] = client_get_utils as *const core::ffi::c_void;
        m[10] = client_get_mm as *const core::ffi::c_void;
        m[11] = client_get_mms as *const core::ffi::c_void;
        m[12] = client_get_generic as *const core::ffi::c_void;
        m[13] = client_get_stats as *const core::ffi::c_void;
        m[15] = client_get_apps as *const core::ffi::c_void;
        leak_iface(&m) as usize
    }) as *mut core::ffi::c_void
}

fn iface_dummy() -> *mut core::ffi::c_void {
    static ONCE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *ONCE.get_or_init(|| leak_iface(&nops(64)) as usize) as *mut core::ffi::c_void
}

fn iface_for_version(ver: &str) -> *mut core::ffi::c_void {
    let v = ver.to_ascii_lowercase();
    if v.contains("client") {
        iface_client()
    } else if v.contains("userstats") {
        iface_stats()
    } else if v.contains("steamuser") && !v.contains("stats") {
        iface_user()
    } else if v.contains("utils") {
        iface_utils()
    } else if v.contains("apps") {
        iface_apps()
    } else {
        iface_dummy()
    }
}

unsafe extern "C" fn ret_true() -> u8 {
    1
}

unsafe extern "C" fn user_blogged_on(_this: *mut core::ffi::c_void) -> u8 {
    1
}

unsafe extern "C" fn user_get_steamid(_this: *mut core::ffi::c_void) -> u64 {
    // account-type individual, universe public, account 1
    0x0110_0001_0000_0001
}

unsafe extern "C" fn user_get_auth_session_ticket(
    _this: *mut core::ffi::c_void,
    p_ticket: *mut u8,
    cb_max: i32,
    pcb: *mut u32,
) -> u32 {
    if cb_max <= 0 {
        return 0;
    }
    let n = TICKET.len().min(cb_max as usize);
    if !p_ticket.is_null() {
        core::ptr::copy_nonoverlapping(TICKET.as_ptr(), p_ticket, n);
    }
    if !pcb.is_null() {
        *pcb = n as u32;
    }
    #[repr(C)]
    struct TicketResp {
        handle: u32,
        eresult: i32,
    }
    let mut resp = TicketResp {
        handle: TICKET_HANDLE,
        eresult: ERESULT_OK,
    };
    fire(163, &mut resp as *mut TicketResp as *mut core::ffi::c_void);
    logging::line(
        "STEAM",
        &format!("stub: GetAuthSessionTicket {n} bytes, callback 163"),
    );
    TICKET_HANDLE
}

unsafe extern "C" fn apps_language(_this: *mut core::ffi::c_void) -> *const i8 {
    language()
}

unsafe extern "C" fn utils_ip_country(_this: *mut core::ffi::c_void) -> *const i8 {
    country()
}

unsafe extern "C" fn utils_ui_language(_this: *mut core::ffi::c_void) -> *const i8 {
    language()
}

unsafe extern "C" fn utils_appid(_this: *mut core::ffi::c_void) -> u32 {
    // appmanifest_1973710.acf — ヘブンバーンズレッド. 1313860 was a different app.
    1973710
}

unsafe extern "C" fn stats_get_achievement(
    _this: *mut core::ffi::c_void,
    _name: *const i8,
    pb: *mut u8,
) -> u8 {
    if !pb.is_null() {
        *pb = 0;
    }
    1
}

unsafe extern "C" fn stats_set_achievement(_this: *mut core::ffi::c_void, _name: *const i8) -> u8 {
    1
}

unsafe extern "C" fn client_connect_user(_this: *mut core::ffi::c_void, _pipe: i32) -> i32 {
    HUSER
}

unsafe extern "C" fn client_get_user(
    _this: *mut core::ffi::c_void,
    _huser: i32,
    _hpipe: i32,
    ver: *const i8,
) -> *mut core::ffi::c_void {
    iface_for_version(cstr(ver))
}

unsafe extern "C" fn client_get_friends(
    _this: *mut core::ffi::c_void,
    _huser: i32,
    _hpipe: i32,
    ver: *const i8,
) -> *mut core::ffi::c_void {
    iface_for_version(cstr(ver))
}

unsafe extern "C" fn client_get_utils(
    _this: *mut core::ffi::c_void,
    _hpipe: i32,
    ver: *const i8,
) -> *mut core::ffi::c_void {
    iface_for_version(cstr(ver))
}

unsafe extern "C" fn client_get_mm(
    _this: *mut core::ffi::c_void,
    _huser: i32,
    _hpipe: i32,
    ver: *const i8,
) -> *mut core::ffi::c_void {
    iface_for_version(cstr(ver))
}

unsafe extern "C" fn client_get_mms(
    _this: *mut core::ffi::c_void,
    _huser: i32,
    _hpipe: i32,
    ver: *const i8,
) -> *mut core::ffi::c_void {
    iface_for_version(cstr(ver))
}

unsafe extern "C" fn client_get_generic(
    _this: *mut core::ffi::c_void,
    _huser: i32,
    _hpipe: i32,
    ver: *const i8,
) -> *mut core::ffi::c_void {
    iface_for_version(cstr(ver))
}

unsafe extern "C" fn client_get_stats(
    _this: *mut core::ffi::c_void,
    _huser: i32,
    _hpipe: i32,
    ver: *const i8,
) -> *mut core::ffi::c_void {
    iface_for_version(cstr(ver))
}

unsafe extern "C" fn client_get_apps(
    _this: *mut core::ffi::c_void,
    _huser: i32,
    _hpipe: i32,
    ver: *const i8,
) -> *mut core::ffi::c_void {
    iface_for_version(cstr(ver))
}

// --- flat exports C# Steamworks.NET binds --------------------------------

macro_rules! client_get4 {
    ($fn_name:ident) => {
        pub unsafe extern "C" fn $fn_name(
            this: *mut core::ffi::c_void,
            huser: i32,
            hpipe: i32,
            ver: *const i8,
        ) -> *mut core::ffi::c_void {
            client_get_generic(this, huser, hpipe, ver)
        }
    };
}

client_get4!(SteamAPI_ISteamClient_GetISteamUser);
client_get4!(SteamAPI_ISteamClient_GetISteamFriends);
client_get4!(SteamAPI_ISteamClient_GetISteamMatchmaking);
client_get4!(SteamAPI_ISteamClient_GetISteamMatchmakingServers);
client_get4!(SteamAPI_ISteamClient_GetISteamGenericInterface);
client_get4!(SteamAPI_ISteamClient_GetISteamUserStats);
client_get4!(SteamAPI_ISteamClient_GetISteamGameServerStats);
client_get4!(SteamAPI_ISteamClient_GetISteamApps);
client_get4!(SteamAPI_ISteamClient_GetISteamNetworking);
client_get4!(SteamAPI_ISteamClient_GetISteamRemoteStorage);
client_get4!(SteamAPI_ISteamClient_GetISteamScreenshots);
client_get4!(SteamAPI_ISteamClient_GetISteamGameSearch);
client_get4!(SteamAPI_ISteamClient_GetISteamHTTP);
client_get4!(SteamAPI_ISteamClient_GetISteamController);
client_get4!(SteamAPI_ISteamClient_GetISteamUGC);
client_get4!(SteamAPI_ISteamClient_GetISteamAppList);
client_get4!(SteamAPI_ISteamClient_GetISteamMusic);
client_get4!(SteamAPI_ISteamClient_GetISteamMusicRemote);
client_get4!(SteamAPI_ISteamClient_GetISteamHTMLSurface);
client_get4!(SteamAPI_ISteamClient_GetISteamInventory);
client_get4!(SteamAPI_ISteamClient_GetISteamVideo);
client_get4!(SteamAPI_ISteamClient_GetISteamParentalSettings);
client_get4!(SteamAPI_ISteamClient_GetISteamInput);
client_get4!(SteamAPI_ISteamClient_GetISteamParties);
client_get4!(SteamAPI_ISteamClient_GetISteamRemotePlay);
client_get4!(SteamAPI_ISteamClient_GetISteamGameServer);

pub unsafe extern "C" fn SteamAPI_ISteamClient_GetISteamUtils(
    this: *mut core::ffi::c_void,
    hpipe: i32,
    ver: *const i8,
) -> *mut core::ffi::c_void {
    client_get_utils(this, hpipe, ver)
}

pub unsafe extern "C" fn SteamAPI_ISteamClient_ConnectToGlobalUser(
    this: *mut core::ffi::c_void,
    pipe: i32,
) -> i32 {
    client_connect_user(this, pipe)
}

pub unsafe extern "C" fn SteamAPI_ISteamUtils_GetIPCountry(
    this: *mut core::ffi::c_void,
) -> *const i8 {
    utils_ip_country(this)
}

pub unsafe extern "C" fn SteamAPI_ISteamUtils_GetSteamUILanguage(
    this: *mut core::ffi::c_void,
) -> *const i8 {
    utils_ui_language(this)
}

pub unsafe extern "C" fn SteamAPI_ISteamUtils_GetAppID(this: *mut core::ffi::c_void) -> u32 {
    utils_appid(this)
}

pub unsafe extern "C" fn SteamAPI_ISteamUtils_SetWarningMessageHook(
    _this: *mut core::ffi::c_void,
    _f: *mut core::ffi::c_void,
) {
}

pub unsafe extern "C" fn SteamAPI_ISteamApps_GetCurrentGameLanguage(
    this: *mut core::ffi::c_void,
) -> *const i8 {
    apps_language(this)
}

pub unsafe extern "C" fn SteamAPI_ISteamApps_BIsSubscribed(_this: *mut core::ffi::c_void) -> u8 {
    1
}

pub unsafe extern "C" fn SteamAPI_ISteamUser_BLoggedOn(this: *mut core::ffi::c_void) -> u8 {
    user_blogged_on(this)
}

pub unsafe extern "C" fn SteamAPI_ISteamUser_GetSteamID(this: *mut core::ffi::c_void) -> u64 {
    user_get_steamid(this)
}

pub unsafe extern "C" fn SteamAPI_ISteamUser_GetAuthSessionTicket(
    this: *mut core::ffi::c_void,
    p_ticket: *mut u8,
    cb_max: i32,
    pcb: *mut u32,
) -> u32 {
    user_get_auth_session_ticket(this, p_ticket, cb_max, pcb)
}

pub unsafe extern "C" fn SteamAPI_ISteamUserStats_GetAchievement(
    this: *mut core::ffi::c_void,
    name: *const i8,
    pb: *mut u8,
) -> u8 {
    stats_get_achievement(this, name, pb)
}

pub unsafe extern "C" fn SteamAPI_ISteamUserStats_SetAchievement(
    this: *mut core::ffi::c_void,
    name: *const i8,
) -> u8 {
    stats_set_achievement(this, name)
}

pub unsafe extern "C" fn SteamAPI_ISteamUserStats_StoreStats(_this: *mut core::ffi::c_void) -> u8 {
    1
}

pub unsafe extern "C" fn SteamAPI_ISteamUserStats_RequestCurrentStats(
    _this: *mut core::ffi::c_void,
) -> u8 {
    1
}

/// Look up a stub function by export name for the GetProcAddress hook.
pub fn resolve(name: &[u8]) -> Option<*const core::ffi::c_void> {
    Some(match name {
        b"SteamAPI_Init" => SteamAPI_Init as _,
        b"SteamAPI_InitSafe" => SteamAPI_InitSafe as _,
        b"SteamAPI_Shutdown" => SteamAPI_Shutdown as _,
        b"SteamAPI_RestartAppIfNecessary" => SteamAPI_RestartAppIfNecessary as _,
        b"SteamAPI_IsSteamRunning" => SteamAPI_IsSteamRunning as _,
        b"SteamAPI_GetHSteamUser" => SteamAPI_GetHSteamUser as _,
        b"SteamAPI_GetHSteamPipe" => SteamAPI_GetHSteamPipe as _,
        b"SteamAPI_GetHSteamUserCurrent" => SteamAPI_GetHSteamUserCurrent as _,
        b"SteamAPI_ReleaseCurrentThreadMemory" => SteamAPI_ReleaseCurrentThreadMemory as _,
        b"SteamAPI_SetWarningMessageHook" => SteamAPI_SetWarningMessageHook as _,
        b"SteamAPI_RunCallbacks" => SteamAPI_RunCallbacks as _,
        b"SteamAPI_RegisterCallback" => SteamAPI_RegisterCallback as _,
        b"SteamAPI_UnregisterCallback" => SteamAPI_UnregisterCallback as _,
        b"SteamAPI_RegisterCallResult" => SteamAPI_RegisterCallResult as _,
        b"SteamAPI_UnregisterCallResult" => SteamAPI_UnregisterCallResult as _,
        b"SteamInternal_CreateInterface" => SteamInternal_CreateInterface as _,
        b"SteamInternal_FindOrCreateUserInterface" => SteamInternal_FindOrCreateUserInterface as _,
        b"SteamInternal_FindOrCreateGameServerInterface" => {
            SteamInternal_FindOrCreateGameServerInterface as _
        }
        b"SteamInternal_ContextInit" => SteamInternal_ContextInit as _,
        b"SteamAPI_ISteamClient_GetISteamUser" => SteamAPI_ISteamClient_GetISteamUser as _,
        b"SteamAPI_ISteamClient_GetISteamFriends" => SteamAPI_ISteamClient_GetISteamFriends as _,
        b"SteamAPI_ISteamClient_GetISteamUtils" => SteamAPI_ISteamClient_GetISteamUtils as _,
        b"SteamAPI_ISteamClient_GetISteamMatchmaking" => {
            SteamAPI_ISteamClient_GetISteamMatchmaking as _
        }
        b"SteamAPI_ISteamClient_GetISteamMatchmakingServers" => {
            SteamAPI_ISteamClient_GetISteamMatchmakingServers as _
        }
        b"SteamAPI_ISteamClient_GetISteamGenericInterface" => {
            SteamAPI_ISteamClient_GetISteamGenericInterface as _
        }
        b"SteamAPI_ISteamClient_GetISteamUserStats" => {
            SteamAPI_ISteamClient_GetISteamUserStats as _
        }
        b"SteamAPI_ISteamClient_GetISteamGameServerStats" => {
            SteamAPI_ISteamClient_GetISteamGameServerStats as _
        }
        b"SteamAPI_ISteamClient_GetISteamApps" => SteamAPI_ISteamClient_GetISteamApps as _,
        b"SteamAPI_ISteamClient_GetISteamNetworking" => {
            SteamAPI_ISteamClient_GetISteamNetworking as _
        }
        b"SteamAPI_ISteamClient_GetISteamRemoteStorage" => {
            SteamAPI_ISteamClient_GetISteamRemoteStorage as _
        }
        b"SteamAPI_ISteamClient_GetISteamScreenshots" => {
            SteamAPI_ISteamClient_GetISteamScreenshots as _
        }
        b"SteamAPI_ISteamClient_GetISteamGameSearch" => {
            SteamAPI_ISteamClient_GetISteamGameSearch as _
        }
        b"SteamAPI_ISteamClient_GetISteamHTTP" => SteamAPI_ISteamClient_GetISteamHTTP as _,
        b"SteamAPI_ISteamClient_GetISteamController" => {
            SteamAPI_ISteamClient_GetISteamController as _
        }
        b"SteamAPI_ISteamClient_GetISteamUGC" => SteamAPI_ISteamClient_GetISteamUGC as _,
        b"SteamAPI_ISteamClient_GetISteamAppList" => SteamAPI_ISteamClient_GetISteamAppList as _,
        b"SteamAPI_ISteamClient_GetISteamMusic" => SteamAPI_ISteamClient_GetISteamMusic as _,
        b"SteamAPI_ISteamClient_GetISteamMusicRemote" => {
            SteamAPI_ISteamClient_GetISteamMusicRemote as _
        }
        b"SteamAPI_ISteamClient_GetISteamHTMLSurface" => {
            SteamAPI_ISteamClient_GetISteamHTMLSurface as _
        }
        b"SteamAPI_ISteamClient_GetISteamInventory" => {
            SteamAPI_ISteamClient_GetISteamInventory as _
        }
        b"SteamAPI_ISteamClient_GetISteamVideo" => SteamAPI_ISteamClient_GetISteamVideo as _,
        b"SteamAPI_ISteamClient_GetISteamParentalSettings" => {
            SteamAPI_ISteamClient_GetISteamParentalSettings as _
        }
        b"SteamAPI_ISteamClient_GetISteamInput" => SteamAPI_ISteamClient_GetISteamInput as _,
        b"SteamAPI_ISteamClient_GetISteamParties" => SteamAPI_ISteamClient_GetISteamParties as _,
        b"SteamAPI_ISteamClient_GetISteamRemotePlay" => {
            SteamAPI_ISteamClient_GetISteamRemotePlay as _
        }
        b"SteamAPI_ISteamClient_GetISteamGameServer" => {
            SteamAPI_ISteamClient_GetISteamGameServer as _
        }
        b"SteamAPI_ISteamClient_ConnectToGlobalUser" => {
            SteamAPI_ISteamClient_ConnectToGlobalUser as _
        }
        b"SteamAPI_ISteamUtils_GetIPCountry" => SteamAPI_ISteamUtils_GetIPCountry as _,
        b"SteamAPI_ISteamUtils_GetSteamUILanguage" => SteamAPI_ISteamUtils_GetSteamUILanguage as _,
        b"SteamAPI_ISteamUtils_GetAppID" => SteamAPI_ISteamUtils_GetAppID as _,
        b"SteamAPI_ISteamUtils_SetWarningMessageHook" => {
            SteamAPI_ISteamUtils_SetWarningMessageHook as _
        }
        b"SteamAPI_ISteamApps_GetCurrentGameLanguage" => {
            SteamAPI_ISteamApps_GetCurrentGameLanguage as _
        }
        b"SteamAPI_ISteamApps_BIsSubscribed" => SteamAPI_ISteamApps_BIsSubscribed as _,
        b"SteamAPI_ISteamUser_BLoggedOn" => SteamAPI_ISteamUser_BLoggedOn as _,
        b"SteamAPI_ISteamUser_GetSteamID" => SteamAPI_ISteamUser_GetSteamID as _,
        b"SteamAPI_ISteamUser_GetAuthSessionTicket" => {
            SteamAPI_ISteamUser_GetAuthSessionTicket as _
        }
        b"SteamAPI_ISteamUserStats_GetAchievement" => SteamAPI_ISteamUserStats_GetAchievement as _,
        b"SteamAPI_ISteamUserStats_SetAchievement" => SteamAPI_ISteamUserStats_SetAchievement as _,
        b"SteamAPI_ISteamUserStats_StoreStats" => SteamAPI_ISteamUserStats_StoreStats as _,
        b"SteamAPI_ISteamUserStats_RequestCurrentStats" => {
            SteamAPI_ISteamUserStats_RequestCurrentStats as _
        }
        other => {
            if other.starts_with(b"SteamAPI") || other.starts_with(b"SteamInternal") {
                let mut u = UNKNOWN.lock().unwrap_or_else(|e| e.into_inner());
                if !u.iter().any(|n| n == other) {
                    u.push(other.to_vec());
                    logging::line(
                        "WARN",
                        &format!(
                            "steam stub: unhandled export {} -> nop",
                            String::from_utf8_lossy(other)
                        ),
                    );
                }
                return Some(nop0 as _);
            }
            return None;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::iface_for_version;

    #[test]
    fn routes_known_versions() {
        let u = iface_for_version("SteamUser020");
        let a = iface_for_version("STEAMAPPS_INTERFACE_VERSION008");
        let t = iface_for_version("SteamUtils009");
        let s = iface_for_version("STEAMUSERSTATS_INTERFACE_VERSION011");
        let c = iface_for_version("SteamClient019");
        let d = iface_for_version("SteamFriends017");
        assert!(!u.is_null() && !a.is_null() && !t.is_null() && !s.is_null() && !c.is_null());
        assert!(!d.is_null());
        assert_ne!(u, a);
        assert_ne!(u, t);
        assert_eq!(iface_for_version("SteamUser020"), u);
    }

    #[test]
    fn resolve_covers_gamelib_static_iat() {
        // cpp_gamelib_steam.dll IAT → steam_api64.dll (not delay-load).
        for name in [
            &b"SteamInternal_ContextInit"[..],
            b"SteamInternal_FindOrCreateUserInterface",
            b"SteamAPI_Init",
            b"SteamAPI_RestartAppIfNecessary",
            b"SteamAPI_RegisterCallback",
            b"SteamAPI_UnregisterCallback",
            b"SteamAPI_RunCallbacks",
            b"SteamAPI_Shutdown",
            b"SteamAPI_GetHSteamUser",
        ] {
            assert!(
                super::resolve(name).is_some(),
                "missing stub for {}",
                String::from_utf8_lossy(name)
            );
        }
    }
}
