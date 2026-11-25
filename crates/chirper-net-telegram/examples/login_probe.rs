//! Live check of the phone login: sends a login code to the phone number, signs in with it and,
//! when two-step verification is on, with the password. Prints only the user id.
//!
//! Environment: `CHIRPER_PROBE_PHONE` (with the country code), `CHIRPER_PROBE_CODE` (the code
//! Telegram sends), and optionally `CHIRPER_PROBE_PASSWORD`. The credentials come from the
//! `CHIRPER_TELEGRAM_API_ID` and `CHIRPER_TELEGRAM_API_HASH` the crate was built with.
//! Run with `cargo run -p chirper-net-telegram --example login_probe`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chirper_bridge::model::LoginStep;
use chirper_bridge::{LoginCtx, LoginInput, Network, Secrets};
use chirper_net_telegram::Telegram;
use zeroize::Zeroizing;

/// Secrets kept in memory for the length of the probe.
#[derive(Default)]
struct MemorySecrets(Mutex<BTreeMap<String, Vec<u8>>>);

impl Secrets for MemorySecrets {
    fn get(&self, key: &str) -> chirper_bridge::Result<Option<Zeroizing<Vec<u8>>>> {
        let map = self
            .0
            .lock()
            .map_err(|_| chirper_bridge::Error::Internal("poisoned".into()))?;
        Ok(map.get(key).cloned().map(Zeroizing::new))
    }

    fn set(&self, key: &str, value: &[u8]) -> chirper_bridge::Result<()> {
        let mut map = self
            .0
            .lock()
            .map_err(|_| chirper_bridge::Error::Internal("poisoned".into()))?;
        map.insert(key.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, key: &str) -> chirper_bridge::Result<()> {
        let mut map = self
            .0
            .lock()
            .map_err(|_| chirper_bridge::Error::Internal("poisoned".into()))?;
        map.remove(key);
        Ok(())
    }
}

fn env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("set {name}"))
}

#[tokio::main]
async fn main() {
    if let Err(message) = probe().await {
        println!("login probe failed: {message}");
        std::process::exit(1);
    }
}

async fn probe() -> Result<(), String> {
    let phone = env("CHIRPER_PROBE_PHONE")?;
    let code = env("CHIRPER_PROBE_CODE")?;
    let password = std::env::var("CHIRPER_PROBE_PASSWORD").unwrap_or_default();
    let dir = std::env::temp_dir().join(format!("chirper-login-probe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let ctx = LoginCtx {
        network: "telegram".into(),
        login_id: "probe".into(),
        data_dir: dir,
        secrets: Arc::new(MemorySecrets::default()),
        http: chirper_bridge::http_client(),
    };
    let network = Telegram::new();
    let mut process = network
        .start_login("phone", ctx)
        .await
        .map_err(|e| e.to_string())?;
    let mut input = LoginInput::start();
    for _ in 0..6 {
        let step = process.next(input).await.map_err(|e| e.to_string())?;
        input = match step {
            LoginStep::Complete { login_id } => {
                println!("logged in as user {login_id}");
                return Ok(());
            }
            LoginStep::UserInput { step_id, .. } if step_id.ends_with("phone_number") => {
                LoginInput::values(BTreeMap::from([(step_id, phone.clone())]))
            }
            LoginStep::UserInput { step_id, .. } if step_id.ends_with(".password") => {
                LoginInput::values(BTreeMap::from([(step_id, password.clone())]))
            }
            LoginStep::UserInput { step_id, .. } => {
                LoginInput::values(BTreeMap::from([(step_id, code.clone())]))
            }
            other => return Err(format!("unexpected step {}", other.step_id())),
        };
    }
    Err("the login did not finish".into())
}
