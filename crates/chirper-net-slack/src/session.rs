//! One live Slack login: `connect` checks the stored token and starts the socket task; the session
//! then sends commands over the Web API and pages history for backfill.
//!
//! Commands follow `connector/handlematrix.go` and `msgconv/from-matrix.go` of the Go bridge: a
//! reply in a thread posts with `thread_ts`; an edit is `chat.update`; a redaction is `chat.delete`;
//! reactions use the emoji name; read receipts are `conversations.mark` (bot tokens cannot mark, so
//! they are accepted and do nothing, as the Go bridge does); typing goes over the socket.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use log::debug;
use tokio::sync::mpsc;

use chirper_bridge::model::{Ack, BackfillPage, DeliveryStatus, Event, PortalRef, RemoteUser};
use chirper_bridge::{Command, Error, LoginCtx, Result, Session};

use crate::api::Api;
use crate::convert::{self, Scope};
use crate::directory::Directory;
use crate::ids;
use crate::rtm::{self, Link, Outgoing};
use crate::Endpoints;

/// The most messages one backfill page asks for.
const MAX_PAGE: usize = 200;

/// A live login. Events come from the socket task; commands go over the Web API.
pub struct SlackSession {
    api: Api,
    team: String,
    me: String,
    login_id: String,
    me_user: RemoteUser,
    directory: Arc<Directory>,
    events: mpsc::UnboundedReceiver<Event>,
    events_tx: mpsc::UnboundedSender<Event>,
    outgoing: mpsc::UnboundedSender<Outgoing>,
}

/// Check the stored credentials, then start the socket task. The first event is the connecting
/// state; the connection itself is reported from the task.
pub async fn connect(endpoints: &Endpoints, ctx: LoginCtx) -> Result<SlackSession> {
    let token = stored_token(&ctx)?;
    let cookie = stored_cookie(&ctx)?;
    let api = Api::new(ctx.http.clone(), &endpoints.api, &token, cookie);
    let who = api.auth_test().await?;
    if who.team_id.is_empty() || who.user_id.is_empty() {
        return Err(Error::Auth("Slack named no team or user".to_owned()));
    }
    let me_user = RemoteUser {
        id: ids::user_id(&who.team_id, &who.user_id),
        display_name: who.user.clone(),
        username: who.user.clone(),
        ..RemoteUser::default()
    };
    let directory = Arc::new(Directory::default());
    let (events_tx, events) = mpsc::unbounded_channel();
    let (outgoing, outgoing_rx) = mpsc::unbounded_channel();
    let login_id = ids::user_login_id(&who.team_id, &who.user_id);
    let link = Link {
        api: api.clone(),
        team: who.team_id.clone(),
        team_name: who.team.clone(),
        me: who.user_id.clone(),
        login_id: login_id.clone(),
        me_user: me_user.clone(),
        directory: Arc::clone(&directory),
        events: events_tx.clone(),
    };
    debug!("slack: connecting login {login_id}");
    tokio::spawn(rtm::run(link, outgoing_rx));
    Ok(SlackSession {
        api,
        team: who.team_id,
        me: who.user_id,
        login_id,
        me_user,
        directory,
        events,
        events_tx,
        outgoing,
    })
}

fn stored_token(ctx: &LoginCtx) -> Result<String> {
    let secret = ctx
        .get_secret("token")?
        .ok_or_else(|| Error::Auth("no Slack token is stored".to_owned()))?;
    String::from_utf8(secret.to_vec())
        .map_err(|_| Error::Auth("the stored Slack token is not text".to_owned()))
}

/// The `d` cookie of a browser login, when one is stored.
fn stored_cookie(ctx: &LoginCtx) -> Result<Option<String>> {
    let Some(secret) = ctx.get_secret("cookies")? else {
        return Ok(None);
    };
    let stored: HashMap<String, String> = serde_json::from_slice(&secret)
        .map_err(|_| Error::Auth("the stored Slack cookie is unreadable".to_owned()))?;
    Ok(stored.get("d").cloned())
}

