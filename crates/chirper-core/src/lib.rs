//! Safe Rust wrapper over the Chirper Go core.
//!
//! [`Core`] owns one handle of the core. Blocking FFI calls run on `spawn_blocking`, events arrive
//! on a bounded channel, and every input is validated before it crosses the boundary. The JSON
//! model is in [`model`], the typed methods are in [`api`] (on `Core`), the library loaders are in
//! [`ffi`] and the key store is in [`keystore`].

pub mod api;
pub mod ffi;
pub mod keystore;
pub mod model;
pub mod validate;

use std::ffi::{CString, c_char, c_void};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;
use zeroize::Zeroizing;

use crate::ffi::Backend;
use crate::model::CoreEvent;

/// Capacity of the event channel. Events beyond it are dropped and counted.
pub const EVENT_CAPACITY: usize = 4096;
/// Largest argument document accepted by the core (CONTRACT.md section B).
pub const MAX_ARGS_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// The core answered with `{"ok":false}`; the message is the core's error text.
    #[error("core error: {0}")]
    Go(String),
    /// The library could not be loaded, a symbol is missing, or the envelope is malformed.
    #[error("ffi error: {0}")]
    Ffi(String),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// The handle was closed with [`Core::close`] or the core is shutting down.
    #[error("core is closed")]
    Closed,
    /// A value failed validation before reaching the core. The text names the field only.
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("key store error: {0}")]
    KeyStore(String),
}

pub type Result<T, E = CoreError> = std::result::Result<T, E>;

/// Receives events from the core. It lives in a box whose address is the callback context, so it
/// must outlive the handle: [`Inner`] closes the handle before dropping it.
struct EventSink {
    tx: SyncSender<CoreEvent>,
    dropped: AtomicU64,
}

impl EventSink {
    fn deliver(&self, kind: Option<String>, payload: Option<String>) {
        let event = kind.zip(payload).and_then(|(kind, payload)| {
            Some(CoreEvent {
                kind,
                payload: RawValue::from_string(payload).ok()?,
            })
        });
        let Some(event) = event else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        };
        if self.tx.try_send(event).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn read_c_str(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    // SAFETY: the core passes NUL-terminated strings that are valid for the duration of the call.
    unsafe { std::ffi::CStr::from_ptr(p) }
        .to_str()
        .ok()
        .map(str::to_owned)
}

/// Event callback handed to the core. It never unwinds across the C boundary.
extern "C" fn on_event(ctx: *mut c_void, kind: *const c_char, payload: *const c_char) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if ctx.is_null() {
            return;
        }
        // SAFETY: `ctx` is the `EventSink` of the `Core` that opened the handle. The sink is alive
        // until that handle is closed, and the core does not call back after close.
        let sink = unsafe { &*ctx.cast::<EventSink>() };
        sink.deliver(read_c_str(kind), read_c_str(payload));
    }));
}

struct Inner {
    backend: Arc<dyn Backend>,
    handle: i64,
    closed: AtomicBool,
    sink: Box<EventSink>,
}

