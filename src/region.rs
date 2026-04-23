use std::cell::RefCell;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreatePen, CreateSolidBrush, DeleteObject, EndPaint, FillRect, FrameRect, HBRUSH,
    InvalidateRect, PAINTSTRUCT, PS_SOLID, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetSystemMetrics,
    GetWindowLongPtrW, HCURSOR, HICON, HMENU, IDC_CROSS, LWA_ALPHA, LoadCursorW, PostMessageW,
    RegisterClassW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    SW_SHOW, SetLayeredWindowAttributes, SetWindowLongPtrW, ShowWindow, WM_APP, WM_DESTROY,
    WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::PCWSTR;

use crate::wstr;

/// Sent to the application's main window when the user finishes a region
/// selection. `wparam.0 == 1` carries the four `i32` corners packed into
/// the `LPARAM`-pointed `RECT`; `wparam.0 == 0` means cancelled.
pub const WM_APP_REGION_DONE: u32 = WM_APP + 2;

/// Per-overlay state kept alive via `GWLP_USERDATA`.
struct Overlay {
    main: HWND,
    dragging: bool,
    start: POINT,
    current: POINT,
    rect: RECT,
}

thread_local! {
    static SELECTED_RECT: RefCell<Option<RECT>> = const { RefCell::new(None) };
}

const CLASS_NAME: &str = "lens.region.overlay";

fn register_class() {
    let class_w = wstr(CLASS_NAME);
    unsafe {
        let hinstance = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: PCWSTR(class_w.as_ptr()),
            hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or(HCURSOR::default()),
            ..Default::default()
        };
        // Ignore "class already registered" — registering twice is harmless
        // because the WNDCLASSW values are stable for the process lifetime.
        let _ = RegisterClassW(&wc);
    }
}

/// Show the fullscreen transparent overlay and let the user drag a region.
/// `main_hwnd` will receive `WM_APP_REGION_DONE` when the user releases the
/// mouse (`wparam == 1`) or hits Escape (`wparam == 0`).
pub fn show(main_hwnd: HWND) {
    register_class();
    let class_w = wstr(CLASS_NAME);
    let title_w = wstr("lens region");
    unsafe {
        let hinstance = GetModuleHandleW(None).unwrap_or_default();
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let h = GetSystemMetrics(SM_CYVIRTUALSCREEN);

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            PCWSTR(class_w.as_ptr()),
            PCWSTR(title_w.as_ptr()),
            WS_POPUP,
            x,
            y,
            w,
            h,
            None,
            Some(HMENU::default()),
            Some(hinstance.into()),
            None,
        );
        let hwnd = match hwnd {
            Ok(h) if !h.is_invalid() => h,
            _ => return,
        };

        let state = Box::new(Overlay {
            main: main_hwnd,
            dragging: false,
            start: POINT::default(),
            current: POINT::default(),
            rect: RECT::default(),
        });
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);

        // ~25% alpha — visible enough to dim the desktop without hiding it.
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 64, LWA_ALPHA);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetCapture(hwnd);
    }
}

/// Most-recently committed selection, in virtual-screen coordinates.
pub fn take_last() -> Option<RECT> {
    SELECTED_RECT.with(|c| c.borrow_mut().take())
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        let user = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if user == 0 {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let st = &mut *(user as *mut Overlay);

        let virt_x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let virt_y = GetSystemMetrics(SM_YVIRTUALSCREEN);

        match msg {
            WM_LBUTTONDOWN => {
                let (lx, ly) = lo_hi(lparam);
                st.dragging = true;
                st.start = POINT { x: lx, y: ly };
                st.current = st.start;
                let _ = InvalidateRect(Some(hwnd), None, true);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if st.dragging {
                    let (lx, ly) = lo_hi(lparam);
                    st.current = POINT { x: lx, y: ly };
                    let _ = InvalidateRect(Some(hwnd), None, true);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                if st.dragging {
                    st.dragging = false;
                    let (lx, ly) = lo_hi(lparam);
                    st.current = POINT { x: lx, y: ly };
                    let left = st.start.x.min(st.current.x) + virt_x;
                    let top = st.start.y.min(st.current.y) + virt_y;
                    let right = st.start.x.max(st.current.x) + virt_x;
                    let bottom = st.start.y.max(st.current.y) + virt_y;
                    st.rect = RECT { left, top, right, bottom };
                }
                let _ = ReleaseCapture();
                let valid = st.rect.right - st.rect.left > 8 && st.rect.bottom - st.rect.top > 8;
                if valid {
                    SELECTED_RECT.with(|c| *c.borrow_mut() = Some(st.rect));
                }
                let main = st.main;
                let _ = DestroyWindow(hwnd);
                let _ = PostMessageW(
                    Some(main),
                    WM_APP_REGION_DONE,
                    WPARAM(if valid { 1 } else { 0 }),
                    LPARAM(0),
                );
                LRESULT(0)
            }
            WM_KEYDOWN if wparam.0 as i32 == 0x1B /* VK_ESCAPE */ => {
                let main = st.main;
                let _ = ReleaseCapture();
                let _ = DestroyWindow(hwnd);
                let _ = PostMessageW(Some(main), WM_APP_REGION_DONE, WPARAM(0), LPARAM(0));
                LRESULT(0)
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                // Dim background.
                let dim = CreateSolidBrush(COLORREF(0x0000_0000));
                let full = ps.rcPaint;
                FillRect(hdc, &full, dim);
                let _ = DeleteObject(dim.into());

                if st.dragging || (st.rect.right > st.rect.left) {
                    let lx = st.start.x.min(st.current.x);
                    let ly = st.start.y.min(st.current.y);
                    let rx = st.start.x.max(st.current.x);
                    let ry = st.start.y.max(st.current.y);
                    let sel = RECT {
                        left: lx,
                        top: ly,
                        right: rx,
                        bottom: ry,
                    };

                    // Cut the selected region back to a clear rectangle by
                    // overpainting it with a fully-transparent black brush.
                    let clear = CreateSolidBrush(COLORREF(0x0010_1010));
                    FillRect(hdc, &sel, clear);
                    let _ = DeleteObject(clear.into());

                    let pen = CreatePen(PS_SOLID, 2, COLORREF(0x00FF_FFFF));
                    let old = SelectObject(hdc, pen.into());
                    let frame_brush = CreateSolidBrush(COLORREF(0x00FF_FFFF));
                    FrameRect(hdc, &sel, frame_brush);
                    let _ = SelectObject(hdc, old);
                    let _ = DeleteObject(pen.into());
                    let _ = DeleteObject(frame_brush.into());
                }

                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_DESTROY => {
                let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
                if raw != 0 {
                    drop(Box::from_raw(raw as *mut Overlay));
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

#[inline]
fn lo_hi(lp: LPARAM) -> (i32, i32) {
    let v = lp.0 as u32;
    let lo = (v & 0xFFFF) as i16 as i32;
    let hi = ((v >> 16) & 0xFFFF) as i16 as i32;
    (lo, hi)
}

// HBRUSH import retained for type annotations on returns above.
#[allow(dead_code)]
fn _unused() -> Option<HBRUSH> {
    None
}
#[allow(dead_code)]
fn _unused2() -> Option<HICON> {
    None
}
