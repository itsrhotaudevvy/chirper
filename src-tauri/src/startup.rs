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
    let (core, events) =
        tauri::async_runtime::block_on(open_core(dir, key, crate::platform::host_networks(app)))?;
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

/// Open the database. When the file is unreadable or the key does not match, the old data directory
/// is moved aside (never deleted: the key may come back, for example once the OS keyring unlocks)
/// and a fresh one is created. Refusing to start forever is worse than starting empty.
async fn open_core(
    dir: std::path::PathBuf,
    key: Option<Zeroizing<[u8; 32]>>,
    host_networks: chirper_core::HostNetworks,
) -> Result<(Core, chirper_core::EventStream), String> {
    let options = Options {
        data_dir: dir.clone(),
        db_key: key.as_ref().map(|k| **k),
        demo_network: cfg!(feature = "mock"),
        host_networks,
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
            let aside = set_aside(&dir)
                .map_err(|e2| format!("cannot move the unreadable data directory aside: {e2}"))?;
            log::warn!(
                "database unreadable or key mismatched ({message}); the old data directory was \
                 moved to {}",
                aside.display()
            );
            Core::open(options).map_err(|e2| format!("cannot open the core after reset: {e2}"))
        }
    }
}

/// Rename `dir` to a sibling `<name>.unreadable-<unix seconds>` and return the new path. At most
/// [`KEEP_ASIDE`] such copies are kept; older ones are removed, so a key that never comes back does
/// not fill the disk.
fn set_aside(dir: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    const KEEP_ASIDE: usize = 2;
    let parent = dir.parent().unwrap_or(std::path::Path::new("."));
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "core".to_owned());
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let prefix = format!("{name}.unreadable-");
    let target = parent.join(format!("{prefix}{stamp}"));
    std::fs::rename(dir, &target)?;
    if let Ok(entries) = std::fs::read_dir(parent) {
        let mut old = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
            .map(|e| e.path())
            .collect::<Vec<_>>();
        // Names end in a unix timestamp of equal width for centuries, so they sort by age.
        old.sort();
        let excess = old.len().saturating_sub(KEEP_ASIDE);
        for path in old.into_iter().take(excess) {
            let _ = std::fs::remove_dir_all(path);
        }
    }
    Ok(target)
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
