//! `chirper://` deep links. Links are forwarded to the webview as `deep-link` events, and the main
//! window is brought to the front. Links that arrive before the webview listens are queued.

use tauri::{AppHandle, Emitter, Manager, Url};
use tauri_plugin_deep_link::DeepLinkExt;

use crate::state::AppState;

pub const SCHEME: &str = "chirper";
pub const EVENT_DEEP_LINK: &str = "deep-link";
const MAX_URL_BYTES: usize = 2048;

/// Registers the open-URL listener and collects the link the app was launched with.
pub fn init(app: &AppHandle) {
    let handle = app.clone();
    app.deep_link().on_open_url(move |event| {
        for url in event.urls() {
            forward(&handle, &url);
        }
    });
    match app.deep_link().get_current() {
        Ok(Some(urls)) => {
            for url in urls {
                forward(app, &url);
            }
        }
        Ok(None) => {}
        Err(e) => log::warn!("cannot read the launch link: {e}"),
    }
}

/// Registers the scheme with the OS during development on Windows and Linux.
#[cfg(all(debug_assertions, any(windows, target_os = "linux")))]
pub fn register_dev_scheme(app: &AppHandle) {
    if let Err(e) = app.deep_link().register_all() {
        log::warn!("cannot register the chirper:// scheme: {e}");
    }
}

/// Sends the links queued before the webview was ready.
pub fn flush_pending(app: &AppHandle) {
    for link in app.state::<AppState>().take_pending_links() {
        emit(app, &link);
    }
}

/// Accepts only `chirper:` URLs of a sane length, and shows the window.
pub fn forward(app: &AppHandle, url: &Url) {
    if !is_chirper_link(url.as_str()) {
        log::debug!("ignoring a link with another scheme");
        return;
    }
    crate::show_main_window(app);
    let link = url.to_string();
    if let Some(link) = app.state::<AppState>().queue_or_take_link(link) {
        emit(app, &link);
    }
}

pub fn is_chirper_link(text: &str) -> bool {
    text.len() <= MAX_URL_BYTES
        && text
            .strip_prefix(SCHEME)
            .is_some_and(|rest| rest.starts_with(':'))
}

fn emit(app: &AppHandle, link: &str) {
    if let Err(e) = app.emit(EVENT_DEEP_LINK, serde_json::json!({ "url": link })) {
        log::warn!("cannot emit a deep link: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_chirper_links_pass() {
        assert!(is_chirper_link("chirper://chat/!abc:chirper.local"));
        assert!(!is_chirper_link("https://example.invalid/chirper://x"));
        assert!(!is_chirper_link("chirperx://a"));
        let long = format!("chirper://{}", "a".repeat(MAX_URL_BYTES));
        assert!(!is_chirper_link(&long));
    }
}
