//! The DGW transport: one websocket carrying many logical streams.
//!
//! A single actor task owns the websocket. Callers talk to it through a cloneable [`Handle`]:
//! a *request* opens a one-off stream, sends one payload and resolves with the single response
//! payload; a *persistent* stream is opened with an init payload and then delivers every payload the
//! server pushes on it to a channel, and [`Handle::post`] sends further payloads on it with acks.
//!
//! The wire behaviour follows `pkg/messagix/dgw/socket.go`, `oneoffstream.go` and
//! `persistentstream.go` (AGPL-3.0, mautrix-meta): establish frames answer with a `code`, data frames
//! are acknowledged when they ask for it, end-of-data closes a stream, and pings are answered
//! while a pong is expected within the timeout.

use std::collections::HashMap;
use std::time::Duration;

use chirper_bridge::Error;
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use log::{debug, warn};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, interval, timeout};
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::dgw::{Frame, MAX_ACK_ID};

/// How long a caller waits for an establish, ack or response before giving up.
pub const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
/// How often the transport sends a ping frame.
pub const PING_INTERVAL: Duration = Duration::from_secs(10);
/// How long the transport waits for a pong before it treats the socket as dead.
pub const PONG_TIMEOUT: Duration = Duration::from_secs(30);

/// Requests the transport actor serves.
enum Command {
    /// One-off request: open a stream, send `payload`, resolve with the response.
    Request {
        payload: Vec<u8>,
        reply: oneshot::Sender<Result<Vec<u8>, Error>>,
    },
    /// Persistent stream: open it with `init` and forward every payload to `pushes`.
    Open {
        init: Vec<u8>,
        pushes: mpsc::UnboundedSender<Vec<u8>>,
        reply: oneshot::Sender<Result<u16, Error>>,
    },
    /// Send a payload on a one-off stream that expects no answer.
    Notify {
        payload: Vec<u8>,
        reply: oneshot::Sender<Result<(), Error>>,
    },
    /// Send one payload on an open persistent stream and wait for its ack.
    Post {
        stream: u16,
        payload: Vec<u8>,
        reply: oneshot::Sender<Result<(), Error>>,
    },
}

/// A cloneable way to use one live transport.
#[derive(Clone, Debug)]
pub struct Handle {
    commands: mpsc::UnboundedSender<Command>,
}

fn not_connected() -> Error {
    Error::Network("the realtime connection closed".to_owned())
}

impl Handle {
    /// Send one payload on a fresh one-off stream and return the response payload.
    pub async fn request(&self, payload: Vec<u8>) -> Result<Vec<u8>, Error> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command::Request { payload, reply })
            .map_err(|_| not_connected())?;
        await_reply(answer).await?
    }

    /// Open a persistent stream whose init payload is acknowledged; `pushes` receives everything
    /// the server sends on it from then on. Returns the stream id.
    pub async fn open(
        &self,
        init: Vec<u8>,
        pushes: mpsc::UnboundedSender<Vec<u8>>,
    ) -> Result<u16, Error> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command::Open { init, pushes, reply })
            .map_err(|_| not_connected())?;
        await_reply(answer).await?
    }

    /// Send a payload that expects no answer (a stateless task) and return once it is written.
    pub async fn notify(&self, payload: Vec<u8>) -> Result<(), Error> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command::Notify { payload, reply })
            .map_err(|_| not_connected())?;
        await_reply(answer).await?
    }

    /// Send a payload on an open persistent stream and wait until the server acknowledges it.
    pub async fn post(&self, stream: u16, payload: Vec<u8>) -> Result<(), Error> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command::Post {
                stream,
                payload,
                reply,
            })
            .map_err(|_| not_connected())?;
        await_reply(answer).await?
    }
}

async fn await_reply<T>(answer: oneshot::Receiver<Result<T, Error>>) -> Result<Result<T, Error>, Error> {
    match timeout(RESPONSE_TIMEOUT, answer).await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(_)) => Err(not_connected()),
        Err(_) => Err(Error::Network("the realtime connection timed out".to_owned())),
    }
}

/// An outstanding open: waiting for the establish answer and the init payload's ack.
struct PendingOpen {
    established: bool,
    acked: bool,
    reply: Option<oneshot::Sender<Result<u16, Error>>>,
}

