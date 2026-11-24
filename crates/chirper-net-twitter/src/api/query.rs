//! Form and query-string encoding that matches the reference's `go-querystring` output: keys are
//! sorted, values keep their insertion order, and `url.QueryEscape` spelling is used.

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

/// Everything `QueryEscape` escapes: all but ASCII letters, digits and `-_.~`.
const ESCAPE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// An ordered set of query or form pairs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pairs: Vec<(String, String)>,
}

impl Query {
    /// An empty query.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a pair. Keys may repeat; they are grouped and sorted on encoding.
    pub fn push(&mut self, key: &str, value: impl ToString) -> &mut Self {
        self.pairs.push((key.to_owned(), value.to_string()));
        self
    }

    /// Append a pair only when the value is not empty, like `omitempty`.
    pub fn push_some(&mut self, key: &str, value: &str) -> &mut Self {
        if !value.is_empty() {
            self.push(key, value);
        }
        self
    }

    /// The encoded query string, keys sorted, without a leading `?`.
    pub fn encode(&self) -> String {
        let mut sorted: Vec<&(String, String)> = self.pairs.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        sorted
            .iter()
            .map(|(k, v)| format!("{}={}", escape(k), escape(v)))
            .collect::<Vec<_>>()
            .join("&")
    }
}

/// `url.QueryEscape`: spaces become `+`, everything outside the unreserved set is `%XX`.
pub fn escape(value: &str) -> String {
    utf8_percent_encode(value, ESCAPE)
        .to_string()
        .replace("%20", "+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_sorted_and_values_escaped() {
        let mut query = Query::new();
        query.push("b", "x y").push("a", 1).push("c", "a&b=c");
        assert_eq!(query.encode(), "a=1&b=x+y&c=a%26b%3Dc");
    }

    #[test]
    fn repeated_keys_keep_their_order() {
        let mut query = Query::new();
        query.push("k", "2").push("k", "1");
        assert_eq!(query.encode(), "k=2&k=1");
    }

    #[test]
    fn empty_optional_values_are_skipped() {
        let mut query = Query::new();
        query.push_some("cursor", "").push_some("max_id", "42");
        assert_eq!(query.encode(), "max_id=42");
    }

    #[test]
    fn unreserved_characters_stay_plain() {
        assert_eq!(escape("a-b_c.d~e"), "a-b_c.d~e");
    }
}
