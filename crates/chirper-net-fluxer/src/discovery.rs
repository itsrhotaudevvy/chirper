//! Instance discovery: which origin a login talks to, and the URL policy every endpoint must pass.
//!
//! Port of `fluxerapi/discovery.go` and the policy half of `httpsec.go`. An instance is untrusted
//! input: its discovery document must not be able to point us at a private or local address, so
//! every endpoint URL it returns is checked here, not trusted.

use serde::Deserialize;
use url::{Host, Url};

use chirper_bridge::{Error, Result};

use crate::api::{MAX_JSON_BYTES, USER_AGENT, network_error, read_limited};

/// The official instance.
pub const DEFAULT_INSTANCE: &str = "https://fluxer.app";

/// The URL policy. Strict is the only policy a normal account can reach; the insecure policy
/// exists for tests only and has no constructor outside the test build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    allow_insecure: bool,
}

impl Policy {
    /// Https and wss only, and no local, private or IP-literal hosts.
    pub const fn strict() -> Self {
        Self {
            allow_insecure: false,
        }
    }

    /// Plain http and ws, and local hosts, for tests against a loopback server.
    #[cfg(test)]
    pub const fn insecure_for_tests() -> Self {
        Self {
            allow_insecure: true,
        }
    }

    /// Accept an absolute URL whose scheme is `secure`, or `insecure` when the policy allows it,
    /// and whose host passes [`Self::check_host`]. Error text never repeats the URL.
    pub fn check_url(&self, raw: &str, secure: &str, insecure: &str) -> Result<Url> {
        let url = Url::parse(raw).map_err(|_| Error::Protocol("not an absolute URL".to_owned()))?;
        if url.host().is_none() {
            return Err(Error::Protocol("not an absolute URL".to_owned()));
        }
        let scheme = url.scheme();
        if scheme != secure && !(scheme == insecure && self.allow_insecure) {
            return Err(Error::Protocol(format!("unexpected URL scheme {scheme}")));
        }
        match url.host() {
            Some(host) => self.check_host(&host)?,
            None => return Err(Error::Protocol("not an absolute URL".to_owned())),
        }
        Ok(url)
    }

    /// Refuse IP literals, local names and private, loopback, link-local and multicast addresses.
    /// The insecure policy accepts everything.
    fn check_host(&self, host: &Host<&str>) -> Result<()> {
        if self.allow_insecure {
            return Ok(());
        }
        match host {
            Host::Domain(name) => {
                let name = name.trim_end_matches('.').to_ascii_lowercase();
                if name == "localhost"
                    || name.ends_with(".localhost")
                    || name == "local"
                    || name.ends_with(".local")
                {
                    return Err(Error::Protocol("local host names are refused".to_owned()));
                }
                Ok(())
            }
            // The spec refuses IP literals outright: an instance has a name, and a literal is the
            // usual way to reach an internal address.
            Host::Ipv4(_) | Host::Ipv6(_) => {
                Err(Error::Protocol("IP-literal hosts are refused".to_owned()))
            }
        }
    }
}

/// Normalize an instance input. `example.com`, `https://example.com/` and similar become
/// `https://example.com`. An empty input means the official instance.
pub fn normalize_origin(origin: &str, policy: Policy) -> Result<String> {
    let trimmed = origin.trim();
    let raw = if trimmed.is_empty() {
        DEFAULT_INSTANCE.to_owned()
    } else if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };
    let url = Url::parse(&raw).map_err(|_| Error::Protocol("invalid instance URL".to_owned()))?;
    if url.host().is_none() {
        return Err(Error::Protocol("invalid instance URL".to_owned()));
    }
    policy.check_url(&raw, "https", "http")?;
    let host = url
        .host_str()
        .ok_or_else(|| Error::Protocol("invalid instance URL".to_owned()))?;
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    Ok(format!("{}://{host}{port}", url.scheme()))
}