/// The state of the actor: everything it is waiting for, keyed by stream (and ack) id.
struct Actor<S> {
    socket: S,
    next_stream: u16,
    next_ack: HashMap<u16, u16>,
    oneoff: HashMap<u16, oneshot::Sender<Result<Vec<u8>, Error>>>,
    persistent: HashMap<u16, mpsc::UnboundedSender<Vec<u8>>>,
    opening: HashMap<u16, PendingOpen>,
    acks: HashMap<(u16, u16), oneshot::Sender<Result<(), Error>>>,
    last_pong: Instant,
}

/// Open a websocket transport over an already connected socket and start its actor.
///
/// The socket is any websocket stream, so tests can use a local listener and production passes the
/// connected `tokio-tungstenite` stream.
pub fn spawn<S>(socket: S) -> Handle
where
    S: Stream<Item = Result<WsMessage, WsError>> + Sink<WsMessage, Error = WsError> + Unpin + Send + 'static,
{
    let (commands, receiver) = mpsc::unbounded_channel();
    let actor = Actor {
        socket,
        next_stream: 0,
        next_ack: HashMap::new(),
        oneoff: HashMap::new(),
        persistent: HashMap::new(),
        opening: HashMap::new(),
        acks: HashMap::new(),
        last_pong: Instant::now(),
    };
    tokio::spawn(actor.run(receiver));
    Handle { commands }
}

