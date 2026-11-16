//! Windows. Windows Hello for the app lock, the taskbar overlay badge, and a titlebar that follows
//! the theme. Screen privacy is Tauri's `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`.
//! Windows has no haptics, so they are a no-op.

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::null_mut;

use tauri::{AppHandle, Theme, WebviewWindow};
use windows::Security::Credentials::UI::{
    UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability,
};
use windows::Win32::Foundation::{HWND, TRUE};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject, HBITMAP,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::WinRT::{
    IUserConsentVerifierInterop, RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize,
};
use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, DestroyIcon, HICON, ICONINFO};
use windows::core::{BOOL, HSTRING, PCWSTR, Result as WinResult, factory, w};
use windows_future::IAsyncOperation;

use super::{HapticKind, main_window};
use crate::error::{CmdError, CmdResult, code};

const DOT_SIZE: i32 = 16;
/// Radius of the badge dot in pixels, inside the 16 px overlay.
const DOT_RADIUS: f32 = 7.0;
/// The badge colour #F372AB as BGR bytes, in the order a 32-bit DIB stores them.
const DOT_BGR: [u8; 3] = [0xAB, 0x72, 0xF3];

/// Runs WinRT work on a fresh multithreaded apartment. Windows Hello answers asynchronously and
/// may need a message loop, so waiting for it on the window thread could stall the UI.
fn on_winrt_thread<T, F>(work: F) -> Result<T, String>
where
    T: Send,
    F: FnOnce() -> WinResult<T> + Send,
{
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                // SAFETY: a fresh thread has no apartment yet. A failed init means none was
                // joined, so only a successful init is balanced by the uninit below.
                let initialized = unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.is_ok();
                let result = work().map_err(|e| e.to_string());
                if initialized {
                    // SAFETY: balances the successful RoInitialize above on this thread.
                    unsafe { RoUninitialize() };
                }
                result
            })
            .join()
            .unwrap_or_else(|_| Err("the Windows Hello worker panicked".to_owned()))
    })
}

/// Whether Windows Hello can verify this user. Its answer is logged at debug level.
pub fn app_lock_supported() -> bool {
    let availability = on_winrt_thread(|| UserConsentVerifier::CheckAvailabilityAsync()?.join());
    let supported = matches!(availability, Ok(UserConsentVerifierAvailability::Available));
    match availability {
        Ok(state) => log::debug!("Windows Hello availability: {}", state.0),
        Err(e) => log::debug!("Windows Hello availability check failed: {e}"),
    }
    supported
}

/// Shows the Windows Hello prompt parented to the main window. `Ok(false)` means the user did not
/// verify (cancelled, unavailable or failed). The reason is not logged.
pub fn authenticate(app: &AppHandle, reason: &str) -> CmdResult<bool> {
    let hwnd = window_handle(&main_window(app)?)?;
    let message = reason.to_owned();
    let verified = on_winrt_thread(move || {
        let interop = factory::<UserConsentVerifier, IUserConsentVerifierInterop>()?;
        let message = HSTRING::from(message);
        // SAFETY: the window handle belongs to the live main window and the message is a valid
        // HSTRING for the call.
        let operation: IAsyncOperation<UserConsentVerificationResult> = unsafe {
            interop.RequestVerificationForWindowAsync(HWND(hwnd as *mut c_void), &message)?
        };
        Ok(operation.join()? == UserConsentVerificationResult::Verified)
    });
    match verified {
        Ok(verified) => Ok(verified),
        Err(e) => {
            log::debug!("Windows Hello did not verify the user: {e}");
            Ok(false)
        }
    }
}

/// Haptics do not exist on Windows; the contract makes them a no-op.
pub fn haptic(_app: &AppHandle, _kind: HapticKind) -> CmdResult<()> {
    Ok(())
}

/// Shows a dot on the taskbar icon while `count` is above zero. The dot is not numeric, so the
/// count itself is only conveyed by the tray and the window title bar.
pub fn set_badge(app: &AppHandle, count: i64) -> CmdResult<()> {
    let hwnd = window_handle(&main_window(app)?)?;
    let visible = count > 0;
    // The taskbar work needs the window's apartment, so it runs on the main thread. The result
    // cannot be returned from there; failures are logged instead.
    app.run_on_main_thread(move || {
        if let Err(e) = apply_overlay(HWND(hwnd as *mut c_void), visible) {
            log::warn!("cannot update the taskbar badge: {e}");
        }
    })
    .map_err(|e| CmdError::new(code::CORE, e.to_string()))
}

