// Hotkey-binding dialog. Shows a small always-on-top window that captures the
// next key combination pressed by the user (modifier + non-modifier key) and
// posts `WM_APP_HOTKEY_BOUND` back to the main window.

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, PAINTSTRUCT,
    SetBkMode, SetTextColor, TRANSPARENT,     DT_END_ELLIPSIS, DT_SINGLELINE, DT_VCENTER,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyNameTextW, GetKeyState, HOT_KEY_MODIFIERS, MAPVK_VK_TO_VSC, MOD_ALT, MOD_CONTROL,
    MOD_SHIFT, MapVirtualKeyW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetSystemMetrics, GWLP_USERDATA,
    GetWindowLongPtrW, HMENU, IDC_ARROW, LoadCursorW, PostMessageW, RegisterClassExW, SM_CXSCREEN,
    SM_CYSCREEN, SW_SHOW, SetWindowLongPtrW, ShowWindow, WM_APP, WM_DESTROY, WM_KEYDOWN, WM_PAINT,
    WM_SYSKEYDOWN, WNDCLASSEXW, WS_CAPTION, WS_EX_TOPMOST, WS_POPUP, WS_SYSMENU,
};
use windows::core::PCWSTR;

use crate::wstr;

/// Posted to the main window when the user completes or cancels a bind.
///
/// `wparam == 1`: accepted; `lparam` carries the packed combo.
/// `wparam == 0`: Escape pressed; `lparam` is 0.
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

/// Build a human-readable string like "Ctrl+Alt+F1" from a binding.
pub fn format_binding(mods: HOT_KEY_MODIFIERS, vkey: u32) -> String {
    let mut s = String::new();
    if mods.0 & MOD_CONTROL.0 != 0 {
        s.push_str("Ctrl+");
    }
    if mods.0 & MOD_ALT.0 != 0 {
        s.push_str("Alt+");
    }
    if mods.0 & MOD_SHIFT.0 != 0 {
        s.push_str("Shift+");
    }
    // MapVirtualKeyW → scan code → GetKeyNameTextW → key label.
    let scan = unsafe { MapVirtualKeyW(vkey, MAPVK_VK_TO_VSC) };
    // Bit 25 = "don't distinguish left/right" so we get "Ctrl" not "Left Ctrl".
    let lp = ((scan << 16) | (1 << 25)) as i32;
    let mut buf = [0u16; 64];
    let n = unsafe { GetKeyNameTextW(lp, &mut buf) } as usize;
    if n > 0 {
        s.push_str(&String::from_utf16_lossy(&buf[..n]));
    } else {
        s.push_str(&format!("VK{:#04X}", vkey));
    }
    s
}

const BIND_CLASS: &str = "loupe.hotkey.bind";
const BIND_W: i32 = 420;
const BIND_H: i32 = 240;

/// State kept in `GWLP_USERDATA` for the bind window.
struct BindState {
    main: HWND,
    current: Option<(HOT_KEY_MODIFIERS, u32)>,
}

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

/// Show the binding capture window. `current` is the currently active binding
/// (if any) and is shown in the dialog so the user knows what they have now.
pub fn show(main_hwnd: HWND, current: Option<(HOT_KEY_MODIFIERS, u32)>) {
    register_class();
    let class = wstr(BIND_CLASS);
    let title = wstr("Loupe \u{2014} Bind hotkey");
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

        let state = Box::new(BindState { main: main_hwnd, current });
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
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

                if vk == 0x1B {
                    // Escape → cancel.
                    let st = state_ref(hwnd);
                    let _ =
                        PostMessageW(Some(st.main), WM_APP_HOTKEY_BOUND, WPARAM(0), LPARAM(0));
                    let _ = DestroyWindow(hwnd);
                    return LRESULT(0);
                }

                if is_modifier(vk) {
                    return LRESULT(0);
                }

                let mods = current_modifiers();
                if mods.0 == 0 {
                    return LRESULT(0);
                }

                let st = state_ref(hwnd);
                let _ = PostMessageW(
                    Some(st.main),
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

                // Background.
                let bg = CreateSolidBrush(COLORREF(0x00F8F8F8));
                let _ = FillRect(hdc, &rc, bg);
                let _ = DeleteObject(bg.into());

                SetBkMode(hdc, TRANSPARENT);

                let pad = 18i32;
                let line_h = 22i32;
                let mut y = pad;

                // ── Instruction lines ──────────────────────────────
                SetTextColor(hdc, COLORREF(0x00_20_20_20));
                draw_line(hdc, &rc, pad, y,
                    "Bind a shortcut for \u{201c}New loupe\u{201d}:");
                y += line_h;

                SetTextColor(hdc, COLORREF(0x00_60_60_60));
                draw_line(hdc, &rc, pad + 8, y,
                    "Hold Ctrl, Alt or Shift — then press any other key.");
                y += line_h + 10;

                // ── Separator line ─────────────────────────────────
                let sep = CreateSolidBrush(COLORREF(0x00_D0_D0_D0));
                let sep_r = RECT { left: pad, top: y, right: rc.right - pad, bottom: y + 1 };
                let _ = FillRect(hdc, &sep_r, sep);
                let _ = DeleteObject(sep.into());
                y += 12;

                // ── Current binding ────────────────────────────────
                SetTextColor(hdc, COLORREF(0x00_20_20_20));
                draw_line(hdc, &rc, pad, y, "Current shortcut:");
                y += line_h;

                let st = state_ref(hwnd);
                let binding_str = match st.current {
                    Some((mods, vkey)) => format_binding(mods, vkey),
                    None => "(none — not set)".to_string(),
                };
                SetTextColor(hdc, COLORREF(0x00_00_80_00)); // dark green for emphasis
                draw_line(hdc, &rc, pad + 8, y, &binding_str);
                y += line_h + 10;

                // ── Cancel hint ────────────────────────────────────
                SetTextColor(hdc, COLORREF(0x00_80_80_80));
                draw_line(hdc, &rc, pad, y, "Esc to cancel without changes.");

                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_DESTROY => {
                // Free the BindState box.
                let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
                if raw != 0 {
                    drop(Box::from_raw(raw as *mut BindState));
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// Draw a single text line at (pad_left, y). Uses DT_SINGLELINE so the text
/// never wraps; long text gets an ellipsis rather than overflowing.
unsafe fn draw_line(
    hdc: windows::Win32::Graphics::Gdi::HDC,
    window_rc: &RECT,
    pad_left: i32,
    y: i32,
    text: &str,
) {
    unsafe {
        let mut v: Vec<u16> = text.encode_utf16().collect();
        let row_h = 22i32;
        let mut r = RECT {
            left: window_rc.left + pad_left,
            top: window_rc.top + y,
            right: window_rc.right - pad_left,
            bottom: window_rc.top + y + row_h,
        };
        let _ = DrawTextW(
            hdc,
            &mut v,
            &mut r,
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );
    }
}

#[inline]
unsafe fn state_ref(hwnd: HWND) -> &'static BindState {
    unsafe { &*(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const BindState) }
}

fn is_modifier(vk: u16) -> bool {
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
