//! The signal websocket: request/response multiplexing over one `WebSocketMessage` stream.
//!
//! Ported from signalmeow's `web/signalwebsocket.go` (AGPL-3.0): requests get increasing ids
//! (from 1), responses are matched back by id, incoming requests are queued for the handler, and
//! a ping goes out every 30 seconds with five unanswered pings meaning the connection is dead.
//! One task owns the socket; everything else talks to it through [`WsClient`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as Frame;

use chirper_bridge::{Error, Result};

use crate::proto::{WS_TYPE_REQUEST, WS_TYPE_RESPONSE, WebSocketMessage};
use crate::proto::{WebSocketRequestMessage, WebSocketResponseMessage};

/// Time between keep-alive pings.
pub const PING_INTERVAL: Duration = Duration::from_secs(30);
/// Unanswered pings after which the connection is declared dead.
pub const PING_MISS_LIMIT: u32 = 5;
/// How long a request may wait for its response.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<WebSocketResponseMessage>>>>;

/// A request the server pushed to us (`PUT /api/v1/message`, `/api/v1/queue/empty`, ...).
#[derive(Debug, Clone)]
pub struct IncomingRequest {
    /// The id the response must carry.
    pub id: u64,
    pub verb: String,
    pub path: String,
    pub body: Vec<u8>,
}

/// The handle the rest of the driver uses to talk on one websocket.
#[derive(Clone, Debug)]
pub struct WsClient {
    out: mpsc::Sender<WebSocketMessage>,
    pending: Pending,
    next_id: Arc<AtomicU64>,
}

/// The receiving half: server requests, and the task that owns the socket.
pub struct WsPump {
    /// Requests from the server, in arrival order.
    pub incoming: mpsc::UnboundedReceiver<IncomingRequest>,
    /// The socket task; it finishes when the connection ends.
    pub task: JoinHandle<Result<()>>,
}

/// Starts the pump over an established websocket. The returned client is cheap to clone.
pub fn spawn<S>(ws: WebSocketStream<S>) -> (WsClient, WsPump)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (out_tx, out_rx) = mpsc::channel(64);
    let (in_tx, in_rx) = mpsc::unbounded_channel();
    let pending: Pending = Arc::default();
    let client = WsClient {
        out: out_tx,
        pending: pending.clone(),
        next_id: Arc::new(AtomicU64::new(1)),
    };
    let task = tokio::spawn(run(ws, out_rx, in_tx, pending.clone()));
    (
        client,
        WsPump {
            incoming: in_rx,
            task,
        },
    )
}

impl WsClient {
    /// Sends a request and waits for its response. The response status is not checked here.
    pub async fn request(
        &self,
        verb: &str,
        path: &str,
        body: Option<Vec<u8>>,
        headers: Vec<String>,
    ) -> Result<WebSocketResponseMessage> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        lock(&self.pending)?.insert(id, tx);
        let request = WebSocketRequestMessage {
            verb: Some(verb.to_owned()),
            path: Some(path.to_owned()),
            body,
            headers,
            id: Some(id),
        };
        let frame = WebSocketMessage {
            r#type: Some(WS_TYPE_REQUEST),
            request: Some(request),
            response: None,
        };
        if self.out.send(frame).await.is_err() {
            lock(&self.pending)?.remove(&id);
            return Err(Error::Network("the signal websocket is closed".to_owned()));
        }
        match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err(Error::Network(format!("{verb} {path} got no response"))),
            Err(_) => {
                lock(&self.pending)?.remove(&id);
                Err(Error::Network(format!("{verb} {path} timed out")))
            }
        }
    }

    /// Answers a server request with `status` ("OK" for 200, "Unknown" for anything else).
    pub async fn respond(&self, id: u64, status: u32) -> Result<()> {
        let frame = response_frame(id, status, Vec::new());
        self.out
            .send(frame)
            .await
            .map_err(|_| Error::Network("the signal websocket is closed".to_owned()))
    }

    /// Asks the socket task to close the connection.
    pub async fn close(&self) {
        let _ = self.out.send(close_marker()).await;
    }
}

