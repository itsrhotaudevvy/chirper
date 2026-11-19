//! A demo network that exists only in this process.
//!
//! It is the reference driver: small, deterministic, no credentials and no I/O, so it runs in unit
//! tests, in the UI development build (`--features mock`) and, because nothing here does network or
//! file I/O, on wasm. It also documents the shape a real driver takes.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chirper_bridge::model::{BackfillPage, ChatMeta, Content, MediaInfo, Member, RemoteEventRef};
use chirper_bridge::{
    Ack, Capabilities, Command, Error, Event, LoginCtx, LoginFlow, LoginInput, LoginProcess,
    LoginStep, Network, PortalRef, RemoteMessage, RemoteUser, Result, Session,
};

/// The network id used in settings and on the wire.
pub const NETWORK_ID: &str = "testnet";
/// The login id of every demo account.
pub const LOGIN_ID: &str = "demo";

const ME: &str = "me";
const ADA: &str = "ada";
const GRACE: &str = "grace";

fn user(id: &str, name: &str) -> RemoteUser {
    RemoteUser {
        id: id.to_owned(),
        display_name: name.to_owned(),
        username: id.to_owned(),
        ..Default::default()
    }
}

/// The demo network.
#[derive(Default)]
pub struct TestNet;

impl TestNet {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Default)]
struct State {
    chats: Vec<ChatMeta>,
    history: HashMap<String, Vec<RemoteMessage>>,
    queue: VecDeque<Event>,
    counter: u64,
}

impl State {
    fn next_id(&mut self) -> String {
        self.counter += 1;
        format!("demo-{}", self.counter)
    }
}

fn seeded() -> State {
    let mut state = State::default();
    let dm = PortalRef {
        id: "dm-ada".to_owned(),
        receiver: String::new(),
    };
    let group = PortalRef {
        id: "group-design".to_owned(),
        receiver: String::new(),
    };
    state.chats = vec![
        ChatMeta {
            portal: dm.clone(),
            name: "Ada Lovelace".to_owned(),
            topic: "Analytical engines".to_owned(),
            is_dm: true,
            member_count: 2,
            dm_user: Some(user(ADA, "Ada Lovelace")),
            ..Default::default()
        },
        ChatMeta {
            portal: group.clone(),
            name: "Design".to_owned(),
            topic: "A demo group chat".to_owned(),
            member_count: 3,
            ..Default::default()
        },
    ];
    state.history.insert(
        dm.id.clone(),
        vec![RemoteMessage {
            portal: dm.clone(),
            id: "dm-ada-1".to_owned(),
            part: String::new(),
            sender: user(ADA, "Ada Lovelace"),
            timestamp_ms: 1_700_000_000_000,
            content: Content::Text {
                body: "Welcome to the demo network.".to_owned(),
                formatted: None,
            },
            reply_to: None,
            edit_of: None,
            live: false,
            from_me: false,
        }],
    );
    state.history.insert(
        group.id.clone(),
        vec![RemoteMessage {
            portal: group.clone(),
            id: "group-design-1".to_owned(),
            part: String::new(),
            sender: user(GRACE, "Grace Hopper"),
            timestamp_ms: 1_700_000_100_000,
            content: Content::Text {
                body: "This chat lives entirely inside the app.".to_owned(),
                formatted: None,
            },
            reply_to: None,
            edit_of: None,
            live: false,
            from_me: false,
        }],
    );
    state
}

#[async_trait]
impl Network for TestNet {
    fn id(&self) -> &'static str {
        NETWORK_ID
    }

    fn name(&self) -> &'static str {
        "Demo"
    }

    fn color(&self) -> &'static str {
        "#8E8E93"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::ALL
    }

    fn login_flows(&self) -> Vec<LoginFlow> {
        vec![LoginFlow {
            id: "demo".to_owned(),
            name: "Demo account".to_owned(),
            description: "Adds the in-app demo network. No credentials are needed.".to_owned(),
        }]
    }

    async fn start_login(&self, flow_id: &str, _ctx: LoginCtx) -> Result<Box<dyn LoginProcess>> {
        if flow_id != "demo" {
            return Err(Error::Unsupported("this login flow"));
        }
        Ok(Box::new(DemoLogin))
    }

    async fn connect(&self, _ctx: LoginCtx) -> Result<Box<dyn Session>> {
        let state = Arc::new(Mutex::new(seeded()));
        {
            let mut state = state.lock().expect("demo state");
            let chats = state.chats.clone();
            for chat in chats {
                state.queue.push_back(Event::Chat(chat));
            }
            let dm = user(ADA, "Ada Lovelace");
            state.queue.push_back(Event::Member {
                portal: PortalRef {
                    id: "dm-ada".to_owned(),
                    receiver: String::new(),
                },
                member: Member {
                    user: dm,
                    membership: "join".to_owned(),
                },
            });
        }
        Ok(Box::new(DemoSession {
            state,
            me: user(ME, "You"),
        }))
    }

    fn supports_remote_logout(&self) -> bool {
        false
    }
}

struct DemoLogin;

#[async_trait]
impl LoginProcess for DemoLogin {
    async fn next(&mut self, _input: LoginInput) -> Result<LoginStep> {
        Ok(LoginStep::Complete {
            login_id: LOGIN_ID.to_owned(),
        })
    }
}

struct DemoSession {
    state: Arc<Mutex<State>>,
    me: RemoteUser,
}

impl DemoSession {
    fn push(&self, event: Event) {
        self.state
            .lock()
            .expect("demo state")
            .queue
            .push_back(event);
    }