impl<S> Actor<S>
where
    S: Stream<Item = Result<WsMessage, WsError>> + Sink<WsMessage, Error = WsError> + Unpin + Send,
{
    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<Command>) {
        let mut ping = interval(PING_INTERVAL);
        ping.tick().await;
        let reason = loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => {
                        if let Err(err) = self.command(command).await {
                            break err;
                        }
                    }
                    None => break Error::Network("the realtime connection was closed".to_owned()),
                },
                message = self.socket.next() => match message {
                    Some(Ok(WsMessage::Binary(bytes))) => {
                        if let Err(err) = self.incoming(&bytes).await {
                            break err;
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) | None => {
                        break Error::Network("the realtime connection closed".to_owned());
                    }
                    Some(Ok(_)) => {}
                    Some(Err(err)) => break Error::Network(format!("realtime socket: {err}")),
                },
                _ = ping.tick() => {
                    if self.last_pong.elapsed() > PONG_TIMEOUT {
                        break Error::Network("the realtime connection stopped answering pings".to_owned());
                    }
                    if let Err(err) = self.write(&[Frame::Ping]).await {
                        break err;
                    }
                }
            }
        };
        debug!("meta transport stopped");
        self.fail_all(reason).await;
    }

    async fn fail_all(&mut self, reason: Error) {
        let message = reason.to_string();
        for (_, waiter) in self.oneoff.drain() {
            let _ = waiter.send(Err(Error::Network(message.clone())));
        }
        for (_, pending) in self.opening.drain() {
            if let Some(reply) = pending.reply {
                let _ = reply.send(Err(Error::Network(message.clone())));
            }
        }
        for (_, waiter) in self.acks.drain() {
            let _ = waiter.send(Err(Error::Network(message.clone())));
        }
        self.persistent.clear();
        let _ = self.socket.close().await;
    }

    async fn write(&mut self, frames: &[Frame]) -> Result<(), Error> {
        // A websocket carries plain data frames; the extended variant is for HTTP streams only.
        let bytes = Frame::encode_all(frames, false);
        self.socket
            .send(WsMessage::Binary(bytes.into()))
            .await
            .map_err(|err| Error::Network(format!("realtime write: {err}")))
    }

    /// Allocate a stream id that no open stream uses.
    fn allocate_stream(&mut self) -> Result<u16, Error> {
        for _ in 0..=u16::MAX {
            let candidate = self.next_stream;
            self.next_stream = self.next_stream.wrapping_add(1);
            let busy = self.oneoff.contains_key(&candidate)
                || self.persistent.contains_key(&candidate)
                || self.opening.contains_key(&candidate);
            if !busy {
                return Ok(candidate);
            }
        }
        Err(Error::Internal("every realtime stream id is in use".to_owned()))
    }

    fn next_ack_id(&mut self, stream: u16) -> u16 {
        let counter = self.next_ack.entry(stream).or_insert(0);
        let current = *counter;
        *counter = if *counter >= MAX_ACK_ID { 0 } else { *counter + 1 };
        current
    }

    async fn command(&mut self, command: Command) -> Result<(), Error> {
        match command {
            Command::Request { payload, reply } => {
                let stream = self.allocate_stream()?;
                self.oneoff.insert(stream, reply);
                // A one-off stream asks for an ack on its single init payload, with ack id 0.
                let frames = [
                    Frame::EstablishStream {
                        stream,
                        parameters: b"{}".to_vec(),
                    },
                    Frame::Data {
                        stream,
                        ack_id: 0,
                        requires_ack: true,
                        content_type: None,
                        payload,
                    },
                ];
                self.write(&frames).await
            }
            Command::Open {
                init,
                pushes,
                reply,
            } => {
                let stream = self.allocate_stream()?;
                self.persistent.insert(stream, pushes);
                self.opening.insert(
                    stream,
                    PendingOpen {
                        established: false,
                        acked: false,
                        reply: Some(reply),
                    },
                );
                let ack_id = self.next_ack_id(stream);
                let frames = [
                    Frame::EstablishStream {
                        stream,
                        parameters: b"{}".to_vec(),
                    },
                    Frame::Data {
                        stream,
                        ack_id,
                        requires_ack: true,
                        content_type: None,
                        payload: init,
                    },
                ];
                self.write(&frames).await
            }
            Command::Notify { payload, reply } => {
                let stream = self.allocate_stream()?;
                let frames = [
                    Frame::EstablishStream {
                        stream,
                        parameters: b"{}".to_vec(),
                    },
                    Frame::Data {
                        stream,
                        ack_id: 0,
                        requires_ack: false,
                        content_type: None,
                        payload,
                    },
                ];
                let written = self.write(&frames).await;
                let answer = match &written {
                    Ok(()) => Ok(()),
                    Err(err) => Err(Error::Network(err.to_string())),
                };
                let _ = reply.send(answer);
                written
            }
            Command::Post {
                stream,
                payload,
                reply,
            } => {
                if !self.persistent.contains_key(&stream) {
                    let _ = reply.send(Err(not_connected()));
                    return Ok(());
                }
                let ack_id = self.next_ack_id(stream);
                self.acks.insert((stream, ack_id), reply);
                self.write(&[Frame::Data {
                    stream,
                    ack_id,
                    requires_ack: true,
                    content_type: None,
                    payload,
                }])
                .await
            }
        }
    }

    async fn incoming(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let frames = match Frame::decode_all(bytes) {
            Ok(frames) => frames,
            Err(err) => {
                warn!("meta transport dropped a message that did not decode: {err}");
                return Ok(());
            }
        };
        for frame in frames {
            self.frame(frame).await?;
        }
        Ok(())
    }

    async fn frame(&mut self, frame: Frame) -> Result<(), Error> {
        match frame {
            Frame::Ping => self.write(&[Frame::Pong]).await,
            Frame::Pong => {
                self.last_pong = Instant::now();
                Ok(())
            }
            Frame::Deauth => Err(Error::Auth("the realtime connection was deauthorised".to_owned())),
            Frame::Drain { reason } => {
                debug!("meta transport asked to drain (reason {reason})");
                Ok(())
            }
            Frame::EstablishStream { stream, parameters } => {
                self.establish_answer(stream, &parameters).await
            }
            Frame::Data {
                stream,
                ack_id,
                requires_ack,
                payload,
                ..
            } => self.data(stream, ack_id, requires_ack, payload).await,
            Frame::Ack { stream, ack_id } => {
                if let Some(waiter) = self.acks.remove(&(stream, ack_id)) {
                    let _ = waiter.send(Ok(()));
                }
                // The init payload of an opening stream is always ack id 0.
                if ack_id == 0 {
                    if let Some(pending) = self.opening.get_mut(&stream) {
                        pending.acked = true;
                        self.finish_open_if_ready(stream).await;
                    }
                }
                Ok(())
            }
            Frame::EndOfData { stream } | Frame::EndOfDataWithReason { stream, .. } => {
                self.close_stream(stream);
                Ok(())
            }
        }
    }

    async fn establish_answer(&mut self, stream: u16, parameters: &[u8]) -> Result<(), Error> {
        let code = establish_code(parameters);
        if let Some(pending) = self.opening.get_mut(&stream) {
            if code == Some(200) {
                pending.established = true;
            } else {
                let reply = self.opening.remove(&stream).and_then(|pending| pending.reply);
                self.persistent.remove(&stream);
                if let Some(reply) = reply {
                    let _ = reply.send(Err(Error::Network(format!(
                        "the realtime stream was refused (code {code:?})"
                    ))));
                }
                return Ok(());
            }
            self.finish_open_if_ready(stream).await;
        }
        Ok(())
    }

    async fn data(
        &mut self,
        stream: u16,
        ack_id: u16,
        requires_ack: bool,
        payload: Vec<u8>,
    ) -> Result<(), Error> {
        if let Some(reply) = self.oneoff.remove(&stream) {
            if requires_ack {
                self.write(&[Frame::Ack { stream, ack_id }]).await?;
            }
            let _ = reply.send(Ok(payload));
            return self.write(&[Frame::EndOfData { stream }]).await;
        }
        if let Some(pushes) = self.persistent.get(&stream) {
            if pushes.send(payload).is_err() {
                self.persistent.remove(&stream);
            }
        } else {
            debug!("meta transport: data for unknown stream {stream}");
        }
        if requires_ack {
            self.write(&[Frame::Ack { stream, ack_id }]).await?;
        }
        Ok(())
    }

    async fn finish_open_if_ready(&mut self, stream: u16) {
        let ready = self
            .opening
            .get(&stream)
            .is_some_and(|pending| pending.established && pending.acked);
        if !ready {
            return;
        }
        if let Some(mut pending) = self.opening.remove(&stream) {
            if let Some(reply) = pending.reply.take() {
                let _ = reply.send(Ok(stream));
            }
        }
    }

    fn close_stream(&mut self, stream: u16) {
        self.persistent.remove(&stream);
        if let Some(reply) = self.oneoff.remove(&stream) {
            let _ = reply.send(Err(Error::Network("the realtime stream ended early".to_owned())));
        }
        if let Some(mut pending) = self.opening.remove(&stream) {
            if let Some(reply) = pending.reply.take() {
                let _ = reply.send(Err(Error::Network("the realtime stream ended early".to_owned())));
            }
        }
    }
}

