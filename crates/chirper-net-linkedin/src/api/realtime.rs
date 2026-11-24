//! The realtime stream's HTTP calls: the server-sent-events connection and its heartbeat.
//!
//! The two data tables the connection sends in headers are the reference's
//! `linkedingo/x-li-query-map.json` and `x-li-recipe-map.json` (AGPL-3.0, copied unchanged).

use reqwest::Method;
use serde_json::json;

use chirper_bridge::{Error, Result};

use super::{Api, CONTENT_NORMALIZED, CONTENT_PLAIN, SERVICE_VERSION};

/// The query decoration map sent as `X-LI-Query-Map`.
const QUERY_MAP: &str = include_str!("../../data/x-li-query-map.json");
/// The recipe decoration map sent as `X-LI-Recipe-Map`.
const RECIPE_MAP: &str = include_str!("../../data/x-li-recipe-map.json");

/// The stream endpoint.
const CONNECT: &str = "/realtime/connect?rc=1";
/// The heartbeat endpoint.
const HEARTBEAT: &str = "/realtime/realtimeFrontendClientConnectivityTracking?action=sendHeartbeat";

/// The decoration map as compact JSON, the form the header carries.
fn compact(map: &str) -> String {
    serde_json::from_str::<serde_json::Value>(map)
        .map(|value| value.to_string())
        .unwrap_or_else(|_| "{}".to_owned())
}

impl Api {
    /// Open the event stream for `session_id`. The response body is read as it arrives.
    pub async fn open_stream(&self, session_id: &str) -> Result<reqwest::Response> {
        let builder = self
            .with_voyager_headers(self.request(Method::GET, CONNECT))
            .header("Accept", "text/event-stream")
            .header("Priority", "u=1, i")
            .header("Sec-Fetch-Dest", "empty")
            .header("Sec-Fetch-Mode", "cors")
            .header("Sec-Fetch-Site", "same-origin")
            .header("X-LI-Accept", CONTENT_NORMALIZED)
            .header("X-LI-Query-Accept", "application/graphql")
            .header("X-LI-Query-Map", compact(QUERY_MAP))
            .header("X-LI-Recipe-Accept", CONTENT_NORMALIZED)
            .header("X-LI-Recipe-Map", compact(RECIPE_MAP))
            .header("X-LI-Realtime-Session", session_id);
        let response = builder.send().await.map_err(|_| {
            Error::Network("LinkedIn: could not open the realtime stream".to_owned())
        })?;
        super::check(response.status(), response.headers())?;
        Ok(response)
    }

    /// Send one heartbeat for the stream `session_id` of the member `actor_urn`.
    pub async fn heartbeat(&self, session_id: &str, actor_urn: &str, first: bool) -> Result<()> {
        let body = json!({
            "isFirstHeartbeat": first,
            "isLastHeartbeat": false,
            "realtimeSessionId": session_id,
            "mpName": "voyager-web",
            "mpVersion": SERVICE_VERSION,
            "clientId": "voyager-web",
            "actorUrn": actor_urn,
            "contextUrns": [actor_urn],
        });
        let builder = self
            .with_voyager_headers(self.request(Method::POST, HEARTBEAT))
            .header("accept", "*/*")
            .header("origin", super::WWW)
            .header("Priority", "u=1, i");
        let builder = Api::json_body(builder, CONTENT_PLAIN, &body)?;
        self.expect_ok(builder).await.map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_decoration_maps_are_valid_json() {
        assert!(serde_json::from_str::<serde_json::Value>(QUERY_MAP).is_ok());
        assert!(serde_json::from_str::<serde_json::Value>(RECIPE_MAP).is_ok());
        assert!(compact(QUERY_MAP).contains("conversationsTopic"));
        assert!(!compact(RECIPE_MAP).contains('\n'));
    }
}
