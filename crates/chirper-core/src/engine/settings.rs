//! User settings, stored as JSON next to the database.

use std::path::Path;

use crate::model::CoreSettings;

use super::{Error, Result};

/// The settings file inside the data directory.
pub const FILE: &str = "settings.json";

/// The defaults of a fresh install: read receipts and typing on for every network.
pub fn defaults() -> CoreSettings {
    CoreSettings {
        demo_network: false,
        send_read_receipts: serde_json::json!({ "*": true }),
        send_typing: serde_json::json!({ "*": true }),
        telegram: serde_json::json!({ "apiId": 0, "apiHash": "" }),
    }
}

/// Read the settings; a missing or damaged file yields the defaults.
pub fn load(data_dir: &Path) -> CoreSettings {
    let path = data_dir.join(FILE);
    let Ok(raw) = std::fs::read(&path) else {
        return defaults();
    };
    match serde_json::from_slice(&raw) {
        Ok(settings) => settings,
        Err(err) => {
            log::warn!("{FILE} is unreadable ({err}); using defaults");
            defaults()
        }
    }
}

/// Write the settings with owner-only permissions, atomically.
pub fn save(data_dir: &Path, settings: &CoreSettings) -> Result<()> {
    let path = data_dir.join(FILE);
    let tmp = data_dir.join(format!(".{FILE}.tmp"));
    crate::secure::write_private(&tmp, &serde_json::to_vec_pretty(settings)?)?;
    std::fs::rename(&tmp, &path).map_err(Error::Io)?;
    Ok(())
}
