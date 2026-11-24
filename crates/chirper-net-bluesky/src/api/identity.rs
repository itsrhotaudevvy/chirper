//! Identity: handles to DIDs, DIDs to their documents, and documents to the PDS of an account.

use percent_encoding::percent_decode_str;
use serde::Deserialize;
use serde_json::Value;

use chirper_bridge::{Error, Result};

use super::{ApiError, Auth, Pds, read_capped, transport};
use crate::Hosts;
use crate::ids;

#[derive(Deserialize)]
struct Resolved {
    did: String,
}

/// Resolve a handle to its DID through a PDS (`com.atproto.identity.resolveHandle`).
pub async fn resolve_handle(pds: &Pds, handle: &str) -> std::result::Result<String, ApiError> {
    let params = [("handle", handle.to_owned())];
    let reply: Resolved = pds
        .query("com.atproto.identity.resolveHandle", &params, Auth::NONE)
        .await?;
    Ok(reply.did)
}

/// The DID document of `did`: from the PLC directory for `did:plc`, from the web for `did:web`.
pub async fn did_document(http: &reqwest::Client, hosts: &Hosts, did: &str) -> Result<Value> {
    let url = document_url(hosts, did)?;
    let mut response = http.get(&url).send().await.map_err(transport)?;
    let status = response.status();
    if !status.is_success() {
        return Err(if status.is_server_error() {
            Error::Network(format!("the DID directory answered {status}"))
        } else {
            Error::Protocol(format!("the DID document could not be read ({status})"))
        });
    }
    let body = read_capped(&mut response).await?;
    serde_json::from_slice(&body)
        .map_err(|err| Error::Protocol(format!("the DID document is not JSON: {err}")))
}

/// Where the document of a DID lives.
fn document_url(hosts: &Hosts, did: &str) -> Result<String> {
    let (method, value) = ids::parse_did(did)
        .ok_or_else(|| Error::Protocol("a Bluesky account id is not a DID".to_owned()))?;
    match method {
        "plc" => Ok(format!("{}/{did}", hosts.plc.trim_end_matches('/'))),
        "web" => web_document_url(&hosts.scheme, value),
        _ => Err(Error::Protocol(
            "this kind of DID is not supported".to_owned(),
        )),
    }
}

/// `did:web:host` is `/.well-known/did.json`; `did:web:host:a:b` is `/a/b/did.json`. Each part is
/// percent-decoded (a port is `%3A`) and must be a plain host or path segment.
fn web_document_url(scheme: &str, value: &str) -> Result<String> {
    let mut parts = value.split(':').map(decode_segment);
    let host = parts
        .next()
        .flatten()
        .filter(|host| {
            host.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
        })
        .ok_or_else(|| Error::Protocol("the did:web host is not valid".to_owned()))?;
    let path = parts
        .collect::<Option<Vec<String>>>()
        .ok_or_else(|| Error::Protocol("the did:web path is not valid".to_owned()))?;
    if path.is_empty() {
        Ok(format!("{scheme}://{host}/.well-known/did.json"))
    } else {
        Ok(format!("{scheme}://{host}/{}/did.json", path.join("/")))
    }
}

/// One percent-decoded DID part, or `None` when it is empty, a relative path, or not UTF-8.
fn decode_segment(part: &str) -> Option<String> {
    let text = percent_decode_str(part).decode_utf8().ok()?.into_owned();
    let plain = !text.is_empty()
        && text != "."
        && text != ".."
        && !text.contains('/')
        && !text.contains('\\');
    plain.then_some(text)
}

/// The PDS endpoint of a DID document, when it names one that uses `scheme`.
pub fn pds_from_doc(doc: &Value, scheme: &str) -> Option<String> {
    let prefix = format!("{scheme}://");
    doc.get("service")?.as_array()?.iter().find_map(|service| {
        let id = service.get("id")?.as_str()?;
        if !id.ends_with("#atproto_pds") {
            return None;
        }
        let endpoint = service.get("serviceEndpoint")?.as_str()?;
        (endpoint.starts_with(prefix.as_str()) && endpoint.len() > prefix.len())
            .then(|| endpoint.trim_end_matches('/').to_owned())
    })
}

/// The PDS of a DID, or `None` when its document names none.
pub async fn pds_for(http: &reqwest::Client, hosts: &Hosts, did: &str) -> Result<Option<String>> {
    let doc = did_document(http, hosts, did).await?;
    Ok(pds_from_doc(&doc, &hosts.scheme))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hosts() -> Hosts {
        Hosts {
            plc: "https://plc.directory/".to_owned(),
            scheme: "https".to_owned(),
        }
    }

    #[test]
    fn plc_documents_come_from_the_directory() {
        assert_eq!(
            document_url(&hosts(), "did:plc:abc123").unwrap(),
            "https://plc.directory/did:plc:abc123"
        );
    }

    #[test]
    fn web_documents_follow_the_did_web_rules() {
        assert_eq!(
            document_url(&hosts(), "did:web:example.com").unwrap(),
            "https://example.com/.well-known/did.json"
        );
        assert_eq!(
            document_url(&hosts(), "did:web:localhost%3A8080").unwrap(),
            "https://localhost:8080/.well-known/did.json"
        );
        assert_eq!(
            document_url(&hosts(), "did:web:example.com:users:ada").unwrap(),
            "https://example.com/users/ada/did.json"
        );
    }

    #[test]
    fn unsafe_web_dids_and_other_methods_are_refused() {
        for did in [
            "did:web:..",
            "did:web:a%2Fb",
            "did:web:ex%20ample",
            "did:key:z6Mk",
        ] {
            assert!(
                document_url(&hosts(), did).is_err(),
                "{did} must be refused"
            );
        }
    }

    #[test]
    fn the_pds_is_the_atproto_service_on_the_right_scheme() {
        let doc = json!({"service": [
            {"id": "#atproto_labeler", "serviceEndpoint": "https://labeler.example"},
            {"id": "#atproto_pds", "serviceEndpoint": "https://pds.example/"}
        ]});
        assert_eq!(
            pds_from_doc(&doc, "https").as_deref(),
            Some("https://pds.example")
        );
        assert_eq!(pds_from_doc(&doc, "http"), None);
        assert_eq!(pds_from_doc(&json!({}), "https"), None);
    }
}
