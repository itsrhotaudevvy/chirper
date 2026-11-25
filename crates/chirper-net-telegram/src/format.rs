//! Telegram message entities to the HTML subset Chirper shows (`b i u s code pre a blockquote br`
//! and `span data-mx-spoiler`).
//!
//! Entity offsets and lengths count UTF-16 code units. Entities may nest or overlap. A child is
//! clamped to the end of the entity it sits in, so the output is always well formed and no text
//! is lost. Newlines become `<br>` except inside code and pre, where they stay.

/// What an entity means in HTML.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tag {
    Bold,
    Italic,
    Underline,
    Strike,
    Code,
    /// A preformatted block, with its language when Telegram gave one.
    Pre {
        language: String,
    },
    /// A link to this URL.
    Link {
        url: String,
    },
    Blockquote,
    Spoiler,
}

/// One entity: a tag over a UTF-16 range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub offset: i64,
    pub length: i64,
    pub tag: Tag,
}

/// Escape text for an HTML text node.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// The open and close markup of a tag.
fn markup(tag: &Tag) -> (String, &'static str) {
    match tag {
        Tag::Bold => ("<b>".into(), "</b>"),
        Tag::Italic => ("<i>".into(), "</i>"),
        Tag::Underline => ("<u>".into(), "</u>"),
        Tag::Strike => ("<s>".into(), "</s>"),
        Tag::Code => ("<code>".into(), "</code>"),
        Tag::Pre { language } if language.is_empty() => ("<pre>".into(), "</pre>"),
        Tag::Pre { language } => (
            format!("<pre><code class=\"language-{}\">", escape(language)),
            "</code></pre>",
        ),
        Tag::Link { url } => (format!("<a href=\"{}\">", escape(url)), "</a>"),
        Tag::Blockquote => ("<blockquote>".into(), "</blockquote>"),
        Tag::Spoiler => ("<span data-mx-spoiler>".into(), "</span>"),
    }
}

fn is_literal(tag: &Tag) -> bool {
    matches!(tag, Tag::Code | Tag::Pre { .. })
}

/// Text between two UTF-16 offsets, escaped. Line breaks become `<br>` unless `literal`.
fn text_html(units: &[u16], from: usize, to: usize, literal: bool) -> String {
    let slice = units.get(from..to).unwrap_or(&[]);
    let text = String::from_utf16_lossy(slice);
    let escaped = escape(&text);
    if literal {
        escaped
    } else {
        escaped.replace('\n', "<br>")
    }
}

/// Clamp an entity to the text: `None` when it is empty or outside the text.
fn bounds(span: &Span, len: usize) -> Option<(usize, usize)> {
    let start = usize::try_from(span.offset).ok()?;
    let length = usize::try_from(span.length).ok()?;
    if start >= len || length == 0 {
        return None;
    }
    Some((start, start.saturating_add(length).min(len)))
}

/// Render `text` with `spans` as HTML. Returns `None` when no span produces markup.
pub fn to_html(text: &str, spans: &[Span]) -> Option<String> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let len = units.len();
    let mut ranges: Vec<(usize, usize, &Tag)> = spans
        .iter()
        .filter_map(|span| bounds(span, len).map(|(start, end)| (start, end, &span.tag)))
        .collect();
    if ranges.is_empty() {
        return None;
    }
    // Outer entities first: earlier start, then longer.
    ranges.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));

    let mut out = String::with_capacity(text.len() + 16);
    // Open entities: (end, close markup, literal).
    let mut stack: Vec<(usize, &'static str, bool)> = Vec::new();
    let mut pos = 0usize;
    for (start, end, tag) in ranges {
        while let Some(&(top_end, close, _)) = stack.last() {
            if top_end > start {
                break;
            }
            out.push_str(&text_html(&units, pos, top_end, in_literal(&stack)));
            out.push_str(close);
            pos = top_end;
            stack.pop();
        }
        out.push_str(&text_html(&units, pos, start, in_literal(&stack)));
        pos = start;
        let end = stack
            .last()
            .map_or(end, |&(top_end, _, _)| end.min(top_end));
        if end <= start {
            continue;
        }
        let (open, close) = markup(tag);
        out.push_str(&open);
        stack.push((end, close, is_literal(tag)));
    }
    while let Some((top_end, close, literal)) = stack.pop() {
        // The text up to this entity's end is inside it, so its own flag applies until it closes.
        out.push_str(&text_html(
            &units,
            pos,
            top_end,
            literal || in_literal(&stack),
        ));
        out.push_str(close);
        pos = top_end;
    }
    out.push_str(&text_html(&units, pos, len, false));
    Some(out)
}

