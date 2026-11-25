//! Errors of the client layer, and their mapping onto the bridge's error model.
//!
//! Telegram answers with an RPC error name (`PHONE_CODE_INVALID`, `FLOOD_WAIT_30`, ...). The
//! login and session code match on the name; everything else is mapped by [`From`] into
//! [`chirper_bridge::Error`]. Error strings carry the RPC name only, never the request or the
//! response bytes.

use chirper_bridge::Error;

use crate::mtproto::InvokeError;

/// RPC names that mean the saved credentials no longer work.
const AUTH_NAMES: &[&str] = &[
    "AUTH_KEY_UNREGISTERED",
    "AUTH_KEY_INVALID",
    "AUTH_KEY_PERM_EMPTY",
    "SESSION_REVOKED",
    "SESSION_EXPIRED",
    "USER_DEACTIVATED",
    "USER_DEACTIVATED_BAN",
];

/// A failed call to Telegram, before it is mapped for the bridge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallError {
    /// The server refused the call. `name` is Telegram's error string.
    Rpc { code: i32, name: String },
    /// `FLOOD_WAIT_X`: wait this many seconds.
    Flood { seconds: u32 },
    /// `*_MIGRATE_X`: the request belongs to data centre `dc`.
    Migrate { dc: i32 },
    /// The transport failed, or no answer came in time.
    Network(String),
    /// The server sent something this client cannot use.
    Protocol(String),
}

impl CallError {
    /// True when the RPC error is one of Telegram's "credentials are dead" names.
    pub fn is_auth(&self) -> bool {
        matches!(self, Self::Rpc { name, .. } if is_auth_name(name))
    }

    /// True when the RPC error has this exact name.
    pub fn is_named(&self, wanted: &str) -> bool {
        matches!(self, Self::Rpc { name, .. } if name == wanted)
    }
}

/// True for the RPC names that mean the account is logged out or deactivated.
pub fn is_auth_name(name: &str) -> bool {
    AUTH_NAMES.contains(&name)
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rpc { code, name } => write!(f, "Telegram error {code} {name}"),
            Self::Flood { seconds } => write!(f, "Telegram asked to wait {seconds} s"),
            Self::Migrate { dc } => write!(f, "account is on data centre {dc}"),
            Self::Network(message) | Self::Protocol(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for CallError {}

impl From<InvokeError> for CallError {
    fn from(error: InvokeError) -> Self {
        match error {
            InvokeError::Transport(e) => Self::Network(e.to_string()),
            InvokeError::Crypto(e) => Self::Protocol(e.to_string()),
            InvokeError::Deserialize(e) => Self::Protocol(e.to_string()),
            InvokeError::Protocol(message) => Self::Protocol(message.to_owned()),
            InvokeError::Rpc { code, message } => Self::Rpc {
                code,
                name: message,
            },
            InvokeError::RateLimited { seconds } => Self::Flood { seconds },
            InvokeError::Migrate { dc } => Self::Migrate { dc },
            InvokeError::Closed => Self::Network("the connection closed".to_owned()),
            InvokeError::Timeout => Self::Network("no answer from Telegram in time".to_owned()),
        }
    }
}

impl From<CallError> for Error {
    fn from(error: CallError) -> Self {
        match error {
            CallError::Flood { seconds } => Error::RateLimited {
                retry_after_ms: u64::from(seconds) * 1000,
            },
            CallError::Rpc { name, .. } if is_auth_name(&name) => Error::Auth(name),
            CallError::Rpc { code, name } if code >= 500 => {
                Error::Network(format!("Telegram error {code} {name}"))
            }
            CallError::Rpc { code, name } => {
                Error::Protocol(format!("Telegram error {code} {name}"))
            }
            CallError::Migrate { dc } => {
                Error::Network(format!("account moved to data centre {dc}"))
            }
            CallError::Network(message) => Error::Network(message),
            CallError::Protocol(message) => Error::Protocol(message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_names_map_to_the_bridge_model() {
        let auth: Error = CallError::Rpc {
            code: 401,
            name: "SESSION_REVOKED".into(),
        }
        .into();
        assert!(matches!(auth, Error::Auth(_)));
        let flood: Error = CallError::Flood { seconds: 30 }.into();
        assert!(matches!(
            flood,
            Error::RateLimited {
                retry_after_ms: 30_000
            }
        ));
        let server: Error = CallError::Rpc {
            code: 500,
            name: "INTERNAL".into(),
        }
        .into();
        assert!(matches!(server, Error::Network(_)));
        let other: Error = CallError::Rpc {
            code: 400,
            name: "CHAT_WRITE_FORBIDDEN".into(),
        }
        .into();
        assert!(matches!(other, Error::Protocol(_)));
    }

    #[test]
    fn invoke_errors_keep_the_rpc_name() {
        let mapped = CallError::from(InvokeError::Rpc {
            code: 400,
            message: "PHONE_CODE_INVALID".into(),
        });
        assert!(mapped.is_named("PHONE_CODE_INVALID"));
        assert!(!mapped.is_auth());
    }
}
