//! The Telegram API id and hash the user enters in Settings → Advanced: validation, and what the
//! network list says before and after a restart.

use chirper_core::model::{CoreSettings, Network};
use chirper_core::{Core, Error, Options};
use serde_json::json;

const HASH: &str = "0123456789abcdef0123456789abcdef";

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "chirper-telegram-settings-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn with_telegram(telegram: serde_json::Value) -> CoreSettings {
    CoreSettings {
        telegram,
        ..Default::default()
    }
}

async fn telegram_row(core: &Core) -> Network {
    core.networks()
        .await
        .unwrap()
        .into_iter()
        .find(|network| network.id == "telegram")
        .expect("telegram is listed")
}

#[tokio::test]
async fn invalid_telegram_settings_are_refused_and_not_stored() {
    let dir = temp_dir("refused");
    let (core, _events) = Core::open(Options::new(&dir)).unwrap();
    let before = core.settings().await.unwrap();

    let bad = [
        json!({ "apiId": -1, "apiHash": HASH }),
        json!({ "apiId": 1.5, "apiHash": HASH }),
        json!({ "apiId": 12, "apiHash": "not-a-hash" }),
    ];
    for telegram in bad {
        let result = core.set_settings(&with_telegram(telegram.clone())).await;
        assert!(
            matches!(result, Err(Error::InvalidInput(_))),
            "accepted {telegram}"
        );
    }
    assert_eq!(core.settings().await.unwrap(), before);
    core.shutdown();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn valid_telegram_settings_make_the_row_available_after_a_restart() {
    let dir = temp_dir("restart");
    let (core, _events) = Core::open(Options::new(&dir)).unwrap();
    core.set_settings(&with_telegram(json!({ "apiId": 12345, "apiHash": HASH })))
        .await
        .unwrap();
    core.shutdown();

    // The registry is built at start, so the new credentials show after a restart.
    let (core, _events) = Core::open(Options::new(&dir)).unwrap();
    let stored = core.settings().await.unwrap();
    assert_eq!(stored.telegram["apiId"], json!(12345));
    let row = telegram_row(&core).await;
    assert!(row.available, "still unavailable: {}", row.reason);
    assert!(row.reason.is_empty());
    core.shutdown();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn without_credentials_the_row_says_where_to_enter_them() {
    let dir = temp_dir("empty");
    let (core, _events) = Core::open(Options::new(&dir)).unwrap();
    core.set_settings(&with_telegram(json!({ "apiId": 0, "apiHash": "" })))
        .await
        .unwrap();
    core.shutdown();

    let (core, _events) = Core::open(Options::new(&dir)).unwrap();
    let row = telegram_row(&core).await;
    // Whether the build carries its own pair decides the availability; either way the row is
    // listed, and an unavailable one names the settings page.
    if !row.available {
        assert!(row.reason.contains("Settings → Advanced"), "{}", row.reason);
    }
    core.shutdown();
    std::fs::remove_dir_all(&dir).unwrap();
}
