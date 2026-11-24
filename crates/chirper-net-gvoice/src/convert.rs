//! Google Voice threads and messages as the bridge model sees them.
//!
//! Ported from `pkg/connector/{chatinfo,handlegvoice}.go` of mautrix-gvoice (AGPL-3.0): the same
//! portal and sender ids, the same call, voicemail and missed-call wording, and the same rules for
//! outgoing messages, MMS subjects and attachment parts. Where the bridge model has no place for a
//! reference feature (formatted call actions, the Matrix content types) the nearest model value is
//! used, and the difference is listed in the crate's report.

use std::collections::BTreeMap;
use std::sync::Arc;

use chirper_bridge::model::{ChatMeta, Content, MediaInfo, PortalRef, RemoteMessage, RemoteUser};

use crate::api::{Api, MEDIA_AUDIO, MEDIA_VIDEO};
use crate::ids;
use crate::wire::Msg;

/// Message types of `Message.Type` that are sent by the local user.
const TYPE_OUTGOING_CALL: i64 = 7;
const TYPE_MISSED_CALL: i64 = 0;
const TYPE_INCOMING_CALL: i64 = 1;
const TYPE_VOICEMAIL: i64 = 2;
const TYPE_RECORDED_CALL: i64 = 4;
const TYPE_SMS_OUT: i64 = 11;
const TYPE_OUTGOING_CALL_CANCELLED: i64 = 14;
/// `Message.CoarseType` values.
const COARSE_MISSED: i64 = 0;
const COARSE_INCOMING: i64 = 1;
const COARSE_OUTGOING: i64 = 2;
const COARSE_VOICEMAIL: i64 = 3;
const COARSE_RECORDED: i64 = 4;
/// `Attachment.Status` value of a file Google Voice cannot show.
const STATUS_NOT_SUPPORTED: i64 = 3;
/// `Attachment.Metadata.SizeSpec` value of the original file.
const SIZE_ORIGINAL: i64 = 1;
/// The text of a message Chirper cannot show.
const UNKNOWN_MESSAGE: &str = "Unknown message type, please view it on the Google Voice app";

/// The login a thread belongs to: its id, which namespaces portals and users, and its number.
#[derive(Clone, Debug)]
pub struct Account {
    pub login_id: String,
    pub e164: String,
}

impl Account {
    /// The local user, as the remote user the bridge shows for messages it sent.
    pub fn me(&self) -> RemoteUser {
        self.user_for(&self.e164, "")
    }

    /// The remote user of a phone number, with the name the thread gives it.
    pub fn user_for(&self, phone: &str, name: &str) -> RemoteUser {
        RemoteUser {
            id: ids::user_id(&self.login_id, phone),
            display_name: name.to_owned(),
            avatar_url: String::new(),
            username: String::new(),
            phone: phone.to_owned(),
        }
    }

    /// The portal of a thread, in this login's namespace.
    pub fn portal(&self, thread: &Msg) -> PortalRef {
        PortalRef {
            id: ids::portal_id(&thread.string(1), &thread.strings(9), thread.flag(8)),
            receiver: self.login_id.clone(),
        }
    }

    /// The chat metadata of a thread: a one-to-one thread is a direct message with its peer.
    pub fn chat(&self, portal: PortalRef, thread: &Msg) -> ChatMeta {
        let phones = thread.strings(9);
        let names = contact_names(thread);
        let members: Vec<RemoteUser> = phones
            .iter()
            .map(|phone| self.user_for(phone, names.get(phone).map_or("", String::as_str)))
            .collect();
        let is_dm = phones.len() == 1;
        let name = if is_dm {
            member_label(&members[0])
        } else {
            members.iter().map(member_label).collect::<Vec<_>>().join(", ")
        };
        ChatMeta {
            portal,
            name,
            topic: String::new(),
            avatar_url: String::new(),
            is_dm,
            is_space: false,
            member_count: i64::try_from(phones.len()).unwrap_or(i64::MAX).saturating_add(1),
            parent: None,
            parent_name: String::new(),
            dm_user: if is_dm { members.into_iter().next() } else { None },
        }
    }
}

fn member_label(user: &RemoteUser) -> String {
    if user.display_name.is_empty() {
        user.phone.clone()
    } else {
        user.display_name.clone()
    }
}

/// The names the thread gives its numbers.
fn contact_names(thread: &Msg) -> BTreeMap<String, String> {
    thread
        .messages(5)
        .map(|contact| (contact.string(2), contact.string(1)))
        .filter(|(phone, name)| !phone.is_empty() && !name.is_empty())
        .collect()
}

