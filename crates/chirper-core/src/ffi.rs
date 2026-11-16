//! Raw C ABI of the Go core (`go/core/capi`, header `chirper.h`) behind the [`Backend`] trait.
//! Every string returned by Go is copied into a Rust `String` and freed with `chirper_free`.

use std::ffi::{CStr, c_char, c_void};

#[cfg(feature = "dynamic-link")]
use crate::{CoreError, Result};

/// Event callback of the core. `kind` and `payload` are NUL-terminated and valid only during the call.
pub type EventCallback =
    extern "C" fn(ctx: *mut c_void, kind: *const c_char, payload: *const c_char);

/// The five functions of the core, abstracted so that tests can replace the Go library.
pub trait Backend: Send + Sync {
    /// Opens the core. An empty `key` means no at-rest encryption, otherwise it is 32 bytes.
    fn open(
        &self,
        data_dir: &CStr,
        key: &[u8],
        cb: EventCallback,
        ctx: *mut c_void,
    ) -> std::result::Result<i64, String>;
    /// Calls a facade method and returns the JSON envelope `{"ok":..}`.
    fn call(&self, handle: i64, method: &CStr, args: &CStr) -> String;
    fn close(&self, handle: i64);
    fn version(&self) -> String;
}

#[cfg(any(feature = "static-link", feature = "dynamic-link"))]
type OpenFn = unsafe extern "C" fn(
    *const c_char,
    *const u8,
    i32,
    EventCallback,
    *mut c_void,
    *mut *mut c_char,
) -> i64;
#[cfg(any(feature = "static-link", feature = "dynamic-link"))]
type CallFn = unsafe extern "C" fn(i64, *const c_char, *const c_char) -> *mut c_char;
#[cfg(any(feature = "static-link", feature = "dynamic-link"))]
type FreeFn = unsafe extern "C" fn(*mut c_char);
#[cfg(any(feature = "static-link", feature = "dynamic-link"))]
type CloseFn = unsafe extern "C" fn(i64);
#[cfg(any(feature = "static-link", feature = "dynamic-link"))]
type VersionFn = unsafe extern "C" fn() -> *const c_char;

/// Resolved entry points of the core.
#[cfg(any(feature = "static-link", feature = "dynamic-link"))]
#[derive(Clone, Copy)]
struct Api {
    open: OpenFn,
    call: CallFn,
    free: FreeFn,
    close: CloseFn,
    version: VersionFn,
}

#[cfg(any(feature = "static-link", feature = "dynamic-link"))]
/// Copies a Go-allocated string and frees it. A null pointer yields an empty string.
///
/// # Safety
/// `p` must be null or a NUL-terminated string allocated by the core, and `free` must be the
/// matching `chirper_free`.
unsafe fn take_go_string(p: *mut c_char, free: FreeFn) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: the caller guarantees `p` is a valid NUL-terminated string from the core.
    let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    // SAFETY: `p` was allocated by the core and is freed exactly once here.
    unsafe { free(p) };
    s
}

#[cfg(any(feature = "static-link", feature = "dynamic-link"))]
impl Api {
    fn open(
        &self,
        data_dir: &CStr,
        key: &[u8],
        cb: EventCallback,
        ctx: *mut c_void,
    ) -> std::result::Result<i64, String> {
        let key_ptr = if key.is_empty() {
            std::ptr::null()
        } else {
            key.as_ptr()
        };
        let key_len = i32::try_from(key.len()).map_err(|_| "key too long".to_owned())?;
        let mut err: *mut c_char = std::ptr::null_mut();
        // SAFETY: all pointers are valid for the duration of the call; `key` is non-empty only
        // when its length is 32 bytes, as the caller guarantees.
        let handle = unsafe { (self.open)(data_dir.as_ptr(), key_ptr, key_len, cb, ctx, &mut err) };
        if handle > 0 {
            return Ok(handle);
        }
        // SAFETY: on failure the core sets `err` to a message it allocated, or leaves it null.
        let message = unsafe { take_go_string(err, self.free) };
        Err(if message.is_empty() {
            "open failed".to_owned()
        } else {
            message
        })
    }

