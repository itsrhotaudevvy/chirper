//! The direct-message endpoints: the inbox, the poll for updates, one conversation's history, the
//! send, read markers, typing, reactions and delete-for-me.
//! Reference: `twittermeow/messaging.go` and `data/payload/{form,json}.go`.

use reqwest::Method;
use serde::Deserialize;
use serde_json::{Value, json};

use chirper_bridge::{Error, Result};

use super::model::{Inbox, Message};
use super::query::Query;
use super::{Api, Body, Opts, WEB, parse};

/// The client name X expects in `cards_platform`.
const CARDS_PLATFORM: &str = "Web-12";
/// The context a history request names.
pub const CONTEXT_HISTORY: &str = "FETCH_DM_CONVERSATION_HISTORY";
/// The GraphQL operation of a reaction being added.
const REACT_ADD: &str = "VyDyV9pC2oZEj6g52hgnhA/useDMReactionMutationAddMutation";
/// The GraphQL operation of a reaction being removed.
const REACT_REMOVE: &str = "bV_Nim3RYHsaJwMkTXJ6ew/useDMReactionMutationRemoveMutation";
/// The GraphQL operation of a typing notice, as its query id and its path.
const TYPING_ID: &str = "HL96-xZ3Y81IEzAdczDokg";
const TYPING_PATH: &str = "HL96-xZ3Y81IEzAdczDokg/useTypingNotifierMutation";
/// The GraphQL operation that deletes a message for the local user only.
const DELETE_FOR_ME: &str = "BJ6DtxA2llfjnRoRjaiIiw/DMMessageDeleteMutation";
/// The query id of a delete-for-me mutation, as the GraphQL payload spells it.
const DELETE_FOR_ME_ID: &str = "BJ6DtxA2llfjnRoRjaiIiw";

/// The inbox query of the reference's `DMRequestQuery.Default()`, with the one switch the poll
/// and the initial sync change: whether low-quality (message request) conversations are filtered.
pub fn request_query(filter_low_quality: bool) -> Query {
    let mut query = Query::new();
    query
        .push("nsfw_filtering_enabled", false)
        .push("filter_low_quality", filter_low_quality)
        .push("include_quality", "all")
        .push("include_profile_interstitial_type", 1)
        .push("include_blocking", 1)
        .push("include_blocked_by", 1)
        .push("include_followed_by", 1)
        .push("include_want_retweets", 1)
        .push("include_mute_edge", 1)
        .push("include_can_dm", 1)
        .push("include_can_media_tag", 1)
        .push("include_ext_is_blue_verified", 1)
        .push("include_ext_verified_type", 1)
        .push("include_ext_profile_image_shape", 1)
        .push("skip_status", 1)
        .push("include_conversation_info", false)
        .push("dm_secret_conversations_enabled", false)
        .push("krs_registration_enabled", false)
        .push("cards_platform", CARDS_PLATFORM)
        .push("include_cards", 1)
        .push("include_ext_alt_text", true)
        .push("include_ext_limited_action_results", true)
        .push("include_quote_count", true)
        .push("include_reply_count", 1)
        .push("tweet_mode", "extended")
        .push("include_ext_views", true)
        .push("dm_users", true)
        .push("include_groups", true)
        .push("include_inbox_timelines", true)
        .push("include_ext_media_color", true)
        .push("supports_reactions", true)
        .push("supports_edit", true)
        .push("include_ext_edit_control", true)
        .push("include_ext_business_affiliations_label", true)
        .push("include_ext_parody_commentary_fan_label", true)
        .push(
            "ext",
            "mediaColor,altText,businessAffiliationsLabel,mediaStats,highlightedLabel,parodyCommentaryFanLabel,voiceInfo,birdwatchPivot,superFollowMetadata,unmentionInfo,editControl,article",
        );
    query
}

/// The query of a send, `DMSendQuery.Default()`.
fn send_query() -> Query {
    let mut query = Query::new();
    query
        .push("ext", "mediaColor,altText,mediaStats,highlightedLabel,voiceInfo,birdwatchPivot,superFollowMetadata,unmentionInfo,editControl,article")
        .push("include_ext_alt_text", true)
        .push("include_ext_limited_action_results", true)
        .push("include_reply_count", 1)
        .push("tweet_mode", "extended")
        .push("include_ext_views", true)
        .push("include_groups", true)
        .push("include_inbox_timelines", true)
        .push("include_ext_media_color", true)
        .push("supports_reactions", true)
        .push("supports_edit", true);
    query
}

