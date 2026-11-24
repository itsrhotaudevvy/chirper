//! Slack `rich_text` blocks as the HTML subset Chirper shows.
//!
//! Slack sends the same text twice: as mrkdwn in `text`, and as structured elements in `blocks`.
//! When a message has rich text blocks they win, because they keep mentions and styles exactly as
//! the sender made them. The element kinds and their fields follow Slack's block kit reference.

use serde_json::Value;

use crate::emoji;
use crate::html::{escape, format_timestamp};
use crate::mrkdwn::Lookup;

/// The HTML of every `rich_text` block, or `None` when the message has none.
pub fn to_html(blocks: &[Value], names: &dyn Lookup) -> Option<String> {
    let parts: Vec<String> = blocks
        .iter()
        .filter(|block| kind(block).starts_with("rich_text"))
        .map(|block| {
            elements(block)
                .iter()
                .map(|element| element_html(element, names))
                .collect::<Vec<_>>()
                .join("<br>")
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join("<br>"))
}

fn element_html(element: &Value, names: &dyn Lookup) -> String {
    match kind(element) {
        "rich_text_section" => section(&elements(element), names),
        "rich_text_list" => list(element, names),
        "rich_text_preformatted" => format!("<pre>{}</pre>", section(&elements(element), names)),
        "rich_text_quote" => format!(
            "<blockquote>{}</blockquote>",
            section(&elements(element), names)
        ),
        _ => String::new(),
    }
}

fn list(list: &Value, names: &dyn Lookup) -> String {
    let ordered = list.get("style").and_then(Value::as_str) == Some("ordered");
    let indent = list.get("indent").and_then(Value::as_u64).unwrap_or(0);
    let indentation = "&nbsp;".repeat(4 * indent as usize);
    elements(list)
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let marker = if ordered {
                format!("{}. ", index + 1)
            } else {
                "• ".to_owned()
            };
            format!(
                "{indentation}{marker}{}",
                section(&elements(item), names)
            )
        })
        .collect::<Vec<_>>()
        .join("<br>")
}

/// The inline elements of a section, quote or preformatted block.
fn section(inline: &[Value], names: &dyn Lookup) -> String {
    inline
        .iter()
        .map(|element| {
            let html = inline_html(element, names);
            wrap(&html, element.get("style"))
        })
        .collect()
}

fn inline_html(element: &Value, names: &dyn Lookup) -> String {
    match kind(element) {
        "text" => text(element.get("text").and_then(Value::as_str).unwrap_or_default()),
        "link" => {
            let url = field(element, "url");
            let label = element
                .get("text")
                .and_then(Value::as_str)
                .filter(|label| !label.is_empty())
                .unwrap_or(url);
            format!("<a href=\"{}\">{}</a>", escape(url), escape(label))
        }
        "user" => {
            let id = field(element, "user_id");
            format!("@{}", escape(&names.user_name(id).unwrap_or_else(|| id.to_owned())))
        }
        "channel" => {
            let id = field(element, "channel_id");
            format!("#{}", escape(&names.channel_name(id).unwrap_or_else(|| id.to_owned())))
        }
        "usergroup" => format!("@{}", escape(field(element, "usergroup_id"))),
        "broadcast" => format!("@{}", escape(field(element, "range"))),
        "emoji" => emoji_html(element),
        "date" => {
            let timestamp = element.get("timestamp").and_then(Value::as_i64).unwrap_or(0);
            let fallback = field(element, "fallback");
            if fallback.is_empty() {
                escape(&format_timestamp(timestamp))
            } else {
                escape(fallback)
            }
        }
        "color" => escape(field(element, "value")),
        _ => String::new(),
    }
}

/// A unicode emoji from its code points, or the standard character of its name. A custom emoji
/// stays as its `:name:` text.
fn emoji_html(element: &Value) -> String {
    if let Some(unicode) = element.get("unicode").and_then(Value::as_str) {
        let decoded: Option<String> = unicode
            .split('-')
            .map(|code| {
                u32::from_str_radix(code, 16)
                    .ok()
                    .and_then(char::from_u32)
            })
            .collect();
        if let Some(decoded) = decoded {
            return decoded;
        }
    }
    let name = field(element, "name");
    match emoji::unicode_for(name) {
        Some(character) => character.to_owned(),
        None => escape(&format!(":{name}:")),
    }
}