    fn call(&self, handle: i64, method: &CStr, args: &CStr) -> String {
        // SAFETY: the CStr arguments are valid NUL-terminated strings for the call.
        let envelope = unsafe { (self.call)(handle, method.as_ptr(), args.as_ptr()) };
        // SAFETY: `envelope` is the string returned by chirper_call and is freed here.
        unsafe { take_go_string(envelope, self.free) }
    }

    fn close(&self, handle: i64) {
        // SAFETY: the core accepts any handle value and ignores unknown ones.
        unsafe { (self.close)(handle) }
    }

    fn version(&self) -> String {
        // SAFETY: chirper_version returns a static NUL-terminated string that is never freed.
        let p = unsafe { (self.version)() };
        if p.is_null() {
            return String::new();
        }
        // SAFETY: as above, `p` is a valid static C string.
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

/// The core linked into the binary (feature `static-link`, macOS and iOS).
#[cfg(feature = "static-link")]
pub struct StaticBackend {
    api: Api,
}

#[cfg(feature = "static-link")]
unsafe extern "C" {
    fn chirper_open(
        data_dir: *const c_char,
        key: *const u8,
        key_len: i32,
        cb: EventCallback,
        ctx: *mut c_void,
        err_out: *mut *mut c_char,
    ) -> i64;
    fn chirper_call(handle: i64, method: *const c_char, args_json: *const c_char) -> *mut c_char;
    fn chirper_free(p: *mut c_char);
    fn chirper_close(handle: i64);
    fn chirper_version() -> *const c_char;
}

#[cfg(feature = "static-link")]
impl StaticBackend {
    pub fn new() -> Self {
        Self {
            api: Api {
                open: chirper_open,
                call: chirper_call,
                free: chirper_free,
                close: chirper_close,
                version: chirper_version,
            },
        }
    }
}

#[cfg(feature = "static-link")]
impl Default for StaticBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "static-link")]
impl Backend for StaticBackend {
    fn open(
        &self,
        data_dir: &CStr,
        key: &[u8],
        cb: EventCallback,
        ctx: *mut c_void,
    ) -> std::result::Result<i64, String> {
        self.api.open(data_dir, key, cb, ctx)
    }
    fn call(&self, handle: i64, method: &CStr, args: &CStr) -> String {
        self.api.call(handle, method, args)
    }
    fn close(&self, handle: i64) {
        self.api.close(handle);
    }
    fn version(&self) -> String {
        self.api.version()
    }
}

/// The core loaded at runtime (feature `dynamic-link`, Windows, Linux and Android).
#[cfg(feature = "dynamic-link")]
pub struct DynamicBackend {
    // Keeps the library mapped while the function pointers in `api` are in use.
    _lib: libloading::Library,
    api: Api,
}

#[cfg(feature = "dynamic-link")]
impl DynamicBackend {
    /// Loads the core from `path` and resolves its five symbols once.
    pub fn load(path: &std::path::Path) -> Result<Self> {
        // SAFETY: loading the core runs its initialisers. The library is built by this project
        // (scripts/build-go.sh) and is trusted code.
        let lib = unsafe { libloading::Library::new(path) }
            .map_err(|e| CoreError::Ffi(format!("cannot load core library: {e}")))?;
        let api = resolve(&lib)?;
        Ok(Self { _lib: lib, api })
    }

    /// Loads the core from the locations the app uses on this platform (see [`candidate_paths`]).
    pub fn load_default() -> Result<Self> {
        let mut last = CoreError::Ffi("no core library location".to_owned());
        for path in candidate_paths()? {
            match Self::load(&path) {
                Ok(backend) => return Ok(backend),
                Err(e) => last = e,
            }
        }
        Err(last)
    }
}

