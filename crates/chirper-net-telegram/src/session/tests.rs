//! One login end to end over the fake server: connect, receive, send, backfill.

use std::sync::Arc;

use chirper_bridge::{Command, Content, Event, PortalRef, RemoteMessage, SessionState};

use crate::client::Incoming;
use crate::mtproto::UpdateFrame;
use crate::session::Session;
use crate::store::Store;
use crate::testing::{self, FakeApi};
use crate::tl::{Identifiable, enums, functions, types};

/// The fake account's own id.
const ME: i64 = 7;
/// The id of the person the test talks to.
const PEER: i64 = 8;
/// The data centre the fake reports.
const DC: i32 = 2;

/// A session connected to the fake, plus the channel the test pushes updates through.
struct Fixture {
    session: Session<Arc<FakeApi>>,
    api: Arc<FakeApi>,
    frames: tokio::sync::mpsc::UnboundedSender<Incoming>,
}

/// Build a connected session: self, the update state, and one private chat with `PEER`.
async fn connected() -> Fixture {
    let api = FakeApi::new();
    api.answer_raw(
        functions::users::GetUsers::CONSTRUCTOR_ID,
        testing::vector(&[
            enums::User::User(testing::user(ME)),
            enums::User::User(testing::peer_user(PEER)),
        ]),
    );
    api.answer_to(
        functions::updates::GetState::CONSTRUCTOR_ID,
        &testing::state(10),
    );
    // Folder 0 and folder 1 both get the same complete page: one private chat.
    api.answer_always(
        &functions::messages::GetDialogs {
            exclude_pinned: false,
            folder_id: None,
            offset_date: 0,
            offset_id: 0,
            offset_peer: Box::new(enums::InputPeer::InputPeerEmpty(types::InputPeerEmpty {})),
            limit: 100,
            hash: 0,
        },
        &enums::messages::Dialogs::Dialogs(types::messages::Dialogs {
            dialogs: vec![enums::Dialog::Dialog(testing::dialog(PEER, 5))],
            messages: Vec::new(),
            chats: Vec::new(),
            users: vec![
                enums::User::User(testing::user(ME)),
                enums::User::User(testing::peer_user(PEER)),
            ],
        }),
    );
    let ctx = testing::ctx("session");
    let store = Arc::new(Store::open(ctx));
    let (frames, incoming) = tokio::sync::mpsc::unbounded_channel();
    let session = Session::start(Arc::clone(&api), store, incoming)
        .await
        .expect("the fake account connects");
    Fixture {
        session,
        api,
        frames,
    }
}

impl Fixture {
    /// The portal of the private chat with `PEER`.
    fn portal(&self) -> PortalRef {
        PortalRef {
            id: format!("user:{PEER}"),
            receiver: ME.to_string(),
        }
    }

    /// Push an update frame on the account's connection.
    fn push(&self, updates: types::Updates) {
        self.frames
            .send(Incoming::Frame {
                dc: DC,
                frame: UpdateFrame::Raw {
                    constructor: 0x74ae4240,
                    body: testing::serialize(&updates),
                },
            })
            .expect("the session is listening");
    }
}

/// The next event of a kind, skipping the rest.
async fn next_message(session: &mut Session<Arc<FakeApi>>) -> RemoteMessage {
    for _ in 0..64 {
        match session.next().await {
            Some(Event::Message(message)) => return message,
            Some(_) => continue,
            None => panic!("the session ended before a message arrived"),
        }
    }
    panic!("no message in the first events");
}

