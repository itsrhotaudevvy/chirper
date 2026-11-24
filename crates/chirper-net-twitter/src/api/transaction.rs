//! The `x-client-transaction-id` header: the signature the web client puts on every request. It
//! is made from the verification token of the messages page and an animation derived from that
//! page's loading icons and from one of X's scripts. Reference: `twittermeow/crypto/transaction.go`,
//! `crypto/animation.go`, `methods/html.go` and `client.go` (`parseMainPageHTML`,
//! `fetchAndParseSScript`).
//!
//! When the page cannot be read, the reference still sends the fallback `ZTo` (base64 of `e:`).
//! The driver does the same.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use regex::Regex;
use sha2::{Digest, Sha256};

use super::{Api, Body, Opts};

/// The header value sent when no signature can be made.
pub const FALLBACK_ID: &str = "ZTo";
/// The salt X mixes into every hash.
const SALT: [u8; 11] = [161, 183, 226, 163, 7, 171, 122, 24, 171, 138, 120];
/// The epoch the timestamp counts from, in Unix seconds.
const EPOCH_SECS: i64 = 1_682_924_400;
/// The loading icons of the messages page: four icons, sixteen frames, eleven numbers each.
pub type Loading = [[[i64; 11]; 16]; 4];
/// Milliseconds in the animation's time base.
const TOTAL_TIME: f64 = 4096.0;

/// The part of the page a signature needs, kept for the life of a login.
#[derive(Clone, Debug)]
pub struct Signer {
    verification: Vec<u8>,
    animation: String,
}

impl Signer {
    /// A signer from the verification token (base64) and the animation state. `None` when the token
    /// is not base64 or is too short to read.
    pub fn new(verification_token: &str, animation: String) -> Option<Self> {
        let verification = STANDARD.decode(verification_token.trim()).ok()?;
        (verification.len() >= 9).then_some(Self {
            verification,
            animation,
        })
    }

    /// The header value for a request: a signature for the method and the URL's path.
    pub fn header(&self, method: &str, path: &str) -> String {
        let first: u8 = rand::random();
        transaction_id(
            &self.verification,
            &self.animation,
            method,
            path,
            now_ts(),
            first,
        )
        .unwrap_or_else(|| FALLBACK_ID.to_owned())
    }
}

/// The timestamp the signature carries: seconds since the reference's epoch.
fn now_ts() -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    now - EPOCH_SECS
}

/// The signature itself, for given inputs. `first` is the random leading byte and `ts` the
/// timestamp from the epoch. `None` when the verification token is too short.
pub fn transaction_id(
    verification: &[u8],
    animation: &str,
    method: &str,
    path: &str,
    ts: i64,
    first: u8,
) -> Option<String> {
    // The reference also hashes a byte of the SDP text here, but its output keeps only the first
    // sixteen bytes of the digest, so that byte never reaches the signature.
    if verification.len() < 9 {
        return None;
    }
    let salt = STANDARD_NO_PAD.encode(SALT);
    let input = format!("{method}!{path}!{ts}{salt}{animation}");
    let digest = Sha256::digest(input.as_bytes());

    let mut plain = Vec::with_capacity(1 + verification.len() + 4 + 16 + 1);
    plain.push(first);
    plain.extend_from_slice(verification);
    plain.extend_from_slice(&(ts as u32).to_le_bytes());
    plain.extend_from_slice(&digest[..16]);
    plain.push(3);
    Some(STANDARD_NO_PAD.encode(xor_encode(&plain)))
}

/// Every byte after the first is XORed with the first.
fn xor_encode(plain: &[u8]) -> Vec<u8> {
    let Some((&key, rest)) = plain.split_first() else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(plain.len());
    out.push(key);
    out.extend(rest.iter().map(|b| b ^ key));
    out
}

/// The animation state of a page, from its verification token and its loading icons.
pub fn animation_state(
    indexes: [usize; 4],
    loading: &Loading,
    verification: &[u8],
) -> Option<String> {
    let byte = |i: usize| verification.get(i).copied().map(usize::from);
    let icon = loading.get(byte(5)? % 4)?;
    let frame = icon.get(byte(indexes[0])? % 16)?;
    let time = (byte(indexes[1])? % 16) * (byte(indexes[2])? % 16) * (byte(indexes[3])? % 16);
    Some(animation_frame(frame, time))
}

