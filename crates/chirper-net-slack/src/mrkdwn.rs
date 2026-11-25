//! Slack's message markup (mrkdwn) as the HTML subset Chirper shows, and the escaping of outgoing
//! text.
//!
//! The grammar follows Slack's formatting reference: `*bold*`, `_italic_`, `~strike~`, code spans,
//! fenced blocks, `>` quotes, `<@U…>` users, `<#C…>` channels, `<!here>` and friends, `<!date^…>`,
//! and `<url|label>` links. Emphasis needs a word boundary on the outside of each marker, as Slack
//! and the Go bridge's parser require. The shortcode handling and the regex that finds tags are
//! those of `third_party/slack/pkg/msgconv/mrkdwn/tag.go` (mautrix-slack, AGPL-3.0).

use crate::emoji;
use crate::html::{decode_entities, escape, format_timestamp};

/// Names for the ids a message can mention. A lookup that knows nothing returns `None`, and the
/// id itself is shown.
pub trait Lookup {
    /// The display name of a Slack user or bot id.
    fn user_name(&self, id: &str) -> Option<String>;
    /// The name of a Slack channel id.
    fn channel_name(&self, id: &str) -> Option<String>;
}

/// Render Slack mrkdwn as the HTML subset.
pub fn to_html(text: &str, names: &dyn Lookup) -> String {
    let text = emoji::replace_shortcodes(text);
    let segments: Vec<&str> = text.split("```").collect();
    let last = segments.len().saturating_sub(1);
    let mut out = String::new();
    for (index, segment) in segments.iter().enumerate() {
        if index % 2 == 1 {
            let inner = segment.strip_prefix('\n').unwrap_or(segment);
            let inner = inner.strip_suffix('\n').unwrap_or(inner);
            out.push_str("<pre>");
            out.push_str(&escape(&decode_entities(inner)));
            out.push_str("</pre>");
            continue;
        }
        let mut body: &str = segment;
        if index > 0 {
            body = body.strip_prefix('\n').unwrap_or(body);
        }
        if index < last {
            body = body.strip_suffix('\n').unwrap_or(body);
        }
        let lines: Vec<&str> = body.split('\n').collect();
        out.push_str(&render_lines(&lines, names));
    }
    out
}

/// The user ids a text mentions, in order and without repeats.
pub fn mentioned_users(text: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(index) = rest.find("<@") {
        rest = &rest[index + 2..];
        let end = rest.find(['|', '>']).unwrap_or(rest.len());
        let id = &rest[..end];
        if !id.is_empty() && !ids.iter().any(|known| known == id) {
            ids.push(id.to_owned());
        }
        rest = &rest[end..];
    }
    ids
}

/// Escape text for sending. Slack requires `&`, `<` and `>` to be written as entities, so the text
/// cannot inject mentions or links.
pub fn escape_outbound(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn render_lines(lines: &[&str], names: &dyn Lookup) -> String {
    let mut out = String::new();
    let mut index = 0;
    while index < lines.len() {
        if index > 0 {
            out.push_str("<br>");
        }
        if is_quote(lines[index]) {
            let mut quoted = Vec::new();
            while index < lines.len() && is_quote(lines[index]) {
                quoted.push(inline(strip_quote(lines[index]), names));
                index += 1;
            }
            out.push_str("<blockquote>");
            out.push_str(&quoted.join("<br>"));
            out.push_str("</blockquote>");
        } else {
            out.push_str(&inline(lines[index], names));
            index += 1;
        }
    }
    out
}

fn is_quote(line: &str) -> bool {
    line.starts_with('>') || line.starts_with("&gt;")
}

fn strip_quote(line: &str) -> &str {
    let rest = line
        .strip_prefix('>')
        .or_else(|| line.strip_prefix("&gt;"))
        .unwrap_or(line);
    rest.strip_prefix(' ').unwrap_or(rest)
}

/// One step of an inline line before the emphasis pairs are known.
enum Item {
    /// Escaped text or a finished inline element.
    Html(String),
    /// A formatting marker that may open or close emphasis.
    Mark {
        ch: char,
        can_open: bool,
        can_close: bool,
        role: Role,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Literal,
    Open,
    Close,
}

fn inline(line: &str, names: &dyn Lookup) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut items: Vec<Item> = Vec::new();
    let mut raw = String::new();
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        if let Some((next, item)) = special(&chars, index, names) {
            flush_text(&mut raw, &mut items);
            items.push(item);
            index = next;
            continue;
        }
        if matches!(c, '*' | '_' | '~') {
            flush_text(&mut raw, &mut items);
            let prev = index.checked_sub(1).and_then(|i| chars.get(i)).copied();
            let next = chars.get(index + 1).copied();
            items.push(Item::Mark {
                ch: c,
                can_open: can_open(prev, next),
                can_close: can_close(prev, next),
                role: Role::Literal,
            });
        } else {
            raw.push(c);
        }
        index += 1;
    }
    flush_text(&mut raw, &mut items);
    pair_marks(&mut items);
    items.iter().map(render_item).collect()
}