/// The send payload of a direct message. Field names and defaults follow the reference's
/// `SendDirectMessagePayload`, so an unset flag is sent as `false`, as the reference sends it.
#[derive(Debug, Default)]
pub struct SendPayload<'a> {
    pub conversation_id: &'a str,
    pub media_id: &'a str,
    pub reply_to_dm_id: &'a str,
    pub request_id: &'a str,
    pub text: &'a str,
    pub audio_only: bool,
}

impl SendPayload<'_> {
    /// The JSON body of the send.
    pub fn body(&self) -> Value {
        let mut body = json!({
            "recipient_ids": false,
            "text": self.text,
            "cards_platform": CARDS_PLATFORM,
            "include_cards": 1,
            "include_quote_count": false,
            "dm_users": false,
            "audio_only_media_attachment": self.audio_only,
        });
        let object = body.as_object_mut();
        if let Some(object) = object {
            for (key, value) in [
                ("conversation_id", self.conversation_id),
                ("media_id", self.media_id),
                ("reply_to_dm_id", self.reply_to_dm_id),
                ("request_id", self.request_id),
            ] {
                if !value.is_empty() {
                    object.insert(key.to_owned(), json!(value));
                }
            }
        }
        body
    }
}

/// The GraphQL envelope X uses for every mutation.
fn graphql(query_id: &str, variables: Value) -> Body {
    Body::Json(json!({ "variables": variables, "queryId": query_id }))
}