/// The `code` an establish answer carries: either the object's `code` or the bare number.
fn establish_code(parameters: &[u8]) -> Option<i64> {
    let value: serde_json::Value = serde_json::from_slice(parameters).ok()?;
    match value {
        serde_json::Value::Object(map) => map.get("code")?.as_i64(),
        other => other.as_i64(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::{TcpListener, TcpStream};
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, accept_async, connect_async};

    type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;
    type Server = WebSocketStream<TcpStream>;

    /// A transport connected to a local websocket server the test drives frame by frame.
    async fn pair() -> (Server, Handle) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = async {
            let (stream, _) = listener.accept().await.expect("accept");
            accept_async(stream).await.expect("handshake")
        };
        let client = async {
            let (socket, _) = connect_async(format!("ws://{address}")).await.expect("connect");
            socket
        };
        let (server, client): (Server, Client) = tokio::join!(server, client);
        (server, spawn(client))
    }

    async fn next_frames(server: &mut Server) -> Vec<Frame> {
        loop {
            match server.next().await.expect("a message").expect("no socket error") {
                WsMessage::Binary(bytes) => return Frame::decode_all(&bytes).expect("decodes"),
                _ => continue,
            }
        }
    }

    async fn send_frames(server: &mut Server, frames: &[Frame]) {
        let bytes = Frame::encode_all(frames, false);
        server
            .send(WsMessage::Binary(bytes.into()))
            .await
            .expect("server writes");
    }

    #[tokio::test]
    async fn a_request_resolves_with_the_response_and_is_acknowledged() {
        let (mut server, handle) = pair().await;
        let request = tokio::spawn({
            let handle = handle.clone();
            async move { handle.request(b"{\"tasks\":[]}".to_vec()).await }
        });
        let frames = next_frames(&mut server).await;
        let [Frame::EstablishStream { stream, .. }, Frame::Data { payload, ack_id, requires_ack, .. }] =
            frames.as_slice()
        else {
            panic!("unexpected opening frames: {frames:?}");
        };
        assert_eq!(payload, b"{\"tasks\":[]}");
        assert_eq!(*ack_id, 0);
        assert!(requires_ack);
        let stream = *stream;
        send_frames(
            &mut server,
            &[
                Frame::EstablishStream {
                    stream,
                    parameters: b"{\"code\":200}".to_vec(),
                },
                Frame::Data {
                    stream,
                    ack_id: 9,
                    requires_ack: true,
                    content_type: None,
                    payload: b"{\"payload\":\"ok\"}".to_vec(),
                },
            ],
        )
        .await;
        // The ack and the end of data leave in two websocket messages, in that order.
        assert_eq!(next_frames(&mut server).await, vec![Frame::Ack { stream, ack_id: 9 }]);
        assert_eq!(next_frames(&mut server).await, vec![Frame::EndOfData { stream }]);
        let response = request.await.expect("joins").expect("answered");
        assert_eq!(response, b"{\"payload\":\"ok\"}");
    }

    #[tokio::test]
    async fn a_persistent_stream_opens_after_establish_and_ack_then_delivers_pushes() {
        let (mut server, handle) = pair().await;
        let (pushes_tx, mut pushes) = mpsc::unbounded_channel();
        let open = tokio::spawn({
            let handle = handle.clone();
            async move { handle.open(b"{\"database\":1}".to_vec(), pushes_tx).await }
        });
        let frames = next_frames(&mut server).await;
        let stream = match frames.first() {
            Some(Frame::EstablishStream { stream, .. }) => *stream,
            other => panic!("expected an establish frame, got {other:?}"),
        };
        send_frames(
            &mut server,
            &[
                Frame::EstablishStream {
                    stream,
                    parameters: b"{\"code\":200}".to_vec(),
                },
                Frame::Ack { stream, ack_id: 0 },
            ],
        )
        .await;
        assert_eq!(open.await.expect("joins").expect("opened"), stream);

        send_frames(
            &mut server,
            &[Frame::Data {
                stream,
                ack_id: 3,
                requires_ack: true,
                content_type: None,
                payload: b"push".to_vec(),
            }],
        )
        .await;
        assert_eq!(pushes.recv().await.expect("a push"), b"push");
        assert_eq!(next_frames(&mut server).await, vec![Frame::Ack { stream, ack_id: 3 }]);
    }

    #[tokio::test]
    async fn a_refused_establish_fails_the_open() {
        let (mut server, handle) = pair().await;
        let (pushes_tx, _pushes) = mpsc::unbounded_channel();
        let open = tokio::spawn({
            let handle = handle.clone();
            async move { handle.open(b"{}".to_vec(), pushes_tx).await }
        });
        let frames = next_frames(&mut server).await;
        let Some(Frame::EstablishStream { stream, .. }) = frames.first() else {
            panic!("expected an establish frame");
        };
        send_frames(
            &mut server,
            &[Frame::EstablishStream {
                stream: *stream,
                parameters: b"{\"code\":403}".to_vec(),
            }],
        )
        .await;
        assert!(matches!(open.await.expect("joins"), Err(Error::Network(_))));
    }

    #[tokio::test]
    async fn a_post_waits_for_the_server_ack() {
        let (mut server, handle) = pair().await;
        let (pushes_tx, _pushes) = mpsc::unbounded_channel();
        let open = tokio::spawn({
            let handle = handle.clone();
            async move { handle.open(b"{}".to_vec(), pushes_tx).await }
        });
        let frames = next_frames(&mut server).await;
        let Some(Frame::EstablishStream { stream, .. }) = frames.first() else {
            panic!("expected an establish frame");
        };
        let stream = *stream;
        send_frames(
            &mut server,
            &[
                Frame::EstablishStream {
                    stream,
                    parameters: b"{\"code\":200}".to_vec(),
                },
                Frame::Ack { stream, ack_id: 0 },
            ],
        )
        .await;
        open.await.expect("joins").expect("opened");

        let post = tokio::spawn({
            let handle = handle.clone();
            async move { handle.post(stream, b"next".to_vec()).await }
        });
        let frames = next_frames(&mut server).await;
        let [Frame::Data { ack_id, payload, .. }] = frames.as_slice() else {
            panic!("expected one data frame, got {frames:?}");
        };
        assert_eq!(payload, b"next");
        send_frames(&mut server, &[Frame::Ack { stream, ack_id: *ack_id }]).await;
        post.await.expect("joins").expect("acked");
    }

    #[tokio::test]
    async fn the_transport_answers_pings_with_pongs() {
        let (mut server, _handle) = pair().await;
        send_frames(&mut server, &[Frame::Ping]).await;
        assert_eq!(next_frames(&mut server).await, vec![Frame::Pong]);
    }
}