    fn create(&self, portal: PortalRef, content: Content) -> (String, RemoteMessage) {
        let mut state = self.state.lock().expect("demo state");
        let id = state.next_id();
        let message = RemoteMessage {
            portal: portal.clone(),
            id: id.clone(),
            part: String::new(),
            sender: self.me.clone(),
            timestamp_ms: 1_700_000_200_000 + state.counter as i64,
            content,
            reply_to: None,
            edit_of: None,
            live: true,
            from_me: true,
        };
        state
            .history
            .entry(portal.id.clone())
            .or_default()
            .push(message.clone());
        (id, message)
    }
}

#[async_trait]
impl Session for DemoSession {
    async fn next(&mut self) -> Option<Event> {
        let next = self.state.lock().expect("demo state").queue.pop_front();
        match next {
            Some(event) => Some(event),
            None => std::future::pending().await,
        }
    }

    async fn command(&mut self, cmd: Command) -> Result<Ack> {
        match cmd {
            Command::SendText {
                portal,
                text,
                txn_id,
                ..
            } => {
                let (id, message) = self.create(
                    portal.clone(),
                    Content::Text {
                        body: text,
                        formatted: None,
                    },
                );
                self.push(Event::Message(message));
                self.push(Event::Delivery {
                    portal,
                    txn_id,
                    status: chirper_bridge::DeliveryStatus::Sent,
                    error: String::new(),
                });
                Ok(Ack {
                    remote_event_id: Some(id),
                })
            }
            Command::SendMedia {
                portal,
                mime,
                file_name,
                caption,
                kind,
                txn_id,
                data,
                ..
            } => {
                let (id, message) = self.create(
                    portal.clone(),
                    Content::Media {
                        kind,
                        caption,
                        info: MediaInfo {
                            mime,
                            file_name,
                            size: data.len(),
                            ..Default::default()
                        },
                        data,
                        thumbnail: None,
                    },
                );
                self.push(Event::Message(message));
                self.push(Event::Delivery {
                    portal,
                    txn_id,
                    status: chirper_bridge::DeliveryStatus::Sent,
                    error: String::new(),
                });
                Ok(Ack {
                    remote_event_id: Some(id),
                })
            }
            Command::Edit {
                portal,
                target,
                text,
            } => {
                let message = RemoteMessage {
                    portal,
                    id: target,
                    part: String::new(),
                    sender: self.me.clone(),
                    timestamp_ms: 1_700_000_300_000,
                    content: Content::Text {
                        body: text,
                        formatted: None,
                    },
                    reply_to: None,
                    edit_of: None,
                    live: false,
                    from_me: true,
                };
                self.push(Event::Message(message));
                Ok(Ack::default())
            }
            Command::Redact { portal, target } => {
                self.push(Event::Redact {
                    portal,
                    event: RemoteEventRef::new(target),
                });
                Ok(Ack::default())
            }
            Command::React {
                portal,
                target,
                key,
            } => {
                let mut state = self.state.lock().expect("demo state");
                state.queue.push_back(Event::Reactions {
                    portal,
                    target: RemoteEventRef::new(target),
                    reactions: vec![chirper_bridge::Reaction {
                        key,
                        count: 1,
                        from_me: true,
                        senders: vec![user(ME, "You")],
                    }],
                });
                Ok(Ack::default())
            }
            Command::Unreact { portal, target, .. } => {
                self.push(Event::Reactions {
                    portal,
                    target: RemoteEventRef::new(target),
                    reactions: Vec::new(),
                });
                Ok(Ack::default())
            }
            Command::Read { .. } | Command::Typing { .. } => Ok(Ack::default()),
        }
    }

    async fn backfill(
        &mut self,
        portal: &PortalRef,
        cursor: Option<String>,
        limit: usize,
    ) -> Result<BackfillPage> {
        let state = self.state.lock().expect("demo state");
        let history = state.history.get(&portal.id).cloned().unwrap_or_default();
        let end = cursor
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or(history.len())
            .min(history.len());
        let start = end.saturating_sub(limit);
        let page = history[start..end].to_vec();
        Ok(BackfillPage {
            messages: page,
            cursor: (start > 0).then(|| start.to_string()),
            has_more: start > 0,
        })
    }

    async fn logout_remote(&mut self) -> Result<()> {
        Err(Error::Unsupported("remote logout"))
    }

    fn user(&self) -> Option<RemoteUser> {
        Some(self.me.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connect_reports_the_seeded_chats() {
        let net = TestNet::new();
        let mut session = net
            .connect(LoginCtx {
                network: NETWORK_ID.to_owned(),
                login_id: LOGIN_ID.to_owned(),
                data_dir: std::path::PathBuf::from("/tmp"),
                secrets: Arc::new(NoSecrets),
                http: chirper_bridge::http_client(),
            })
            .await
            .unwrap();
        assert_eq!(session.user().unwrap().id, ME);
        assert!(matches!(session.next().await, Some(Event::Chat(_))));
        assert!(matches!(session.next().await, Some(Event::Chat(_))));
        assert!(matches!(session.next().await, Some(Event::Member { .. })));
    }

    struct NoSecrets;

    impl chirper_bridge::Secrets for NoSecrets {
        fn get(&self, _key: &str) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>> {
            Ok(None)
        }
        fn set(&self, _key: &str, _value: &[u8]) -> Result<()> {
            Ok(())
        }
        fn delete(&self, _key: &str) -> Result<()> {
            Ok(())
        }
    }
}