#[cfg(feature = "dynamic-link")]
fn resolve(lib: &libloading::Library) -> Result<Api> {
    fn sym<T: Copy>(lib: &libloading::Library, name: &str) -> Result<T> {
        // SAFETY: the symbol types match the documented signatures of chirper.h.
        let symbol = unsafe { lib.get::<T>(name) }
            .map_err(|e| CoreError::Ffi(format!("missing symbol {name}: {e}")))?;
        Ok(*symbol)
    }
    Ok(Api {
        open: sym::<OpenFn>(lib, "chirper_open")?,
        call: sym::<CallFn>(lib, "chirper_call")?,
        free: sym::<FreeFn>(lib, "chirper_free")?,
        close: sym::<CloseFn>(lib, "chirper_close")?,
        version: sym::<VersionFn>(lib, "chirper_version")?,
    })
}

/// Where the shared core is searched for, in order.
/// Android resolves the bare name through the app's native library directory. Windows and Linux
/// look next to the executable, and Linux also in `$ORIGIN/../lib/chirper/`.
#[cfg(feature = "dynamic-link")]
pub fn candidate_paths() -> Result<Vec<std::path::PathBuf>> {
    #[cfg(target_os = "android")]
    {
        Ok(vec![std::path::PathBuf::from("libchirpercore.so")])
    }
    #[cfg(target_os = "windows")]
    {
        Ok(vec![exe_dir()?.join("chirpercore.dll")])
    }
    #[cfg(target_os = "linux")]
    {
        let dir = exe_dir()?;
        Ok(vec![
            dir.join("libchirpercore.so"),
            dir.join("../lib/chirper").join("libchirpercore.so"),
        ])
    }
    #[cfg(not(any(target_os = "android", target_os = "windows", target_os = "linux")))]
    {
        Err(CoreError::Ffi(
            "dynamic-link is not supported on this platform".to_owned(),
        ))
    }
}

#[cfg(all(
    feature = "dynamic-link",
    any(target_os = "windows", target_os = "linux")
))]
fn exe_dir() -> Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    exe.parent()
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| CoreError::Ffi("executable has no parent directory".to_owned()))
}

#[cfg(feature = "dynamic-link")]
impl Backend for DynamicBackend {
    fn open(
        &self,
        data_dir: &CStr,
        key: &[u8],
        cb: EventCallback,
        ctx: *mut c_void,
    ) -> std::result::Result<i64, String> {
        self.api.open(data_dir, key, cb, ctx)
    }
    fn call(&self, handle: i64, method: &CStr, args: &CStr) -> String {
        self.api.call(handle, method, args)
    }
    fn close(&self, handle: i64) {
        self.api.close(handle);
    }
    fn version(&self) -> String {
        self.api.version()
    }
}

/// In-memory fake of the core. It answers the common methods with fixture JSON and emits a
/// `message` event for every `sendText`, so that the wrapper can be tested without Go.
#[cfg(any(test, feature = "mock"))]
pub struct MockBackend {
    next: std::sync::atomic::AtomicI64,
    sessions: std::sync::Mutex<std::collections::HashMap<i64, Session>>,
    closes: std::sync::atomic::AtomicUsize,
    calls: std::sync::atomic::AtomicUsize,
}

#[cfg(any(test, feature = "mock"))]
#[derive(Clone, Copy)]
struct Session {
    cb: EventCallback,
    // Stored as an integer so that the session table is Send; it is only dereferenced by `cb`.
    ctx: usize,
}

#[cfg(any(test, feature = "mock"))]
pub const MOCK_NETWORKS: &str = r##"[{"id":"discord","name":"Discord","color":"#5865F2","beta":false,"available":true,"reason":"","loginFlows":[{"id":"token","name":"Token","description":"Paste a token"}],"capabilities":{"edits":true,"reactions":true,"replies":true,"redactions":true,"typing":true,"readReceipts":true,"stickers":false,"voice":false}}]"##;

#[cfg(any(test, feature = "mock"))]
pub const MOCK_CHAT: &str = r##"{"roomId":"!mock1:chirper.local","network":"discord","loginId":"discord_1","name":"General","avatarUrl":"","topic":"","isDm":false,"isGroup":true,"isSpace":false,"memberCount":2,"lastActivity":1700000000000,"preview":null,"unreadCount":1,"mentionCount":0,"pinned":false,"archived":false,"mutedUntil":0,"typing":[]}"##;

