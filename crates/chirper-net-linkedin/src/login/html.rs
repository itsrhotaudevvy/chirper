//! A small HTML tokenizer for the two pages the password login reads: the bootstrap page with its
//! hydration `<script>`, and the checkpoint forms. It understands tags, attributes (quoted or
//! bare), comments, and raw-text elements (`script`, `style`). It does not build a tree.
//!
//! Reference: the reference reads these pages with `golang.org/x/net/html`.

/// One token of a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Token {
    /// A start tag. `text` is the text after the tag, up to the next tag. For raw-text elements
    /// it is the whole element body.
    Start {
        name: String,
        attrs: Vec<(String, String)>,
        text: String,
    },
    /// An end tag.
    End { name: String },
}

impl Token {
    /// The value of an attribute, if this is a start tag that carries it.
    pub(super) fn attr(&self, wanted: &str) -> Option<&str> {
        match self {
            Self::Start { attrs, .. } => attrs
                .iter()
                .find(|(name, _)| name == wanted)
                .map(|(_, value)| value.as_str()),
            Self::End { .. } => None,
        }
    }

    /// Whether a start tag carries an attribute, with or without a value.
    pub(super) fn has_attr(&self, wanted: &str) -> bool {
        matches!(self, Self::Start { attrs, .. } if attrs.iter().any(|(name, _)| name == wanted))
    }

    /// The tag name, lowercase.
    pub(super) fn name(&self) -> &str {
        match self {
            Self::Start { name, .. } | Self::End { name } => name,
        }
    }
}

/// Tokenizes a page. Malformed markup degrades to text, never to a panic.
pub(super) fn tokens(page: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut rest = page;
    while let Some(at) = rest.find('<') {
        rest = &rest[at..];
        if rest.starts_with("<!--") {
            rest = skip_past(rest, "-->");
        } else if rest.starts_with("<!") || rest.starts_with("<?") {
            rest = skip_past(rest, ">");
        } else if let Some(end_tag) = rest.strip_prefix("</") {
            let name = tag_name(end_tag);
            rest = skip_past(rest, ">");
            if !name.is_empty() {
                out.push(Token::End { name });
            }
        } else if rest[1..].starts_with(|c: char| c.is_ascii_alphabetic()) {
            let (token, after) = start_tag(rest);
            rest = after;
            let raw_text =
                matches!(&token, Token::Start { name, .. } if name == "script" || name == "style");
            if raw_text {
                let name = token.name().to_owned();
                out.push(token);
                out.push(Token::End { name });
            } else {
                out.push(token);
            }
        } else {
            rest = &rest[1..];
        }
    }
    out
}

/// The part of `s` after the first `marker`, or nothing when the marker never appears.
fn skip_past<'a>(s: &'a str, marker: &str) -> &'a str {
    s.find(marker).map_or("", |at| &s[at + marker.len()..])
}

/// The lowercase name of the tag that starts `s` (after `<` or `</`).
fn tag_name(s: &str) -> String {
    s.chars()
        .take_while(|c| !c.is_ascii_whitespace() && *c != '>' && *c != '/')
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Parses the start tag at the front of `s` and the text that follows it.
fn start_tag(s: &str) -> (Token, &str) {
    let name = tag_name(&s[1..]);
    let (attrs, rest) = attributes(&s[1 + name.len()..]);
    if name == "script" || name == "style" {
        let close = format!("</{name}");
        let end = rest.to_ascii_lowercase().find(&close).unwrap_or(rest.len());
        let text = rest[..end].to_owned();
        let after = skip_past(&rest[end..], ">");
        return (Token::Start { name, attrs, text }, after);
    }
    let end = rest.find('<').unwrap_or(rest.len());
    (
        Token::Start {
            name,
            attrs,
            text: rest[..end].to_owned(),
        },
        &rest[end..],
    )
}

/// Reads attributes up to and including the closing `>` of a tag.
fn attributes(s: &str) -> (Vec<(String, String)>, &str) {
    let mut attrs = Vec::new();
    let mut rest = s;
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix('>') {
            return (attrs, after);
        }
        if let Some(after) = rest.strip_prefix('/') {
            rest = after;
            continue;
        }
        if rest.is_empty() || rest.starts_with('<') {
            return (attrs, rest);
        }
        let name_len = rest
            .find(|c: char| c.is_ascii_whitespace() || c == '=' || c == '>' || c == '/')
            .unwrap_or(rest.len());
        if name_len == 0 {
            // A stray `=` or similar: skip the character and carry on.
            let skipped = rest.chars().next().map_or(0, char::len_utf8);
            rest = &rest[skipped..];
            continue;
        }
        let name = rest[..name_len].to_ascii_lowercase();
        rest = rest[name_len..].trim_start();
        let mut value = String::new();
        if let Some(after) = rest.strip_prefix('=') {
            rest = after.trim_start();
            let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'');
            if let Some(quote) = quote {
                let body = &rest[1..];
                let end = body.find(quote).unwrap_or(body.len());
                value = decode_entities(&body[..end]);
                rest = skip_past(body, &quote.to_string());
            } else {
                let end = rest
                    .find(|c: char| c.is_ascii_whitespace() || c == '>')
                    .unwrap_or(rest.len());
                value = decode_entities(&rest[..end]);
                rest = &rest[end..];
            }
        }
        attrs.push((name, value));
    }
}

/// The few entities the reference's pages use in attribute values.
fn decode_entities(raw: &str) -> String {
    raw.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attributes_quoted_bare_and_entities() {
        let page =
            r#"<form id="f" method='POST' action=/a/b data-x="a&amp;b"><input name=pin></form>"#;
        let tokens = tokens(page);
        assert_eq!(tokens[0].attr("id"), Some("f"));
        assert_eq!(tokens[0].attr("method"), Some("POST"));
        assert_eq!(tokens[0].attr("action"), Some("/a/b"));
        assert_eq!(tokens[0].attr("data-x"), Some("a&b"));
        assert_eq!(tokens[1].attr("name"), Some("pin"));
        assert_eq!(
            tokens[2],
            Token::End {
                name: "form".to_owned()
            }
        );
    }

    #[test]
    fn script_bodies_are_raw_text_and_comments_are_skipped() {
        let page = "<!-- <form id=\"x\"> --><script id=\"rehydrate-data\">a<b</script><code id=\"c\"> \"v\" </code>";
        let tokens = tokens(page);
        assert_eq!(tokens.len(), 4);
        assert_eq!(tokens[0].attr("id"), Some("rehydrate-data"));
        assert!(matches!(&tokens[0], Token::Start { text, .. } if text == "a<b"));
        assert!(matches!(&tokens[1], Token::End { name } if name == "script"));
        assert!(matches!(&tokens[2], Token::Start { text, .. } if text.trim() == "\"v\""));
    }

    #[test]
    fn malformed_markup_never_panics() {
        for page in ["<", "<a", "<a href=", "<a href=\"x", "</", "<!--", "é<é>"] {
            let _ = tokens(page);
        }
    }
}
