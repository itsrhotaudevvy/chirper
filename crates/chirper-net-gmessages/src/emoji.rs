//! The pairing emoji tables of the Google account login, copied from `libgm/pair_google.go` of the
//! mautrix Google Messages bridge (AGPL-3.0, Copyright (C) Tulir Asokan and contributors). The
//! phone shows one of these; the user taps the same one.

use std::collections::HashSet;
use std::sync::OnceLock;

/// The v0 table, in the order the phone uses it (duplicates included, as in the source).
const EMOJI_V0_RAW: &[&str] = &[
    "😁", "😅", "🤣", "🫠", "🥰", "😇", "🤩", "😘", "😜", "🤗", "🤔", "🤐", "😴", "🥶", "🤯", "🤠", "🥳", "🥸", "😎",
    "🤓", "🧐", "🥹", "😭", "😱", "😖", "🥱", "😮‍💨", "🤡", "💩", "👻", "👽", "🤖", "😻", "💌", "💘", "💕", "❤", "💢",
    "💥", "💫", "💬", "🗯", "💤", "👋", "🙌", "🙏", "✍", "🦶", "👂", "🧠", "🦴", "👀", "🧑", "🧚", "🧍", "👣", "🐵",
    "🐶", "🐺", "🦊", "🦁", "🐯", "🦓", "🦄", "🐑", "🐮", "🐷", "🐿", "🐰", "🦇", "🐻", "🐨", "🐼", "🦥", "🐾", "🐔",
    "🐥", "🐦", "🕊", "🦆", "🦉", "🪶", "🦩", "🐸", "🐢", "🦎", "🐍", "🐳", "🐬", "🦭", "🐠", "🐡", "🦈", "🪸", "🐌",
    "🦋", "🐛", "🐝", "🐞", "🪱", "💐", "🌸", "🌹", "🌻", "🌱", "🌲", "🌴", "🌵", "🌾", "☘", "🍁", "🍂", "🍄", "🪺",
    "🍇", "🍈", "🍉", "🍋", "🍌", "🍍", "🍎", "🍐", "🍒", "🍓", "🥝", "🥥", "🥑", "🥕", "🌽", "🌶", "🫑", "🥦", "🥜",
    "🍞", "🥐", "🥨", "🧀", "🍗", "🍔", "🍟", "🍕", "🌭", "🌮", "🥗", "🥣", "🍿", "🦀", "🦑", "🍦", "🍩", "🍪", "🍫",
    "🍰", "🍬", "🍭", "☕", "🫖", "🍹", "🥤", "🧊", "🥢", "🍽", "🥄", "🧭", "🏔", "🌋", "🏕", "🏖", "🪵", "🏗", "🏡",
    "🏰", "🛝", "🚂", "🛵", "🛴", "🛼", "🚥", "⚓", "🛟", "⛵", "✈", "🚀", "🛸", "🧳", "⏰", "🌙", "🌡", "🌞", "🪐",
    "🌠", "🌧", "🌀", "🌈", "☂", "⚡", "❄", "⛄", "🔥", "🎇", "🧨", "✨", "🎈", "🎉", "🎁", "🏆", "🏅", "⚽", "⚾",
    "🏀", "🏐", "🏈", "🎾", "🎳", "🏓", "🥊", "⛳", "⛸", "🎯", "🪁", "🔮", "🎮", "🧩", "🧸", "🪩", "🖼", "🎨", "🧵",
    "🧶", "🦺", "🧣", "🧤", "🧦", "🎒", "🩴", "👟", "👑", "👒", "🎩", "🧢", "💎", "🔔", "🎤", "📻", "🎷", "🪗", "🎸",
    "🎺", "🎻", "🥁", "📺", "🔋", "💻", "💿", "☎", "🕯", "💡", "📖", "📚", "📬", "✏", "✒", "🖌", "🖍", "📝", "💼",
    "📋", "📌", "📎", "🔑", "🔧", "🧲", "🪜", "🧬", "🔭", "🩹", "🩺", "🪞", "🛋", "🪑", "🛁", "🧹", "🧺", "🔱", "🏁",
    "🐪", "🐘", "🦃", "🍞", "🍜", "🍠", "🚘", "🤿", "🃏", "👕", "📸", "🏷", "✂", "🧪", "🚪", "🧴", "🧻", "🪣", "🧽",
    "🚸",
];

