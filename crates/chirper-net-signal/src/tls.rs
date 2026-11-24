//! Signal's own certificate authorities, and the two clients built on them.
//!
//! The reference embeds Signal's two root certificates (`web/signal-root.crt.der` and
//! `web/signal-root-ed25519.crt.der` in signalmeow, AGPL-3.0) and trusts nothing else, with TLS
//! 1.3 as the minimum. The DER files are copied unchanged into `src/pins/`.

use std::sync::Arc;
use std::time::Duration;

use rustls::ClientConfig;
use rustls::RootCertStore;
use rustls::pki_types::CertificateDer;

use chirper_bridge::{Error, Result};

const ROOT_RSA: &[u8] = include_bytes!("pins/signal-root.crt.der");
const ROOT_ED25519: &[u8] = include_bytes!("pins/signal-root-ed25519.crt.der");

/// The DER encodings of Signal's pinned roots.
pub fn pinned_roots() -> [&'static [u8]; 2] {
    [ROOT_RSA, ROOT_ED25519]
}

/// A rustls client configuration that trusts only Signal's roots and speaks TLS 1.3.
pub fn client_config() -> Result<Arc<ClientConfig>> {
    let mut roots = RootCertStore::empty();
    for der in pinned_roots() {
        roots
            .add(CertificateDer::from(der.to_vec()))
            .map_err(|_| Error::Internal("Signal's pinned root certificate is invalid".to_owned()))?;
    }
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|_| Error::Internal("no TLS 1.3 support in the crypto provider".to_owned()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// The HTTP client for Signal's REST and CDN hosts: pinned roots, no redirects.
pub fn http_client() -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .tls_certs_only(
            pinned_roots()
                .iter()
                .filter_map(|der| reqwest::Certificate::from_der(der).ok()),
        );
    builder = builder.min_tls_version(reqwest::tls::Version::TLS_1_3);
    builder
        .build()
        .map_err(|err| Error::Internal(format!("building the Signal HTTP client: {err}")))
}

/// The user agent sent with every request.
pub const USER_AGENT: &str = concat!("Chirper/", env!("CARGO_PKG_VERSION"));
