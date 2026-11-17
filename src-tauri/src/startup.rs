//! Opens the core off the UI thread. It emits `core-ready` once the core is open and started, or
//! `core-failed` with a message that never contains user data.

use std::sync::Arc;

use chirper_core::Core;
#[cfg(not(feature = "mock"))]
use chirper_core::ffi::Backend;
#[cfg(feature = "mock")]
use chirper_core::ffi::MockBackend;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

pub const EVENT_CORE_READY: &str = "core-ready";
pub const EVENT_CORE_FAILED: &str = "core-failed";

/// Starts the open on its own thread. The result is reported through events only.
pub fn spawn(app: AppHandle) {
    let spawned = std::thread::Builder::new()
        .name("core-open".into())
        .spawn(move || match open(&app) {
            Ok(()) => {
                log::info!("core ready");
                emit(&app, EVENT_CORE_READY, json!({}));
            }
            Err(message) => {
                log::error!("core failed to open: {message}");
                emit(&app, EVENT_CORE_FAILED, json!({ "message": message }));
            }
        });
    if let Err(e) = spawned {
        log::error!("cannot start the core thread: {e}");
    }
}

fn open(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let key = crate::platform::load_db_key(app)?;
    let (backend, backend_name) = backend()?;
    let (core, events) = match Core::open(backend.clone(), state.core_dir(), key.as_ref()) {
        Ok(res) => res,
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("file is not a database")
                || msg.contains("database disk image is malformed")
                || msg.contains("not a database")
            {
                log::warn!(
                    "database unreadable or key mismatched ({msg}), recreating data directory"
                );
                let _ = std::fs::remove_dir_all(state.core_dir());
                Core::open(backend, state.core_dir(), key.as_ref())
                    .map_err(|e2| format!("cannot open the core after reset: {e2}"))?
            } else {
                return Err(format!("cannot open the core: {e}"));
            }
        }
    };
    log::info!("core opened with the {backend_name} backend");

    crate::events::spawn_pump(app.clone(), events);
    tauri::async_runtime::block_on(core.start())
        .map_err(|e| format!("cannot start the core: {e}"))?;
    if state.set_core(core.clone()).is_err() {
        return Err("the core is already open".to_owned());
    }
    crate::deep_link::flush_pending(app);
    tauri::async_runtime::block_on(sync_background_service(app, &core));
    smoke_test_mock(&core);
    Ok(())
}

/// Keeps the background service in step with the accounts: Android runs it while at least one
/// account exists. Failures are logged; the core works without the service.
pub async fn sync_background_service(app: &AppHandle, core: &Core) {
    match core.accounts().await {
        Ok(accounts) => {
            let running = !accounts.is_empty();
            if let Err(e) = crate::platform::set_background_service(app, running) {
                log::warn!("cannot update the background service: {}", e.message);
            }
        }
        Err(e) => log::warn!("cannot read the accounts for the background service: {e}"),
    }
}

#[cfg(feature = "mock")]
fn backend() -> Result<(Arc<dyn chirper_core::ffi::Backend>, &'static str), String> {
    Ok((Arc::new(MockBackend::new()), "mock"))
}

#[cfg(all(not(feature = "mock"), any(target_os = "macos", target_os = "ios")))]
fn backend() -> Result<(Arc<dyn Backend>, &'static str), String> {
    Ok((Arc::new(chirper_core::ffi::StaticBackend::new()), "static"))
}

#[cfg(all(
    not(feature = "mock"),
    not(any(target_os = "macos", target_os = "ios"))
))]
fn backend() -> Result<(Arc<dyn Backend>, &'static str), String> {
    let backend = chirper_core::ffi::DynamicBackend::load_default()
        .map_err(|e| format!("cannot load the core library: {e}"))?;
    Ok((Arc::new(backend), "dynamic"))
}

/// With the mock core and `CHIRPER_MOCK_SMOKE` set, one fake message exercises the event path.
#[cfg(feature = "mock")]
fn smoke_test_mock(core: &Core) {
    if std::env::var_os("CHIRPER_MOCK_SMOKE").is_some() {
        let sent = tauri::async_runtime::block_on(core.send_text(
            "!mock1:chirper.local",
            "smoke",
            None,
            "smoke.1",
        ));
        if let Err(e) = sent {
            log::warn!("mock smoke message failed: {e}");
        }
    }
}

#[cfg(not(feature = "mock"))]
fn smoke_test_mock(_core: &Core) {}

fn emit(app: &AppHandle, event: &str, payload: serde_json::Value) {
    if let Err(e) = app.emit(event, payload) {
        log::warn!("cannot emit {event}: {e}");
    }
}
