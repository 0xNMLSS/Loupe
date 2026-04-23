use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey,
};

/// Hotkey id used in `WM_HOTKEY` messages.
pub const HOTKEY_ID_NEW_LENS: i32 = 1;

/// Register a user-chosen global hotkey. Returns `true` on success; failure
/// usually means another app already owns the combination.
pub fn register(hwnd: HWND, modifiers: HOT_KEY_MODIFIERS, vkey: u32) -> bool {
    unsafe {
        RegisterHotKey(
            Some(hwnd),
            HOTKEY_ID_NEW_LENS,
            HOT_KEY_MODIFIERS(modifiers.0 | MOD_NOREPEAT.0),
            vkey,
        )
        .is_ok()
    }
}

/// Best-effort hotkey deregistration during shutdown or rebinding.
pub fn unregister(hwnd: HWND) {
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), HOTKEY_ID_NEW_LENS);
    }
}
