//! User settings, stored as JSON next to the database.

use std::path::Path;

use serde_json::Value;

use crate::model::CoreSettings;

use super::{Error, Result};

/// The settings file inside the data directory.
pub const FILE: &str = "settings.json";

/// The largest Telegram API id: the driver takes it as an `i32`.
const MAX_TELEGRAM_API_ID: i64 = i32::MAX as i64;

/// The Telegram API id and hash the user entered. A missing or malformed value reads as the
/// unset pair `(0, "")`, which leaves the choice to the build.
pub fn telegram_api(settings: &CoreSettings) -> (i32, String) {
    let id = settings
        .telegram
        .get("apiId")
        .and_then(Value::as_i64)
        .and_then(|id| i32::try_from(id).ok())
        .unwrap_or(0);
    let hash = settings
        .telegram
        .get("apiHash")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    (id, hash)
}

/// Refuse settings the engine cannot use: a Telegram API id that is not a whole number from zero
/// to 2^31 - 1, and an API hash that is neither empty nor 32 hexadecimal characters.
pub fn validate(settings: &CoreSettings) -> Result<()> {
    let telegram = match &settings.telegram {
        Value::Null => return Ok(()),
        Value::Object(map) => map,
        _ => {
            return Err(Error::InvalidInput(
                "the Telegram settings must be an object".to_owned(),
            ));
        }
    };
    if let Some(id) = telegram.get("apiId") {
        let valid = id
            .as_i64()
            .is_some_and(|id| (0..=MAX_TELEGRAM_API_ID).contains(&id));
        if !valid {
            return Err(Error::InvalidInput(
                "the Telegram API id must be a whole number from 0 to 2147483647".to_owned(),
            ));
        }
    }
    if let Some(hash) = telegram.get("apiHash") {
        let valid = hash
            .as_str()
            .is_some_and(|hash| hash.is_empty() || chirper_net_telegram::is_valid_api_hash(hash));
        if !valid {
            return Err(Error::InvalidInput(
                "the Telegram API hash must be empty or 32 hexadecimal characters".to_owned(),
            ));
        }
    }
    Ok(())
}

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

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "0123456789abcdef0123456789abcdef";

    fn with_telegram(telegram: Value) -> CoreSettings {
        CoreSettings {
            telegram,
            ..defaults()
        }
    }

    #[test]
    fn the_defaults_and_an_absent_section_are_valid() {
        assert!(validate(&defaults()).is_ok());
        assert!(validate(&CoreSettings::default()).is_ok());
        assert!(
            validate(&with_telegram(
                serde_json::json!({ "apiId": 0, "apiHash": "" })
            ))
            .is_ok()
        );
        assert!(
            validate(&with_telegram(
                serde_json::json!({ "apiId": 12345, "apiHash": HASH })
            ))
            .is_ok()
        );
    }

    #[test]
    fn a_negative_or_fractional_or_oversized_api_id_is_refused() {
        for id in [
            serde_json::json!(-1),
            serde_json::json!(1.5),
            serde_json::json!("12"),
            serde_json::Value::Null,
            serde_json::json!(2_147_483_648_i64),
        ] {
            let settings = with_telegram(serde_json::json!({ "apiId": id, "apiHash": "" }));
            assert!(
                matches!(validate(&settings), Err(Error::InvalidInput(_))),
                "accepted {id}"
            );
        }
    }

    #[test]
    fn an_api_hash_that_is_neither_empty_nor_32_hex_characters_is_refused() {
        for hash in [
            serde_json::json!("0123"),
            serde_json::json!(format!("{HASH}0")),
            serde_json::json!("g123456789abcdef0123456789abcdef"),
            serde_json::json!(7),
            serde_json::Value::Null,
        ] {
            let settings = with_telegram(serde_json::json!({ "apiId": 1, "apiHash": hash }));
            assert!(
                matches!(validate(&settings), Err(Error::InvalidInput(_))),
                "accepted {hash}"
            );
        }
    }

    #[test]
    fn a_telegram_section_that_is_not_an_object_is_refused() {
        assert!(validate(&with_telegram(serde_json::json!("12:abc"))).is_err());
    }

    #[test]
    fn the_api_pair_reads_valid_values_and_treats_the_rest_as_unset() {
        let set = with_telegram(serde_json::json!({ "apiId": 42, "apiHash": HASH }));
        assert_eq!(telegram_api(&set), (42, HASH.to_owned()));
        let malformed = with_telegram(serde_json::json!({ "apiId": "42", "apiHash": 9 }));
        assert_eq!(telegram_api(&malformed), (0, String::new()));
        assert_eq!(telegram_api(&CoreSettings::default()), (0, String::new()));
    }
}
