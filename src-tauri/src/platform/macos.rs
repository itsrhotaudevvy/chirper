//! macOS. Touch ID or the device password (LocalAuthentication), trackpad haptics
//! (NSHapticFeedbackManager). The Dock badge and screen privacy use Tauri: its badge label is the
//! dock tile's `setBadgeLabel`, and its content protection is `NSWindow.sharingType`.

use std::sync::mpsc;

use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_app_kit::{
    NSHapticFeedbackManager, NSHapticFeedbackPattern, NSHapticFeedbackPerformanceTime,
    NSHapticFeedbackPerformer,
};
use objc2_foundation::{NSError, NSString};
use objc2_local_authentication::{LAContext, LAPolicy};
use tauri::AppHandle;

use super::{HapticKind, main_window};
use crate::error::{CmdError, CmdResult, code};

/// Whether LocalAuthentication can evaluate owner authentication (Touch ID or the device
/// password). It shows no prompt. The LAError code is logged, never the device details.
pub fn app_lock_supported() -> bool {
    // SAFETY: LAContext::new is a plain Objective-C allocation with no preconditions.
    let context = unsafe { LAContext::new() };
    // SAFETY: the policy is a valid LAPolicy constant and the context is live.
    let result = unsafe { context.canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthentication) };
    match result {
        Ok(()) => {
            log::debug!("LocalAuthentication can evaluate owner authentication");
            true
        }
        Err(error) => {
            log::debug!(
                "LocalAuthentication cannot evaluate owner authentication: {}",
                error.code()
            );
            false
        }
    }
}

/// Shows the Touch ID or device-password prompt and waits for its answer. `Ok(false)` means the
/// user cancelled or failed authentication. The reason is not logged.
pub fn authenticate(_app: &AppHandle, reason: &str) -> CmdResult<bool> {
    // SAFETY: see `app_lock_supported`.
    let context = unsafe { LAContext::new() };
    let reason = NSString::from_str(reason);
    // The reply block runs on a LocalAuthentication queue. A one-slot channel carries its answer
    // back to this blocking thread (the unlock path already runs on `spawn_blocking`).
    let (answer, result) = mpsc::sync_channel::<bool>(1);
    let reply = RcBlock::new(move |success: Bool, error: *mut NSError| {
        // SAFETY: the framework passes either null or a live NSError for the duration of the call.
        if let Some(error) = unsafe { error.as_ref() } {
            log::debug!("authentication did not succeed: LAError {}", error.code());
        }
        let _ = answer.send(success.as_bool());
    });
    // SAFETY: the policy, reason and reply block are valid for the call; the framework copies the
    // block, so the local `reply` may be dropped after the call returns.
    unsafe {
        context.evaluatePolicy_localizedReason_reply(
            LAPolicy::DeviceOwnerAuthentication,
            &reason,
            &reply,
        );
    }
    result.recv().map_err(|_| {
        CmdError::new(
            code::CORE,
            "the authentication prompt ended without a result",
        )
    })
}

/// Plays a trackpad haptic on the main thread. Selection uses the alignment pattern, the impact
/// kinds the level-change pattern and the notifications the generic pattern.
pub fn haptic(app: &AppHandle, kind: HapticKind) -> CmdResult<()> {
    app.run_on_main_thread(move || perform_haptic(kind))
        .map_err(|e| CmdError::new(code::CORE, e.to_string()))
}

fn perform_haptic(kind: HapticKind) {
    let pattern = match kind {
        HapticKind::Selection => NSHapticFeedbackPattern::Alignment,
        HapticKind::Light | HapticKind::Medium | HapticKind::Heavy => {
            NSHapticFeedbackPattern::LevelChange
        }
        HapticKind::Success | HapticKind::Warning | HapticKind::Error => {
            NSHapticFeedbackPattern::Generic
        }
    };
    let performer = NSHapticFeedbackManager::defaultPerformer();
    performer
        .performFeedbackPattern_performanceTime(pattern, NSHapticFeedbackPerformanceTime::Default);
}

/// Sets the Dock badge to the count, or clears it at zero.
pub fn set_badge(app: &AppHandle, count: i64) -> CmdResult<()> {
    let badge = (count > 0).then_some(count);
    main_window(app)?
        .set_badge_count(badge)
        .map_err(|e| CmdError::new(code::CORE, e.to_string()))
}

/// Excludes the window from screenshots and screen sharing (`NSWindowSharingNone`).
pub fn set_secure_screen(app: &AppHandle, enabled: bool) -> CmdResult<()> {
    main_window(app)?
        .set_content_protected(enabled)
        .map_err(|e| CmdError::new(code::CORE, e.to_string()))
}

/// The overlay titlebar and the background colour come from the config and `prepare_main_window`.
/// macOS has no separate titlebar theme to set.
pub fn style_titlebar(_window: &tauri::WebviewWindow) {}