impl Inner {
    /// Closes the handle at most once.
    fn close_once(&self) {
        if !self.closed.swap(true, Ordering::SeqCst) {
            self.backend.close(self.handle);
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.close_once();
    }
}

/// A handle to the Chirper core. Cheap to clone; the handle closes when the last clone is dropped.
#[derive(Clone)]
pub struct Core {
    inner: Arc<Inner>,
}

impl fmt::Debug for Core {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Core")
            .field("closed", &self.inner.closed.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl Core {
    /// Opens the core under `data_dir` (created if missing, mode 0700 on Unix) and returns the
    /// event receiver. `key` is the 32-byte database key, or `None` for no at-rest encryption.
    /// This call blocks while the core opens its databases.
    pub fn open(
        backend: Arc<dyn Backend>,
        data_dir: &Path,
        key: Option<&Zeroizing<[u8; 32]>>,
    ) -> Result<(Self, Receiver<CoreEvent>)> {
        let dir = prepare_data_dir(data_dir)?;
        let dir_str = dir
            .to_str()
            .ok_or_else(|| CoreError::InvalidInput("data dir".to_owned()))?;
        let dir_c = to_c_string(dir_str, "data dir")?;
        let (tx, rx) = sync_channel(EVENT_CAPACITY);
        let sink = Box::new(EventSink {
            tx,
            dropped: AtomicU64::new(0),
        });
        let ctx = std::ptr::from_ref::<EventSink>(&sink)
            .cast_mut()
            .cast::<c_void>();
        let key_bytes: &[u8] = key.map_or(&[], |k| &k[..]);
        let handle = backend
            .open(&dir_c, key_bytes, on_event, ctx)
            .map_err(CoreError::Go)?;
        let inner = Inner {
            backend,
            handle,
            closed: AtomicBool::new(false),
            sink,
        };
        Ok((
            Self {
                inner: Arc::new(inner),
            },
            rx,
        ))
    }

    /// Closes the handle now. Later calls fail with [`CoreError::Closed`]. Idempotent.
    pub fn close(&self) {
        self.inner.close_once();
    }

    /// Number of events dropped because the channel was full or an event was malformed.
    pub fn dropped_events(&self) -> u64 {
        self.inner.sink.dropped.load(Ordering::Relaxed)
    }

    /// Version string of the loaded core library.
    pub fn version(&self) -> String {
        self.inner.backend.version()
    }

    /// Calls a facade method with a JSON object of arguments and returns the raw JSON value.
    /// The blocking FFI call runs on `spawn_blocking`.
    pub async fn call_raw(&self, method: &str, args: Value) -> Result<Box<RawValue>> {
        let args_json = serde_json::to_string(&args)?;
        if args_json.len() > MAX_ARGS_BYTES {
            return Err(CoreError::InvalidInput("arguments too large".to_owned()));
        }
        let method_c = to_c_string(method, "method")?;
        let args_c = to_c_string(&args_json, "arguments")?;
        let inner = self.live()?;
        let envelope = tokio::task::spawn_blocking(move || {
            inner.backend.call(inner.handle, &method_c, &args_c)
        })
        .await
        .map_err(|e| CoreError::Ffi(format!("core call task failed: {e}")))?;
        parse_envelope(&envelope)
    }

    fn live(&self) -> Result<Arc<Inner>> {
        if self.inner.closed.load(Ordering::SeqCst) {
            Err(CoreError::Closed)
        } else {
            Ok(Arc::clone(&self.inner))
        }
    }
}

#[derive(Deserialize)]
struct Envelope {
    ok: bool,
    #[serde(default)]
    value: Option<Box<RawValue>>,
    #[serde(default)]
    error: String,
}

/// Reads `{"ok":true,"value":..}` or `{"ok":false,"error":".."}`.
fn parse_envelope(text: &str) -> Result<Box<RawValue>> {
    let envelope: Envelope = serde_json::from_str(text)
        .map_err(|e| CoreError::Ffi(format!("malformed core envelope: {e}")))?;
    if !envelope.ok {
        return Err(CoreError::Go(envelope.error));
    }
    match envelope.value {
        Some(value) => Ok(value),
        None => Ok(RawValue::from_string("null".to_owned())?),
    }
}

fn to_c_string(s: &str, field: &str) -> Result<CString> {
    CString::new(s).map_err(|_| CoreError::InvalidInput(field.to_owned()))
}

fn prepare_data_dir(dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    restrict_to_owner(dir)?;
    Ok(std::fs::canonicalize(dir)?)
}

#[cfg(unix)]
fn restrict_to_owner(dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_to_owner(_dir: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::ffi::MockBackend;

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("chirper-core-{name}-{}", std::process::id()))
    }

    fn open_mock(name: &str) -> (Arc<MockBackend>, Core, Receiver<CoreEvent>, PathBuf) {
        let mock = Arc::new(MockBackend::new());
        let dir = temp_dir(name);
        let (core, rx) = Core::open(mock.clone(), &dir, None).unwrap();
        (mock, core, rx, dir)
    }

    #[tokio::test]
    async fn call_returns_raw_value_and_send_emits_event() {
        let (_mock, core, rx, dir) = open_mock("call");
        let raw = core
            .call_raw("networks", serde_json::json!({}))
            .await
            .unwrap();
        assert!(raw.get().starts_with("[{"));

        let id = core
            .send_text("!mock1:chirper.local", "hi", None, "txn-1")
            .await
            .unwrap();
        assert_eq!(id, ffi::MOCK_EVENT_ID);
        let event = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(event.kind, model::event_kind::MESSAGE);
        assert!(event.payload.get().contains("\"fromMe\":true"));
        assert_eq!(core.dropped_events(), 0);
        drop(core);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn go_error_envelope_becomes_go_error() {
        let (_mock, core, _rx, dir) = open_mock("error");
        let err = core
            .call_raw("doesNotExist", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(
            matches!(err, CoreError::Go(ref m) if m == "unknown method"),
            "{err:?}"
        );
        drop(core);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn oversized_arguments_are_rejected_before_ffi() {
        let (_mock, core, _rx, dir) = open_mock("oversized");
        let big = "x".repeat(MAX_ARGS_BYTES);
        let err = core
            .call_raw("networks", serde_json::json!({ "pad": big }))
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::InvalidInput(_)));
        drop(core);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn drop_closes_handle_exactly_once() {
        let (mock, core, _rx, dir) = open_mock("close");
        let clone = core.clone();
        drop(core);
        assert_eq!(mock.close_count(), 0, "a clone is still alive");
        drop(clone);
        assert_eq!(mock.close_count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn explicit_close_then_drop_still_closes_once() {
        let (mock, core, _rx, dir) = open_mock("explicit");
        core.close();
        core.close();
        let err = core
            .call_raw("networks", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::Closed));
        drop(core);
        assert_eq!(mock.close_count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn malformed_envelope_is_ffi_error() {
        assert!(matches!(parse_envelope("not json"), Err(CoreError::Ffi(_))));
        assert_eq!(parse_envelope(r#"{"ok":true}"#).unwrap().get(), "null");
        assert_eq!(
            parse_envelope(r#"{"ok":true,"value":[1]}"#).unwrap().get(),
            "[1]"
        );
    }

    #[test]
    fn core_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Core>();
        assert_send_sync::<CoreEvent>();
    }

    #[test]
    fn event_sink_counts_drops_when_full_and_on_bad_payload() {
        let (tx, rx) = sync_channel(1);
        let sink = EventSink {
            tx,
            dropped: AtomicU64::new(0),
        };
        sink.deliver(Some("chat".into()), Some("{}".into()));
        sink.deliver(Some("chat".into()), Some("{}".into()));
        sink.deliver(Some("chat".into()), Some("{not json".into()));
        sink.deliver(None, Some("{}".into()));
        assert_eq!(sink.dropped.load(Ordering::Relaxed), 3);
        assert_eq!(rx.try_recv().unwrap().kind, "chat");
    }
}
