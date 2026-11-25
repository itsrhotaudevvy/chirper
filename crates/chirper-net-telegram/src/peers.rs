//! The peer cache in use: remembering access hashes and names from what the server sends, and
//! turning a remote id back into the input peer an API call needs.

use chirper_bridge::{Error, RemoteUser, Result};

use crate::ids::{PeerKind, Portal};
use crate::store::{PeerEntry, Store};
use crate::tl::{enums, types};

/// Cache key of a user.
pub fn user_key(id: i64) -> String {
    format!("u:{id}")
}

/// Cache key of a channel.
pub fn channel_key(id: i64) -> String {
    format!("c:{id}")
}

/// The display name of a user: first and last name, else the username, else the id.
pub fn display_name(
    first: Option<&str>,
    last: Option<&str>,
    username: Option<&str>,
    id: i64,
) -> String {
    let full = [first, last]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if !full.is_empty() {
        return full;
    }
    username.map_or_else(|| id.to_string(), str::to_owned)
}

/// [`display_name`] of a user object.
pub fn user_name(user: &types::User) -> String {
    display_name(
        user.first_name.as_deref(),
        user.last_name.as_deref(),
        user.username.as_deref(),
        user.id,
    )
}

/// Remember a user (access hash and minimal name).
pub fn remember_user(store: &Store, user: &types::User) {
    let Some(access_hash) = user.access_hash else {
        return;
    };
    store.remember_peer(
        user_key(user.id),
        PeerEntry {
            access_hash,
            name: user_name(user),
            username: user.username.clone().unwrap_or_default(),
        },
    );
}

/// Remember a chat or channel. Basic groups carry no access hash.
pub fn remember_chat(store: &Store, chat: &enums::Chat) {
    if let enums::Chat::Channel(channel) = chat
        && let Some(access_hash) = channel.access_hash
    {
        store.remember_peer(
            channel_key(channel.id),
            PeerEntry {
                access_hash,
                name: channel.title.clone(),
                username: channel.username.clone().unwrap_or_default(),
            },
        );
    }
}

/// The remote user of a cached user id, or a placeholder carrying only the id.
pub fn remote_user(store: &Store, id: i64) -> RemoteUser {
    match store.peer(&user_key(id)) {
        Some(entry) => RemoteUser {
            id: crate::ids::user_id(id),
            display_name: entry.name,
            username: entry.username,
            ..RemoteUser::default()
        },
        None => RemoteUser {
            id: crate::ids::user_id(id),
            display_name: id.to_string(),
            ..RemoteUser::default()
        },
    }
}

/// The cached display name of a user, or the id when unknown.
pub fn display_name_of(store: &Store, id: i64) -> String {
    store
        .peer(&user_key(id))
        .map_or_else(|| id.to_string(), |entry| entry.name)
}

/// The remote user of a channel posting as itself.
pub fn remote_channel_user(store: &Store, id: i64) -> RemoteUser {
    let name = store
        .peer(&channel_key(id))
        .map(|entry| entry.name)
        .unwrap_or_else(|| id.to_string());
    RemoteUser {
        id: crate::ids::channel_user_id(id),
        display_name: name,
        ..RemoteUser::default()
    }
}

/// The access hash of a cached channel, if known.
pub fn channel_access(store: &Store, channel: i64) -> Option<i64> {
    store
        .peer(&channel_key(channel))
        .map(|entry| entry.access_hash)
}

/// The input peer of a portal: what `messages.*` calls take.
pub fn input_peer(store: &Store, portal: &Portal) -> Result<enums::InputPeer> {
    match portal.kind {
        PeerKind::User => {
            let entry = store.peer(&user_key(portal.id)).ok_or_else(|| {
                Error::Protocol("this person is not known to the account yet".into())
            })?;
            Ok(enums::InputPeer::InputPeerUser(types::InputPeerUser {
                user_id: portal.id,
                access_hash: entry.access_hash,
            }))
        }
        PeerKind::Chat => Ok(enums::InputPeer::InputPeerChat(types::InputPeerChat {
            chat_id: portal.id,
        })),
        PeerKind::Channel => Ok(enums::InputPeer::InputPeerChannel(
            types::InputPeerChannel {
                channel_id: portal.id,
                access_hash: channel_access(store, portal.id).ok_or_else(|| {
                    Error::Protocol("this channel is not known to the account yet".into())
                })?,
            },
        )),
    }
}

/// The input channel of a channel id, for the `channels.*` calls.
pub fn input_channel(store: &Store, channel: i64) -> Result<enums::InputChannel> {
    let access_hash = channel_access(store, channel)
        .ok_or_else(|| Error::Protocol("this channel is not known to the account yet".into()))?;
    Ok(enums::InputChannel::InputChannel(types::InputChannel {
        channel_id: channel,
        access_hash,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_fall_back_from_full_name_to_username_to_id() {
        assert_eq!(
            display_name(Some(" Ada "), Some("Lovelace"), None, 5),
            "Ada Lovelace"
        );
        assert_eq!(display_name(None, Some(""), Some("ada"), 5), "ada");
        assert_eq!(display_name(None, None, None, 5), "5");
    }
}