/// The public service URLs of one instance, as discovery returned them and the policy accepted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Endpoints {
    /// REST base (`endpoints.api_public`, without the `/v1` suffix).
    pub api: String,
    /// WebSocket URL (`endpoints.gateway`).
    pub gateway: String,
    /// Media proxy base (`endpoints.media`); empty when the instance does not publish one.
    pub media: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct DiscoveryDoc {
    endpoints: RawEndpoints,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawEndpoints {
    api_public: String,
    gateway: String,
    media: String,
}

/// Parse a discovery document and validate every endpoint it names.
pub fn parse_discovery(raw: &[u8], policy: Policy) -> Result<Endpoints> {
    let doc: DiscoveryDoc = serde_json::from_slice(raw)
        .map_err(|e| Error::Protocol(format!("instance discovery JSON: {e}")))?;
    let endpoints = Endpoints {
        api: doc.endpoints.api_public.trim_end_matches('/').to_owned(),
        gateway: doc.endpoints.gateway,
        media: doc.endpoints.media.trim_end_matches('/').to_owned(),
    };
    policy
        .check_url(&endpoints.api, "https", "http")
        .map_err(|e| endpoint_error("api_public", e))?;
    policy
        .check_url(&endpoints.gateway, "wss", "ws")
        .map_err(|e| endpoint_error("gateway", e))?;
    if !endpoints.media.is_empty() {
        policy
            .check_url(&endpoints.media, "https", "http")
            .map_err(|e| endpoint_error("media", e))?;
    }
    Ok(endpoints)
}

fn endpoint_error(name: &str, err: Error) -> Error {
    match err {
        Error::Protocol(reason) => Error::Protocol(format!("endpoints.{name}: {reason}")),
        other => other,
    }
}

/// The most redirects discovery follows. fluxer.app sends its well-known document from
/// api.fluxer.app, so at least one is needed.
const MAX_DISCOVERY_REDIRECTS: usize = 5;

/// Resolve a `Location` header against the URL that sent it and check the target against the
/// policy, so a redirect can never lead discovery to a private address or plain http.
pub fn redirect_target(current: &str, location: &str, policy: Policy) -> Result<String> {
    let base =
        Url::parse(current).map_err(|_| Error::Protocol("invalid discovery URL".to_owned()))?;
    let target = base
        .join(location)
        .map_err(|_| Error::Protocol("instance discovery sent an invalid redirect".to_owned()))?;
    let target = target.to_string();
    policy.check_url(&target, "https", "http")?;
    Ok(target)
}

/// Fetch `{origin}/.well-known/fluxer` and return the endpoints it names. Redirects are followed
/// by hand (the shared client follows none), each target checked against the policy.
pub async fn discover(http: &reqwest::Client, origin: &str, policy: Policy) -> Result<Endpoints> {
    let base = normalize_origin(origin, policy)?;
    let mut url = format!("{base}/.well-known/fluxer");
    let mut redirects = 0;
    let response = loop {
        let response = http
            .get(&url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(network_error)?;
        if !response.status().is_redirection() {
            break response;
        }
        if redirects == MAX_DISCOVERY_REDIRECTS {
            return Err(Error::Protocol(
                "instance discovery redirected too many times".to_owned(),
            ));
        }
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| {
                Error::Protocol("instance discovery redirected without a location".to_owned())
            })?;
        url = redirect_target(&url, location, policy)?;
        redirects += 1;
    };
    let status = response.status();
    if !status.is_success() {
        return Err(if status.is_server_error() {
            Error::Network(format!("instance discovery answered HTTP {status}"))
        } else {
            Error::Protocol(format!("instance discovery answered HTTP {status}"))
        });
    }
    let mut response = response;
    let raw = read_limited(&mut response, MAX_JSON_BYTES).await?;
    parse_discovery(&raw, policy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_safe_redirects_only() {
        let strict = Policy::strict();
        let got = redirect_target(
            "https://fluxer.app/.well-known/fluxer",
            "https://api.fluxer.app/.well-known/fluxer",
            strict,
        );
        assert_eq!(
            got.ok().as_deref(),
            Some("https://api.fluxer.app/.well-known/fluxer")
        );
        let relative = redirect_target("https://fluxer.app/.well-known/fluxer", "/x", strict);
        assert_eq!(relative.ok().as_deref(), Some("https://fluxer.app/x"));
        for bad in [
            "http://api.fluxer.app/.well-known/fluxer",
            "https://127.0.0.1/.well-known/fluxer",
            "https://10.0.0.5/",
            "https://localhost/",
        ] {
            assert!(
                redirect_target("https://fluxer.app/.well-known/fluxer", bad, strict).is_err(),
                "{bad} must be refused"
            );
        }
    }

    #[test]
    fn normalizes_origins() {
        let cases = [
            ("", "https://fluxer.app"),
            ("fluxer.example.org", "https://fluxer.example.org"),
            ("https://chat.example/", "https://chat.example"),
            (
                "  https://chat.example:8443/x?y=1  ",
                "https://chat.example:8443",
            ),
        ];
        for (input, want) in cases {
            let got = normalize_origin(input, Policy::strict());
            assert_eq!(got.ok().as_deref(), Some(want), "input {input:?}");
        }
    }

    #[test]
    fn refuses_unsafe_origins_under_the_strict_policy() {
        let refused = [
            "http://chat.example",
            "http://localhost:8080/foo",
            "https://127.0.0.1:8443",
            "https://10.1.2.3",
            "https://192.168.0.10",
            "https://[fe80::1]",
            "https://[::1]:8080",
            "https://printer.local",
            "https://localhost",
            "https://intranet.localhost",
        ];
        for input in refused {
            assert!(
                normalize_origin(input, Policy::strict()).is_err(),
                "{input} must be refused"
            );
        }
    }

    #[test]
    fn insecure_policy_allows_local_hosts_for_tests() {
        let got = normalize_origin("http://localhost:8080/foo", Policy::insecure_for_tests());
        assert_eq!(got.ok().as_deref(), Some("http://localhost:8080"));
    }

    #[test]
    fn parses_real_style_discovery_document() {
        let doc = br#"{"endpoints":{"api_public":"https://api.example.test/","gateway":"wss://gw.example.test","media":"https://media.example.test"}}"#;
        let ep = parse_discovery(doc, Policy::strict()).expect("valid document");
        assert_eq!(
            ep,
            Endpoints {
                api: "https://api.example.test".to_owned(),
                gateway: "wss://gw.example.test".to_owned(),
                media: "https://media.example.test".to_owned(),
            }
        );
    }

    #[test]
    fn media_is_optional() {
        let doc = br#"{"endpoints":{"api_public":"https://api.example.test","gateway":"wss://gw.example.test"}}"#;
        let ep = parse_discovery(doc, Policy::strict()).expect("valid document");
        assert!(ep.media.is_empty());
    }

    #[test]
    fn rejects_https_gateway_scheme() {
        let doc = br#"{"endpoints":{"api_public":"https://api.example.test","gateway":"https://gw.example.test"}}"#;
        assert!(parse_discovery(doc, Policy::insecure_for_tests()).is_err());
    }

    #[test]
    fn refuses_private_endpoints_from_discovery() {
        let doc = br#"{"endpoints":{"api_public":"https://10.0.0.5/","gateway":"wss://gw.example.test"}}"#;
        assert!(parse_discovery(doc, Policy::strict()).is_err());
        let media = br#"{"endpoints":{"api_public":"https://api.example.test","gateway":"wss://gw.example.test","media":"https://169.254.1.1"}}"#;
        assert!(parse_discovery(media, Policy::strict()).is_err());
    }

    #[test]
    fn refuses_malformed_discovery_json() {
        assert!(parse_discovery(b"not json", Policy::strict()).is_err());
        // A document without endpoints parses, but its empty API URL is refused.
        assert!(parse_discovery(b"{}", Policy::strict()).is_err());
    }
}
