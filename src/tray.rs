use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, MF_CHECKED, MF_POPUP, MF_SEPARATOR,
    MF_STRING, SetForegroundWindow, TPM_BOTTOMALIGN, TPM_RIGHTBUTTON, TrackPopupMenu,
};
use windows::core::PCWSTR;

use crate::magnifier::RendererKind;
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
pub const IDM_TOGGLE_SOURCE_FRAME: u32 = 103;
pub const IDM_RENDERER_CLASSIC: u32 = 200;
pub const IDM_RENDERER_GPU: u32 = 201;

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
    let tip: Vec<u16> = "Loupe — click: new region; right-click: menu\0"
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
/// `hotkey_label` is the currently bound shortcut; `selected_renderer` marks the active pipeline.
/// `source_frame_visible` is the current state of the on-desktop red outline.
pub fn show_menu(
    hwnd: HWND,
    hotkey_label: Option<&str>,
    selected_renderer: RendererKind,
    source_frame_visible: bool,
) {
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
        let frame_toggle = wstr("Show source frame");
        let r_classic = wstr("Classic (WC_MAGNIFIER)\t");
        let r_gpu = wstr("GPU (WGC, Lanczos)\t");
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
        let frame_flags = if source_frame_visible {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        };
        let _ = AppendMenuW(
            menu,
            frame_flags,
            IDM_TOGGLE_SOURCE_FRAME as usize,
            PCWSTR(frame_toggle.as_ptr()),
        );

        // Submenu: Renderer
        let sub = match CreatePopupMenu() {
            Ok(m) => m,
            Err(_) => {
                let _ = DestroyMenu(menu);
                return;
            }
        };
        let f_classic = if selected_renderer == RendererKind::Classic {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        };
        let f_gpu = if selected_renderer == RendererKind::Gpu {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        };
        let _ = AppendMenuW(
            sub,
            f_classic,
            IDM_RENDERER_CLASSIC as usize,
            PCWSTR(r_classic.as_ptr()),
        );
        let _ = AppendMenuW(
            sub,
            f_gpu,
            IDM_RENDERER_GPU as usize,
            PCWSTR(r_gpu.as_ptr()),
        );
        let sub_title = wstr("Renderer");
        let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, PCWSTR(sub_title.as_ptr()));

        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, IDM_QUIT as usize, PCWSTR(quit.as_ptr()));

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
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
        // `menu` owns the popup submenu; destroy the root only.
        let _ = DestroyMenu(menu);
    }
}
