//! Profiles (`app.bsky.actor.getProfiles`).

use serde::Deserialize;

use super::types::Profile;
use super::{ApiError, Auth, Pds};

/// Profiles fetched in one call; the lexicon allows 25.
pub const PROFILE_BATCH: usize = 25;

#[derive(Deserialize)]
struct Profiles {
    #[serde(default)]
    profiles: Vec<Profile>,
}

/// The profiles of up to [`PROFILE_BATCH`] DIDs. Unknown DIDs are simply absent from the answer.
pub async fn get_profiles(
    pds: &Pds,
    access_jwt: &str,
    dids: &[String],
) -> std::result::Result<Vec<Profile>, ApiError> {
    let params: Vec<(&str, String)> = dids.iter().map(|did| ("actors", did.clone())).collect();
    let reply: Profiles = pds
        .query(
            "app.bsky.actor.getProfiles",
            &params,
            Auth::bearer(access_jwt),
        )
        .await?;
    Ok(reply.profiles)
}