impl SlackSession {
    fn scope(&self) -> Scope<'_> {
        Scope {
            team: &self.team,
            me: &self.me,
            login_id: &self.login_id,
            directory: &self.directory,
        }
    }

    /// The team and channel of a portal. The workspace itself takes no messages.
    fn channel_of(&self, portal: &PortalRef) -> Result<String> {
        match ids::parse_portal_id(&portal.id) {
            Some((team, channel)) if team == self.team && !channel.is_empty() => Ok(channel.to_owned()),
            _ => Err(Error::Unsupported("this action in a Slack workspace")),
        }
    }

    fn delivered(&self, portal: &PortalRef, txn_id: String, status: DeliveryStatus) {
        let _ = self.events_tx.send(Event::Delivery {
            portal: portal.clone(),
            txn_id,
            status,
            error: String::new(),
        });
    }

    async fn send_text(&mut self, portal: &PortalRef, text: &str, reply_to: Option<&str>, txn_id: String) -> Result<Ack> {
        let channel = self.channel_of(portal)?;
        let thread = reply_to.and_then(|id| ids::parse_message_id(id)).map(|(_, _, ts)| ts.to_owned());
        let posted = self
            .api
            .post_message(&channel, &convert::outgoing_text(text), thread.as_deref())
            .await?;
        self.delivered(portal, txn_id, DeliveryStatus::Delivered);
        Ok(Ack {
            remote_event_id: message_id_if(&self.team, &channel, &posted.ts),
        })
    }

    async fn send_media(&mut self, portal: &PortalRef, upload: Media<'_>) -> Result<Ack> {
        let channel = self.channel_of(portal)?;
        let thread = upload
            .reply_to
            .and_then(|id| ids::parse_message_id(id))
            .map(|(_, _, ts)| ts.to_owned());
        let reserved = self.api.upload_url(upload.file_name, upload.data.len()).await?;
        self.api
            .put_upload(&reserved.upload_url, upload.mime, upload.data)
            .await?;
        let completed = self
            .api
            .complete_upload(
                &reserved.file_id,
                upload.file_name,
                &channel,
                thread.as_deref(),
                &convert::outgoing_text(upload.caption),
            )
            .await?;
        let ts = completed
            .files
            .iter()
            .filter_map(|file| file.shares.as_ref())
            .flat_map(|shares| shares.public.get(&channel).into_iter().chain(shares.private.get(&channel)))
            .flatten()
            .map(|share| share.ts.clone())
            .find(|ts| !ts.is_empty());
        let status = if ts.is_some() {
            DeliveryStatus::Delivered
        } else {
            DeliveryStatus::Sent
        };
        self.delivered(portal, upload.txn_id, status);
        Ok(Ack {
            remote_event_id: ts.and_then(|ts| message_id_if(&self.team, &channel, &ts)),
        })
    }
}

/// One outgoing upload: the bytes, what Slack should call them, and the message they belong to.
pub(crate) struct Media<'a> {
    pub(crate) data: &'a [u8],
    pub(crate) mime: &'a str,
    pub(crate) file_name: &'a str,
    pub(crate) caption: &'a str,
    pub(crate) reply_to: Option<&'a str>,
    pub(crate) txn_id: String,
}

/// The message id of a Slack timestamp, when there is one.
fn message_id_if(team: &str, channel: &str, ts: &str) -> Option<String> {
    (!ts.is_empty()).then(|| ids::message_id(team, channel, ts))
}

/// The timestamp part of a message id this login may act on.
fn ts_of(id: &str) -> Result<&str> {
    ids::parse_message_id(id)
        .map(|(_, _, ts)| ts)
        .ok_or_else(|| Error::Protocol("the target is not a Slack message".to_owned()))
}

/// The current time as a Slack timestamp, for marking a conversation read when no message is named.
fn now_ts() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    format!("{secs}.000000")
}

/// Whether a message of a history page is one the bridge shows.
fn backfillable(message: &crate::wire::Message) -> bool {
    rtm::is_message_subtype(&message.subtype) && !message.ts.is_empty()
}

#[async_trait]
impl Session for SlackSession {
    async fn next(&mut self) -> Option<Event> {
        self.events.recv().await
    }

