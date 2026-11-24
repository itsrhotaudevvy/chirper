//! Opening Signal websockets, with the reference's headers.

use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode};
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream, connect_async_tls_with_config};

use chirper_bridge::{Error, Result};

use crate::tls::{USER_AGENT, client_config};

/// The agent name sent in `X-Signal-Agent`.
pub const SIGNAL_AGENT: &str = "CHP";

/// The connected websocket, plain for `ws://` test endpoints and TLS for `wss://`.
pub type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Opens a websocket. `authorization` is sent as the `Authorization` header of the handshake
/// when set, as the reference does for its authenticated socket.
pub async fn open(url: &str, authorization: Option<&str>) -> Result<Socket> {
    let mut request = url
        .into_client_request()
        .map_err(|err| Error::Internal(format!("bad websocket url: {err}")))?;
    let headers = request.headers_mut();
    headers.insert(
        "User-Agent",
        HeaderValue::from_static(USER_AGENT),
    );
    headers.insert("X-Signal-Agent", HeaderValue::from_static(SIGNAL_AGENT));
    if let Some(value) = authorization {
        let value = HeaderValue::from_str(value)
            .map_err(|_| Error::Auth("the stored credentials are not a valid header".to_owned()))?;
        headers.insert("Authorization", value);
    }
    let connector = if url.starts_with("wss://") {
        Some(Connector::Rustls(client_config()?))
    } else {
        None
    };
    match connect_async_tls_with_config(request, None, false, connector).await {
        Ok((socket, _response)) => Ok(socket),
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            Err(status_error(response.status()))
        }
        Err(err) => Err(Error::Network(format!("websocket connect: {err}"))),
    }
}

/// Maps the HTTP status of a refused websocket handshake to a driver error.
pub fn status_error(status: StatusCode) -> Error {
    match status.as_u16() {
        401 | 403 => Error::Auth("the Signal server refused these credentials".to_owned()),
        429 => Error::RateLimited {
            retry_after_ms: 60_000,
        },
        500..=599 => Error::Network(format!("Signal answered {}", status.as_u16())),
        code => Error::Protocol(format!("Signal answered {code} to the websocket handshake")),
    }
}
