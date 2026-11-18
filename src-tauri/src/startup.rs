//! Opens the core off the UI thread. It emits `core-ready` once the core is open and started, or
//! `core-failed` with a message that never contains user data.

use chirper_core::{Core, Options};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
use zeroize::Zeroizing;

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
    let dir = state.core_dir().to_path_buf();
    let (core, events) = tauri::async_runtime::block_on(open_core(dir, key))?;
    crate::events::spawn_pump(app.clone(), events);
    tauri::async_runtime::block_on(core.start())
        .map_err(|e| format!("cannot start the core: {e}"))?;
    if state.set_core(core.clone()).is_err() {
        return Err("the core is already open".to_owned());
    }
    crate::deep_link::flush_pending(app);
    tauri::async_runtime::block_on(sync_background_service(app, &core));
    Ok(())
}

/// Open the database, recreating the data directory when the file is unreadable or the key does not
/// match. Losing local history is bad; refusing to start forever is worse.
async fn open_core(
    dir: std::path::PathBuf,
    key: Option<Zeroizing<[u8; 32]>>,
) -> Result<(Core, chirper_core::EventStream), String> {
    let options = Options {
        data_dir: dir.clone(),
        db_key: key.as_ref().map(|k| **k),
        demo_network: cfg!(feature = "mock"),
    };
    match Core::open(options.clone()) {
        Ok(res) => Ok(res),
        Err(e) => {
            let message = e.to_string();
            let unreadable = message.contains("file is not a database")
                || message.contains("database disk image is malformed")
                || message.contains("not a database")
                || message.contains("code 26");
            if !unreadable {
                return Err(format!("cannot open the core: {message}"));
            }
            log::warn!("database unreadable or key mismatched ({message}), recreating the data directory");
            let _ = std::fs::remove_dir_all(&dir);
            Core::open(options).map_err(|e2| format!("cannot open the core after reset: {e2}"))
        }
    }
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

fn emit(app: &AppHandle, event: &str, payload: serde_json::Value) {
    if let Err(e) = app.emit(event, payload) {
        log::warn!("cannot emit {event}: {e}");
    }
}