/// A response frame, as signalmeow's `CreateWSResponse` builds it.
pub fn response_frame(id: u64, status: u32, body: Vec<u8>) -> WebSocketMessage {
    WebSocketMessage {
        r#type: Some(WS_TYPE_RESPONSE),
        request: None,
        response: Some(WebSocketResponseMessage {
            id: Some(id),
            status: Some(status),
            message: Some(if status == 200 { "OK" } else { "Unknown" }.to_owned()),
            headers: Vec::new(),
            body: if body.is_empty() { None } else { Some(body) },
        }),
    }
}

/// An empty frame with no type: the pump treats it as a request to close the socket.
fn close_marker() -> WebSocketMessage {
    WebSocketMessage::default()
}

fn lock<T>(m: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>> {
    m.lock()
        .map_err(|_| Error::Internal("the websocket state lock was poisoned".to_owned()))
}

/// Runs the pump, then drops every waiting request so none is left to time out.
async fn run<S>(
    ws: WebSocketStream<S>,
    out: mpsc::Receiver<WebSocketMessage>,
    incoming: mpsc::UnboundedSender<IncomingRequest>,
    pending: Pending,
) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let result = pump(ws, out, incoming, pending.clone()).await;
    if let Ok(mut waiting) = pending.lock() {
        waiting.clear();
    }
    result
}

/// The socket task: reads frames, writes queued frames and keeps the connection alive.
async fn pump<S>(
    mut ws: WebSocketStream<S>,
    mut out: mpsc::Receiver<WebSocketMessage>,
    incoming: mpsc::UnboundedSender<IncomingRequest>,
    pending: Pending,
) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.tick().await;
    let mut missed = 0u32;
    loop {
        tokio::select! {
            frame = ws.next() => {
                let frame = match frame {
                    None => return Ok(()),
                    Some(Err(err)) => return Err(Error::Network(format!("websocket: {err}"))),
                    Some(Ok(frame)) => frame,
                };
                match frame {
                    Frame::Binary(bytes) => {
                        let message = WebSocketMessage::decode(bytes.as_ref())
                            .map_err(|_| Error::Protocol("a websocket frame is not a WebSocketMessage".to_owned()))?;
                        dispatch(message, &incoming, &pending)?;
                    }
                    Frame::Pong(_) => missed = 0,
                    Frame::Close(_) => return Ok(()),
                    Frame::Ping(_) | Frame::Text(_) | Frame::Frame(_) => {}
                }
            }
            queued = out.recv() => {
                match queued {
                    None => return Ok(()),
                    Some(message) if message.r#type.is_none() => {
                        let _ = ws.close(None).await;
                        return Ok(());
                    }
                    Some(message) => {
                        let bytes = message.encode_to_vec();
                        ws.send(Frame::Binary(bytes.into()))
                            .await
                            .map_err(|err| Error::Network(format!("websocket write: {err}")))?;
                    }
                }
            }
            _ = ping.tick() => {
                if missed >= PING_MISS_LIMIT {
                    return Err(Error::Network("the signal websocket stopped answering pings".to_owned()));
                }
                missed += 1;
                ws.send(Frame::Ping(Vec::new().into()))
                    .await
                    .map_err(|err| Error::Network(format!("websocket ping: {err}")))?;
            }
        }
    }
}

