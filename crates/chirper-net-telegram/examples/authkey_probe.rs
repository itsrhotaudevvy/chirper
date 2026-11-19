//! Live check of the MTProto client: create an auth key with production data centre 2, then send
//! an encrypted `ping` and wait for its `pong`. Prints one line, never any key bytes.
//!
//! Run with `cargo run -p chirper-net-telegram --example authkey_probe`.

use std::time::Duration;

use chirper_net_telegram::mtproto::{self, MtSession};
use chirper_net_telegram::tl::{enums, functions};

const DC_ID: i32 = 2;
const PONG_TIMEOUT: Duration = Duration::from_secs(20);

#[tokio::main]
async fn main() {
    if let Err(message) = probe().await {
        println!("probe failed: {message}");
        std::process::exit(1);
    }
}

async fn probe() -> Result<(), String> {
    let dc = mtproto::production_dc(DC_ID).ok_or("unknown data centre")?;
    let mut transport = mtproto::connect(dc).await.map_err(|e| e.to_string())?;
    let auth = mtproto::generate_auth_key(&mut transport, DC_ID)
        .await
        .map_err(|e| e.to_string())?;
    let fingerprint = auth.key.id();
    let (session, _updates) =
        MtSession::start(transport, auth.key, auth.server_salt, auth.time_offset)
            .map_err(|e| e.to_string())?;

    let ping_id =
        i64::from_le_bytes(mtproto::crypto::random_array::<8>().map_err(|e| e.to_string())?);
    let answer = tokio::time::timeout(PONG_TIMEOUT, session.invoke(&functions::Ping { ping_id }))
        .await
        .map_err(|_| "no pong within 20 s".to_owned())?
        .map_err(|e| e.to_string())?;
    match answer {
        enums::Pong::Pong(pong) if pong.ping_id == ping_id => {
            println!("auth key ok, fingerprint {fingerprint:016x}, pong ok");
            Ok(())
        }
        _ => Err("pong did not echo the ping id".to_owned()),
    }
}