/// Whether the local user sent this message from another device.
pub fn is_outgoing(raw: &Msg) -> bool {
    matches!(
        raw.int(5),
        TYPE_SMS_OUT | TYPE_OUTGOING_CALL | TYPE_OUTGOING_CALL_CANCELLED
    ) || raw.int(13) == COARSE_OUTGOING
}

/// The number a message came from: the MMS sender, or the contact of a plain message.
fn sender_number(raw: &Msg) -> String {
    let mms_sender = raw.message(15).string(5);
    if mms_sender.is_empty() {
        raw.message(4).string(2)
    } else {
        mms_sender
    }
}

/// The messages a raw Google Voice message becomes: one per part, oldest-part first.
pub async fn messages(
    api: &Api,
    account: &Account,
    portal: &PortalRef,
    raw: &Msg,
    live: bool,
) -> Vec<RemoteMessage> {
    let from_me = is_outgoing(raw);
    let sender = if from_me {
        account.me()
    } else {
        account.user_for(&sender_number(raw), "")
    };
    let contents = contents(api, raw).await;
    contents
        .into_iter()
        .enumerate()
        .map(|(index, content)| RemoteMessage {
            portal: portal.clone(),
            id: raw.string(1),
            part: ids::part_id(index),
            sender: sender.clone(),
            timestamp_ms: raw.int(2),
            content,
            reply_to: None,
            edit_of: None,
            live,
            from_me,
        })
        .collect()
}

/// The contents of a message, one per part.
async fn contents(api: &Api, raw: &Msg) -> Vec<Content> {
    let notice = voicemail(raw)
        .or_else(|| voice_call(raw))
        .or_else(|| missed_call(raw));
    if let Some(body) = notice {
        return vec![Content::Notice { body }];
    }
    let mms = raw.message(15);
    let has_mms = !raw.values(15).is_empty();
    let text = raw.string(10);
    if !has_mms {
        return if text.is_empty() {
            vec![unknown()]
        } else {
            vec![Content::Text {
                body: text,
                formatted: None,
            }]
        };
    }
    mms_contents(api, mms).await
}

fn unknown() -> Content {
    Content::Unsupported {
        body: UNKNOWN_MESSAGE.to_owned(),
    }
}

/// An MMS: the subject and text form the first part's caption, and every attachment is a part.
async fn mms_contents(api: &Api, mms: &Msg) -> Vec<Content> {
    let subject = mms.string(2);
    let text = mms.string(1);
    let (body, formatted) = if subject.is_empty() {
        (text.clone(), None)
    } else {
        (
            format!("**{subject}**\n{text}"),
            Some(format!(
                "<b>{}</b><br>{}",
                escape_html(&subject),
                escape_html(&text).replace('\n', "<br>")
            )),
        )
    };
    let attachments: Vec<&Msg> = mms.messages(3).collect();
    if attachments.is_empty() {
        return if body.is_empty() {
            vec![unknown()]
        } else {
            vec![Content::Text { body, formatted }]
        };
    }
    let mut out = Vec::with_capacity(attachments.len());
    for (index, attachment) in attachments.into_iter().enumerate() {
        let caption = if index == 0 { body.clone() } else { String::new() };
        out.push(attachment_content(api, attachment, caption).await);
    }
    out
}

fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
}

/// One attachment as media. A file that cannot be fetched stays as a notice with the caption.
async fn attachment_content(api: &Api, attachment: &Msg, caption: String) -> Content {
    if attachment.int(3) == STATUS_NOT_SUPPORTED {
        return failure("File type not supported by Google Voice", &caption);
    }
    let media_type = attachment.int(5);
    let downloaded = match api.download(&attachment.string(2), media_type).await {
        Ok(Some(found)) => found,
        Ok(None) => return failure("This attachment is larger than 100 MiB", &caption),
        Err(err) => {
            log::debug!("gvoice: attachment download failed: {err}");
            return failure("Failed to download attachment", &caption);
        }
    };
    let (data, downloaded_mime) = downloaded;
    let transcoded = media_type == MEDIA_AUDIO || media_type == MEDIA_VIDEO;
    let hinted = attachment.string(1);
    let mime = if !hinted.is_empty() && (!transcoded || downloaded_mime.is_empty()) {
        hinted
    } else {
        downloaded_mime
    };
    let mime = if mime.is_empty() {
        "application/octet-stream".to_owned()
    } else {
        mime
    };
    let kind = media_kind(&mime);
    let original = attachment
        .messages(4)
        .find(|meta| meta.int(1) == SIZE_ORIGINAL)
        .or_else(|| attachment.messages(4).max_by_key(|meta| meta.int(2)))
        .cloned()
        .unwrap_or_default();
    let info = MediaInfo {
        file_name: format!("{kind}{}", extension(&mime)),
        mime,
        size: data.len(),
        width: original.int(2),
        height: original.int(3),
        duration_ms: 0,
        blurhash: String::new(),
    };
    Content::Media {
        kind: kind.to_owned(),
        caption,
        info,
        data: Arc::new(data),
        thumbnail: None,
    }
}