    async fn command(&mut self, command: Command) -> Result<Ack> {
        match command {
            Command::SendText {
                portal,
                text,
                reply_to,
                txn_id,
            } => self.send_text(&portal, &text, reply_to.as_deref(), txn_id).await,
            Command::SendMedia {
                portal,
                data,
                mime,
                file_name,
                caption,
                reply_to,
                txn_id,
                ..
            } => {
                let upload = Media {
                    data: &data,
                    mime: &mime,
                    file_name: &file_name,
                    caption: &caption,
                    reply_to: reply_to.as_deref(),
                    txn_id,
                };
                self.send_media(&portal, upload).await
            }
            Command::Edit { portal, target, text } => {
                let channel = self.channel_of(&portal)?;
                self.api
                    .update_message(&channel, ts_of(&target)?, &convert::outgoing_text(&text))
                    .await?;
                Ok(Ack::default())
            }
            Command::Redact { portal, target } => {
                let channel = self.channel_of(&portal)?;
                self.api.delete_message(&channel, ts_of(&target)?).await?;
                Ok(Ack::default())
            }
            Command::React { portal, target, key } => {
                let channel = self.channel_of(&portal)?;
                let name = convert::reaction_name(&key)
                    .ok_or(Error::Unsupported("this emoji on Slack"))?;
                self.api.reaction(true, &name, &channel, ts_of(&target)?).await?;
                Ok(Ack::default())
            }
            Command::Unreact { portal, target, key } => {
                let channel = self.channel_of(&portal)?;
                let name = convert::reaction_name(&key)
                    .ok_or(Error::Unsupported("this emoji on Slack"))?;
                self.api.reaction(false, &name, &channel, ts_of(&target)?).await?;
                Ok(Ack::default())
            }
            Command::Read { portal, event } => {
                let channel = self.channel_of(&portal)?;
                if self.api.is_bot() {
                    return Ok(Ack::default());
                }
                let ts = match event {
                    Some(id) => ts_of(&id)?.to_owned(),
                    None => now_ts(),
                };
                self.api.mark(&channel, &ts).await?;
                Ok(Ack::default())
            }
            Command::Typing { portal, typing } => {
                let channel = self.channel_of(&portal)?;
                if typing {
                    self.outgoing
                        .send(Outgoing::Typing(channel))
                        .map_err(|_| Error::NotConnected)?;
                }
                Ok(Ack::default())
            }
        }
    }

    async fn backfill(&mut self, portal: &PortalRef, cursor: Option<String>, limit: usize) -> Result<BackfillPage> {
        let channel = self.channel_of(portal)?;
        let limit = limit.clamp(1, MAX_PAGE);
        let page = self.api.history(&channel, cursor.as_deref(), limit).await?;
        // Slack lists the newest first: the last message of the page is the oldest, and the next
        // page is the one older than it.
        let oldest = page.messages.last().map(|message| message.ts.clone());
        let full = page.messages.len() >= limit;
        let mut messages = Vec::new();
        for message in page.messages.iter().rev().filter(|message| backfillable(message)) {
            let parts = convert::messages(&self.api, &self.scope(), &channel, message, false).await;
            messages.extend(parts);
        }
        let has_more = oldest.is_some() && (page.has_more || full);
        Ok(BackfillPage {
            messages,
            cursor: if has_more { oldest } else { None },
            has_more,
        })
    }

    async fn logout_remote(&mut self) -> Result<()> {
        // A Slack token is revoked from Slack's own settings, not by the bridge.
        Ok(())
    }

    fn user(&self) -> Option<RemoteUser> {
        Some(self.me_user.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_ids_need_a_timestamp() {
        assert_eq!(message_id_if("T1", "C1", "1.2").as_deref(), Some("T1-C1-1.2"));
        assert_eq!(message_id_if("T1", "C1", ""), None);
    }

    #[test]
    fn targets_must_be_messages() {
        assert_eq!(ts_of("T1-C1-1.2").ok(), Some("1.2"));
        assert!(ts_of("T1-C1").is_err());
    }

    #[test]
    fn backfill_skips_subtypes_the_bridge_does_not_show() {
        let join: crate::wire::Message = serde_json::from_value(serde_json::json!({
            "ts": "1.0", "subtype": "channel_join"
        }))
        .expect("fixture parses");
        assert!(!backfillable(&join));
    }
}
