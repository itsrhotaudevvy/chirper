//! The HTML subset Chirper shows (`b i u s code pre a blockquote br`) and the escaping around it.

/// Escape text for an HTML text node or a double-quoted attribute.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// Undo the entities Slack uses in message text (`&amp;`, `&lt;`, `&gt;`) and the ones [`escape`]
/// writes. Any other `&` is kept as it is.
pub fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find('&') {
        out.push_str(&rest[..index]);
        rest = &rest[index..];
        let decoded = [
            ("&amp;", '&'),
            ("&lt;", '<'),
            ("&gt;", '>'),
            ("&quot;", '"'),
            ("&#39;", '\''),
        ]
        .iter()
        .find_map(|(entity, c)| rest.starts_with(entity).then_some((entity.len(), *c)));
        match decoded {
            Some((len, c)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The plain text of a piece of HTML this module wrote: tags are dropped, `br` becomes a newline.
pub fn to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&decode_entities(&rest[..start]));
        let Some(end) = rest[start..].find('>') else {
            break;
        };
        let tag = rest[start + 1..start + end].trim_start_matches('/');
        let name = tag.split_whitespace().next().unwrap_or_default();
        if name.eq_ignore_ascii_case("br") {
            out.push('\n');
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(&decode_entities(rest));
    out
}

/// A Slack timestamp shown as `YYYY-MM-DD HH:MM UTC`, for dates embedded in messages.
pub fn format_timestamp(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let secs_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60
    )
}

/// The proleptic Gregorian date of a day count since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_and_decode_are_inverses_for_the_html_specials() {
        let raw = r#"a & b < c > d "e" 'f'"#;
        assert_eq!(decode_entities(&escape(raw)), raw);
        assert_eq!(escape("<b>"), "&lt;b&gt;");
    }

    #[test]
    fn decode_keeps_unknown_ampersands() {
        assert_eq!(
            decode_entities("fish &amp; chips &copy; &"),
            "fish & chips &copy; &"
        );
    }

    #[test]
    fn to_text_drops_tags_and_keeps_breaks() {
        assert_eq!(to_text("<b>hi</b><br>&lt;there&gt;"), "hi\n<there>");
        assert_eq!(to_text("<a href=\"x\">link</a>"), "link");
    }

    #[test]
    fn timestamps_format_as_utc() {
        assert_eq!(format_timestamp(0), "1970-01-01 00:00 UTC");
        assert_eq!(format_timestamp(1_700_000_000), "2023-11-14 22:13 UTC");
        assert_eq!(format_timestamp(951_782_400), "2000-02-29 00:00 UTC");
    }
}