fn apply_overlay(hwnd: HWND, visible: bool) -> WinResult<()> {
    // SAFETY: COM was initialised on the main thread by the windowing library; the class id is
    // the shell's TaskbarList.
    let taskbar: ITaskbarList3 =
        unsafe { CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER)? };
    // SAFETY: HrInit must run once before any other taskbar call on a new instance.
    unsafe { taskbar.HrInit()? };
    if !visible {
        // SAFETY: a null icon removes the overlay.
        return unsafe { taskbar.SetOverlayIcon(hwnd, HICON::default(), PCWSTR::null()) };
    }
    let icon = dot_icon()?;
    // SAFETY: the taskbar copies the icon, so it is destroyed right after the call.
    let result = unsafe { taskbar.SetOverlayIcon(hwnd, icon, w!("Unread messages")) };
    // SAFETY: the icon was created by `dot_icon` and is not used afterwards.
    let _ = unsafe { DestroyIcon(icon) };
    result
}

/// A 16 px icon with an anti-aliased filled dot in the badge colour, on a transparent background.
fn dot_icon() -> WinResult<HICON> {
    let header = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: DOT_SIZE,
        // A negative height stores the rows top-down.
        biHeight: -DOT_SIZE,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    };
    let info = BITMAPINFO {
        bmiHeader: header,
        ..Default::default()
    };
    let mut bits: *mut c_void = null_mut();
    // SAFETY: the header describes a 32-bit top-down DIB; `bits` receives the pixel memory that
    // lives until the bitmap is deleted below.
    let color: HBITMAP =
        unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)? };
    if bits.is_null() {
        delete_bitmap(color);
        return Err(windows::core::Error::from_thread());
    }
    // SAFETY: the DIB holds DOT_SIZE x DOT_SIZE pixels of four bytes each, at `bits`.
    let pixels = unsafe {
        std::slice::from_raw_parts_mut(bits.cast::<u8>(), (DOT_SIZE * DOT_SIZE * 4) as usize)
    };
    paint_dot(pixels);

    // The mask is all zeros: the alpha channel decides what is transparent. Rows of a 1-bit
    // bitmap are padded to 16 bits, so 16 rows of two bytes are needed.
    let mask_bits = [0u8; 32];
    // SAFETY: the pixel data holds the 16 rows of two bytes that a 16 x 16 monochrome bitmap needs.
    let mask: HBITMAP =
        unsafe { CreateBitmap(DOT_SIZE, DOT_SIZE, 1, 1, Some(mask_bits.as_ptr().cast())) };
    let icon_info = ICONINFO {
        fIcon: TRUE,
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: color,
    };
    // SAFETY: both bitmaps are valid and sized for the icon; they are deleted after the call.
    let icon = unsafe { CreateIconIndirect(&icon_info) };
    delete_bitmap(color);
    delete_bitmap(mask);
    icon
}

/// Frees a GDI bitmap. The icon keeps its own copy, so the bitmaps can go once it exists.
fn delete_bitmap(bitmap: HBITMAP) {
    // SAFETY: the bitmap was created by this module and is not used after this call.
    let _ = unsafe { DeleteObject(bitmap.into()) };
}

/// Writes anti-aliased dot pixels: straight alpha equal to the dot's coverage of each pixel.
fn paint_dot(pixels: &mut [u8]) {
    let centre = DOT_SIZE as f32 / 2.0;
    for y in 0..DOT_SIZE {
        for x in 0..DOT_SIZE {
            let dx = x as f32 + 0.5 - centre;
            let dy = y as f32 + 0.5 - centre;
            let coverage = (DOT_RADIUS - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
            let index = ((y * DOT_SIZE + x) * 4) as usize;
            pixels[index..index + 3].copy_from_slice(&DOT_BGR);
            pixels[index + 3] = (coverage * 255.0).round() as u8;
        }
    }
}

/// Excludes the window from screenshots and screen sharing (`WDA_EXCLUDEFROMCAPTURE`).
pub fn set_secure_screen(app: &AppHandle, enabled: bool) -> CmdResult<()> {
    main_window(app)?
        .set_content_protected(enabled)
        .map_err(|e| CmdError::new(code::CORE, e.to_string()))
}

/// Follows the theme for the titlebar: dark when the system theme is dark. The choice is made at
/// startup, so a later system theme change applies after a restart.
pub fn style_titlebar(window: &WebviewWindow) {
    let dark = BOOL::from(matches!(window.theme(), Ok(Theme::Dark)));
    let hwnd = match window_handle(window) {
        Ok(hwnd) => HWND(hwnd as *mut c_void),
        Err(e) => {
            log::warn!("cannot style the titlebar: {}", e.message);
            return;
        }
    };
    // SAFETY: the attribute takes a BOOL, and the pointer and size describe that value.
    let result = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            std::ptr::from_ref(&dark).cast(),
            size_of::<BOOL>() as u32,
        )
    };
    if let Err(e) = result {
        log::warn!("cannot set the titlebar theme: {e}");
    }
}

/// The native window handle as an integer, so it can cross threads.
fn window_handle(window: &WebviewWindow) -> CmdResult<isize> {
    window
        .hwnd()
        .map(|hwnd| hwnd.0 as isize)
        .map_err(|e| CmdError::new(code::CORE, e.to_string()))
}
