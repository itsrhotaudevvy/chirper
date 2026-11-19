//! Keeps secrets out of log lines and out of error strings the UI shows.
//!
//! Remote errors often echo what the network sent back: a URL with a token in its query, an
//! `Authorization` header, a JSON body with an access token. [`redact`] removes three shapes:
//!
//! * the value after a marker word (`token=`, `"password":`, `Bearer `, `cookie: `, …),
//! * a Discord bot or user token (`<24+ chars>.<6 chars>.<27+ chars>`),
//! * any other long opaque run that mixes letters and digits (base64 or hex keys).

/// Words after which the next value is a secret.
const MARKERS: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "cookie",
    "session",
    "authorization",
    "bearer",
    "api_key",
    "apikey",
];
/// Authentication schemes that precede the real secret after `authorization:`.
const SCHEMES: &[&str] = &["bearer", "basic"];
/// Characters that may sit between a marker and its value.
const SEPARATORS: &[u8] = b" \"':=";
/// Characters that end a marked value.
const DELIMITERS: &[u8] = b" ,}\"')\r\n";
/// Shortest opaque run that is treated as a key.
const OPAQUE_MIN: usize = 32;

/// Remove token-shaped and marker-led secrets from `input`, keeping everything else.
pub fn redact(input: &str) -> String {
    redact_tokens(&redact_marked(input))
}

/// The value after each marker word becomes `[redacted]`.
fn redact_marked(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    if !MARKERS.iter().any(|marker| lower.contains(marker)) {
        return input.to_owned();
    }
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        let marker = MARKERS
            .iter()
            .find(|marker| lower[i..].starts_with(**marker));
        if let Some(marker) = marker {
            out.push_str(&input[i..i + marker.len()]);
            i += marker.len();
            while i < bytes.len() && SEPARATORS.contains(&bytes[i]) {
                out.push(bytes[i] as char);
                i += 1;
            }
            let mut start = i;
            while i < bytes.len() && !DELIMITERS.contains(&bytes[i]) {
                i += 1;
            }
            // `Authorization: Bearer <token>`: the scheme word is not the secret, the next value is.
            if SCHEMES
                .iter()
                .any(|s| input[start..i].eq_ignore_ascii_case(s))
            {
                out.push_str(&input[start..i]);
                start = i;
                while i < bytes.len() && SEPARATORS.contains(&bytes[i]) {
                    out.push(bytes[i] as char);
                    i += 1;
                }
                start = start.max(i);
                while i < bytes.len() && !DELIMITERS.contains(&bytes[i]) {
                    i += 1;
                }
            }
            if i > start {
                out.push_str("[redacted]");
            }
        } else {
            let ch = input[i..].chars().next().unwrap_or(' ');
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// Each run of token characters that has the shape of a key becomes `[redacted]`.
fn redact_tokens(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut run_start: Option<usize> = None;
    for (i, ch) in input.char_indices() {
        if is_token_char(ch) {
            run_start.get_or_insert(i);
            continue;
        }
        if let Some(start) = run_start.take() {
            push_token(&mut out, &input[start..i]);
        }
        out.push(ch);
    }
    if let Some(start) = run_start {
        push_token(&mut out, &input[start..]);
    }
    out
}

fn is_token_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '+' | '/' | '=' | '.')
}

fn push_token(out: &mut String, run: &str) {
    if is_discord_token(run) || is_opaque_key(run) {
        out.push_str("[redacted]");
    } else {
        out.push_str(run);
    }
}

/// `[\w-]{24,}\.[\w-]{6}\.[\w-]{27,}`, the shape of a Discord token.
fn is_discord_token(run: &str) -> bool {
    let parts: Vec<&str> = run.split('.').collect();
    let [first, middle, last] = parts.as_slice() else {
        return false;
    };
    let word = |part: &str| {
        part.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    };
    first.len() >= 24
        && middle.len() == 6
        && last.len() >= 27
        && word(first)
        && word(middle)
        && word(last)
}

/// A long run of base64 or hex characters that mixes letters and digits.
fn is_opaque_key(run: &str) -> bool {
    run.len() >= OPAQUE_MIN
        && run
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'+' | b'/' | b'='))
        && run.bytes().any(|b| b.is_ascii_digit())
        && run.bytes().any(|b| b.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn redacts_values_after_a_marker() {
        let out = redact("login failed token=abc123 code=1");
        assert!(!out.contains("abc123"), "{out}");
        assert!(out.contains("code=1"), "{out}");
        let json = redact(r#"{"access_token":"xyz","ok":true}"#);
        assert!(!json.contains("xyz"), "{json}");
    }

    #[test]
    fn redacts_a_bearer_credential() {
        let out = redact("request failed: Authorization: Bearer qwe.rty-uio_123");
        assert!(!out.contains("qwe.rty"), "{out}");
        assert!(out.contains("request failed"), "{out}");
    }

    #[test]
    fn redacts_a_discord_token_without_a_marker() {
        // Built from parts so secret scanners do not mistake this synthetic fixture for a token
        // (the first part is base64 of "123456789012345678").
        let token = [
            "MTIzNDU2Nzg5MDEyMzQ1Njc4",
            "AbCdEf",
            "abcdefghijklmnopqrstuvwxyz0",
        ]
        .join(".");
        let token = token.as_str();
        let out = redact(&format!("gateway rejected {token} for login"));
        assert!(!out.contains(token), "{out}");
        assert!(out.contains("gateway rejected"), "{out}");
        assert!(out.contains("for login"), "{out}");
    }

    #[test]
    fn redacts_a_long_opaque_key() {
        let key = "QWxhZGRpbjpvcGVuIHNlc2FtZQ0xmZTZiNWJkM2Q2";
        let out = redact(&format!("key {key} rejected"));
        assert!(!out.contains(key), "{out}");
    }

    #[test]
    fn keeps_ids_versions_and_plain_words() {
        assert_eq!(redact("connected to discord"), "connected to discord");
        let ids = "discord_1234567890123456789 chirper-bridge 0.2.1 fluxer/4242";
        assert_eq!(redact(ids), ids);
    }

    #[test]
    fn keeps_text_that_is_not_a_key_shape() {
        // A long run of letters only, and digits only, are not keys.
        let letters = "abcdefghijklmnopqrstuvwxyzabcdefgh";
        let digits = "12345678901234567890123456789012";
        assert_eq!(redact(letters), letters);
        assert_eq!(redact(digits), digits);
    }

    #[test]
    fn survives_non_ascii_text() {
        let out = redact("grüße token=abc ✓ done");
        assert!(!out.contains("abc"), "{out}");
        assert!(out.contains("grüße") && out.contains("✓ done"), "{out}");
    }
}