/// A notice for an attachment that could not be used, with the caption kept after it.
fn failure(description: &str, caption: &str) -> Content {
    let body = if caption.is_empty() {
        description.to_owned()
    } else {
        format!("{description}\n\n{caption}")
    };
    Content::Unsupported { body }
}

/// The kind of a media type, as the bridge names it.
fn media_kind(mime: &str) -> &'static str {
    match mime.split('/').next().unwrap_or_default() {
        "image" => "image",
        "audio" => "audio",
        "video" => "video",
        _ => "file",
    }
}

/// A file extension for the common types the network serves.
fn extension(mime: &str) -> &'static str {
    match mime.split(';').next().unwrap_or_default().trim() {
        "image/jpeg" => ".jpg",
        "image/png" => ".png",
        "image/gif" => ".gif",
        "image/webp" => ".webp",
        "image/bmp" => ".bmp",
        "image/tiff" => ".tiff",
        "audio/mpeg" => ".mp3",
        "audio/mp4" => ".m4a",
        "video/mp4" => ".mp4",
        "video/3gpp" => ".3gp",
        _ => "",
    }
}

/// The text of a voicemail record: its transcript, or its length.
fn voicemail(raw: &Msg) -> Option<String> {
    if raw.int(5) != TYPE_VOICEMAIL || raw.int(13) != COARSE_VOICEMAIL {
        return None;
    }
    let mut body = "Voicemail".to_owned();
    let transcript = transcript_text(raw.message(7));
    if !transcript.is_empty() {
        body.push_str(": ");
        body.push_str(&transcript);
    } else {
        let duration = format_duration(raw.float(9));
        if !duration.is_empty() {
            body.push_str(" • ");
            body.push_str(&duration);
        }
    }
    Some(body)
}

