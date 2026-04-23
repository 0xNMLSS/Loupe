// Hotkey-binding dialog. Shows a small always-on-top window that captures the
// next key combination pressed by the user (modifier + non-modifier key) and
// posts `WM_APP_HOTKEY_BOUND` back to the main window.

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DT_CENTER, DT_WORDBREAK, DeleteObject, DrawTextW, EndPaint,
    FillRect, PAINTSTRUCT, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetClientRect, GetSystemMetrics,
    GetWindowLongPtrW, HMENU, IDC_ARROW, LoadCursorW, PostMessageW, RegisterClassExW, SM_CXSCREEN,
    SM_CYSCREEN, SW_SHOW, SetWindowLongPtrW, ShowWindow, WM_APP, WM_DESTROY, WM_KEYDOWN, WM_PAINT,
    WM_SYSKEYDOWN, WNDCLASSEXW, WS_CAPTION, WS_EX_TOPMOST, WS_POPUP, WS_SYSMENU,
};
use windows::core::PCWSTR;

use crate::wstr;

/// Posted to the main window when the user completes or cancels a bind.
///
/// `wparam == 1`:  binding accepted;  `lparam` carries the packed combo.
/// `wparam == 0`:  user pressed Escape; `lparam` is 0.
pub const WM_APP_HOTKEY_BOUND: u32 = WM_APP + 3;

/// Pack `(HOT_KEY_MODIFIERS, vkey)` into an `LPARAM`.
pub fn pack(mods: HOT_KEY_MODIFIERS, vkey: u32) -> LPARAM {
    LPARAM((((mods.0 as usize) << 16) | (vkey as usize & 0xFFFF)) as isize)
}

/// Unpack an `LPARAM` produced by `pack`.
pub fn unpack(lp: LPARAM) -> (HOT_KEY_MODIFIERS, u32) {
    let v = lp.0 as u32;
    (HOT_KEY_MODIFIERS(v >> 16), v & 0xFFFF)
}

const BIND_CLASS: &str = "lens.hotkey.bind";
const BIND_W: i32 = 360;
const BIND_H: i32 = 130;

fn register_class() {
    let class = wstr(BIND_CLASS);
    unsafe {
        let hinstance = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(bind_wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: PCWSTR(class.as_ptr()),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        let _ = RegisterClassExW(&wc);
    }
}

/// Show the binding capture window. The result is delivered asynchronously via
/// `WM_APP_HOTKEY_BOUND` posted to `main_hwnd`.
pub fn show(main_hwnd: HWND) {
    register_class();
    let class = wstr(BIND_CLASS);
    let title = wstr("lens — Bind hotkey");
    unsafe {
        let hinstance = GetModuleHandleW(None).unwrap_or_default();
        let sw = GetSystemMetrics(SM_CXSCREEN);
        let sh = GetSystemMetrics(SM_CYSCREEN);
        let x = (sw - BIND_W) / 2;
        let y = (sh - BIND_H) / 2;

        let hwnd = match CreateWindowExW(
            WS_EX_TOPMOST,
            PCWSTR(class.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_POPUP | WS_CAPTION | WS_SYSMENU,
            x,
            y,
            BIND_W,
            BIND_H,
            None,
            Some(HMENU::default()),
            Some(hinstance.into()),
            None,
        ) {
            Ok(h) if !h.is_invalid() => h,
            _ => return,
        };

        // Store the main HWND so the WndProc can post to it.
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, main_hwnd.0 as isize);
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
}

unsafe extern "system" fn bind_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let vk = (wparam.0 & 0xFFFF) as u16;

                // Escape → cancel.
                if vk == 0x1B {
                    let main = main_from(hwnd);
                    let _ = PostMessageW(Some(main), WM_APP_HOTKEY_BOUND, WPARAM(0), LPARAM(0));
                    let _ = DestroyWindow(hwnd);
                    return LRESULT(0);
                }

                // Ignore bare modifier keypresses — wait for a "real" key.
                if is_modifier(vk) {
                    return LRESULT(0);
                }

                let mods = current_modifiers();
                if mods.0 == 0 {
                    // No modifier held — not a valid global hotkey; ignore.
                    return LRESULT(0);
                }

                let main = main_from(hwnd);
                let _ = PostMessageW(
                    Some(main),
                    WM_APP_HOTKEY_BOUND,
                    WPARAM(1),
                    pack(mods, vk as u32),
                );
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);

                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);

                let bg = CreateSolidBrush(COLORREF(0x00F5F5F5));
                FillRect(hdc, &rc, bg);
                let _ = DeleteObject(bg.into());

                SetBkMode(hdc, TRANSPARENT);
                SetTextColor(hdc, COLORREF(0x00_20_20_20));

                let mut text: Vec<u16> = concat!(
                    "Press a key combination to use as the \"New lens\" shortcut.\r\n",
                    "(Hold Ctrl, Alt, or Shift — then press another key)\r\n\r\n",
                    "Esc to cancel."
                )
                .encode_utf16()
                .collect();

                let mut inner = RECT {
                    left: rc.left + 16,
                    top: rc.top + 14,
                    right: rc.right - 16,
                    bottom: rc.bottom - 14,
                };
                let _ = DrawTextW(hdc, &mut text, &mut inner, DT_CENTER | DT_WORDBREAK);

                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_DESTROY => LRESULT(0),
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

#[inline]
unsafe fn main_from(hwnd: HWND) -> HWND {
    unsafe { HWND(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut _) }
}

fn is_modifier(vk: u16) -> bool {
    // VK_SHIFT=0x10, VK_CONTROL=0x11, VK_MENU(Alt)=0x12,
    // VK_LSHIFT=0xA0, VK_RSHIFT=0xA1, VK_LCONTROL=0xA2, VK_RCONTROL=0xA3,
    // VK_LMENU=0xA4, VK_RMENU=0xA5, VK_LWIN=0x5B, VK_RWIN=0x5C
    matches!(
        vk,
        0x10 | 0x11 | 0x12 | 0xA0 | 0xA1 | 0xA2 | 0xA3 | 0xA4 | 0xA5 | 0x5B | 0x5C
    )
}

fn current_modifiers() -> HOT_KEY_MODIFIERS {
    let mut m = 0u32;
    unsafe {
        if (GetKeyState(0x11) as u16 & 0x8000) != 0 {
            m |= MOD_CONTROL.0;
        }
        if (GetKeyState(0x12) as u16 & 0x8000) != 0 {
            m |= MOD_ALT.0;
        }
        if (GetKeyState(0x10) as u16 & 0x8000) != 0 {
            m |= MOD_SHIFT.0;
        }
    }
    HOT_KEY_MODIFIERS(m)
}