/// Emoji the v1 table adds to v0.
const EMOJI_V1_ADDED: &[&str] = &[
    "🍋‍🟩", "🐦‍🔥", "🐲", "🪅", "🦜", "🏺", "🗿", "🫐", "⛽", "🍱", "🥡", "🧋", "🍼", "📐",
];

/// Emoji the v1 table removes from v0.
const EMOJI_V1_REMOVED: &[&str] = &[
    "💻", "🤗", "💬", "👋", "😁", "😎", "😇", "🥰", "🤓", "🤩",
];

/// The v0 table with duplicates dropped, keeping the first occurrence.
fn v0() -> &'static [String] {
    static TABLE: OnceLock<Vec<String>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut seen = HashSet::new();
        EMOJI_V0_RAW
            .iter()
            .filter(|emoji| seen.insert(**emoji))
            .map(|emoji| (*emoji).to_owned())
            .collect()
    })
}

/// The v1 table: v0 without the removed emoji, then the added ones.
fn v1() -> &'static [String] {
    static TABLE: OnceLock<Vec<String>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let removed: HashSet<&str> = EMOJI_V1_REMOVED.iter().copied().collect();
        v0().iter()
            .filter(|emoji| !removed.contains(emoji.as_str()))
            .cloned()
            .chain(EMOJI_V1_ADDED.iter().map(|emoji| (*emoji).to_owned()))
            .collect()
    })
}

/// The emoji the phone shows for a verification code, given the version the server confirmed.
/// Version 0 indexes the raw v0 table (duplicates included), as the Go client does. `None` for a
/// version this driver does not know.
pub fn pairing_emoji(version: i32, auth_number: u32) -> Option<&'static str> {
    match version {
        0 => {
            let index = (auth_number as usize) % EMOJI_V0_RAW.len();
            EMOJI_V0_RAW.get(index).copied()
        }
        1 => {
            let table = v1();
            let index = (auth_number as usize) % table.len();
            table.get(index).map(String::as_str)
        }
        _ => None,
    }
}

/// The Noto emoji SVG of an emoji, the image the login step shows (`libgm.GetEmojiSVG`).
pub fn emoji_svg_url(emoji: &str) -> String {
    let parts: Vec<String> = emoji
        .chars()
        .map(|c| format!("{:x}", u32::from(c)))
        .collect();
    format!(
        "https://fonts.gstatic.com/s/e/notoemoji/latest/{}/emoji.svg",
        parts.join("_")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tables_have_the_sizes_the_go_client_computes() {
        // v0 deduplicated, v1 = v0 minus the removed names plus the added names.
        let unique_v0: HashSet<&str> = EMOJI_V0_RAW.iter().copied().collect();
        assert_eq!(v0().len(), unique_v0.len());
        assert_eq!(v1().len(), v0().len() - EMOJI_V1_REMOVED.len() + EMOJI_V1_ADDED.len());
    }

    #[test]
    fn a_code_picks_an_index_modulo_the_table() {
        assert_eq!(pairing_emoji(0, 0), Some(EMOJI_V0_RAW[0]));
        let n = EMOJI_V0_RAW.len() as u32;
        assert_eq!(pairing_emoji(0, n + 1), Some(EMOJI_V0_RAW[1]));
        assert_eq!(pairing_emoji(1, 0), Some(v1()[0].as_str()));
        assert_eq!(pairing_emoji(7, 0), None);
    }

    #[test]
    fn the_svg_url_joins_code_points_in_hex() {
        assert_eq!(
            emoji_svg_url("\u{1f601}"),
            "https://fonts.gstatic.com/s/e/notoemoji/latest/1f601/emoji.svg"
        );
    }
}