/// Routes one decoded frame: responses to their waiting request, requests to the handler.
fn dispatch(
    message: WebSocketMessage,
    incoming: &mpsc::UnboundedSender<IncomingRequest>,
    pending: &Pending,
) -> Result<()> {
    match message.r#type {
        Some(WS_TYPE_REQUEST) => {
            let request = message
                .request
                .ok_or_else(|| Error::Protocol("a request frame has no request".to_owned()))?;
            let Some(id) = request.id else {
                return Err(Error::Protocol("a request frame has no id".to_owned()));
            };
            let _ = incoming.send(IncomingRequest {
                id,
                verb: request.verb.unwrap_or_default(),
                path: request.path.unwrap_or_default(),
                body: request.body.unwrap_or_default(),
            });
            Ok(())
        }
        Some(WS_TYPE_RESPONSE) => {
            let response = message
                .response
                .ok_or_else(|| Error::Protocol("a response frame has no response".to_owned()))?;
            let Some(id) = response.id else {
                return Err(Error::Protocol("a response frame has no id".to_owned()));
            };
            // An unknown id is a response to a request that timed out; it is dropped.
            if let Some(waiter) = lock(pending)?.remove(&id) {
                let _ = waiter.send(response);
            }
            Ok(())
        }
        _ => Err(Error::Protocol("a websocket frame has an unknown type".to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::protocol::Role;

    /// Two ends of one socket: the driver side (client) and a scripted server side.
    async fn pair() -> (WebSocketStream<tokio::io::DuplexStream>, WebSocketStream<tokio::io::DuplexStream>) {
        let (a, b) = tokio::io::duplex(1 << 16);
        let client = WebSocketStream::from_raw_socket(a, Role::Client, None).await;
        let server = WebSocketStream::from_raw_socket(b, Role::Server, None).await;
        (client, server)
    }

    async fn read_frame(
        server: &mut WebSocketStream<tokio::io::DuplexStream>,
    ) -> WebSocketMessage {
        let frame = server.next().await.expect("a frame").expect("no error");
        match frame {
            Frame::Binary(bytes) => WebSocketMessage::decode(bytes.as_ref()).expect("protobuf"),
            other => panic!("expected a binary frame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn responses_are_matched_to_their_requests_by_id() {
        let (client_ws, mut server) = pair().await;
        let (client, _pump) = spawn(client_ws);
        let asking = {
            let client = client.clone();
            tokio::spawn(async move {
                client.request("GET", "/v1/a", None, Vec::new()).await
            })
        };
        let first = read_frame(&mut server).await;
        let request = first.request.expect("a request");
        assert_eq!(request.id, Some(1));
        assert_eq!(request.verb.as_deref(), Some("GET"));
        assert_eq!(request.path.as_deref(), Some("/v1/a"));
        // The server answers out of order relative to a second request, which must still work.
        let answer = response_frame(1, 200, b"{}".to_vec());
        server
            .send(Frame::Binary(answer.encode_to_vec().into()))
            .await
            .expect("send");
        let response = asking.await.expect("join").expect("response");
        assert_eq!(response.status, Some(200));
        assert_eq!(response.body.as_deref(), Some(&b"{}"[..]));
    }

    #[tokio::test]
    async fn server_requests_are_queued_and_answered() {
        let (client_ws, mut server) = pair().await;
        let (client, mut pump) = spawn(client_ws);
        let pushed = WebSocketMessage {
            r#type: Some(WS_TYPE_REQUEST),
            request: Some(WebSocketRequestMessage {
                verb: Some("PUT".to_owned()),
                path: Some("/api/v1/queue/empty".to_owned()),
                body: None,
                headers: Vec::new(),
                id: Some(77),
            }),
            response: None,
        };
        server
            .send(Frame::Binary(pushed.encode_to_vec().into()))
            .await
            .expect("send");
        let request = pump.incoming.recv().await.expect("a queued request");
        assert_eq!(request.id, 77);
        assert_eq!(request.path, "/api/v1/queue/empty");
        client.respond(request.id, 200).await.expect("respond");
        let answer = read_frame(&mut server).await;
        let response = answer.response.expect("a response");
        assert_eq!(response.id, Some(77));
        assert_eq!(response.status, Some(200));
        assert_eq!(response.message.as_deref(), Some("OK"));
    }

    #[tokio::test]
    async fn a_closed_socket_fails_pending_requests() {
        let (client_ws, server) = pair().await;
        let (client, pump) = spawn(client_ws);
        drop(server);
        let result = client.request("GET", "/v1/x", None, Vec::new()).await;
        assert!(matches!(result, Err(Error::Network(_))));
        // The socket may end cleanly or with a transport error; either way the task is finished.
        let ended = pump.task.await.expect("join");
        assert!(matches!(ended, Ok(()) | Err(Error::Network(_))));
    }

    #[tokio::test]
    async fn a_frame_that_is_not_protobuf_is_a_protocol_error() {
        let (client_ws, mut server) = pair().await;
        let (_client, pump) = spawn(client_ws);
        server
            .send(Frame::Binary(vec![0xff, 0xff, 0xff].into()))
            .await
            .expect("send");
        let result = pump.task.await.expect("join");
        assert!(matches!(result, Err(Error::Protocol(_))));
    }
}