/// Plain text as HTML: escaped, with line breaks kept.
fn text(raw: &str) -> String {
    escape(raw).replace('\n', "<br>")
}

fn wrap(html: &str, style: Option<&Value>) -> String {
    let flag = |name: &str| {
        style
            .and_then(|style| style.get(name))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    let mut out = html.to_owned();
    for (name, tag) in [("code", "code"), ("strike", "s"), ("italic", "i"), ("bold", "b")] {
        if flag(name) {
            out = format!("<{tag}>{out}</{tag}>");
        }
    }
    out
}

fn kind(value: &Value) -> &str {
    value.get("type").and_then(Value::as_str).unwrap_or_default()
}

fn elements(value: &Value) -> Vec<Value> {
    value
        .get("elements")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn field<'a>(value: &'a Value, name: &str) -> &'a str {
    value.get(name).and_then(Value::as_str).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    struct Names(HashMap<&'static str, &'static str>);

    impl Lookup for Names {
        fn user_name(&self, id: &str) -> Option<String> {
            self.0.get(id).map(|name| (*name).to_owned())
        }
        fn channel_name(&self, id: &str) -> Option<String> {
            self.0.get(id).map(|name| (*name).to_owned())
        }
    }

    fn names() -> Names {
        Names(HashMap::from([("U1", "Ada"), ("C1", "general")]))
    }

    #[test]
    fn sections_keep_styles_mentions_links_and_emoji() {
        let blocks = vec![json!({
            "type": "rich_text",
            "elements": [{
                "type": "rich_text_section",
                "elements": [
                    {"type": "text", "text": "a <b>", "style": {"bold": true}},
                    {"type": "user", "user_id": "U1"},
                    {"type": "text", "text": " in "},
                    {"type": "channel", "channel_id": "C1"},
                    {"type": "text", "text": " "},
                    {"type": "emoji", "name": "thumbsup", "unicode": "1f44d"},
                    {"type": "text", "text": " "},
                    {"type": "link", "url": "https://x.example", "text": "x"}
                ]
            }]
        })];
        assert_eq!(
            to_html(&blocks, &names()).as_deref(),
            Some("<b>a &lt;b&gt;</b>@Ada in #general 👍 <a href=\"https://x.example\">x</a>")
        );
    }

    #[test]
    fn lists_quotes_and_preformatted_blocks_render() {
        let blocks = vec![json!({
            "type": "rich_text",
            "elements": [
                {"type": "rich_text_list", "style": "ordered", "indent": 0, "elements": [
                    {"type": "rich_text_section", "elements": [{"type": "text", "text": "one"}]},
                    {"type": "rich_text_section", "elements": [{"type": "text", "text": "two"}]}
                ]},
                {"type": "rich_text_quote", "elements": [{"type": "text", "text": "q"}]},
                {"type": "rich_text_preformatted", "elements": [{"type": "text", "text": "x\ny"}]}
            ]
        })];
        assert_eq!(
            to_html(&blocks, &names()).as_deref(),
            Some(
                "1. one<br>2. two<br><blockquote>q</blockquote><br><pre>x<br>y</pre>"
            )
        );
    }

    #[test]
    fn a_custom_emoji_stays_as_text() {
        let blocks = vec![json!({
            "type": "rich_text",
            "elements": [{"type": "rich_text_section", "elements": [
                {"type": "emoji", "name": "partyparrot"}
            ]}]
        })];
        assert_eq!(
            to_html(&blocks, &names()).as_deref(),
            Some(":partyparrot:")
        );
    }

    #[test]
    fn messages_without_rich_text_have_no_html() {
        let blocks = vec![json!({"type": "section", "text": {"type": "mrkdwn", "text": "x"}})];
        assert_eq!(to_html(&blocks, &names()), None);
        assert_eq!(to_html(&[], &names()), None);
    }
}
