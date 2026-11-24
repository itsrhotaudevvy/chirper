//! Wire types of the messaging API and the realtime stream. Every field is optional on the wire,
//! so every field defaults; unknown fields are ignored.

use serde::Deserialize;
use serde::de::IgnoredAny;

/// `com.linkedin.pemberly.text.AttributedText`: plain text with formatting ranges.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct AttributedText {
    pub text: String,
    pub attributes: Vec<Attribute>,
}

/// One formatting range. `start` and `length` count characters, as the reference does.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Attribute {
    pub start: i64,
    pub length: i64,
    #[serde(rename = "attributeKind")]
    pub kind: AttributeKind,
}

/// The kind of a formatting range. Each variant is present or absent.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct AttributeKind {
    pub bold: Option<IgnoredAny>,
    pub entity: Option<Entity>,
    pub hyperlink: Option<Hyperlink>,
    pub italic: Option<IgnoredAny>,
    #[serde(rename = "lineBreak")]
    pub line_break: Option<IgnoredAny>,
    pub list: Option<IgnoredAny>,
    #[serde(rename = "listItem")]
    pub list_item: Option<IgnoredAny>,
    pub paragraph: Option<IgnoredAny>,
    pub subscript: Option<IgnoredAny>,
    pub superscript: Option<IgnoredAny>,
    pub underline: Option<IgnoredAny>,
}

/// A mention of a member.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Entity {
    pub urn: String,
}

/// A link.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Hyperlink {
    pub url: String,
}

/// `com.linkedin.messenger.Conversation`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Conversation {
    pub title: String,
    #[serde(rename = "entityUrn")]
    pub entity_urn: String,
    #[serde(rename = "lastActivityAt")]
    pub last_activity_at: i64,
    #[serde(rename = "lastReadAt")]
    pub last_read_at: i64,
    #[serde(rename = "groupChat")]
    pub group_chat: Option<bool>,
    #[serde(rename = "conversationParticipants")]
    pub participants: Vec<MessagingParticipant>,
    pub read: bool,
    pub messages: Option<MessagesPage>,
    pub categories: Vec<String>,
}

/// A page of conversations, as the category query returns it.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ConversationsPage {
    pub metadata: ConversationsMetadata,
    pub elements: Vec<Conversation>,
}

/// The cursor of a conversations page.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ConversationsMetadata {
    #[serde(rename = "nextCursor")]
    pub next_cursor: String,
}

/// A page of messages, newest or oldest first as the query asked.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MessagesPage {
    pub metadata: MessagesMetadata,
    pub elements: Vec<Message>,
}

/// The cursors of a messages page.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MessagesMetadata {
    #[serde(rename = "prevCursor")]
    pub prev_cursor: String,
}

/// `com.linkedin.messenger.MessagingParticipant`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MessagingParticipant {
    #[serde(rename = "participantType")]
    pub participant_type: ParticipantType,
    #[serde(rename = "entityUrn")]
    pub entity_urn: String,
}

/// The kind of a participant. One branch is present.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ParticipantType {
    pub member: Option<MemberInfo>,
    pub organization: Option<NamedInfo>,
    pub custom: Option<NamedInfo>,
    pub agent: Option<NamedInfo>,
}

/// A person.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MemberInfo {
    #[serde(rename = "profileUrl")]
    pub profile_url: String,
    #[serde(rename = "firstName")]
    pub first_name: AttributedText,
    #[serde(rename = "lastName")]
    pub last_name: AttributedText,
    #[serde(rename = "profilePicture")]
    pub profile_picture: Option<VectorImage>,
}

/// An organisation, a custom participant or an agent: a name and an image.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct NamedInfo {
    pub name: AttributedText,
    #[serde(alias = "logo", alias = "image", alias = "profilePicture")]
    pub image: Option<VectorImage>,
}

/// `com.linkedin.common.VectorImage`: a root url and the sizes under it.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct VectorImage {
    #[serde(rename = "rootUrl")]
    pub root_url: String,
    pub artifacts: Vec<VectorArtifact>,
}

/// One size of a vector image.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct VectorArtifact {
    #[serde(rename = "fileIdentifyingUrlPathSegment")]
    pub path_segment: String,
    pub height: i64,
    pub width: i64,
}

