//! Desktop tray icon: Open Chirper, a separator and Quit. A left click toggles the window. The
//! icon switches to the unread variant while the badge count is above zero. On macOS the icons are
//! template images so the system tints them.

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::error::{CmdError, CmdResult, code};
use crate::state::AppState;

pub const TRAY_ID: &str = "main";
/// Passed by the login item so that the app starts in the tray instead of showing the window.
pub const LAUNCH_HIDDEN_ARG: &str = "--hidden";
const MENU_OPEN: &str = "open";
const MENU_QUIT: &str = "quit";

fn tray_error(e: tauri::Error) -> CmdError {
    CmdError::new(code::CORE, e.to_string())
}

/// The tray image for the current state, at 64 px on macOS (retina) and 32 px elsewhere.
fn icon(unread: bool) -> CmdResult<Image<'static>> {
    let bytes: &[u8] = match (cfg!(target_os = "macos"), unread) {
        (true, false) => include_bytes!("../icons/tray-template@2x.png"),
        (true, true) => include_bytes!("../icons/tray-unread-template@2x.png"),
        (false, false) => include_bytes!("../icons/tray.png"),
        (false, true) => include_bytes!("../icons/tray-unread.png"),
    };
    Image::from_bytes(bytes).map_err(tray_error)
}

/// Creates the tray icon. Call once from `setup`.
pub fn install(app: &AppHandle) -> CmdResult<()> {
    let open = MenuItem::with_id(app, MENU_OPEN, "Open Chirper", true, None::<&str>)
        .map_err(tray_error)?;
    let separator = PredefinedMenuItem::separator(app).map_err(tray_error)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "Quit Chirper", true, None::<&str>)
        .map_err(tray_error)?;
    let menu = Menu::with_items(app, &[&open, &separator, &quit]).map_err(tray_error)?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon(false)?)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("Chirper")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_OPEN => crate::show_main_window(app),
            MENU_QUIT => exit_app(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_main_window(tray.app_handle());
            }
        })
        .build(app)
        .map_err(tray_error)?;
    // Only after the tray exists: a window hidden without a tray icon could not be shown again.
    if std::env::args().any(|arg| arg == LAUNCH_HIDDEN_ARG) {
        log::debug!("started by the login item: the window stays in the tray");
        hide_main_window(app);
    }
    Ok(())
}

/// Closes the core on a blocking thread, then exits. Closing the core is idempotent, so the exit
/// hook may call it again.
fn exit_app(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(state) = app.try_state::<AppState>() {
            state.close_core();
        }
        app.exit(0);
    });
}

/// Swaps the tray icon for the unread variant while `unread` is set.
pub fn set_unread(app: &AppHandle, unread: bool) -> CmdResult<()> {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return Ok(());
    };
    apply_icon(&tray, unread)
}

fn apply_icon(tray: &TrayIcon, unread: bool) -> CmdResult<()> {
    tray.set_icon(Some(icon(unread)?)).map_err(tray_error)?;
    tray.set_icon_as_template(cfg!(target_os = "macos"))
        .map_err(tray_error)
}

/// Hides the main window (close to tray) and records that it is not focused.
pub fn hide_main_window(app: &AppHandle) {
    if let Ok(window) = crate::platform::main_window(app)
        && let Err(e) = window.hide()
    {
        log::warn!("cannot hide the window: {e}");
    }
    app.state::<AppState>().set_window_focused(false);
}

fn toggle_main_window(app: &AppHandle) {
    let visible = crate::platform::main_window(app)
        .and_then(|w| w.is_visible().map_err(tray_error))
        .unwrap_or(false);
    if visible {
        hide_main_window(app);
    } else {
        crate::show_main_window(app);
    }
}