/// The inbox at first connection: the users, conversations and the polling cursor.
pub async fn inbox_initial_state(api: &Api, filter_low_quality: bool) -> Result<Inbox> {
    let url = format!(
        "{}/i/api/1.1/dm/inbox_initial_state.json?{}",
        api.bases().web,
        request_query(filter_low_quality).encode()
    );
    let answer: InboxInitialState = api
        .call_json(Method::GET, &url, Body::None, &dm_opts())
        .await?;
    answer
        .inbox_initial_state
        .ok_or_else(|| Error::Protocol("X sent no inbox".to_owned()))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct InboxInitialState {
    inbox_initial_state: Option<Inbox>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct UserUpdates {
    user_events: Option<Inbox>,
}

/// One poll for updates since `cursor`. An empty page has no `user_events`; that is not an error.
pub async fn user_updates(api: &Api, cursor: &str) -> Result<Inbox> {
    let mut query = request_query(false);
    if !cursor.is_empty() {
        query.push("cursor", cursor);
    }
    let url = format!(
        "{}/i/api/1.1/dm/user_updates.json?{}",
        api.bases().web,
        query.encode()
    );
    let answer: UserUpdates = api
        .call_json(Method::GET, &url, Body::None, &dm_opts())
        .await?;
    Ok(answer.user_events.unwrap_or_default())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ConversationTimeline {
    conversation_timeline: Option<Inbox>,
}

/// One page of a conversation's history, older than `max_id` when it is given.
pub async fn conversation_page(
    api: &Api,
    conversation_id: &str,
    max_id: &str,
    count: usize,
) -> Result<Inbox> {
    let mut query = request_query(false);
    query.push("count", count).push("context", CONTEXT_HISTORY);
    query.push_some("max_id", max_id);
    let url = format!(
        "{}/i/api/1.1/dm/conversation/{conversation_id}.json?{}",
        api.bases().web,
        query.encode()
    );
    let answer: ConversationTimeline = api
        .call_json(Method::GET, &url, Body::None, &dm_opts())
        .await?;
    answer
        .conversation_timeline
        .ok_or_else(|| Error::Protocol("X sent no conversation timeline".to_owned()))
}

/// Send a direct message. The answer's inbox holds the created message.
pub async fn send(api: &Api, payload: &SendPayload<'_>, referer: &str) -> Result<Inbox> {
    let url = format!(
        "{}/i/api/1.1/dm/new2.json?{}",
        api.bases().web,
        send_query().encode()
    );
    let opts = Opts {
        referer: Some(referer),
        client_uuid: true,
    };
    api.call_json(Method::POST, &url, Body::Json(payload.body()), &opts)
        .await
}

/// The created message of a send: the first message of the answer in this conversation.
pub fn created_message(inbox: &Inbox, conversation_id: &str) -> Option<Message> {
    inbox
        .parsed_entries()
        .into_iter()
        .find_map(|entry| match entry {
            super::model::Entry::Message(message) if message.conversation_id == conversation_id => {
                Some(*message)
            }
            _ => None,
        })
}

/// Tell X the local user has read up to `last_read_event_id`.
pub async fn mark_read(api: &Api, conversation_id: &str, last_read_event_id: &str) -> Result<()> {
    let mut form = Query::new();
    form.push("conversationId", conversation_id)
        .push("last_read_event_id", last_read_event_id);
    let url = format!(
        "{}/i/api/1.1/dm/conversation/{conversation_id}/mark_read.json",
        api.bases().web
    );
    api.call(Method::POST, &url, Body::Form(form.encode()), &dm_opts())
        .await
        .map(drop)
}

/// The typing notice of a conversation, by its REST id.
pub async fn typing(api: &Api, conversation_id: &str) -> Result<()> {
    let url = format!("{}/i/api/graphql/{TYPING_PATH}", api.bases().web);
    let body = graphql(TYPING_ID, json!({ "conversationId": conversation_id }));
    api.call(Method::POST, &url, body, &dm_opts())
        .await
        .map(drop)
}

/// Add or remove one emoji reaction. X answers a refused reaction with a typed failure.
pub async fn react(
    api: &Api,
    conversation_id: &str,
    message_id: &str,
    emoji: &str,
    remove: bool,
) -> Result<()> {
    let operation = if remove { REACT_REMOVE } else { REACT_ADD };
    let url = format!("{}/i/api/graphql/{operation}", api.bases().web);
    let query_id = operation.split('/').next().unwrap_or_default();
    let body = graphql(
        query_id,
        json!({
            "conversationId": conversation_id,
            "messageId": message_id,
            "reactionTypes": ["Emoji"],
            "emojiReactions": [emoji],
        }),
    );
    let reply: Value = api.call_json(Method::POST, &url, body, &dm_opts()).await?;
    let data = reply.get("data");
    let refused = if remove {
        data.and_then(|d| d.get("delete_dm_reaction"))
            .and_then(|d| d.get("__typename"))
            .and_then(Value::as_str)
            == Some("DeleteDMReactionFailure")
    } else {
        data.and_then(|d| d.get("create_dm_reaction"))
            .and_then(|d| d.get("__typename"))
            .and_then(Value::as_str)
            == Some("CreateDMReactionFailure")
    };
    if refused {
        return Err(Error::Protocol("X refused the reaction".to_owned()));
    }
    Ok(())
}

/// Delete a message for the local user only. X has no delete-for-everyone for direct messages.
pub async fn delete_for_me(api: &Api, message_id: &str, request_id: &str) -> Result<()> {
    let url = format!("{}/i/api/graphql/{DELETE_FOR_ME}", api.bases().web);
    let body = graphql(
        DELETE_FOR_ME_ID,
        json!({ "messageId": message_id, "requestId": request_id }),
    );
    let reply = api.call(Method::POST, &url, body, &dm_opts()).await?;
    let _: Value = parse(&reply)?;
    Ok(())
}

/// The options every DM request shares: it sends the client uuid.
fn dm_opts() -> Opts<'static> {
    Opts {
        referer: None,
        client_uuid: true,
    }
}

/// The origin of the web client, for callers that build referers.
pub fn messages_referer(conversation_id: &str) -> String {
    format!("{WEB}/messages/{conversation_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_inbox_query_is_sorted_and_carries_the_filter_switch() {
        let encoded = request_query(false).encode();
        assert!(encoded.starts_with("cards_platform=Web-12&"));
        assert!(encoded.contains("&filter_low_quality=false&"));
        assert!(
            request_query(true)
                .encode()
                .contains("&filter_low_quality=true&")
        );
    }

    #[test]
    fn a_send_body_sends_unset_flags_as_false() {
        let body = SendPayload {
            conversation_id: "111-222",
            text: "hello",
            request_id: "r1",
            ..Default::default()
        }
        .body();
        assert_eq!(body["conversation_id"], "111-222");
        assert_eq!(body["recipient_ids"], false);
        assert_eq!(body["dm_users"], false);
        assert_eq!(body["include_cards"], 1);
        assert!(body.get("media_id").is_none());
        assert!(body.get("reply_to_dm_id").is_none());
    }

    #[test]
    fn a_created_message_is_found_by_its_conversation() {
        let inbox: Inbox = serde_json::from_value(serde_json::json!({
            "entries": [
                {"message": {"id": "7", "conversation_id": "111-222", "message_data": {"text": "x"}}}
            ]
        }))
        .unwrap();
        assert_eq!(
            created_message(&inbox, "111-222").map(|m| m.id).as_deref(),
            Some("7")
        );
        assert!(created_message(&inbox, "999-999").is_none());
    }
}