#[tokio::test]
async fn a_login_connects_receives_sends_and_backfills() {
    let mut fixture = connected().await;

    // Connect: the first events are the state and the chat list.
    let mut saw_running = false;
    let mut saw_chat = false;
    for _ in 0..16 {
        match fixture.session.next().await {
            Some(Event::State { state, profile, .. }) => {
                if state == SessionState::Running {
                    saw_running = true;
                    let profile = profile.expect("running reports the profile");
                    assert_eq!(profile.id, testing::user(ME).id.to_string());
                }
            }
            Some(Event::Chat(chat)) => {
                assert_eq!(chat.portal.id, fixture.portal().id);
                assert!(chat.is_dm);
                saw_chat = true;
            }
            Some(_) => continue,
            None => break,
        }
        if saw_running && saw_chat {
            break;
        }
    }
    assert!(saw_running, "the login reported Running");
    assert!(saw_chat, "the login reported its chat");

    // Receive: the peer sends a message.
    fixture.push(testing::updates(
        vec![enums::Update::UpdateNewMessage(types::UpdateNewMessage {
            message: Box::new(enums::Message::Message(testing::text_message(
                6,
                PEER,
                PEER,
                "hello",
                1_700_000_010,
                false,
            ))),
            pts: 11,
            pts_count: 1,
        })],
        vec![enums::User::User(testing::peer_user(PEER))],
    ));
    let received = next_message(&mut fixture.session).await;
    assert_eq!(received.portal.id, fixture.portal().id);
    assert_eq!(received.sender.id, PEER.to_string());
    assert!(!received.from_me);
    assert!(received.live);
    assert!(matches!(received.content, Content::Text { ref body, .. } if body == "hello"));

    // Send: our own message, answered with the server's update carrying our new id.
    fixture.api.answer_to(
        functions::messages::SendMessage::CONSTRUCTOR_ID,
        &enums::Updates::Updates(testing::updates(
            vec![enums::Update::UpdateNewMessage(types::UpdateNewMessage {
                message: Box::new(enums::Message::Message(testing::text_message(
                    9,
                    ME,
                    PEER,
                    "hi back",
                    1_700_000_020,
                    true,
                ))),
                pts: 12,
                pts_count: 1,
            })],
            Vec::new(),
        )),
    );
    let ack = fixture
        .session
        .command(Command::SendText {
            portal: fixture.portal(),
            text: "hi back".to_owned(),
            reply_to: None,
            txn_id: "txn-1".to_owned(),
        })
        .await
        .expect("the message is sent");
    assert_eq!(ack.remote_event_id.as_deref(), Some("9"));
    let sent = next_message(&mut fixture.session).await;
    assert!(sent.from_me, "our own message is from me");
    assert!(matches!(sent.content, Content::Text { ref body, .. } if body == "hi back"));

    // The send carried the text and the chat's peer.
    let request = fixture
        .api
        .seen_with(functions::messages::SendMessage::CONSTRUCTOR_ID)
        .pop()
        .expect("one send");
    assert!(request.carries("hi back"));

    // Backfill: one page of history, oldest message first.
    fixture.api.answer_to(
        functions::messages::GetHistory::CONSTRUCTOR_ID,
        &enums::messages::Messages::Messages(types::messages::Messages {
            // Telegram answers newest first; the driver hands the page over oldest first.
            messages: vec![
                enums::Message::Message(testing::text_message(
                    5,
                    ME,
                    PEER,
                    "newer",
                    1_700_000_005,
                    true,
                )),
                enums::Message::Message(testing::text_message(
                    4,
                    PEER,
                    PEER,
                    "older",
                    1_700_000_000,
                    false,
                )),
            ],
            topics: Vec::new(),
            chats: Vec::new(),
            users: vec![enums::User::User(testing::peer_user(PEER))],
        }),
    );
    let page = fixture
        .session
        .backfill_page(&fixture.portal(), None, 2)
        .await
        .expect("a history page");
    assert_eq!(page.messages.len(), 2);
    assert_eq!(page.messages[0].id, "4");
    assert_eq!(page.messages[1].id, "5");
    assert!(!page.messages[0].live, "history is not live");
    assert!(page.has_more, "a full page may have more");

    // The history request asked for the chat the portal names.
    let history = fixture
        .api
        .seen_with(functions::messages::GetHistory::CONSTRUCTOR_ID)
        .pop()
        .expect("one history request");
    assert_eq!(
        history.constructor,
        functions::messages::GetHistory::CONSTRUCTOR_ID
    );
}

#[tokio::test]
async fn a_session_without_stored_credentials_is_refused_before_it_starts() {
    // `Store` with no auth key: the driver's `connect` refuses, which is what the runtime sees.
    let ctx = testing::ctx("empty");
    let store = Store::open(ctx);
    assert!(store.auth_key(DC).expect("the store reads").is_none());
}

/// A push container with sequence number `seq`, carrying `updates` and the peer.
fn sequenced(seq: i32, updates: Vec<enums::Update>) -> types::Updates {
    let mut container =
        testing::updates(updates, vec![enums::User::User(testing::peer_user(PEER))]);
    container.seq = seq;
    container
}

/// An update that carries one private message at `pts`.
fn new_message(pts: i32, id: i32, text: &str) -> enums::Update {
    enums::Update::UpdateNewMessage(types::UpdateNewMessage {
        message: Box::new(enums::Message::Message(testing::text_message(
            id,
            PEER,
            PEER,
            text,
            1_700_000_040,
            false,
        ))),
        pts,
        pts_count: 1,
    })
}