/// The transcript tokens that are valid text, trimmed and joined by spaces.
fn transcript_text(transcript: &Msg) -> String {
    transcript
        .messages(2)
        .filter_map(|token| String::from_utf8(token.bytes(1)).ok())
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The text of a voice call record.
fn voice_call(raw: &Msg) -> Option<String> {
    if !raw.string(10).is_empty() || !raw.values(15).is_empty() {
        return None;
    }
    let duration = format_duration(raw.float(9));
    match raw.int(13) {
        COARSE_INCOMING if raw.int(5) == TYPE_INCOMING_CALL => Some(call_body(&duration)),
        COARSE_OUTGOING if raw.int(5) == TYPE_OUTGOING_CALL_CANCELLED => {
            Some("Unanswered voice call".to_owned())
        }
        COARSE_OUTGOING => Some(call_body(&duration)),
        COARSE_RECORDED if raw.int(5) == TYPE_RECORDED_CALL => {
            let mut body = "Recorded voice call".to_owned();
            if !duration.is_empty() {
                body.push_str(" • ");
                body.push_str(&duration);
            }
            Some(body)
        }
        _ => None,
    }
}

fn call_body(duration: &str) -> String {
    if duration.is_empty() {
        "Voice call".to_owned()
    } else {
        format!("Voice call • {duration}")
    }
}

/// The text of a missed call.
fn missed_call(raw: &Msg) -> Option<String> {
    (raw.int(5) == TYPE_MISSED_CALL && raw.int(13) == COARSE_MISSED)
        .then(|| "Missed voice call".to_owned())
}

/// A call length as the reference writes it: `2 mins and 5 secs`, or empty for no length.
pub fn format_duration(seconds: f64) -> String {
    let total = (seconds + 0.5) as i64;
    if total <= 0 {
        return String::new();
    }
    let mut parts = Vec::with_capacity(2);
    if total >= 60 {
        parts.push(plural(total / 60, "min"));
    }
    let rest = total % 60;
    if rest >= 1 {
        parts.push(plural(rest, "sec"));
    }
    parts.join(" and ")
}

fn plural(value: i64, unit: &str) -> String {
    if value == 1 {
        format!("1 {unit}")
    } else if unit == "min" {
        format!("{value} mins")
    } else {
        format!("{value} secs")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Hosts;
    use crate::schema::{MESSAGE, THREAD};
    use crate::wire::{Val, decode_pblite};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde_json::json;

    fn account() -> Account {
        Account {
            login_id: "gvoice_x".to_owned(),
            e164: "+15550000".to_owned(),
        }
    }

    fn offline_api() -> Api {
        let cookies = BTreeMap::from([("SAPISID".to_owned(), "s".to_owned())]);
        Api::new(reqwest::Client::new(), Hosts::single("http://127.0.0.1:1"), &cookies)
            .expect("api")
    }

    /// A raw message with the given type, coarse type and duration, and no text.
    fn call_record(kind: i64, coarse: i64, seconds: f64) -> Msg {
        Msg::new()
            .add(1, Val::Str("c".to_owned()))
            .add(5, Val::Int(kind))
            .add(9, Val::Float(seconds))
            .add(13, Val::Int(coarse))
    }

    fn raw_thread(id: &str, phones: &[&str], names: &[(&str, &str)], is_text: bool) -> Msg {
        let contacts: Vec<serde_json::Value> = names
            .iter()
            .map(|(phone, name)| json!([name, phone]))
            .collect();
        // pblite indexes the array by field number - 1, so field 8 (`is_text`) sits at index 7.
        let value = json!([id, false, null, null, contacts, null, null, is_text, phones]);
        decode_pblite(&THREAD, &value).expect("thread fixture")
    }

    #[tokio::test]
    async fn a_text_from_a_contact_keeps_its_id_sender_and_time() {
        let message = Msg::new()
            .add(1, Val::Str("m-1".to_owned()))
            .add(2, Val::Int(1_700_000_000_000))
            .add(4, Val::Msg(Msg::new().add(2, Val::Str("+15550001".to_owned()))))
            .add(5, Val::Int(10))
            .add(10, Val::Str("hello".to_owned()))
            .add(13, Val::Int(5));
        let portal = PortalRef {
            id: "1234".to_owned(),
            receiver: "gvoice_x".to_owned(),
        };
        let parts = messages(&offline_api(), &account(), &portal, &message, true).await;
        assert_eq!(parts.len(), 1);
        let converted = &parts[0];
        assert_eq!(converted.id, "m-1");
        assert_eq!(converted.part, "");
        assert_eq!(converted.sender.id, "gvoice_x.+15550001");
        assert_eq!(converted.timestamp_ms, 1_700_000_000_000);
        assert!(!converted.from_me);
        assert!(converted.live);
        assert!(matches!(
            converted.content,
            Content::Text { ref body, formatted: None } if body == "hello"
        ));
    }

    #[tokio::test]
    async fn an_outgoing_text_is_from_me() {
        let message = Msg::new()
            .add(1, Val::Str("m-2".to_owned()))
            .add(2, Val::Int(5))
            .add(5, Val::Int(TYPE_SMS_OUT))
            .add(10, Val::Str("sent".to_owned()));
        let parts = messages(&offline_api(), &account(), &PortalRef::default(), &message, false).await;
        assert!(parts[0].from_me);
        assert_eq!(parts[0].sender.id, account().me().id);
        assert!(!parts[0].live);
    }

    #[test]
    fn outgoing_is_by_type_or_coarse_type() {
        assert!(is_outgoing(&Msg::new().add(5, Val::Int(7))));
        assert!(is_outgoing(&Msg::new().add(5, Val::Int(14))));
        assert!(is_outgoing(&Msg::new().add(13, Val::Int(2))));
        assert!(!is_outgoing(&Msg::new().add(5, Val::Int(10)).add(13, Val::Int(5))));
    }

    #[test]
    fn call_records_read_like_the_reference() {
        let incoming = call_record(TYPE_INCOMING_CALL, COARSE_INCOMING, 65.0);
        assert_eq!(voice_call(&incoming).as_deref(), Some("Voice call • 1 min and 5 secs"));
        let cancelled = call_record(TYPE_OUTGOING_CALL_CANCELLED, COARSE_OUTGOING, 0.0);
        assert_eq!(voice_call(&cancelled).as_deref(), Some("Unanswered voice call"));
        let recorded = call_record(TYPE_RECORDED_CALL, COARSE_RECORDED, 3.0);
        assert_eq!(voice_call(&recorded).as_deref(), Some("Recorded voice call • 3 secs"));
        let missed = call_record(TYPE_MISSED_CALL, COARSE_MISSED, 0.0);
        assert_eq!(missed_call(&missed).as_deref(), Some("Missed voice call"));
        let voicemail_record = call_record(TYPE_VOICEMAIL, COARSE_VOICEMAIL, 9.0);
        assert_eq!(voicemail(&voicemail_record).as_deref(), Some("Voicemail • 9 secs"));
    }

    #[test]
    fn a_voicemail_transcript_replaces_the_length() {
        let token = |text: &[u8]| Val::Msg(Msg::new().add(1, Val::Bytes(text.to_vec())));
        let transcript = Msg::new()
            .add(2, token(b"hi"))
            .add(2, token(b"  "))
            .add(2, token(b"there"))
            .add(2, token(&[0xff]));
        let record = call_record(TYPE_VOICEMAIL, COARSE_VOICEMAIL, 4.0)
            .add(7, Val::Msg(transcript));
        assert_eq!(voicemail(&record).as_deref(), Some("Voicemail: hi there"));
    }

    #[test]
    fn pblite_message_fixtures_decode_into_the_same_records() {
        let value = json!([
            "c1", 1, null, [null, "+15550009"], 1, null, null, null, 65.0, null, null, null, 1
        ]);
        let record = decode_pblite(&MESSAGE, &value).expect("decodes");
        assert_eq!(voice_call(&record).as_deref(), Some("Voice call • 1 min and 5 secs"));
        assert_eq!(sender_number(&record), "+15550009");
        let encoded = STANDARD.encode(b"hi");
        assert_eq!(encoded, "aGk=");
    }

    #[test]
    fn durations_follow_the_reference_wording() {
        assert_eq!(format_duration(0.4), "");
        assert_eq!(format_duration(1.0), "1 sec");
        assert_eq!(format_duration(5.0), "5 secs");
        assert_eq!(format_duration(60.0), "1 min");
        assert_eq!(format_duration(120.0), "2 mins");
        assert_eq!(format_duration(61.0), "1 min and 1 sec");
    }

    #[test]
    fn a_thread_with_one_peer_is_a_direct_message_and_more_is_a_group() {
        let account = account();
        let portal = PortalRef {
            id: "5".to_owned(),
            receiver: "gvoice_x".to_owned(),
        };
        let direct = raw_thread("5", &["+15550001"], &[("+15550001", "Ada")], false);
        let meta = account.chat(portal.clone(), &direct);
        assert!(meta.is_dm);
        assert_eq!(meta.name, "Ada");
        assert_eq!(meta.member_count, 2);
        assert_eq!(meta.dm_user.map(|user| user.id), Some("gvoice_x.+15550001".to_owned()));

        let group = raw_thread("6", &["+15550001", "+15550002"], &[], false);
        let meta = account.chat(portal, &group);
        assert!(!meta.is_dm);
        assert_eq!(meta.name, "+15550001, +15550002");
        assert_eq!(meta.member_count, 3);
        assert!(meta.dm_user.is_none());
    }

    #[test]
    fn the_portal_of_a_call_thread_is_the_peer() {
        let account = account();
        let thread = raw_thread("999", &["+15550001"], &[], false);
        assert_eq!(account.portal(&thread).id, "t.+15550001");
        assert_eq!(account.portal(&thread).receiver, "gvoice_x");
        let text_thread = raw_thread("999", &["+15550001"], &[], true);
        assert_eq!(account.portal(&text_thread).id, "999");
    }

    #[test]
    fn html_in_an_mms_subject_is_escaped() {
        assert_eq!(
            escape_html("<a href=\"x\">&</a>"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&lt;/a&gt;"
        );
    }

    #[test]
    fn media_kinds_and_extensions() {
        assert_eq!(media_kind("image/png"), "image");
        assert_eq!(media_kind("application/pdf"), "file");
        assert_eq!(extension("image/jpeg; charset=x"), ".jpg");
        assert_eq!(extension("application/x-unknown"), "");
    }

    #[test]
    fn failures_and_unknown_messages_are_unsupported_with_their_caption() {
        assert!(matches!(
            unknown(),
            Content::Unsupported { ref body } if body.starts_with("Unknown message")
        ));
        assert!(matches!(failure("nope", ""), Content::Unsupported { ref body } if body == "nope"));
        assert!(matches!(
            failure("nope", "cap"),
            Content::Unsupported { ref body } if body == "nope\n\ncap"
        ));
    }
}
