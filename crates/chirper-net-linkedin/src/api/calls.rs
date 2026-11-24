//! The messaging endpoints the driver uses, one method per request the reference makes.

use reqwest::Method;
use serde::Deserialize;
use serde_json::json;

use chirper_bridge::{Error, Result};

use super::model::{
    ConversationsData, ConversationsPage, Data, Message, MessagesData, MessagesPage, Profile,
    UploadEnvelope,
};
use super::outbound::SendPayload;
use super::{Api, CONTENT_JSON, CONTENT_NORMALIZED, CONTENT_PLAIN, path_escape, query_escape};

/// The GraphQL endpoint of the messaging app.
const GRAPHQL: &str = "/voyager/api/voyagerMessagingGraphQL/graphql";
/// `messengerConversations`, paged by the time of last activity.
const QUERY_CONVERSATIONS_BEFORE: &str = "messengerConversations.9501074288a12f3ae9e3c7ea243bccbf";
/// `messengerMessages`, the page of messages delivered before a time.
const QUERY_MESSAGES_BEFORE: &str = "messengerMessages.d8ea76885a52fd5dc5c317078ab7c977";
/// `messengerMessages`, the page of messages before a cursor.
const QUERY_MESSAGES_PREV: &str = "messengerMessages.34c9888be71c8010fecfb575cb38308f";
/// The primary-inbox filter, sent raw as the reference does.
const PRIMARY_INBOX: &str =
    "(predicateUnions:List((conversationCategoryPredicate:(category:PRIMARY_INBOX))))";
/// Messages: send, edit, recall, reactions.
const MESSAGES: &str = "/voyager/api/voyagerMessagingDashMessengerMessages";
/// Conversations: typing, read state.
const CONVERSATIONS: &str = "/voyager/api/voyagerMessagingDashMessengerConversations";
/// Delivery acknowledgements of realtime messages.
const DELIVERY_ACKS: &str =
    "/voyager/api/voyagerMessagingDashMessengerMessageDeliveryAcknowledgements";
/// The media upload metadata endpoint.
const UPLOAD_METADATA: &str = "/voyager/api/voyagerVideoDashMediaUploadMetadata";

/// The answer of a create call: the message under `value`.
#[derive(Deserialize)]
struct Created {
    value: Message,
}

/// A downloaded file.
#[derive(Debug)]
pub struct Download {
    pub mime: String,
    pub data: Vec<u8>,
}

impl Api {
    /// The logged-in user's profile. This is also how a login checks its cookies.
    pub async fn profile(&self) -> Result<Profile> {
        let builder = self
            .request(Method::GET, "/voyager/api/me")
            .header("csrf-token", self.csrf());
        self.json(builder).await
    }

    /// One page of primary-inbox conversations last active before `before_ms`.
    pub async fn conversations_before(
        &self,
        mailbox: &str,
        before_ms: i64,
        count: u32,
    ) -> Result<ConversationsPage> {
        let builder = self.graphql(
            QUERY_CONVERSATIONS_BEFORE,
            &[
                ("mailboxUrn", query_escape(mailbox)),
                ("lastUpdatedBefore", before_ms.to_string()),
                ("count", count.to_string()),
                ("query", PRIMARY_INBOX.to_owned()),
            ],
        );
        let data: Data<ConversationsData> = self.json(builder).await?;
        data.data
            .page
            .ok_or_else(|| Error::Protocol("LinkedIn sent no conversation page".to_owned()))
    }

    /// One page of messages of `conversation` delivered before `before_ms`, newest first.
    pub async fn messages_before(
        &self,
        conversation: &str,
        before_ms: i64,
        count: u32,
    ) -> Result<MessagesPage> {
        let builder = self.graphql(
            QUERY_MESSAGES_BEFORE,
            &[
                ("deliveredAt", before_ms.to_string()),
                ("conversationUrn", query_escape(conversation)),
                ("countBefore", count.to_string()),
                ("countAfter", "0".to_owned()),
            ],
        );
        let data: Data<MessagesData> = self.json(builder).await?;
        data.data
            .by_anchor
            .ok_or_else(|| Error::Protocol("LinkedIn sent no message page".to_owned()))
    }

