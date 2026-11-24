//! Attributed text to the bridge's HTML subset (`b i u a br`), by the reference's rules: ranges
//! are measured in characters, overlapping ranges nest as `LinkedRangeTree` nests them, and
//! mentions, lists and paragraphs keep their text without markup.

use crate::api::model::{Attribute, AttributedText};

/// A message body: the plain text, and the HTML when there is formatting to show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rendered {
    pub body: String,
    pub html: Option<String>,
}

/// The formatting a range applies.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Style {
    Bold,
    Italic,
    Underline,
    Link(String),
    LineBreak,
    /// Mentions, lists, paragraphs and scripts: the text stays, the markup goes.
    Transparent,
}

/// A range of characters `[start, end)` with its style.
#[derive(Clone, Debug)]
struct Span {
    start: usize,
    end: usize,
    style: Style,
}

/// Renders a text with its ranges.
pub fn render(text: &AttributedText) -> Rendered {
    let chars: Vec<char> = text.text.chars().collect();
    let mut spans: Vec<Span> = text
        .attributes
        .iter()
        .filter_map(|attribute| span_of(attribute, chars.len()))
        .collect();
    let styled = spans.iter().any(|span| span.style != Style::Transparent);
    if !styled {
        return Rendered {
            body: text.text.clone(),
            html: None,
        };
    }
    spans.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    let mut html = String::new();
    render_range(&chars, 0, chars.len(), spans, &mut html);
    Rendered {
        body: text.text.clone(),
        html: Some(html),
    }
}

/// The span of one attribute, or `None` when it is out of range or of an unknown kind.
fn span_of(attribute: &Attribute, len: usize) -> Option<Span> {
    let start = usize::try_from(attribute.start).ok()?;
    let length = usize::try_from(attribute.length).ok()?;
    if length == 0 || start >= len {
        return None;
    }
    let end = start.saturating_add(length).min(len);
    let kind = &attribute.kind;
    let style = if kind.bold.is_some() {
        Style::Bold
    } else if kind.italic.is_some() {
        Style::Italic
    } else if kind.underline.is_some() {
        Style::Underline
    } else if let Some(link) = &kind.hyperlink {
        Style::Link(link.url.clone())
    } else if kind.line_break.is_some() {
        Style::LineBreak
    } else if kind.entity.is_some()
        || kind.list.is_some()
        || kind.list_item.is_some()
        || kind.paragraph.is_some()
        || kind.subscript.is_some()
        || kind.superscript.is_some()
    {
        Style::Transparent
    } else {
        return None;
    };
    Some(Span { start, end, style })
}

/// Renders `[lo, hi)`, where `spans` are sorted by start and then longest first, and lie inside.
fn render_range(chars: &[char], lo: usize, hi: usize, spans: Vec<Span>, out: &mut String) {
    let mut spans = spans.into_iter();
    let Some(first) = spans.next() else {
        push_text(out, &chars[lo..hi]);
        return;
    };
    let (start, end) = (first.start, first.end);
    push_text(out, &chars[lo..start]);
    let mut inner = Vec::new();
    let mut rest = Vec::new();
    for span in spans {
        if span.start < end {
            if span.end > end {
                rest.push(Span {
                    start: end,
                    end: span.end,
                    style: span.style.clone(),
                });
            }
            inner.push(Span {
                start: span.start,
                end: span.end.min(end),
                style: span.style,
            });
        } else {
            rest.push(span);
        }
    }
    match &first.style {
        Style::LineBreak => out.push_str("<br>"),
        style => {
            let mut body = String::new();
            render_range(chars, start, end, inner, &mut body);
            out.push_str(&wrap(style, &body));
        }
    }
    rest.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    render_range(chars, end, hi, rest, out);
}

