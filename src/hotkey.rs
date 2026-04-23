use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, RegisterHotKey, UnregisterHotKey,
};

/// Hotkey id used in `WM_HOTKEY` messages.
pub const HOTKEY_ID_NEW_LENS: i32 = 1;

/// Virtual-key code for `Z`.
const VK_Z: u32 = 0x5A;

/// Register the global `Ctrl + Alt + Z` hotkey. Returns `true` on success;
/// failure most commonly means another process already owns the combination.
pub fn register(hwnd: HWND) -> bool {
    unsafe {
        RegisterHotKey(
            Some(hwnd),
            HOTKEY_ID_NEW_LENS,
            HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_ALT.0),
            VK_Z,
        )
        .is_ok()
    }
}

/// Best-effort hotkey deregistration during shutdown.
pub fn unregister(hwnd: HWND) {
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), HOTKEY_ID_NEW_LENS);
    }
}