/// A code span or a `<…>` tag starting at `index`: the index after it, and what it renders to.
fn special(chars: &[char], index: usize, names: &dyn Lookup) -> Option<(usize, Item)> {
    match chars[index] {
        '`' => {
            let close = find(chars, index + 1, '`')?;
            let inner: String = chars[index + 1..close].iter().collect();
            let html = format!("<code>{}</code>", escape(&decode_entities(&inner)));
            Some((close + 1, Item::Html(html)))
        }
        '<' => {
            let close = find(chars, index + 1, '>')?;
            let inner: String = chars[index + 1..close].iter().collect();
            let html = tag(&inner, names)?;
            Some((close + 1, Item::Html(html)))
        }
        _ => None,
    }
}

fn find(chars: &[char], from: usize, target: char) -> Option<usize> {
    chars
        .get(from..)?
        .iter()
        .position(|c| *c == target)
        .map(|offset| from + offset)
}

fn flush_text(raw: &mut String, items: &mut Vec<Item>) {
    if !raw.is_empty() {
        items.push(Item::Html(escape(&decode_entities(raw))));
        raw.clear();
    }
}

fn can_open(prev: Option<char>, next: Option<char>) -> bool {
    next.is_some_and(|n| !n.is_whitespace())
        && prev.is_none_or(|p| p.is_whitespace() || !p.is_alphanumeric())
}

fn can_close(prev: Option<char>, next: Option<char>) -> bool {
    prev.is_some_and(|p| !p.is_whitespace())
        && next.is_none_or(|n| n.is_whitespace() || !n.is_alphanumeric())
}

/// Match each closing marker with the nearest open marker of the same kind. Openers between them
/// are dropped, so the pairs always nest.
fn pair_marks(items: &mut [Item]) {
    let mut openers: Vec<usize> = Vec::new();
    for index in 0..items.len() {
        let Item::Mark {
            ch,
            can_open,
            can_close,
            ..
        } = items[index]
        else {
            continue;
        };
        if can_close {
            let found = openers.iter().rposition(
                |&opener| matches!(items[opener], Item::Mark { ch: open_ch, .. } if open_ch == ch),
            );
            if let Some(position) = found {
                let opener = openers[position];
                openers.truncate(position);
                set_role(items, opener, Role::Open);
                set_role(items, index, Role::Close);
                continue;
            }
        }
        if can_open {
            openers.push(index);
        }
    }
}

fn set_role(items: &mut [Item], index: usize, value: Role) {
    if let Some(Item::Mark { role, .. }) = items.get_mut(index) {
        *role = value;
    }
}

fn render_item(item: &Item) -> String {
    match item {
        Item::Html(html) => html.clone(),
        Item::Mark { ch, role, .. } => {
            let tag = match ch {
                '*' => "b",
                '_' => "i",
                _ => "s",
            };
            match role {
                Role::Open => format!("<{tag}>"),
                Role::Close => format!("</{tag}>"),
                Role::Literal => ch.to_string(),
            }
        }
    }
}

/// The HTML of a Slack `<…>` token, or `None` when the token is not one Slack writes.
fn tag(inner: &str, names: &dyn Lookup) -> Option<String> {
    let (head, label) = match inner.split_once('|') {
        Some((head, label)) => (head, Some(decode_entities(label))),
        None => (inner, None),
    };
    let label = label.filter(|label| !label.is_empty());
    if let Some(id) = head.strip_prefix('@') {
        let name = names
            .user_name(id)
            .or(label)
            .unwrap_or_else(|| id.to_owned());
        return Some(escape(&format!("@{name}")));
    }
    if let Some(id) = head.strip_prefix('#') {
        let name = names
            .channel_name(id)
            .or(label)
            .unwrap_or_else(|| id.to_owned());
        return Some(escape(&format!("#{name}")));
    }
    if let Some(special) = head.strip_prefix('!') {
        return Some(escape(&special_text(special, label)));
    }
    if head.contains(char::is_whitespace) || !head.contains(':') {
        return None;
    }
    let url = decode_entities(head);
    let text = label.unwrap_or_else(|| url.clone());
    Some(format!(
        "<a href=\"{}\">{}</a>",
        escape(&url),
        escape(&text)
    ))
}

