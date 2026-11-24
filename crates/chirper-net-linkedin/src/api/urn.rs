//! LinkedIn URNs (`prefix:id`), parsed the way the reference client parses them.
//!
//! The reference splits a URN at the first colon whose remainder either has no colon left or is a
//! parenthesised group, so `urn:li:msg_message:(urn:li:fsd_profile:1,2)` keeps its inner colons
//! in the id. Round-tripping `prefix + ":" + id` reproduces the input exactly.

/// A parsed URN.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Urn {
    prefix: String,
    id: String,
}

impl Urn {
    /// Parses a URN. A string that does not look like a URN becomes an id with no prefix.
    pub fn parse(text: &str) -> Self {
        for (index, _) in text.match_indices(':') {
            let rest = &text[index + 1..];
            let grouped = rest.len() >= 2 && rest.starts_with('(') && rest.ends_with(')');
            if grouped || !rest.contains(':') {
                return Self {
                    prefix: text[..index].to_owned(),
                    id: rest.to_owned(),
                };
            }
        }
        Self {
            prefix: String::new(),
            id: text.to_owned(),
        }
    }

    /// The id part: the last segment, or the parenthesised group.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The full string form.
    pub fn to_urn_string(&self) -> String {
        format!("{}:{}", self.prefix, self.id)
    }
}

impl std::fmt::Display for Urn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_urn_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_urn_splits_into_prefix_and_id() {
        let urn = Urn::parse("urn:li:fsd_profile:ACoAAB1");
        assert_eq!(urn.id(), "ACoAAB1");
        assert_eq!(urn.to_urn_string(), "urn:li:fsd_profile:ACoAAB1");
    }

    #[test]
    fn a_grouped_id_keeps_its_inner_colons() {
        let text = "urn:li:msg_message:(urn:li:fsd_profile:ACoAAB1,2-abc==)";
        let urn = Urn::parse(text);
        assert_eq!(urn.id(), "(urn:li:fsd_profile:ACoAAB1,2-abc==)");
        assert_eq!(urn.to_urn_string(), text);
    }

    #[test]
    fn a_participant_urn_yields_the_profile_id() {
        let urn = Urn::parse("urn:li:msg_messagingParticipant:urn:li:fsd_profile:ACoAAB1");
        assert_eq!(urn.id(), "ACoAAB1");
    }

    #[test]
    fn a_bare_string_is_an_id_without_prefix() {
        let urn = Urn::parse("12345");
        assert_eq!(urn.id(), "12345");
        assert_eq!(urn.to_urn_string(), ":12345");
    }
}