impl VectorImage {
    /// The URL of the tallest size, or an empty string when there is none.
    pub fn largest_url(&self) -> String {
        if self.root_url.is_empty() {
            return String::new();
        }
        let tallest =
            self.artifacts
                .iter()
                .fold(None::<&VectorArtifact>, |best, next| match best {
                    Some(current) if current.height >= next.height => Some(current),
                    _ => Some(next),
                });
        format!(
            "{}{}",
            self.root_url,
            tallest.map(|a| a.path_segment.as_str()).unwrap_or("")
        )
    }
}

/// `com.linkedin.messenger.Message`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Message {
    pub body: AttributedText,
    pub subject: String,
    #[serde(rename = "deliveredAt")]
    pub delivered_at: i64,
    #[serde(rename = "entityUrn")]
    pub entity_urn: String,
    pub sender: MessagingParticipant,
    #[serde(rename = "messageBodyRenderFormat")]
    pub render_format: String,
    pub conversation: ConversationRef,
    #[serde(rename = "conversationUrn")]
    pub conversation_urn: String,
    #[serde(rename = "renderContent")]
    pub render_content: Vec<RenderContent>,
    #[serde(rename = "reactionSummaries")]
    pub reaction_summaries: Vec<ReactionSummary>,
}

/// The conversation a message belongs to, as the message carries it.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ConversationRef {
    #[serde(rename = "entityUrn")]
    pub entity_urn: String,
    #[serde(rename = "groupChat")]
    pub group_chat: Option<bool>,
}

/// One render unit of a message. Several may be present.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RenderContent {
    pub audio: Option<AudioMetadata>,
    #[serde(rename = "externalMedia")]
    pub external_media: Option<ExternalMedia>,
    pub file: Option<FileAttachment>,
    #[serde(rename = "forwardedMessageContent")]
    pub forwarded: Option<ForwardedMessage>,
    #[serde(rename = "hostUrnData")]
    pub host_urn: Option<HostUrnData>,
    #[serde(rename = "repliedMessageContent")]
    pub replied: Option<RepliedMessage>,
    #[serde(rename = "vectorImage")]
    pub vector_image: Option<VectorImage>,
    pub video: Option<VideoPlayMetadata>,
}

/// A voice message.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct AudioMetadata {
    pub duration: i64,
    pub url: String,
}

/// An image linked from outside LinkedIn.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ExternalMedia {
    pub media: ExternalProxyImage,
    pub title: String,
}

/// The address of an external image.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ExternalProxyImage {
    pub url: String,
}

/// A file attachment.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct FileAttachment {
    #[serde(rename = "byteSize")]
    pub byte_size: i64,
    #[serde(rename = "mediaType")]
    pub media_type: String,
    pub name: String,
    pub url: String,
}

/// A forwarded message, quoted.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ForwardedMessage {
    #[serde(rename = "footerText")]
    pub footer: AttributedText,
    #[serde(rename = "forwardedBody")]
    pub body: AttributedText,
    #[serde(rename = "originalSender")]
    pub original_sender: MessagingParticipant,
}

/// A reference to a feed update shared into the chat.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct HostUrnData {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "hostUrn")]
    pub host_urn: String,
}

/// The message a reply quotes.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RepliedMessage {
    #[serde(rename = "originalMessage")]
    pub original: Message,
}

/// A video with its progressive streams.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct VideoPlayMetadata {
    #[serde(rename = "progressiveStreams")]
    pub streams: Vec<ProgressiveStream>,
    pub duration: i64,
}

/// One progressive download of a video.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ProgressiveStream {
    #[serde(rename = "streamingLocations")]
    pub locations: Vec<StreamingLocation>,
    pub size: i64,
    pub width: i64,
    pub height: i64,
    #[serde(rename = "mediaType")]
    pub media_type: String,
}

/// The address of one streaming location.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct StreamingLocation {
    pub url: String,
}

/// `com.linkedin.messenger.ReactionSummary`: one emoji and how many reacted with it.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ReactionSummary {
    pub count: i64,
    pub emoji: String,
    #[serde(rename = "viewerReacted")]
    pub viewer_reacted: bool,
}

/// The `messengerConversations` GraphQL answer, wrapped in `data`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ConversationsData {
    #[serde(rename = "messengerConversationsByCategoryQuery")]
    pub page: Option<ConversationsPage>,
}