    /// The page of messages before `cursor`, which the previous page returned.
    pub async fn messages_prev(
        &self,
        conversation: &str,
        cursor: &str,
        count: u32,
    ) -> Result<MessagesPage> {
        let builder = self.graphql(
            QUERY_MESSAGES_PREV,
            &[
                ("conversationUrn", query_escape(conversation)),
                ("count", count.to_string()),
                ("prevCursor", query_escape(cursor)),
            ],
        );
        let data: Data<MessagesData> = self.json(builder).await?;
        data.data
            .by_conversation
            .ok_or_else(|| Error::Protocol("LinkedIn sent no message page".to_owned()))
    }

    /// Create a message. The answer carries the stored message, with its URN and time.
    pub async fn create_message(&self, payload: &SendPayload) -> Result<Message> {
        let path = format!("{MESSAGES}?action=createMessage");
        let builder = self.with_voyager_headers(self.request(Method::POST, &path));
        let builder = Api::json_body(builder, CONTENT_PLAIN, payload)?;
        let created: Created = self.json(builder).await?;
        Ok(created.value)
    }

    /// Replace the text of a message.
    pub async fn edit_message(&self, urn: &str, text: &str) -> Result<()> {
        let path = format!("{MESSAGES}/{}", path_escape(urn));
        let body = json!({ "patch": { "$set": { "body": { "text": text } } } });
        let builder = self
            .with_voyager_headers(self.request(Method::POST, &path))
            .header("accept", "application/json");
        let builder = raw_json(builder, &body)?;
        self.expect_ok(builder).await.map(drop)
    }

    /// Recall (delete) a message.
    pub async fn recall_message(&self, urn: &str) -> Result<()> {
        let path = format!("{MESSAGES}?action=recall");
        let builder = self.with_voyager_headers(self.request(Method::POST, &path));
        let builder = Api::json_body(builder, CONTENT_PLAIN, &json!({ "messageUrn": urn }))?;
        self.expect_ok(builder).await.map(drop)
    }

    /// Add (`add == true`) or remove one emoji reaction of the local user.
    pub async fn react(&self, urn: &str, emoji: &str, add: bool) -> Result<()> {
        let action = if add {
            "reactWithEmoji"
        } else {
            "unreactWithEmoji"
        };
        let path = format!("{MESSAGES}?action={action}");
        let builder = self
            .with_voyager_headers(self.request(Method::POST, &path))
            .header("accept", "application/json");
        let builder = Api::json_body(
            builder,
            CONTENT_PLAIN,
            &json!({ "messageUrn": urn, "emoji": emoji }),
        )?;
        self.expect_ok(builder).await.map(drop)
    }

    /// Tell a conversation the local user is typing.
    pub async fn start_typing(&self, conversation: &str) -> Result<()> {
        let path = format!("{CONVERSATIONS}?action=typing");
        let builder = self
            .with_voyager_headers(self.request(Method::POST, &path))
            .header("accept", "application/json");
        let builder = Api::json_body(
            builder,
            CONTENT_PLAIN,
            &json!({ "conversationUrn": conversation }),
        )?;
        self.expect_ok(builder).await.map(drop)
    }