/// One frame: a colour, a rotation and a cubic easing, rendered as hex.
fn animation_frame(row: &[i64; 11], time: usize) -> String {
    if time as f64 >= TOTAL_TIME - 1.0 {
        return "000".to_owned();
    }
    let from_color = [row[0] as f64, row[1] as f64, row[2] as f64, 1.0];
    let to_color = [row[3] as f64, row[4] as f64, row[5] as f64, 1.0];
    let to_rotation = (map_range(row[6] as f64, 60.0, 360.0)).floor();
    let mut curves = [0.0f64; 4];
    for (i, curve) in curves.iter_mut().enumerate() {
        *curve = to_fixed(map_range(row[7 + i] as f64, is_even(i), 1.0), 2);
    }
    let value = cubic_value(
        curves,
        (round_half_away(time as f64 / 10.0) * 10.0) / TOTAL_TIME,
    );
    let color = interpolate(&from_color, &to_color, value);
    let rotation = interpolate(&[0.0], &[to_rotation], value);
    let matrix = rotation_matrix(rotation[0]);

    let mut out = String::new();
    for channel in &color[..3] {
        let clamped = channel.max(0.0);
        out.push_str(&format!("{:x}", round_half_away(clamped) as i64));
    }
    for entry in &matrix[..4] {
        let mut rounded = to_fixed(*entry, 2);
        if rounded < 0.0 {
            rounded = -rounded;
        }
        out.push_str(&float_to_hex(rounded));
    }
    out.push_str("00");
    out
}

fn map_range(value: f64, min: f64, max: f64) -> f64 {
    value * (max - min) / 255.0 + min
}

fn is_even(index: usize) -> f64 {
    if index % 2 == 1 { -1.0 } else { 0.0 }
}

fn round_half_away(value: f64) -> f64 {
    value.round()
}

/// The reference's `toFixed`: round half away from zero to `precision` decimals.
fn to_fixed(num: f64, precision: i32) -> f64 {
    let scale = 10f64.powi(precision);
    let scaled = num * scale;
    let rounded = (scaled + 0.5f64.copysign(scaled)) as i64;
    rounded as f64 / scale
}

fn interpolate(from: &[f64], to: &[f64], f: f64) -> Vec<f64> {
    from.iter()
        .zip(to)
        .map(|(a, b)| a * (1.0 - f) + b * f)
        .collect()
}

fn rotation_matrix(degrees: f64) -> [f64; 6] {
    let radians = degrees * std::f64::consts::PI / 180.0;
    let (s, c) = radians.sin_cos();
    [c, s, -s, c, 0.0, 0.0]
}

/// A cubic Bézier easing with control points `curves`, evaluated at `time` in `0..=1`.
fn cubic_value(curves: [f64; 4], time: f64) -> f64 {
    if time <= 0.0 {
        let start = if curves[0] > 0.0 {
            curves[1] / curves[0]
        } else if curves[1] == 0.0 && curves[2] > 0.0 {
            curves[3] / curves[2]
        } else {
            0.0
        };
        return start * time;
    }
    if time >= 1.0 {
        let end = if curves[2] < 1.0 {
            (curves[3] - 1.0) / (curves[2] - 1.0)
        } else if curves[2] == 1.0 && curves[0] < 1.0 {
            (curves[1] - 1.0) / (curves[0] - 1.0)
        } else {
            0.0
        };
        return 1.0 + end * (time - 1.0);
    }
    let (mut start, mut end) = (0.0f64, 1.0f64);
    let mut mid = 0.0f64;
    while start < end {
        mid = (start + end) / 2.0;
        let estimate = bezier(curves[0], curves[2], mid);
        if (time - estimate).abs() < 0.00001 {
            return bezier(curves[1], curves[3], mid);
        }
        if estimate < time {
            start = mid;
        } else {
            end = mid;
        }
    }
    bezier(curves[1], curves[3], mid)
}

fn bezier(a: f64, b: f64, m: f64) -> f64 {
    3.0 * a * (1.0 - m) * (1.0 - m) * m + 3.0 * b * (1.0 - m) * m * m + m * m * m
}

/// The reference's `floatToHex`: the integer part in hex, then the fraction in hex digits.
fn float_to_hex(x: f64) -> String {
    let quotient = x as i64;
    let mut fraction = x - quotient as f64;
    let mut out = format!("{quotient:x}");
    if fraction == 0.0 {
        return out;
    }
    while fraction > 0.0 {
        fraction *= 16.0;
        let digit = fraction as i64;
        out.push_str(&format!("{digit:x}"));
        fraction -= digit as f64;
    }
    out
}