/// The `messengerMessages` GraphQL answer, wrapped in `data`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MessagesData {
    #[serde(rename = "messengerMessagesByAnchorTimestamp")]
    pub by_anchor: Option<MessagesPage>,
    #[serde(rename = "messengerMessagesByConversation")]
    pub by_conversation: Option<MessagesPage>,
}

/// Something wrapped in a `data` envelope.
#[derive(Clone, Debug, Deserialize)]
pub struct Data<T> {
    pub data: T,
}

/// The `result` envelope of a decorated realtime payload.
#[derive(Clone, Debug, Deserialize)]
pub struct Decorated<T> {
    pub result: T,
}

/// `/voyager/api/me`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Profile {
    #[serde(rename = "miniProfile")]
    pub mini: MiniProfile,
}

/// The profile fields the driver uses.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MiniProfile {
    #[serde(rename = "firstName")]
    pub first_name: String,
    #[serde(rename = "lastName")]
    pub last_name: String,
    #[serde(rename = "entityUrn")]
    pub entity_urn: String,
    pub picture: Option<Picture>,
}

/// A profile picture.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Picture {
    #[serde(rename = "com.linkedin.common.VectorImage")]
    pub vector: Option<VectorImage>,
}

/// The media-upload metadata answer.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct UploadEnvelope {
    pub data: UploadAction,
}

/// The action wrapper around an upload.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct UploadAction {
    pub value: UploadMetadata,
}

/// Where an asset goes and the URN it will have.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct UploadMetadata {
    pub urn: String,
    #[serde(rename = "singleUploadUrl")]
    pub upload_url: String,
}

/// `com.linkedin.messenger.SeenReceipt`, from the realtime stream.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct SeenReceipt {
    #[serde(rename = "seenAt")]
    pub seen_at: i64,
    pub message: Message,
    #[serde(rename = "seenByParticipant")]
    pub seen_by: MessagingParticipant,
}

/// `RealtimeReactionSummary`, from the realtime stream.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ReactionChange {
    #[serde(rename = "reactionAdded")]
    pub added: bool,
    pub actor: MessagingParticipant,
    pub message: Message,
    #[serde(rename = "reactionSummary")]
    pub summary: ReactionSummary,
}

/// `RealtimeTypingIndicator`, from the realtime stream.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Typing {
    #[serde(rename = "typingParticipant")]
    pub participant: MessagingParticipant,
    pub conversation: Conversation,
}

/// One line of the realtime stream, after the `data:` prefix.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RealtimeLine {
    #[serde(rename = "com.linkedin.realtimefrontend.Heartbeat")]
    pub heartbeat: Option<IgnoredAny>,
    #[serde(rename = "com.linkedin.realtimefrontend.ClientConnection")]
    pub client_connection: Option<ClientConnection>,
    #[serde(rename = "com.linkedin.realtimefrontend.DecoratedEvent")]
    pub decorated: Option<DecoratedEvent>,
}

/// The server's acknowledgement that a realtime connection is up.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ClientConnection {
    pub id: String,
}

/// A decorated realtime event: a topic and one payload.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct DecoratedEvent {
    pub topic: String,
    pub payload: Payload,
}

/// The payload of a decorated event.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Payload {
    pub data: PayloadData,
}

/// Every decorated shape the driver understands; only one is present per event.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct PayloadData {
    #[serde(rename = "doDecorateConversationMessengerRealtimeDecoration")]
    pub conversation: Option<Decorated<Conversation>>,
    #[serde(rename = "doDecorateConversationDeleteMessengerRealtimeDecoration")]
    pub conversation_delete: Option<Decorated<Conversation>>,
    #[serde(rename = "doDecorateMessageMessengerRealtimeDecoration")]
    pub message: Option<Decorated<Message>>,
    #[serde(rename = "doDecorateTypingIndicatorMessengerRealtimeDecoration")]
    pub typing: Option<Decorated<Typing>>,
    #[serde(rename = "doDecorateSeenReceiptMessengerRealtimeDecoration")]
    pub seen: Option<Decorated<SeenReceipt>>,
    #[serde(rename = "doDecorateRealtimeReactionSummaryMessengerRealtimeDecoration")]
    pub reaction: Option<Decorated<ReactionChange>>,
}
