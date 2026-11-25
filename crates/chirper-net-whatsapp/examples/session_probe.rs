//! Live probe: connects a stored WhatsApp login and prints how many chats and incoming messages
//! arrive in one minute. Never prints names, numbers, keys or message content.
//!
//! The device record (the `device` secret of a linked login, hex-encoded) comes from the
//! `CHIRPER_WHATSAPP_DEVICE` environment variable. The login's secrets live in memory only, and
//! the probe is never run by `cargo test`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chirper_bridge::model::SessionState;
use chirper_bridge::{Event, LoginCtx, Network, Result as BridgeResult, Secrets, Session};
use chirper_net_whatsapp::WhatsApp;

/// Secrets kept in memory for the length of the probe.
#[derive(Default)]
struct Memory(Mutex<HashMap<String, Vec<u8>>>);

impl Secrets for Memory {
    fn get(&self, key: &str) -> BridgeResult<Option<zeroize::Zeroizing<Vec<u8>>>> {
        Ok(self
            .0
            .lock()
            .map_err(|_| chirper_bridge::Error::Internal("poisoned".to_owned()))?
            .get(key)
            .map(|value| zeroize::Zeroizing::new(value.clone())))
    }

    fn set(&self, key: &str, value: &[u8]) -> BridgeResult<()> {
        self.0
            .lock()
            .map_err(|_| chirper_bridge::Error::Internal("poisoned".to_owned()))?
            .insert(key.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, key: &str) -> BridgeResult<()> {
        self.0
            .lock()
            .map_err(|_| chirper_bridge::Error::Internal("poisoned".to_owned()))?
            .remove(key);
        Ok(())
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let record = hex::decode(std::env::var("CHIRPER_WHATSAPP_DEVICE")?.trim())?;
    let secrets = Arc::new(Memory::default());
    let ctx = LoginCtx {
        network: "whatsapp".to_owned(),
        login_id: "probe".to_owned(),
        data_dir: std::env::temp_dir().join("chirper-whatsapp-probe"),
        secrets: Arc::clone(&secrets) as Arc<dyn Secrets>,
        http: chirper_bridge::http_client(),
    };
    secrets.set(&ctx.secret_key("device"), &record)?;

    let mut session: Box<dyn Session> = WhatsApp::new().connect(ctx).await?;
    let deadline = Instant::now() + Duration::from_secs(60);
    let (mut chats, mut messages) = (0usize, 0usize);
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline.into(), session.next()).await {
        match event {
            Event::Chat(_) => chats += 1,
            Event::Message(_) => messages += 1,
            Event::State { state, .. } if state.is_terminal() || state.should_retry() => {
                println!("state {state:?}");
                break;
            }
            Event::State {
                state: SessionState::Running,
                ..
            } => println!("running"),
            _ => {}
        }
    }
    println!("chats {chats}, incoming messages {messages}");
    Ok(())
}