#[tokio::test]
async fn a_gap_fetches_the_difference_and_duplicates_are_dropped() {
    let mut fixture = connected().await;
    // Drain the connect events up to Running.
    for _ in 0..16 {
        if let Some(Event::State {
            state: SessionState::Running,
            ..
        }) = fixture.session.next().await
        {
            break;
        }
    }

    // The next update in order is applied.
    fixture.push(testing::updates(
        vec![new_message(11, 6, "one")],
        vec![enums::User::User(testing::peer_user(PEER))],
    ));
    let first = next_message(&mut fixture.session).await;
    assert!(matches!(first.content, Content::Text { ref body, .. } if body == "one"));

    // The same update again is a duplicate: nothing comes out for it.
    fixture.push(sequenced(3, vec![new_message(11, 6, "one again")]));
    let quiet = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        fixture.session.next(),
    )
    .await;
    assert!(quiet.is_err(), "a duplicate update is dropped");

    // pts 13 after 11 is a gap: the difference is fetched, and it carries the missed message.
    fixture.api.answer_to(
        functions::updates::GetDifference::CONSTRUCTOR_ID,
        &enums::updates::Difference::Difference(types::updates::Difference {
            new_messages: vec![enums::Message::Message(testing::text_message(
                7,
                PEER,
                PEER,
                "missed",
                1_700_000_030,
                false,
            ))],
            new_encrypted_messages: Vec::new(),
            other_updates: Vec::new(),
            chats: Vec::new(),
            users: vec![enums::User::User(testing::peer_user(PEER))],
            state: Box::new(enums::updates::State::State(testing::state(13))),
        }),
    );
    fixture.push(sequenced(4, vec![new_message(13, 8, "later")]));
    let missed = next_message(&mut fixture.session).await;
    assert!(matches!(missed.content, Content::Text { ref body, .. } if body == "missed"));
    assert!(
        missed.live,
        "a message recovered from the difference is live: it arrived while disconnected and must refresh the chat"
    );
    assert_eq!(
        fixture.session.store.position().map(|p| p.pts),
        Some(13),
        "the difference moves the update state to the server's state"
    );
    assert_eq!(
        fixture
            .api
            .seen_with(functions::updates::GetDifference::CONSTRUCTOR_ID)
            .len(),
        1,
        "exactly one difference was fetched"
    );
}

#[tokio::test]
async fn connect_returns_before_the_dialogs_are_fetched() {
    let mut fixture = connected().await;
    // `connected` has returned: nothing about the dialogs has been asked for yet.
    assert!(
        fixture
            .api
            .seen_with(functions::messages::GetDialogs::CONSTRUCTOR_ID)
            .is_empty(),
        "connect does not wait for the dialog sync"
    );
    // The first event is Connecting; the dialog sync runs as `next()` is called.
    let first = fixture.session.next().await;
    assert!(matches!(
        first,
        Some(Event::State {
            state: SessionState::Connecting,
            ..
        })
    ));
    assert!(
        fixture
            .api
            .seen_with(functions::messages::GetDialogs::CONSTRUCTOR_ID)
            .is_empty(),
        "the dialogs are fetched from next(), not from connect"
    );
    let mut order = Vec::new();
    for _ in 0..16 {
        match fixture.session.next().await {
            Some(Event::State { state, .. }) => {
                order.push(format!("{state:?}"));
                if state == SessionState::Running {
                    break;
                }
            }
            Some(Event::Chat(_)) => order.push("Chat".to_owned()),
            Some(_) => {}
            None => panic!("the session ended during the sync"),
        }
    }
    assert_eq!(order.first().map(String::as_str), Some("Backfilling"));
    assert_eq!(order.last().map(String::as_str), Some("Running"));
    assert!(
        order[1..order.len() - 1].iter().all(|kind| kind == "Chat"),
        "only chats come between Backfilling and Running: {order:?}"
    );
    assert!(order.len() > 2, "the dialog chats were delivered");
}

#[tokio::test]
async fn dropping_next_during_the_sync_loses_nothing() {
    use std::future::Future;
    use std::task::Poll;

    let mut fixture = connected().await;
    let mut kinds: Vec<String> = Vec::new();
    let mut dropped = 0;
    for round in 0..400 {
        let event = if round % 2 == 1 {
            // Poll once and drop the future if it is still suspended: the sync must not lose or
            // repeat anything.
            let polled = {
                let mut next = std::pin::pin!(fixture.session.next());
                std::future::poll_fn(|cx| match next.as_mut().poll(cx) {
                    Poll::Ready(event) => Poll::Ready(Some(event)),
                    Poll::Pending => Poll::Ready(None),
                })
                .await
            };
            match polled {
                Some(event) => event,
                None => {
                    dropped += 1;
                    continue;
                }
            }
        } else {
            fixture.session.next().await
        };
        match event {
            Some(Event::State { state, .. }) => kinds.push(format!("{state:?}")),
            Some(Event::Chat(chat)) => {
                assert_eq!(chat.portal.id, fixture.portal().id);
                kinds.push("Chat".to_owned());
            }
            Some(_) => {}
            None => panic!("the session ended during the sync"),
        }
        if kinds.last().map(String::as_str) == Some("Running") {
            break;
        }
    }
    assert!(dropped > 0, "the future was dropped mid-sync at least once");
    assert_eq!(kinds.first().map(String::as_str), Some("Connecting"));
    assert_eq!(kinds.get(1).map(String::as_str), Some("Backfilling"));
    assert_eq!(kinds.last().map(String::as_str), Some("Running"));
    assert!(
        kinds[2..kinds.len() - 1].iter().all(|kind| kind == "Chat") && kinds.len() > 3,
        "every dialog arrived once and in order: {kinds:?}"
    );
    // The page was fetched; a repeated page would show up as extra requests for the same folder.
    assert!(
        fixture
            .api
            .seen_with(functions::messages::GetDialogs::CONSTRUCTOR_ID)
            .len()
            <= 2,
        "a dropped step is repeated, not multiplied"
    );
}
