//! Account sessions: sign-in, token refresh and sign-out (`com.atproto.server.*`).

use serde_json::{Value, json};

use super::types::AuthSession;
use super::{ApiError, Auth, Pds};

/// Sign in with a handle or e-mail address. `factor` answers a two-factor challenge.
pub async fn create_session(
    pds: &Pds,
    identifier: &str,
    password: &str,
    factor: Option<&str>,
) -> std::result::Result<AuthSession, ApiError> {
    let mut body = json!({"identifier": identifier, "password": password});
    if let Some(code) = factor {
        body["authFactorToken"] = Value::String(code.to_owned());
    }
    pds.procedure("com.atproto.server.createSession", Some(&body), Auth::NONE)
        .await
}

/// Exchange a refresh token for a new access and refresh token.
pub async fn refresh_session(
    pds: &Pds,
    refresh_jwt: &str,
) -> std::result::Result<AuthSession, ApiError> {
    pds.procedure(
        "com.atproto.server.refreshSession",
        None,
        Auth::bearer(refresh_jwt),
    )
    .await
}

/// End the session on the server.
pub async fn delete_session(pds: &Pds, access_jwt: &str) -> std::result::Result<(), ApiError> {
    pds.procedure::<()>(
        "com.atproto.server.deleteSession",
        None,
        Auth::bearer(access_jwt),
    )
    .await
}