    /// Mark a conversation read up to now.
    pub async fn mark_read(&self, conversation: &str) -> Result<()> {
        let path = format!("{CONVERSATIONS}?ids=List({})", query_escape(conversation));
        let body = json!({
            "entities": { conversation: { "patch": { "$set": { "read": true } } } }
        });
        let builder = self
            .with_voyager_headers(self.request(Method::POST, &path))
            .header("accept", "application/json")
            .header("origin", super::WWW);
        let builder = Api::json_body(builder, CONTENT_PLAIN, &body)?;
        let answer: serde_json::Value = self.json(builder).await?;
        let refused = answer
            .get("errors")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|errors| !errors.is_empty());
        if refused {
            return Err(Error::Protocol(
                "LinkedIn did not mark the conversation read".to_owned(),
            ));
        }
        Ok(())
    }

    /// Acknowledge that realtime messages were delivered to this device.
    pub async fn acknowledge_delivery(&self, urns: &[String], consumed_at_ms: i64) -> Result<()> {
        let path = format!("{DELIVERY_ACKS}?action=sendDeliveryAcknowledgement");
        let body = json!({
            "messageUrns": urns,
            "clientId": "voyager-web",
            "deliveryMechanism": "REALTIME",
            "clientConsumedAt": consumed_at_ms,
        });
        let builder = self
            .with_voyager_headers(self.request(Method::POST, &path))
            .header("accept", "application/json");
        let builder = Api::json_body(builder, CONTENT_PLAIN, &body)?;
        self.expect_ok(builder).await.map(drop)
    }

    /// End the session on LinkedIn's side. Any answer counts: the server may redirect once the
    /// session is gone, and that is the outcome asked for.
    pub async fn logout(&self) -> Result<()> {
        let path = format!("/uas/logout?csrfToken={}", query_escape(self.csrf()));
        self.send(self.request(Method::GET, &path)).await.map(drop)
    }

    /// Upload an attachment and return the asset URN the message will refer to.
    pub async fn upload_asset(
        &self,
        upload_type: &str,
        file_name: &str,
        mime: &str,
        data: &[u8],
    ) -> Result<String> {
        let body = json!({
            "mediaUploadType": upload_type,
            "fileSize": data.len(),
            "filename": file_name,
        });
        let builder = self
            .with_voyager_headers(
                self.request(Method::POST, &format!("{UPLOAD_METADATA}?action=upload")),
            )
            .header("accept", CONTENT_NORMALIZED);
        let builder = Api::json_body(builder, CONTENT_JSON, &body)?;
        let envelope: UploadEnvelope = self.json(builder).await?;
        let target = envelope.data.value;
        if target.urn.is_empty() || target.upload_url.is_empty() {
            return Err(Error::Protocol("LinkedIn sent no upload target".to_owned()));
        }
        let put = self
            .request_url(Method::PUT, &target.upload_url)
            .header("csrf-token", self.csrf())
            .header("content-length", data.len().to_string())
            .header("content-type", mime)
            .body(data.to_vec());
        self.expect_ok(put).await?;
        Ok(target.urn)
    }

    /// Download a file LinkedIn hosts. `None` when it is larger than `limit` bytes.
    pub async fn download(&self, url: &str, limit: usize) -> Result<Option<Download>> {
        let mut response = self
            .request_url(Method::GET, url)
            .send()
            .await
            .map_err(|_| Error::Network("LinkedIn: the download failed".to_owned()))?;
        super::check(response.status(), response.headers())?;
        if response
            .content_length()
            .is_some_and(|len| len > limit as u64)
        {
            return Ok(None);
        }
        let mime = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.split(';').next().unwrap_or("").trim().to_owned())
            .unwrap_or_default();
        let mut data = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Error::Network("LinkedIn: the download was interrupted".to_owned()))?
        {
            if data.len() + chunk.len() > limit {
                return Ok(None);
            }
            data.extend_from_slice(&chunk);
        }
        Ok(Some(Download { mime, data }))
    }

    /// A GraphQL query: the query id and the variables, in the raw query string the reference uses.
    fn graphql(&self, query_id: &str, variables: &[(&str, String)]) -> reqwest::RequestBuilder {
        let mut sorted: Vec<&(&str, String)> = variables.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        let joined = sorted
            .iter()
            .filter(|(_, value)| !value.is_empty())
            .map(|(key, value)| format!("{key}:{value}"))
            .collect::<Vec<_>>()
            .join(",");
        let path = format!("{GRAPHQL}?queryId={query_id}&variables=({joined})");
        self.with_voyager_headers(self.request(Method::GET, &path))
            .header("accept", "application/graphql")
    }
}

/// A JSON body without a content type, as the reference sends for edits.
fn raw_json(
    builder: reqwest::RequestBuilder,
    body: &serde_json::Value,
) -> Result<reqwest::RequestBuilder> {
    let bytes = serde_json::to_vec(body)
        .map_err(|e| Error::Internal(format!("encoding a LinkedIn payload: {e}")))?;
    Ok(builder.body(bytes))
}
