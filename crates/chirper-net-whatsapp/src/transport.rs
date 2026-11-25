//! The one thing the login and the session need from the socket: exchange binary nodes.
//!
//! [`Connection`] implements it for the real Noise-encrypted WebSocket. Tests implement it with an
//! in-memory server that speaks nodes directly, so the login, receive and send paths run end to
//! end without a network.

use async_trait::async_trait;
use chirper_bridge::Result;

use crate::binary::Node;
use crate::socket::Connection;

/// A connection that carries binary nodes in both directions.
#[async_trait]
pub trait Wire: Send {
    /// Sends one node.
    async fn send_node(&mut self, node: &Node) -> Result<()>;
    /// The next node from the server. Dropping the future loses no bytes.
    async fn next_node(&mut self) -> Result<Node>;
    /// Closes the connection. Errors are ignored: the connection is gone either way.
    async fn close(&mut self);
}

#[async_trait]
impl Wire for Connection {
    async fn send_node(&mut self, node: &Node) -> Result<()> {
        Connection::send_node(self, node).await
    }

    async fn next_node(&mut self) -> Result<Node> {
        Connection::next_node(self).await
    }

    async fn close(&mut self) {
        Connection::close(self).await;
    }
}
