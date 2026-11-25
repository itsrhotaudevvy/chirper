//! Slack's remote ids, formatted exactly as the Go bridge formats them.
//!
//! Source: `third_party/slack/pkg/slackid/id.go` and `mediaid.go` (mautrix-slack, AGPL-3.0).
//! Databases written by the Go build are keyed by these strings, so every format here is pinned by
//! a test. A portal is `TEAM-CHANNEL` (or `TEAM` for the workspace space), a message is
//! `TEAM-CHANNEL-TS`, a user is `team-user` in lower case, and a login is `TEAM-USER`.

/// The portal id of a workspace: its team id.
pub fn team_portal_id(team: &str) -> String {
    team.to_owned()
}

/// The portal id of a channel, conversation or the workspace itself when `channel` is empty.
pub fn portal_id(team: &str, channel: &str) -> String {
    if channel.is_empty() {
        team_portal_id(team)
    } else {
        format!("{team}-{channel}")
    }
}

/// Split a portal id into team and channel. A workspace portal has an empty channel.
pub fn parse_portal_id(id: &str) -> Option<(&str, &str)> {
    match id.split('-').collect::<Vec<_>>().as_slice() {
        [team] => Some((team, "")),
        [team, channel] => Some((team, channel)),
        _ => None,
    }
}

/// The message id of a Slack message: team, channel and timestamp joined by dashes.
pub fn message_id(team: &str, channel: &str, timestamp: &str) -> String {
    format!("{team}-{channel}-{timestamp}")
}

/// Split a message id into team, channel and timestamp.
pub fn parse_message_id(id: &str) -> Option<(&str, &str, &str)> {
    match id.split('-').collect::<Vec<_>>().as_slice() {
        [team, channel, timestamp] => Some((team, channel, timestamp)),
        _ => None,
    }
}

/// The sender id of a Slack user or bot: the team and the id, both lower case.
pub fn user_id(team: &str, user: &str) -> String {
    format!(
        "{}-{}",
        team.to_ascii_lowercase(),
        user.to_ascii_lowercase()
    )
}

/// The login id of a Slack account: team and user, as they are written.
pub fn user_login_id(team: &str, user: &str) -> String {
    format!("{team}-{user}")
}

/// Split a login id into team and user.
pub fn parse_user_login_id(id: &str) -> Option<(&str, &str)> {
    match id.split('-').collect::<Vec<_>>().as_slice() {
        [team, user] => Some((team, user)),
        _ => None,
    }
}

/// The id of one part of a multi-part message: `file-INDEX-FILEID` or `attachment-INDEX-ID`.
pub fn part_id(kind: &str, index: usize, id: &str) -> String {
    format!("{kind}-{index}-{id}")
}

/// The portal receiver of a chat: direct messages and group DMs are per login, channels are shared.
pub fn portal_receiver(is_dm: bool, login_id: &str) -> String {
    if is_dm {
        login_id.to_owned()
    } else {
        String::new()
    }
}

/// A Slack timestamp (`seconds.micros`) in Unix milliseconds. Unparseable input gives 0.
pub fn timestamp_ms(ts: &str) -> i64 {
    let (seconds, fraction) = ts.split_once('.').unwrap_or((ts, ""));
    let Ok(seconds) = seconds.parse::<i64>() else {
        return 0;
    };
    let millis: i64 = fraction
        .get(..fraction.len().min(3))
        .filter(|digits| digits.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|digits| format!("{digits:0<3}").parse().ok())
        .unwrap_or(0);
    seconds.saturating_mul(1000).saturating_add(millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_ids_match_the_go_bridge() {
        assert_eq!(portal_id("T1", "C2"), "T1-C2");
        assert_eq!(portal_id("T1", ""), "T1");
        assert_eq!(parse_portal_id("T1-C2"), Some(("T1", "C2")));
        assert_eq!(parse_portal_id("T1"), Some(("T1", "")));
        assert_eq!(parse_portal_id("T1-C2-X"), None);
    }

    #[test]
    fn message_ids_match_the_go_bridge() {
        let id = message_id("T1", "C2", "1700000000.000100");
        assert_eq!(id, "T1-C2-1700000000.000100");
        assert_eq!(
            parse_message_id(&id),
            Some(("T1", "C2", "1700000000.000100"))
        );
        assert_eq!(parse_message_id("T1-C2"), None);
    }

    #[test]
    fn user_ids_are_lower_case_and_login_ids_are_not() {
        assert_eq!(user_id("T0ABC", "U1XYZ"), "t0abc-u1xyz");
        assert_eq!(user_login_id("T0ABC", "U1XYZ"), "T0ABC-U1XYZ");
        assert_eq!(parse_user_login_id("T0ABC-U1XYZ"), Some(("T0ABC", "U1XYZ")));
        assert_eq!(parse_user_login_id("T0ABC"), None);
    }

    #[test]
    fn part_ids_match_the_go_bridge() {
        assert_eq!(part_id("file", 0, "F9"), "file-0-F9");
        assert_eq!(part_id("attachment", 3, "77"), "attachment-3-77");
    }

    #[test]
    fn dms_are_per_login_and_channels_are_shared() {
        assert_eq!(portal_receiver(true, "T1-U1"), "T1-U1");
        assert_eq!(portal_receiver(false, "T1-U1"), "");
    }

    #[test]
    fn slack_timestamps_convert_to_milliseconds() {
        assert_eq!(timestamp_ms("1700000000.123456"), 1_700_000_000_123);
        assert_eq!(timestamp_ms("1700000000"), 1_700_000_000_000);
        assert_eq!(timestamp_ms("1700000000.5"), 1_700_000_000_500);
        assert_eq!(timestamp_ms("nope"), 0);
        assert_eq!(timestamp_ms("1700000000.1x"), 1_700_000_000_000);
    }
}