/// The text of a `<!…>` token: a broadcast, a user group or a date.
fn special_text(special: &str, label: Option<String>) -> String {
    if matches!(special, "here" | "channel" | "everyone") {
        return format!("@{special}");
    }
    if let Some(group) = special.strip_prefix("subteam^") {
        return label.unwrap_or_else(|| format!("@{group}"));
    }
    if let Some(date) = special.strip_prefix("date^") {
        if let Some(seconds) = date.split('^').next().and_then(|s| s.parse::<i64>().ok()) {
            return label.unwrap_or_else(|| format_timestamp(seconds));
        }
        return label.unwrap_or_default();
    }
    label.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Names(HashMap<&'static str, &'static str>);

    impl Lookup for Names {
        fn user_name(&self, id: &str) -> Option<String> {
            self.0.get(id).map(|name| (*name).to_owned())
        }
        fn channel_name(&self, id: &str) -> Option<String> {
            self.0.get(id).map(|name| format!("{name}-room"))
        }
    }

    fn render(text: &str) -> String {
        let names = Names(HashMap::from([("U1", "Ada"), ("C1", "general")]));
        to_html(text, &names)
    }

    #[test]
    fn emphasis_needs_word_boundaries() {
        assert_eq!(
            render("*bold* _it_ ~strike~ `code`"),
            "<b>bold</b> <i>it</i> <s>strike</s> <code>code</code>"
        );
        assert_eq!(render("snake_case_name"), "snake_case_name");
        assert_eq!(render("2 * 3 * 4"), "2 * 3 * 4");
        assert_eq!(render("*unclosed"), "*unclosed");
    }

    #[test]
    fn code_spans_are_not_formatted_inside() {
        assert_eq!(render("`*x* <@U1>`"), "<code>*x* &lt;@U1&gt;</code>");
    }

    #[test]
    fn mentions_show_names_and_fall_back_to_ids() {
        assert_eq!(render("hi <@U1>!"), "hi @Ada!");
        assert_eq!(render("<@U9|Grace> there"), "@Grace there");
        assert_eq!(render("<@U9> there"), "@U9 there");
        assert_eq!(render("in <#C1>"), "in #general-room");
        assert_eq!(render("<!here> and <!channel>"), "@here and @channel");
    }

    #[test]
    fn links_keep_their_label_and_escape_their_url() {
        assert_eq!(
            render("<https://example.com/a?b=1&amp;c=2|site> & more"),
            "<a href=\"https://example.com/a?b=1&amp;c=2\">site</a> &amp; more"
        );
        assert_eq!(
            render("<https://example.com>"),
            "<a href=\"https://example.com\">https://example.com</a>"
        );
    }

    #[test]
    fn entities_are_decoded_once_and_escaped_for_html() {
        assert_eq!(render("&lt;tag&gt; &amp;"), "&lt;tag&gt; &amp;");
        assert_eq!(render("a < b"), "a &lt; b");
    }

    #[test]
    fn quotes_and_code_blocks_become_blocks() {
        assert_eq!(
            render("> quoted\n> two\nreply"),
            "<blockquote>quoted<br>two</blockquote><br>reply"
        );
        assert_eq!(render("&gt; quoted"), "<blockquote>quoted</blockquote>");
        assert_eq!(render("```\ncode <x>\n```"), "<pre>code &lt;x&gt;</pre>");
    }

    #[test]
    fn standard_shortcodes_become_characters() {
        assert_eq!(render(":wave: hi"), "👋 hi");
        assert_eq!(render(":custom_thing:"), ":custom_thing:");
    }

    #[test]
    fn dates_show_their_fallback_or_the_time() {
        assert_eq!(render("<!date^0^{date}|fallback text>"), "fallback text");
        assert_eq!(render("<!date^0^{date}>"), "1970-01-01 00:00 UTC");
    }

    #[test]
    fn mentioned_users_are_collected_once() {
        assert_eq!(
            mentioned_users("<@U1> <@U2|bob> <@U1> <#C1>"),
            vec!["U1".to_owned(), "U2".to_owned()]
        );
    }

    #[test]
    fn outgoing_text_is_escaped_for_slack() {
        assert_eq!(escape_outbound("a <@U1> & b"), "a &lt;@U1&gt; &amp; b");
    }
}
