use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, MF_SEPARATOR, MF_STRING,
    SetForegroundWindow, TPM_BOTTOMALIGN, TPM_RIGHTBUTTON, TrackPopupMenu,
};
use windows::core::PCWSTR;

use crate::{load_app_icon, wstr};

/// Application-defined message Windows posts back to us for tray events.
pub const WM_APP_TRAY: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 1;

/// Identifier used in our `NOTIFYICONDATAW`. A constant is enough because we
/// only ever own one tray icon.
const TRAY_UID: u32 = 1;

/// Menu command ids returned by `TrackPopupMenu` via `WM_COMMAND`.
pub const IDM_NEW_LENS: u32 = 100;
pub const IDM_QUIT: u32 = 101;
pub const IDM_BIND_HOTKEY: u32 = 102;

fn build_nid(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_UID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_APP_TRAY,
        ..Default::default()
    };
    nid.hIcon = load_app_icon();
    let tip: Vec<u16> = "Loupe — right-click to configure\0"
        .encode_utf16()
        .collect();
    let copy_len = tip.len().min(nid.szTip.len());
    nid.szTip[..copy_len].copy_from_slice(&tip[..copy_len]);
    nid
}

/// Add the tray icon. Returns `true` if Explorer accepted it.
pub fn add(hwnd: HWND) -> bool {
    let nid = build_nid(hwnd);
    unsafe { Shell_NotifyIconW(NIM_ADD, &nid).as_bool() }
}

/// Remove the tray icon during shutdown.
pub fn remove(hwnd: HWND) {
    let nid = build_nid(hwnd);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

/// Show the right-click menu at the current cursor position.
///
/// `hotkey_label` is the currently bound shortcut (e.g. "Ctrl+Alt+Z"); when
/// present it is appended to the first menu item so users can see at a glance
/// what key triggers a new loupe.
pub fn show_menu(hwnd: HWND, hotkey_label: Option<&str>) {
    unsafe {
        let menu = match CreatePopupMenu() {
            Ok(m) => m,
            Err(_) => return,
        };
        let new_lens_text = match hotkey_label {
            Some(k) => format!("New loupe  [{k}]"),
            None => "New loupe".to_string(),
        };
        let new_lens = wstr(&new_lens_text);
        let bind = wstr("Bind hotkey\u{2026}");
        let quit = wstr("Quit");
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            IDM_NEW_LENS as usize,
            PCWSTR(new_lens.as_ptr()),
        );
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            IDM_BIND_HOTKEY as usize,
            PCWSTR(bind.as_ptr()),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, IDM_QUIT as usize, PCWSTR(quit.as_ptr()));

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Required by docs so the menu dismisses if the user clicks elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let _ = TrackPopupMenu(
            menu,
            TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
    }
}
