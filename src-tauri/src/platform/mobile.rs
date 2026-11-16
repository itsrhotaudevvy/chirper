//! Haptics and biometric authentication on Android and iOS, through the Tauri plugins.

use tauri::AppHandle;
use tauri_plugin_biometric::{AuthOptions, BiometricExt};
use tauri_plugin_haptics::{HapticsExt, ImpactFeedbackStyle, NotificationFeedbackType};

use super::HapticKind;
use crate::error::{CmdError, CmdResult, code};

pub fn haptic(app: &AppHandle, kind: HapticKind) -> CmdResult<()> {
    let haptics = app.haptics();
    let result = match kind {
        HapticKind::Selection => haptics.selection_feedback(),
        HapticKind::Light => haptics.impact_feedback(ImpactFeedbackStyle::Light),
        HapticKind::Medium => haptics.impact_feedback(ImpactFeedbackStyle::Medium),
        HapticKind::Heavy => haptics.impact_feedback(ImpactFeedbackStyle::Heavy),
        HapticKind::Success => haptics.notification_feedback(NotificationFeedbackType::Success),
        HapticKind::Warning => haptics.notification_feedback(NotificationFeedbackType::Warning),
        HapticKind::Error => haptics.notification_feedback(NotificationFeedbackType::Error),
    };
    result.map_err(|e| CmdError::new(code::CORE, e.to_string()))
}

/// Shows the system prompt (biometrics, or the device passcode as fallback). `Ok(false)` means the
/// user did not authenticate; the reason is not logged because it can name the user's device.
pub fn authenticate(app: &AppHandle, reason: &str) -> CmdResult<bool> {
    let options = AuthOptions {
        allow_device_credential: true,
        cancel_title: Some("Cancel".to_owned()),
        ..AuthOptions::default()
    };
    match app.biometric().authenticate(reason.to_owned(), options) {
        Ok(()) => Ok(true),
        Err(e) => {
            log::debug!("authentication did not succeed: {e}");
            Ok(false)
        }
    }
}
