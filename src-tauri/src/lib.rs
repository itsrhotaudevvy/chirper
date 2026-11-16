//! Chirper application shell (CONTRACT.md section C): Tauri setup, the command surface, core
//! event batching, the `chirper-media` protocol, app lock, preferences and platform integration.

mod commands;
mod deep_link;
mod error;
mod events;
mod lock;
mod logging;
mod media;
mod platform;
mod prefs;
mod startup;
mod state;
#[cfg(desktop)]
mod tray;

use tauri::{App, AppHandle, Manager, RunEvent, Window, WindowEvent};

use crate::state::AppState;

/// Brings the main window to the front. Failures are logged, never shown.
pub fn show_main_window(app: &AppHandle) {
    match platform::main_window(app) {
        Ok(window) => {
            if let Err(e) = bring_to_front(&window) {
                log::warn!("cannot show the main window: {e}");
            }
        }
        Err(e) => log::warn!("{}", e.message),
    }
}

fn bring_to_front(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    #[cfg(desktop)]
    window.unminimize()?;
    window.show()?;
    window.set_focus()
}

fn builder() -> tauri::Builder<tauri::Wry> {
    // The single-instance plugin must come first. A second launch focuses this window; its deep
    // link arrives through the deep-link plugin.
    let builder = tauri::Builder::default();
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
        show_main_window(app);
    }));
    let builder = builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init());
    #[cfg(desktop)]
    let builder = builder
        // Visibility is not restored: a window closed to the tray must not keep the app hidden.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::all()
                        .difference(tauri_plugin_window_state::StateFlags::VISIBLE),
                )
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![tray::LAUNCH_HIDDEN_ARG]),
        ));
    #[cfg(mobile)]
    let builder = builder
        .plugin(tauri_plugin_haptics::init())
        .plugin(tauri_plugin_biometric::init())
        .plugin(tauri_plugin_chirper_native::init());

    builder
        .register_asynchronous_uri_scheme_protocol(media::SCHEME, |ctx, request, responder| {
            media::handle(ctx, request, responder);
        })
        .invoke_handler(tauri::generate_handler![
            commands::start,
            commands::networks,
            commands::accounts,
            commands::start_login,
            commands::submit_login_input,
            commands::submit_login_cookies,
            commands::wait_login_step,
            commands::cancel_login,
            commands::logout,
            commands::list_chats,
            commands::get_chat,
            commands::list_messages,
            commands::load_older,
            commands::sync_history,
            commands::mark_unread,
            commands::send_text,
            commands::send_media,
            commands::edit,
            commands::redact,
            commands::react,
            commands::unreact,
            commands::mark_read,
            commands::set_typing,
            commands::set_chat_flags,
            commands::media_file,
            commands::search,
            commands::settings,
            commands::set_settings,
            commands::app_info,
            commands::haptic,
            commands::set_badge,
            commands::notify_message,
            commands::lock_status,
            commands::unlock,
            commands::set_secure_screen,
            commands::open_external,
            commands::open_login_webview,
            commands::set_app_prefs,
            commands::get_app_prefs,
            commands::pick_files,
        ])
        .on_window_event(on_window_event)
        .setup(setup)
}

fn setup(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = app.path().app_data_dir()?;
    let state = AppState::new(&data_dir)?;
    let prefs = state.prefs();
    app.manage(state);
    let handle = app.handle().clone();
    #[cfg(desktop)]
    platform::prepare_main_window(&handle);
    if let Ok(window) = platform::main_window(&handle) {
        let focused = window.is_focused().unwrap_or(false);
        handle.state::<AppState>().set_window_focused(focused);
    }

    if prefs.secure_screen
        && let Err(e) = platform::set_secure_screen(&handle, true)
    {
        log::warn!("cannot apply screen privacy at startup: {}", e.message);
    }
    deep_link::init(&handle);
    if let Err(e) = platform::connect_replies(&handle) {
        log::warn!("cannot connect notification replies: {}", e.message);
    }
    #[cfg(all(debug_assertions, any(windows, target_os = "linux")))]
    deep_link::register_dev_scheme(&handle);
    #[cfg(desktop)]
    if let Err(e) = tray::install(&handle) {
        log::warn!("no tray icon: {}", e.message);
    }
    startup::spawn(handle);
    Ok(())
}

fn on_window_event(window: &Window, event: &WindowEvent) {
    let app = window.app_handle();
    if app.try_state::<AppState>().is_none() {
        return;
    }
    match event {
        WindowEvent::Focused(focused) => {
            app.state::<AppState>().set_window_focused(*focused);
            if *focused {
                lock::foreground(app);
            } else {
                lock::background(app);
            }
        }
        #[cfg(desktop)]
        WindowEvent::CloseRequested { api, .. }
            if app.state::<AppState>().prefs().close_to_tray =>
        {
            api.prevent_close();
            tray::hide_main_window(app);
        }
        #[cfg(mobile)]
        WindowEvent::Suspended => lock::background(app),
        _ => {}
    }
}

fn on_run_event(app: &AppHandle, event: RunEvent) {
    match event {
        RunEvent::Exit => {
            if let Some(state) = app.try_state::<AppState>() {
                state.close_core();
            }
        }
        #[cfg(mobile)]
        RunEvent::Resumed => lock::foreground(app),
        #[cfg(target_os = "macos")]
        RunEvent::Reopen {
            has_visible_windows: false,
            ..
        } => show_main_window(app),
        _ => {}
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    logging::init();
    let app = builder()
        .build(tauri::generate_context!())
        .expect("error while building Chirper");
    app.run(on_run_event);
}
