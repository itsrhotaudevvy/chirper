//! What the driver remembers about the workspace: its users, bots and conversations, by Slack id.
//!
//! Names are needed where a message is rendered (a mention shows the person's name), where a chat
//! is named (a DM shows the other person), and where a portal receiver is chosen (DMs are per
//! login). The cache is filled from `users.list` at connect time, and misses are fetched one by one
//! with `users.info` or `bots.info`.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use log::debug;

use chirper_bridge::model::RemoteUser;

use crate::api::Api;
use crate::ids;
use crate::mrkdwn::Lookup;
use crate::wire::{Bot, User};

/// What the driver knows about one conversation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Conversation {
    /// The display name of the conversation, as Slack writes it.
    pub name: String,
    /// Whether the conversation is a DM or a group DM.
    pub is_dm: bool,
}

/// The users and conversations of one workspace.
#[derive(Default)]
pub struct Directory {
    users: Mutex<HashMap<String, RemoteUser>>,
    conversations: Mutex<HashMap<String, Conversation>>,
}

impl Directory {
    /// The user with this Slack id (a user `U…` or a bot `B…`), when known.
    pub fn user(&self, slack_id: &str) -> Option<RemoteUser> {
        lock(&self.users).and_then(|users| users.get(slack_id).cloned())
    }

    /// Store the user with this Slack id.
    pub fn remember_user(&self, slack_id: &str, user: RemoteUser) {
        if let Some(mut users) = lock(&self.users) {
            users.insert(slack_id.to_owned(), user);
        }
    }

    /// The conversation with this id, when known.
    pub fn conversation(&self, id: &str) -> Option<Conversation> {
        lock(&self.conversations).and_then(|map| map.get(id).cloned())
    }

    /// Store the conversation with this id.
    pub fn remember_conversation(&self, id: &str, conversation: Conversation) {
        if let Some(mut map) = lock(&self.conversations) {
            map.insert(id.to_owned(), conversation);
        }
    }

    /// Whether a conversation is a DM. Unknown conversations are DMs when their id starts with `D`,
    /// which is how Slack numbers them.
    pub fn is_dm(&self, id: &str) -> bool {
        self.conversation(id)
            .map_or_else(|| id.starts_with('D'), |conversation| conversation.is_dm)
    }
}

impl Lookup for Directory {
    fn user_name(&self, id: &str) -> Option<String> {
        self.user(id)
            .map(|user| user.display_name)
            .filter(|name| !name.is_empty())
    }

    fn channel_name(&self, id: &str) -> Option<String> {
        self.conversation(id)
            .map(|conversation| conversation.name)
            .filter(|name| !name.is_empty())
    }
}

fn lock<T>(mutex: &Mutex<T>) -> Option<MutexGuard<'_, T>> {
    mutex.lock().ok()
}

/// The remote identity of a user. The display name follows the default template of the Go bridge:
/// display name, then real name, then the account name, with ` (bot)` for app users.
pub fn remote_user(team: &str, slack_id: &str, user: &User) -> RemoteUser {
    let base = [
        &user.profile.display_name,
        &user.profile.real_name,
        &user.name,
    ]
    .into_iter()
    .find(|name| !name.is_empty())
    .cloned()
    .unwrap_or_else(|| slack_id.to_owned());
    let display_name = if user.is_bot || user.is_app_user {
        format!("{base} (bot)")
    } else {
        base
    };
    RemoteUser {
        id: ids::user_id(team, slack_id),
        display_name,
        avatar_url: user.profile.image_512.clone(),
        username: user.name.clone(),
        phone: String::new(),
    }
}

/// The remote identity of a bot.
pub fn bot_user(team: &str, slack_id: &str, bot: &Bot) -> RemoteUser {
    let name = if bot.name.is_empty() {
        slack_id.to_owned()
    } else {
        bot.name.clone()
    };
    RemoteUser {
        id: ids::user_id(team, slack_id),
        display_name: format!("{name} (bot)"),
        avatar_url: String::new(),
        username: bot.name.clone(),
        phone: String::new(),
    }
}

/// The identity of a Slack id the driver has not resolved. It shows the raw id.
pub fn placeholder(team: &str, slack_id: &str) -> RemoteUser {
    RemoteUser {
        id: ids::user_id(team, slack_id),
        display_name: slack_id.to_owned(),
        ..RemoteUser::default()
    }
}

/// Store every member of a `users.list` page.
pub fn load_members(directory: &Directory, team: &str, members: &[User]) {
    for member in members {
        if member.id.is_empty() {
            continue;
        }
        directory.remember_user(&member.id, remote_user(team, &member.id, member));
    }
}

/// The identity of a user or bot: from the cache, or fetched once and cached. A failed lookup gives
/// the placeholder and is not cached, so it is tried again later.
pub async fn resolve(api: &Api, directory: &Directory, team: &str, slack_id: &str) -> RemoteUser {
    if let Some(user) = directory.user(slack_id) {
        return user;
    }
    let fetched = if slack_id.starts_with('B') {
        api.bot_info(slack_id)
            .await
            .map(|bot| bot_user(team, slack_id, &bot))
    } else {
        api.user_info(slack_id)
            .await
            .map(|user| remote_user(team, slack_id, &user))
    };
    match fetched {
        Ok(user) => {
            directory.remember_user(slack_id, user.clone());
            user
        }
        Err(err) => {
            debug!("slack: a user could not be resolved: {err}");
            placeholder(team, slack_id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(id: &str, display: &str, real: &str, name: &str) -> User {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "name": name,
            "real_name": real,
            "profile": {"display_name": display, "real_name": real, "image_512": "https://img/a.png"}
        }))
        .expect("fixture parses")
    }

    #[test]
    fn display_names_prefer_the_display_name_then_the_real_name() {
        let ada = remote_user("T1", "U1", &member("U1", "ada", "Ada L", "ada.l"));
        assert_eq!(ada.id, "t1-u1");
        assert_eq!(ada.display_name, "ada");
        let grace = remote_user("T1", "U2", &member("U2", "", "Grace", "g"));
        assert_eq!(grace.display_name, "Grace");
        let nameless = remote_user("T1", "U3", &member("U3", "", "", ""));
        assert_eq!(nameless.display_name, "U3");
    }

    #[test]
    fn bots_are_marked() {
        let bot: Bot = serde_json::from_value(serde_json::json!({"id": "B9", "name": "deploy"}))
            .expect("fixture parses");
        let user = bot_user("T1", "B9", &bot);
        assert_eq!(user.id, "t1-b9");
        assert_eq!(user.display_name, "deploy (bot)");
    }

    #[test]
    fn a_dm_is_known_by_its_cache_entry_or_its_id() {
        let directory = Directory::default();
        assert!(directory.is_dm("D123"));
        assert!(!directory.is_dm("C123"));
        directory.remember_conversation(
            "G1",
            Conversation {
                name: "ada, grace".to_owned(),
                is_dm: true,
            },
        );
        assert!(directory.is_dm("G1"));
        assert_eq!(directory.channel_name("G1").as_deref(), Some("ada, grace"));
    }

    #[test]
    fn names_for_mentions_come_from_the_cache() {
        let directory = Directory::default();
        directory.remember_user("U1", remote_user("T1", "U1", &member("U1", "ada", "", "")));
        assert_eq!(directory.user_name("U1").as_deref(), Some("ada"));
        assert_eq!(directory.user_name("U2"), None);
    }
}