fn in_literal(stack: &[(usize, &'static str, bool)]) -> bool {
    stack.iter().any(|&(_, _, literal)| literal)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(offset: i64, length: i64, tag: Tag) -> Span {
        Span {
            offset,
            length,
            tag,
        }
    }

    #[test]
    fn plain_text_has_no_markup() {
        assert_eq!(to_html("hi", &[]), None);
        assert_eq!(
            to_html(
                "hi",
                &[span(
                    0,
                    2,
                    Tag::Pre {
                        language: String::new()
                    }
                )]
            ),
            Some("<pre>hi</pre>".to_owned())
        );
    }

    #[test]
    fn simple_bold_and_escaping() {
        let html = to_html("a <b> & c", &[span(0, 1, Tag::Bold)]).unwrap();
        assert_eq!(html, "<b>a</b> &lt;b&gt; &amp; c");
    }

    #[test]
    fn newlines_become_breaks_outside_code() {
        let html = to_html("one\ntwo", &[span(0, 3, Tag::Italic)]).unwrap();
        assert_eq!(html, "<i>one</i><br>two");
        let code = to_html("x\ny", &[span(0, 3, Tag::Code)]).unwrap();
        assert_eq!(code, "<code>x\ny</code>");
    }

    #[test]
    fn offsets_count_utf16_units() {
        // "😀" is two UTF-16 units; "bold" starts after it.
        let text = "😀 bold";
        let html = to_html(text, &[span(3, 4, Tag::Bold)]).unwrap();
        assert_eq!(html, "😀 <b>bold</b>");
        // CJK units are one unit each.
        let cjk = to_html("你好世界", &[span(1, 2, Tag::Underline)]).unwrap();
        assert_eq!(cjk, "你<u>好世</u>界");
    }

    #[test]
    fn nested_entities_close_in_order() {
        let html = to_html(
            "abcdef",
            &[
                span(0, 6, Tag::Bold),
                span(1, 3, Tag::Italic),
                span(2, 1, Tag::Strike),
            ],
        )
        .unwrap();
        assert_eq!(html, "<b>a<i>b<s>c</s>d</i>ef</b>");
    }

    #[test]
    fn overlapping_entities_are_clamped() {
        // Italic runs 1..5 but bold ends at 3: italic is cut at 3 and nothing is lost.
        let html = to_html("abcdef", &[span(0, 3, Tag::Bold), span(1, 4, Tag::Italic)]).unwrap();
        assert_eq!(html, "<b>a<i>bc</i></b>def");
    }

    #[test]
    fn links_quotes_and_spoilers() {
        let html = to_html(
            "go here",
            &[
                span(
                    0,
                    2,
                    Tag::Link {
                        url: "https://x.test/?a=1&b=\"2\"".into(),
                    },
                ),
                span(3, 4, Tag::Spoiler),
            ],
        )
        .unwrap();
        assert_eq!(
            html,
            "<a href=\"https://x.test/?a=1&amp;b=&quot;2&quot;\">go</a> <span data-mx-spoiler>here</span>"
        );
    }

    #[test]
    fn pre_with_language_and_blockquote() {
        let html = to_html(
            "let x\nquote",
            &[
                span(
                    0,
                    5,
                    Tag::Pre {
                        language: "rust".into(),
                    },
                ),
                span(6, 5, Tag::Blockquote),
            ],
        )
        .unwrap();
        assert_eq!(
            html,
            "<pre><code class=\"language-rust\">let x</code></pre><br><blockquote>quote</blockquote>"
        );
    }

    #[test]
    fn out_of_range_and_empty_entities_are_ignored() {
        assert_eq!(
            to_html("abc", &[span(5, 2, Tag::Bold), span(0, 0, Tag::Bold)]),
            None
        );
        assert_eq!(to_html("abc", &[span(-1, 2, Tag::Bold)]), None);
        let html = to_html("abc", &[span(1, 99, Tag::Bold)]).unwrap();
        assert_eq!(html, "a<b>bc</b>");
    }
}