/// The HTML of a styled body.
fn wrap(style: &Style, body: &str) -> String {
    match style {
        Style::Bold => format!("<b>{body}</b>"),
        Style::Italic => format!("<i>{body}</i>"),
        Style::Underline => format!("<u>{body}</u>"),
        Style::Link(url) if !url.is_empty() => {
            format!("<a href=\"{}\">{body}</a>", escape_attribute(url))
        }
        _ => body.to_owned(),
    }
}

/// Escapes plain text for HTML, with newlines as line breaks.
fn push_text(out: &mut String, chars: &[char]) {
    for c in chars {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("<br>"),
            other => out.push(*other),
        }
    }
}

/// Escapes plain text for use inside HTML, turning newlines into line breaks.
pub fn escape_text(value: &str) -> String {
    let mut out = String::new();
    push_text(&mut out, &value.chars().collect::<Vec<_>>());
    out
}

fn escape_attribute(value: &str) -> String {
    let mut out = String::new();
    push_text(&mut out, &value.chars().collect::<Vec<_>>());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::model::{AttributeKind, Hyperlink};
    use serde::de::IgnoredAny;

    fn attr(start: i64, length: i64, kind: AttributeKind) -> Attribute {
        Attribute {
            start,
            length,
            kind,
        }
    }

    fn bold() -> AttributeKind {
        AttributeKind {
            bold: Some(IgnoredAny),
            ..AttributeKind::default()
        }
    }

    fn text(body: &str, attributes: Vec<Attribute>) -> AttributedText {
        AttributedText {
            text: body.to_owned(),
            attributes,
        }
    }

    #[test]
    fn plain_text_has_no_html() {
        let rendered = render(&text("hello <world>", Vec::new()));
        assert_eq!(rendered.body, "hello <world>");
        assert_eq!(rendered.html, None);
    }

    #[test]
    fn bold_ranges_are_measured_in_characters() {
        // The emoji is one character, so "bold" starts at character 2.
        let rendered = render(&text("😀 bold", vec![attr(2, 4, bold())]));
        assert_eq!(rendered.html.as_deref(), Some("😀 <b>bold</b>"));
    }

    #[test]
    fn nested_and_overlapping_ranges_follow_the_reference_tree() {
        let italic = AttributeKind {
            italic: Some(IgnoredAny),
            ..AttributeKind::default()
        };
        // "abcdef": bold over 0..4, italic over 2..6 overlaps the bold range's tail.
        let rendered = render(&text(
            "abcdef",
            vec![attr(0, 4, bold()), attr(2, 4, italic)],
        ));
        assert_eq!(
            rendered.html.as_deref(),
            Some("<b>ab<i>cd</i></b><i>ef</i>")
        );
    }

    #[test]
    fn links_are_anchors_with_escaped_targets() {
        let link = AttributeKind {
            hyperlink: Some(Hyperlink {
                url: "https://example.com/?a=1&b=\"2\"".to_owned(),
            }),
            ..AttributeKind::default()
        };
        let rendered = render(&text("go here", vec![attr(3, 4, link)]));
        assert_eq!(
            rendered.html.as_deref(),
            Some("go <a href=\"https://example.com/?a=1&amp;b=&quot;2&quot;\">here</a>")
        );
    }

    #[test]
    fn text_is_escaped_and_newlines_become_breaks() {
        let rendered = render(&text("a < b\nc & d", vec![attr(0, 1, bold())]));
        assert_eq!(
            rendered.html.as_deref(),
            Some("<b>a</b> &lt; b<br>c &amp; d")
        );
    }

    #[test]
    fn mentions_keep_their_text_and_out_of_range_ranges_are_ignored() {
        let mention = AttributeKind {
            entity: Some(crate::api::model::Entity::default()),
            ..AttributeKind::default()
        };
        let only_mention = render(&text("@Ada hi", vec![attr(0, 4, mention)]));
        assert_eq!(only_mention.html, None);
        let out_of_range = render(&text("hi", vec![attr(9, 4, bold()), attr(0, 0, bold())]));
        assert_eq!(out_of_range.html, None);
    }
}
