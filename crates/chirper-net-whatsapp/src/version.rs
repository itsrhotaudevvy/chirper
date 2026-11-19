//! The WhatsApp web client version. The server rejects a client whose version is too old (failure
//! reason 405), so the version is read from the web client's service worker at login, as the web
//! client itself does, rather than pinned.
//!
//! The parsing follows wacore 0.7.0 (MIT), https://github.com/jlucaso1/whatsapp-rust
//! (`wacore::version::parse_sw_js`).

use chirper_bridge::{Error, Result};

/// The service worker that names the current client revision.
pub const SW_URL: &str = "https://web.whatsapp.com/sw.js";

/// A browser user agent for the fetch; the endpoint serves the worker to browsers.
const BROWSER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

/// The largest service worker accepted: 4 MiB. The real file is about 16 KiB.
const MAX_BODY: usize = 4 << 20;

/// The current `(2, 3000, revision)` version, fetched from the service worker.
pub async fn fetch(http: &reqwest::Client) -> Result<(u32, u32, u32)> {
    let mut response = http
        .get(SW_URL)
        .header("sec-fetch-site", "none")
        .header(reqwest::header::USER_AGENT, BROWSER_AGENT)
        .send()
        .await
        .map_err(|err| {
            if err.is_timeout() || err.is_connect() {
                Error::Network("could not reach WhatsApp for the client version".to_owned())
            } else {
                Error::Protocol("the client version request failed".to_owned())
            }
        })?;
    let status = response.status();
    if status.is_server_error() {
        return Err(Error::Network(format!(
            "WhatsApp answered {status} for the client version"
        )));
    }
    if !status.is_success() {
        return Err(Error::Protocol(format!(
            "WhatsApp answered {status} for the client version"
        )));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Error::Network("reading the client version failed".to_owned()))?
    {
        if body.len() + chunk.len() > MAX_BODY {
            return Err(Error::Protocol(
                "the client version file is too large".to_owned(),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    let text = String::from_utf8_lossy(&body);
    let revision = parse_revision(&text)
        .ok_or_else(|| Error::Protocol("no client revision in the service worker".to_owned()))?;
    Ok((2, 3000, revision))
}

/// The revision number after the `client_revision` key. The number must follow the key closely,
/// so an unrelated number further on is not mistaken for it.
pub fn parse_revision(text: &str) -> Option<u32> {
    const KEY: &str = "client_revision";
    let after = &text[text.find(KEY)? + KEY.len()..];
    let start = after.find(|c: char| c.is_ascii_digit())?;
    if start > 8 {
        return None;
    }
    let digits: String = after[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_revision_in_each_serialised_form() {
        assert_eq!(
            parse_revision(r#"{"client_revision":1055354264}"#),
            Some(1_055_354_264)
        );
        assert_eq!(
            parse_revision(r#"x = {\"client_revision\":123};"#),
            Some(123)
        );
        assert_eq!(parse_revision("client_revision: 42;"), Some(42));
    }

    #[test]
    fn a_distant_number_is_not_the_revision() {
        assert_eq!(parse_revision("client_revision ........... 7"), None);
        assert_eq!(parse_revision("no key here 999"), None);
    }
}
