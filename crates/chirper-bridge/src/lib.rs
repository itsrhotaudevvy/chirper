//! Chirper's unified bridge runtime.
//!
//! One framework for every network: [`Network`] describes a network, [`Session`] runs one live
//! login, [`Runtime`] supervises logins and turns remote ids into local ones. There is no Matrix
//! in this crate and no database: drivers report domain [`Event`]s and the engine persists them.
//!
//! ```text
//! driver (per network)  ->  Event (remote ids)  ->  Runtime (local ids)  ->  engine (store, UI)
//!                                                            ^
//!                                                            └── Command (send, edit, react, …)
//! ```

pub mod ids;
pub mod model;
pub mod network;
pub mod runtime;

pub use ids::{ChatId, EventId, SenderId, chat_id, event_id_for, sender_id};
pub use model::{
    Ack, BackfillPage, Capabilities, ChatMeta, Content, DeliveryStatus, Event, LoginFlow,
    LoginStep, MediaInfo, Member, PortalRef, Reaction, RemoteEventRef, RemoteMessage, RemoteUser,
    SessionState,
};
pub use network::{
    Command, Error, LoginCtx, LoginInput, LoginProcess, Network, Result, Secrets, Session,
};
pub use runtime::{Routed, RoutedEvent, Runtime};

/// Build the HTTP client every driver shares: rustls only, no redirects to other hosts, bounded
/// response bodies and a fixed user agent.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("Chirper/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(60))
        .pool_max_idle_per_host(4)
        .build()
        .expect("the shared HTTP client configuration is valid")
}
