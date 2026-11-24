//! Slack emoji names and their Unicode characters.
//!
//! The table in `emoji.json` is copied from `third_party/slack/pkg/emoji/emoji.json` of the mautrix
//! Slack bridge (AGPL-3.0, mautrix-slack). Lookups mirror that package: `:name:` becomes the
//! character, and a character maps back to its first name with the variation selector removed.

use std::collections::HashMap;
use std::sync::OnceLock;

const TABLE: &str = include_str!("emoji.json");
/// The variation selector that Slack sometimes appends to a character; it is ignored in lookups.
const VARIATION_SELECTOR: char = '\u{FE0F}';

struct Table {
    by_name: HashMap<String, String>,
    by_char: HashMap<String, String>,
}

fn table() -> &'static Table {
    static TABLE_ONCE: OnceLock<Table> = OnceLock::new();
    TABLE_ONCE.get_or_init(|| {
        let by_name: HashMap<String, String> = serde_json::from_str(TABLE).unwrap_or_default();
        let mut names: Vec<(&String, &String)> = by_name.iter().collect();
        // Several names can share one character. The choice is made deterministically: the name
        // without a skin-tone suffix, then the shortest, then the lexically first.
        names.sort_by(|a, b| {
            (a.0.contains("::"), a.0.len(), a.0).cmp(&(b.0.contains("::"), b.0.len(), b.0))
        });
        let mut by_char = HashMap::new();
        for (name, emoji) in names {
            by_char
                .entry(strip_selector(emoji))
                .or_insert_with(|| name.clone());
        }
        Table { by_name, by_char }
    })
}

fn strip_selector(text: &str) -> String {
    text.chars().filter(|c| *c != VARIATION_SELECTOR).collect()
}

/// The character of a Slack emoji name, without colons. `None` when the name is not a standard one.
pub fn unicode_for(name: &str) -> Option<&'static str> {
    table().by_name.get(name).map(String::as_str)
}

/// The Slack name of a character, without colons. `None` when the character has no name.
pub fn name_for(unicode: &str) -> Option<&'static str> {
    table()
        .by_char
        .get(&strip_selector(unicode))
        .map(String::as_str)
}

/// Replace every `:name:` of a standard emoji with its character. Other text is kept as it is.
///
/// The scan follows the Go regex `:[^:\s]*:`: a match never contains whitespace or a second colon
/// inside, and a match that is not an emoji is kept and scanning resumes after its closing colon.
pub fn replace_shortcodes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(':') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find(|c: char| c == ':' || c.is_whitespace()) {
            Some(end) if after[end..].starts_with(':') => {
                match unicode_for(&after[..end]) {
                    Some(emoji) => out.push_str(emoji),
                    None => out.push_str(&rest[start..start + end + 2]),
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push(':');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_map_to_characters_and_back() {
        assert_eq!(unicode_for("+1"), Some("👍\u{fe0f}"));
        assert_eq!(name_for("👍"), Some("+1"));
        assert_eq!(unicode_for("no_such_emoji_xyz"), None);
    }

    #[test]
    fn characters_without_the_selector_still_resolve() {
        // The table stores "👍️" with a selector; the lookup ignores it.
        assert!(name_for("👍").is_some());
        assert!(name_for("🅰").is_some());
    }

    #[test]
    fn shortcodes_are_replaced_when_they_are_standard() {
        assert_eq!(replace_shortcodes("hi :wave: there"), "hi 👋 there");
        assert_eq!(replace_shortcodes(":nope: x"), ":nope: x");
        assert_eq!(replace_shortcodes("a : b : c"), "a : b : c");
        assert_eq!(replace_shortcodes("no emoji"), "no emoji");
        assert_eq!(replace_shortcodes("end :"), "end :");
        assert_eq!(replace_shortcodes(":a b:wave:"), ":a b👋");
    }
}