#[cfg(any(test, feature = "mock"))]
pub const MOCK_EVENT_ID: &str = "$mock.1";

#[cfg(any(test, feature = "mock"))]
impl Default for MockBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "mock"))]
impl MockBackend {
    pub fn new() -> Self {
        Self {
            next: std::sync::atomic::AtomicI64::new(1),
            sessions: std::sync::Mutex::new(std::collections::HashMap::new()),
            closes: std::sync::atomic::AtomicUsize::new(0),
            calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Number of `call` invocations received so far.
    pub fn call_count(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Number of `close` calls received so far.
    pub fn close_count(&self) -> usize {
        self.closes.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn emit(&self, handle: i64, kind: &str, payload: &str) {
        let session = self
            .sessions
            .lock()
            .ok()
            .and_then(|s| s.get(&handle).copied());
        if let Some(Session { cb, ctx }) = session {
            let (Ok(kind), Ok(payload)) = (
                std::ffi::CString::new(kind),
                std::ffi::CString::new(payload),
            ) else {
                return;
            };
            cb(ctx as *mut c_void, kind.as_ptr(), payload.as_ptr());
        }
    }

    fn answer(&self, handle: i64, method: &str) -> String {
        let ok = |value: &str| format!(r##"{{"ok":true,"value":{value}}}"##);
        match method {
            "start" | "cancelLogin" | "logout" | "syncHistory" | "markUnread" | "markRead"
            | "setTyping" | "setChatFlags" | "edit" | "redact" | "react" | "unreact"
            | "setSettings" => ok("null"),
            "networks" => ok(MOCK_NETWORKS),
            "accounts" => ok("[]"),
            "listChats" => ok(&format!("[{MOCK_CHAT}]")),
            "getChat" => ok(MOCK_CHAT),
            "listMessages" | "search" => ok("[]"),
            "settings" => ok(
                r##"{"demoNetwork":false,"sendReadReceipts":{"*":true},"sendTyping":{"*":true},"telegram":{"apiId":0,"apiHash":""}}"##,
            ),
            "sendText" => {
                let payload = format!(
                    r##"{{"eventId":"{MOCK_EVENT_ID}","roomId":"!mock1:chirper.local","order":1,"sender":"@me:chirper.local","fromMe":true,"type":"m.text","body":"","status":"sent","live":true}}"##
                );
                self.emit(handle, "message", &payload);
                ok(&format!(r##"{{"eventId":"{MOCK_EVENT_ID}"}}"##))
            }
            _ => r##"{"ok":false,"error":"unknown method"}"##.to_owned(),
        }
    }
}

#[cfg(any(test, feature = "mock"))]
impl Backend for MockBackend {
    fn open(
        &self,
        _data_dir: &CStr,
        _key: &[u8],
        cb: EventCallback,
        ctx: *mut c_void,
    ) -> std::result::Result<i64, String> {
        let handle = self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let session = Session {
            cb,
            ctx: ctx as usize,
        };
        self.sessions
            .lock()
            .map_err(|_| "mock state poisoned".to_owned())?
            .insert(handle, session);
        Ok(handle)
    }

    fn call(&self, handle: i64, method: &CStr, _args: &CStr) -> String {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let method = method.to_str().unwrap_or_default();
        self.answer(handle, method)
    }

    fn close(&self, handle: i64) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.remove(&handle);
        }
        self.closes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    fn version(&self) -> String {
        "mock".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_answers_known_methods_and_rejects_unknown() {
        let mock = MockBackend::new();
        extern "C" fn sink(_: *mut c_void, _: *const c_char, _: *const c_char) {}
        let handle = mock.open(c"/tmp", &[], sink, std::ptr::null_mut()).unwrap();
        let networks = mock.call(handle, c"networks", c"{}");
        assert!(networks.starts_with(r##"{"ok":true,"value":[{"id":"discord""##));
        let unknown = mock.call(handle, c"nope", c"{}");
        assert!(unknown.contains(r##""ok":false"##));
        mock.close(handle);
        assert_eq!(mock.close_count(), 1);
        assert_eq!(mock.version(), "mock");
    }
}