/// The verification token of the messages page: the content of its `twitter-site-verification`
/// style meta tag.
pub fn verification_token(html: &str) -> Option<String> {
    let re =
        Regex::new(r#"<meta[^>]*\bname=["']tw[^"']*["'][^>]*\bcontent=["']([^"']+)["']"#).ok()?;
    re.captures(html).map(|c| c[1].to_owned())
}

/// The four loading icons of the messages page, or `None` unless exactly four read correctly.
pub fn loading_animations(html: &str) -> Option<Box<Loading>> {
    let svg =
        Regex::new(r#"(?s)<svg[^>]*\bid=["']loading-x-anim[^"']*["'][^>]*>(.*?)</svg>"#).ok()?;
    let path = Regex::new(r#"<path[^>]*\bd=["']([^"']*)["']"#).ok()?;
    let mut icons = Vec::new();
    for body in svg.captures_iter(html) {
        let Some(d) = path.captures_iter(&body[1]).nth(1).map(|c| c[1].to_owned()) else {
            continue;
        };
        if let Some(icon) = parse_icon(&d) {
            icons.push(icon);
        }
    }
    let array: [[[i64; 11]; 16]; 4] = icons.try_into().ok()?;
    Some(Box::new(array))
}

/// One icon's frames from its path: the numbers after the first nine bytes, one frame per `C`.
fn parse_icon(d: &str) -> Option<[[i64; 11]; 16]> {
    let body = d.get(9..)?;
    let frames: Vec<&str> = body.split('C').collect();
    if frames.len() != 16 {
        return None;
    }
    let mut out = [[0i64; 11]; 16];
    for (slot, frame) in out.iter_mut().zip(frames) {
        let numbers: Vec<i64> = frame
            .split(|c: char| !c.is_ascii_digit())
            .filter(|n| !n.is_empty())
            .map(str::parse)
            .collect::<Result<_, _>>()
            .ok()?;
        *slot = numbers.try_into().ok()?;
    }
    Some(out)
}

/// The four byte indexes X's script reads the animation from, or `None` when the script does not
/// have the expected shape.
pub fn variable_indexes(script: &str) -> Option<[usize; 4]> {
    let re = Regex::new(
        r"\[.+?\(\w{1,2}\[(\d{1,2})\],16\).+?\(\w{1,2}\[(\d{1,2})\],16\).+?\(\w{1,2}\[(\d{1,2})\],16\).+?\(\w{1,2}\[(\d{1,2})\],16\)",
    )
    .ok()?;
    let caps = re.captures(script)?;
    let mut out = [0usize; 4];
    for (slot, index) in out.iter_mut().zip(1..=4) {
        *slot = caps.get(index)?.as_str().parse().ok()?;
    }
    Some(out)
}

/// The address of X's `ondemand.s` script, from the page or the main script that names it, on
/// the given scripts origin.
pub fn ondemand_s_url(source: &str, scripts: &str) -> Option<String> {
    let chunk = Regex::new(r#"(\d+):"ondemand\.s""#).ok()?;
    let found = chunk.captures(source)?;
    let id = found.get(1)?.as_str();
    let after = &source[found.get(0)?.end()..];
    let hash_re = Regex::new(&format!(
        r#"(?:^|[,{{]){}:"([0-9a-f]+)""#,
        regex::escape(id)
    ))
    .ok()?;
    let hash = hash_re.captures(after)?.get(1)?.as_str();
    Some(format!(
        "{scripts}/responsive-web/client-web/ondemand.s.{hash}a.js"
    ))
}

/// The address of the main script the messages page loads.
pub fn main_script_url(html: &str) -> Option<String> {
    let re = Regex::new(r"https://[A-Za-z0-9.-]+/responsive-web/client-web/main\.[0-9A-Za-z]+\.js")
        .ok()?;
    re.find(html).map(|m| m.as_str().to_owned())
}

/// Build the signer of a login: read the messages page, then the script that names the animation.
/// Returns `None` when any part is missing, and the caller sends the fallback instead.
pub async fn load(api: &Api) -> Option<Signer> {
    let page = api
        .call(
            reqwest::Method::GET,
            &format!("{}/messages", api.bases().web),
            Body::None,
            &Opts::default(),
        )
        .await
        .ok()?;
    let html = String::from_utf8_lossy(&page).to_string();
    let verification = verification_token(&html)?;
    let loading = loading_animations(&html)?;
    let scripts = api.bases().scripts.clone();
    let url = match ondemand_s_url(&html, &scripts) {
        Some(url) => url,
        None => {
            let main = main_script_url(&html)?;
            let main_body = api.download(&main, SCRIPT_CAP).await.ok()??;
            ondemand_s_url(&String::from_utf8_lossy(&main_body), &scripts)?
        }
    };
    let script = api.download(&url, SCRIPT_CAP).await.ok()??;
    let indexes = variable_indexes(&String::from_utf8_lossy(&script))?;
    let decoded = STANDARD.decode(verification.trim()).ok()?;
    let animation = animation_state(indexes, &loading, &decoded)?;
    Signer::new(&verification, animation)
}

/// The largest script the bootstrap reads.
const SCRIPT_CAP: u64 = 16 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    /// Vectors computed by the reference's own algorithm (`go run` over a copy of its functions).
    const VERIFICATION_LEN: usize = 40;

    fn fixed_verification() -> Vec<u8> {
        (0..VERIFICATION_LEN)
            .map(|i| ((i * 37 + 11) % 256) as u8)
            .collect()
    }

    fn fixed_loading() -> Loading {
        let mut loading = [[[0i64; 11]; 16]; 4];
        for (i, icon) in loading.iter_mut().enumerate() {
            for (j, frame) in icon.iter_mut().enumerate() {
                for (k, value) in frame.iter_mut().enumerate() {
                    *value = ((i * 97 + j * 31 + k * 53) % 256) as i64;
                }
            }
        }
        loading
    }

    #[test]
    fn the_animation_state_matches_the_reference() {
        let animation = animation_state([3, 7, 12, 20], &fixed_loading(), &fixed_verification())
            .expect("the fixed inputs are valid");
        assert_eq!(
            animation,
            "60518609eb851eb851eb80c7ae147ae147b0c7ae147ae147b09eb851eb851eb800"
        );
    }

    #[test]
    fn the_transaction_id_matches_the_reference() {
        let animation = "60518609eb851eb851eb80c7ae147ae147b0c7ae147ae147b09eb851eb851eb800";
        let id = transaction_id(
            &fixed_verification(),
            animation,
            "GET",
            "/i/api/1.1/dm/user_updates.json",
            123_456,
            0x5a,
        );
        assert_eq!(
            id.as_deref(),
            Some(
                "WlFqDyDFnrNUaQIn+J22S2wB2v+QtU5jBNnyl6hNZjvc8YqvQGU+0/QauFtaYsuJzqXJXp8hz9sMsh1rtVk"
            )
        );
    }

    #[test]
    fn an_empty_animation_gives_the_reference_id() {
        let id = transaction_id(
            &fixed_verification(),
            "",
            "POST",
            "/i/api/1.1/dm/new2.json",
            99,
            0x01,
        );
        assert_eq!(
            id.as_deref(),
            Some(
                "AQoxVHuexegPMll8o8btEDdagaTL7hU4X4KpzPMWPWCHqtH0Gz5liK9iAQEBtNslrsgNfM5cptr/sOc3kgI"
            )
        );
    }

    #[test]
    fn a_short_verification_token_gives_no_signature() {
        assert!(transaction_id(&[1, 2, 3], "", "GET", "/", 1, 0).is_none());
        assert!(Signer::new("AQI=", String::new()).is_none());
        assert!(Signer::new("not base64!", String::new()).is_none());
    }

    #[test]
    fn the_xor_key_is_the_first_byte() {
        assert_eq!(xor_encode(&[2, 3, 4]), vec![2, 1, 6]);
        assert!(xor_encode(&[]).is_empty());
    }

    #[test]
    fn the_page_yields_its_verification_token() {
        let html = r#"<head><meta name="twitter-site-verification" content="AQIDBA=="></head>"#;
        assert_eq!(verification_token(html).as_deref(), Some("AQIDBA=="));
        assert!(verification_token("<head></head>").is_none());
    }

    #[test]
    fn the_script_names_the_four_byte_indexes() {
        let script = "x=[a(b[3],16)+c(d[7],16)+e(f[12],16)+g(h[20],16)]";
        assert_eq!(variable_indexes(script), Some([3, 7, 12, 20]));
        assert_eq!(variable_indexes("nothing here"), None);
    }

    #[test]
    fn the_ondemand_script_address_uses_its_hash_on_the_given_origin() {
        let page = r#"{123:"ondemand.s",456:"zzz"}, {123:"abc123def"}"#;
        assert_eq!(
            ondemand_s_url(page, "https://abs.twimg.com").as_deref(),
            Some("https://abs.twimg.com/responsive-web/client-web/ondemand.s.abc123defa.js")
        );
        assert!(ondemand_s_url("{}", "https://abs.twimg.com").is_none());
    }

    #[test]
    fn loading_icons_need_exactly_four_readable_paths() {
        let frame = (0..16)
            .map(|_| " 1 2 3 4 5 6 7 8 9 10 11")
            .collect::<Vec<_>>()
            .join("C");
        let d = format!("M00000000{frame}");
        let svg = format!(r#"<svg id="loading-x-anim-0"><path d="a"/><path d="{d}"/></svg>"#);
        let one_icon = svg.clone();
        assert!(loading_animations(&one_icon).is_none());
        let four = svg.repeat(4);
        let loading = loading_animations(&four).expect("four icons");
        assert_eq!(loading[0][0], [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
    }
}
