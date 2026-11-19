//! The ALTCHA proof-of-work that Fluxer asks for before some sign-ins.
//!
//! Port of `fluxerapi/captcha.go`. The Go connector solves the challenge inside the login call and
//! never shows the user a captcha step, so this port does the same: there is no `LoginStep` for it,
//! and a challenge that cannot be solved is a protocol error.
//!
//! The search is the one the server verifies: for counter `n` the password is
//! `nonce || uint32_be(n)`, and the PBKDF2-HMAC-SHA256 output must start with `keyPrefix`.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use base64::prelude::{BASE64_STANDARD, Engine as _};
use serde::Deserialize;
use sha2::Sha256;

use chirper_bridge::{Error, Result};

use crate::api::CallErr;

/// The only algorithm Fluxer issues.
const ALGORITHM: &str = "PBKDF2/SHA-256";
/// How many fresh challenges one request may solve (matches the Go `maxCaptchaRounds`).
const MAX_ROUNDS: usize = 3;
/// Upper bounds for server-chosen parameters, so a hostile challenge cannot pin the CPU or memory.
const MAX_COST: u32 = 1_000_000;
const MAX_KEY_LENGTH: usize = 256;
const MAX_FIELD_BYTES: usize = 1024;

#[derive(Deserialize)]
struct Challenge {
    parameters: Parameters,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Parameters {
    algorithm: String,
    nonce: String,
    salt: String,
    cost: i64,
    #[serde(rename = "keyLength")]
    key_length: i64,
    #[serde(rename = "keyPrefix")]
    key_prefix: String,
}

/// A validated challenge, ready to search.
struct Search {
    nonce: Vec<u8>,
    salt: Vec<u8>,
    prefix: Vec<u8>,
    cost: u32,
    key_length: usize,
}

fn parse_search(raw: &str) -> Result<Search> {
    let unsupported = || Error::Protocol("unsupported CAPTCHA challenge parameters".to_owned());
    let challenge: Challenge = serde_json::from_str(raw)
        .map_err(|_| Error::Protocol("malformed CAPTCHA challenge".to_owned()))?;
    let p = challenge.parameters;
    if p.algorithm != ALGORITHM {
        return Err(unsupported());
    }
    let nonce = hex::decode(&p.nonce).map_err(|_| unsupported())?;
    let salt = hex::decode(&p.salt).map_err(|_| unsupported())?;
    let prefix = hex::decode(&p.key_prefix).map_err(|_| unsupported())?;
    let cost = u32::try_from(p.cost)
        .ok()
        .filter(|c| (1..=MAX_COST).contains(c))
        .ok_or_else(unsupported)?;
    let key_length = usize::try_from(p.key_length)
        .ok()
        .filter(|k| (1..=MAX_KEY_LENGTH).contains(k))
        .ok_or_else(unsupported)?;
    if nonce.is_empty()
        || nonce.len() > MAX_FIELD_BYTES
        || salt.len() > MAX_FIELD_BYTES
        || prefix.is_empty()
        || prefix.len() > key_length
    {
        return Err(unsupported());
    }
    Ok(Search {
        nonce,
        salt,
        prefix,
        cost,
        key_length,
    })
}

/// Sets a stop flag when dropped, so an abandoned login stops its search.
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Find the first counter whose derived key starts with the prefix. The counter space is split
/// between the CPUs: thread `w` tries `w, w + N, w + 2N, ...`.
fn search(params: &Search, stop: &AtomicBool) -> Option<(u32, Vec<u8>)> {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 64);
    let found = std::sync::Mutex::new(None);
    std::thread::scope(|scope| {
        for worker in 0..threads {
            let found = &found;
            scope.spawn(move || {
                let mut password = params.nonce.clone();
                password.extend_from_slice(&[0u8; 4]);
                let tail = params.nonce.len();
                let mut key = vec![0u8; params.key_length];
                let mut n = worker as u64;
                while n <= u64::from(u32::MAX) && !stop.load(Ordering::Relaxed) {
                    let counter = u32::try_from(n).unwrap_or(u32::MAX);
                    password[tail..].copy_from_slice(&counter.to_be_bytes());
                    pbkdf2::pbkdf2_hmac::<Sha256>(&password, &params.salt, params.cost, &mut key);
                    if key.starts_with(&params.prefix) {
                        let mut slot = found
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if slot.is_none() {
                            *slot = Some((counter, key.clone()));
                        }
                        stop.store(true, Ordering::Relaxed);
                        return;
                    }
                    n += threads as u64;
                }
            });
        }
    });
    found
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Solve an ALTCHA challenge and return the value for the `X-Captcha-Token` header. The challenge
/// is passed back verbatim, as the server expects it.
pub async fn solve(raw_challenge: &str) -> Result<String> {
    let params = parse_search(raw_challenge)?;
    let stop = Arc::new(AtomicBool::new(false));
    let guard = StopOnDrop(Arc::clone(&stop));
    let solved = tokio::task::spawn_blocking(move || search(&params, &stop))
        .await
        .map_err(|_| Error::Internal("the CAPTCHA search stopped".to_owned()))?;
    drop(guard);
    let (counter, key) = solved
        .ok_or_else(|| Error::Protocol("the CAPTCHA challenge has no solution".to_owned()))?;
    let body = format!(
        r#"{{"challenge":{raw_challenge},"solution":{{"counter":{counter},"derivedKey":"{}"}}}}"#,
        hex::encode(key)
    );
    Ok(BASE64_STANDARD.encode(body))
}

/// Run a call, and when the server asks for a CAPTCHA, solve it and call again with the token.
/// `call` receives the captcha token of the previous round, if any.
pub async fn with_captcha<T, F, Fut>(mut call: F) -> std::result::Result<T, CallErr>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: Future<Output = std::result::Result<T, CallErr>>,
{
    let mut captcha = None;
    for _ in 0..MAX_ROUNDS {
        match call(captcha.take()).await {
            Err(CallErr::Api(err)) if err.is_captcha_required() => {
                let challenge = err.captcha_challenge.unwrap_or_default();
                captcha = Some(solve(&challenge).await?);
            }
            other => return other,
        }
    }
    Err(CallErr::Other(Error::Protocol(
        "could not pass the CAPTCHA challenge".to_owned(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cheap enough for a test: a one-byte prefix needs about 256 attempts at cost 1.
    const CHALLENGE: &str = r#"{"parameters":{"algorithm":"PBKDF2/SHA-256","nonce":"0102","salt":"0a0b","cost":1,"keyLength":16,"keyPrefix":"00","keySignature":"ff","expiresAt":4102444800},"signature":"abcd"}"#;

    #[tokio::test]
    async fn solution_verifies_and_echoes_the_challenge() {
        let token = solve(CHALLENGE).await.expect("the challenge is solvable");
        let decoded = BASE64_STANDARD.decode(token).expect("base64");
        let value: serde_json::Value = serde_json::from_slice(&decoded).expect("JSON");
        assert_eq!(value["challenge"]["signature"], "abcd");
        assert_eq!(value["challenge"]["parameters"]["nonce"], "0102");
        let counter = value["solution"]["counter"].as_u64().expect("counter") as u32;
        let key = value["solution"]["derivedKey"].as_str().expect("key");
        assert!(
            key.starts_with("00"),
            "derived key {key} must start with the prefix"
        );

        // Recompute the derivation the way the server does.
        let mut password = vec![0x01, 0x02];
        password.extend_from_slice(&counter.to_be_bytes());
        let mut check = [0u8; 16];
        pbkdf2::pbkdf2_hmac::<Sha256>(&password, &[0x0a, 0x0b], 1, &mut check);
        assert_eq!(hex::encode(check), key);
    }

    #[test]
    fn unsupported_parameters_are_refused() {
        let scrypt = r#"{"parameters":{"algorithm":"SCRYPT","nonce":"01","salt":"01","cost":1,"keyLength":8,"keyPrefix":"00"},"signature":"x"}"#;
        assert!(parse_search(scrypt).is_err());
        let zero_cost = r#"{"parameters":{"algorithm":"PBKDF2/SHA-256","nonce":"01","salt":"01","cost":0,"keyLength":8,"keyPrefix":"00"}}"#;
        assert!(parse_search(zero_cost).is_err());
        let huge_cost = r#"{"parameters":{"algorithm":"PBKDF2/SHA-256","nonce":"01","salt":"01","cost":4000000000,"keyLength":8,"keyPrefix":"00"}}"#;
        assert!(parse_search(huge_cost).is_err());
        let bad_hex = r#"{"parameters":{"algorithm":"PBKDF2/SHA-256","nonce":"zz","salt":"01","cost":1,"keyLength":8,"keyPrefix":"00"}}"#;
        assert!(parse_search(bad_hex).is_err());
        let long_prefix = r#"{"parameters":{"algorithm":"PBKDF2/SHA-256","nonce":"01","salt":"01","cost":1,"keyLength":1,"keyPrefix":"0000"}}"#;
        assert!(parse_search(long_prefix).is_err());
    }

    #[test]
    fn a_stopped_search_finds_nothing() {
        let params = parse_search(CHALLENGE).expect("valid");
        let stop = AtomicBool::new(true);
        assert!(search(&params, &stop).is_none());
    }

    #[tokio::test]
    async fn with_captcha_retries_once_with_the_solved_token() {
        let mut rounds = Vec::new();
        let result: std::result::Result<&str, CallErr> = with_captcha(|captcha| {
            rounds.push(captcha.is_some());
            let first = rounds.len() == 1;
            async move {
                if first {
                    Err(CallErr::Api(crate::api::ApiError::from_response(
                        400,
                        None,
                        format!(
                            r#"{{"code":"CAPTCHA_REQUIRED","captcha_provider":"altcha","altcha_challenge":{CHALLENGE}}}"#
                        )
                        .as_bytes(),
                    )))
                } else {
                    Ok("done")
                }
            }
        })
        .await;
        assert!(matches!(result, Ok("done")));
        assert_eq!(rounds, vec![false, true]);
    }

    #[tokio::test]
    async fn with_captcha_gives_up_after_three_rounds() {
        let result: std::result::Result<(), CallErr> = with_captcha(|_| async {
            Err(CallErr::Api(crate::api::ApiError::from_response(
                400,
                None,
                format!(
                    r#"{{"code":"INVALID_CAPTCHA","captcha_provider":"altcha","altcha_challenge":{CHALLENGE}}}"#
                )
                .as_bytes(),
            )))
        })
        .await;
        assert!(matches!(result, Err(CallErr::Other(Error::Protocol(_)))));
    }
}
