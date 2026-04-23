//! Persistent on-screen indicator of the current capture source rectangle.
//!
//! After the user selects a region (or wheel-zooms inside the loupe), the
//! captured screen rectangle is also drawn as a thin red outline directly on
//! the desktop at its native virtual-screen coordinates. The frame is:
//!
//! - **Always topmost**, layered with a magenta colour key so the middle is
//!   visually transparent (we draw nothing inside).
//! - **Click-through in the middle** — `WM_NCHITTEST` returns `HTTRANSPARENT`
//!   for the inner area, so applications underneath the frame can still be
//!   interacted with normally. Only the border is `HTCLIENT`.
//! - **Draggable by the border** — clicking on the red edge and moving the
//!   mouse pans the source rectangle. Drag math is **absolute** (each move
//!   computes `start_source_top_left + (mouse_now - start_mouse)`) so the
//!   host can clamp / `MoveWindow` the frame mid-drag without confusing the
//!   delta calculation.
//! - **Excluded from screen capture** via
//!   `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` (Windows 10 v2004+;
//!   silently no-ops on older builds). This prevents both Classic
//!   `WC_MAGNIFIER` and the GPU `WGC` path from ever pulling the frame into
//!   their captured texture, which would otherwise produce a red rectangle
//!   inside the magnified view (and a feedback loop if the source covers the
//!   frame's screen area).

#![cfg(windows)]

use std::cell::RefCell;
use std::sync::OnceLock;

use windows::Win32::Foundation::{
    COLORREF, ERROR_CLASS_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT,
    RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, HGDIOBJ, PAINTSTRUCT,
    RDW_ALLCHILDREN, RDW_ERASE, RDW_INVALIDATE, RedrawWindow,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetCapture, ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GWLP_USERDATA, GetClientRect, GetCursorPos, GetWindowLongPtrW,
    GetWindowRect, HTCLIENT, HTTRANSPARENT, IDC_SIZEALL, LWA_COLORKEY, LoadCursorW, MoveWindow,
    PostMessageW, RegisterClassW, SW_HIDE, SW_SHOWNA, SetCursor, SetLayeredWindowAttributes,
    SetWindowDisplayAffinity, SetWindowLongPtrW, ShowWindow, WDA_EXCLUDEFROMCAPTURE, WM_DESTROY,
    WM_ERASEBKGND, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCHITTEST, WM_PAINT,
    WM_SETCURSOR, WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use windows::core::PCWSTR;

use crate::wstr;

/// Posted to the host (main) window when the user finishes a frame drag step.
/// `WPARAM` = new source `left` (i32, sign-extended into `usize`),
/// `LPARAM` = new source `top` (i32, sign-extended into `isize`). Source size
/// is unchanged — the host re-computes the rect from `current_source` size.
pub const WM_APP_SOURCE_MOVE_TO: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 7;

const SOURCE_FRAME_CLASS: &str = "loupe.sourceframe";

/// Visible border thickness in pixels. Drawn on the inside edge of the window.
const BORDER_PX: i32 = 3;

/// Distance from any edge (in pixels) where `WM_NCHITTEST` returns `HTCLIENT`
/// (= draggable). Slightly larger than the visual border so it is easier to
/// grab without pixel-perfect aim.
const HIT_BORDER_PX: i32 = 6;

/// `MK_LBUTTON` from winuser.h — set in `wparam` while the left button is down.
const MK_LBUTTON: u32 = 1;

/// Magenta — used as the layered window's colour key. Any pixel painted with
/// this exact value becomes visually transparent. Border pixels are red so
/// they remain opaque.
const KEY_COLOR: COLORREF = COLORREF(0x00FF00FF);
const BORDER_COLOR: COLORREF = COLORREF(0x000000FF); // BGR = red

static FRAME_CLASS_ATOM: OnceLock<usize> = OnceLock::new();

#[derive(Clone, Copy)]
struct DragState {
    start_mouse_screen: POINT,
    start_source_top_left: (i32, i32),
}

thread_local! {
    static DRAG_STATE: RefCell<Option<DragState>> = const { RefCell::new(None) };
}

fn class_name(instance: HINSTANCE) -> Option<PCWSTR> {
    let bits = *FRAME_CLASS_ATOM.get_or_init(|| unsafe {
        let name: &'static mut [u16] = Box::leak(wstr(SOURCE_FRAME_CLASS).into_boxed_slice());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance,
            // Cursor is set per-message in WM_SETCURSOR for the border; the
            // window class default is fine for the inside (we return HTTRANSPARENT
            // there so the cursor falls through to whatever app is below).
            lpszClassName: PCWSTR(name.as_ptr()),
            ..Default::default()
        };
        let atom = RegisterClassW(&wc);
        if atom == 0 {
            let err = GetLastError();
            if err != ERROR_CLASS_ALREADY_EXISTS {
                eprintln!("loupe: RegisterClassW({SOURCE_FRAME_CLASS}) failed: {err:?}");
                return 0;
            }
        }
        name.as_ptr() as usize
    });
    if bits == 0 {
        None
    } else {
        Some(PCWSTR(bits as *const u16))
    }
}

/// Create the (initially hidden) source frame window. `host` is the main
/// loupe window — its `HWND` is stashed in `GWLP_USERDATA` so drag events
/// can be posted back to it.
pub fn create(host: HWND, instance: HINSTANCE) -> Option<HWND> {
    let class = class_name(instance)?;
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class,
            PCWSTR::null(),
            WS_POPUP,
            0,
            0,
            16,
            16,
            None,
            // Top-level window: hMenu must be a real HMENU or `None` (passing
            // a child id triggers ERROR_MENU_HANDLE / 1401). Child ids are
            // only valid for `WS_CHILD` windows.
            None,
            Some(instance),
            None,
        )
    };
    let h = match hwnd {
        Ok(w) if !w.is_invalid() => w,
        _ => {
            let err = unsafe { GetLastError() };
            eprintln!("loupe: CreateWindowExW(source frame) failed: {err:?}");
            return None;
        }
    };
    unsafe {
        let _ = SetWindowLongPtrW(h, GWLP_USERDATA, host.0 as isize);
        let _ = SetLayeredWindowAttributes(h, KEY_COLOR, 0, LWA_COLORKEY);
        // Win10 v2004+: hide from any screen-capture API (WGC, GDI mirror,
        // Magnification API). Older Windows builds will return FALSE here —
        // worst case the frame appears inside the magnified view, which is
        // ugly but not fatal.
        let _ = SetWindowDisplayAffinity(h, WDA_EXCLUDEFROMCAPTURE);
    }
    Some(h)
}

/// Reposition the frame to cover `rect` in virtual-screen coordinates.
/// The frame is forced to repaint (border + transparent middle) at the new
/// size. Coordinates that go off the virtual desktop are passed through to
/// Win32 unchanged — the host is expected to clamp `current_source` itself
/// before calling here.
///
/// **Ghost-trail mitigation**: layered windows with `LWA_COLORKEY` are a
/// known source of "stale border" artifacts when shrunk via `MoveWindow` —
/// the DWM compositor keeps the old (now uncovered) border pixels until the
/// underlying windows are explicitly invalidated. Each wheel-zoom step then
/// leaves a concentric ring on screen. We work around it by issuing a
/// `RedrawWindow(NULL, …, RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN)`
/// over the union of the old and new bounds, which forces every top-level
/// window in that area (including the desktop) to repaint and overwrite the
/// stale pixels.
pub fn move_to(hwnd: HWND, rect: RECT) {
    let w = (rect.right - rect.left).max(1);
    let h = (rect.bottom - rect.top).max(1);
    unsafe {
        let mut old = RECT::default();
        let _ = GetWindowRect(hwnd, &mut old);
        let _ = MoveWindow(hwnd, rect.left, rect.top, w, h, true);
        invalidate_screen_union(old, rect);
    }
}

pub fn show(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNA);
    }
}

pub fn hide(hwnd: HWND) {
    unsafe {
        let mut old = RECT::default();
        let _ = GetWindowRect(hwnd, &mut old);
        let _ = ShowWindow(hwnd, SW_HIDE);
        // Same colour-key ghosting story as `move_to`: hiding the layered
        // window doesn't always trigger an underlying repaint of its old
        // bounds, so flush them explicitly.
        invalidate_screen_union(old, old);
    }
}

/// Invalidate the bounding box of `a` and `b` on screen so every window in
/// that area receives `WM_PAINT`. Passing `None` for the `HWND` targets the
/// virtual desktop; `RDW_ALLCHILDREN` cascades into all top-level windows.
unsafe fn invalidate_screen_union(a: RECT, b: RECT) {
    let union = RECT {
        left: a.left.min(b.left),
        top: a.top.min(b.top),
        right: a.right.max(b.right),
        bottom: a.bottom.max(b.bottom),
    };
    if union.right <= union.left || union.bottom <= union.top {
        return;
    }
    unsafe {
        let _ = RedrawWindow(
            None,
            Some(&union),
            None,
            RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
        );
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_NCHITTEST => {
                // `lparam` is the cursor position in screen coordinates as a
                // packed 32-bit value (low = x, high = y). Sign-extend each
                // half so monitors with negative coordinates work.
                let mx = (lparam.0 as i32 & 0xFFFF) as i16 as i32;
                let my = ((lparam.0 as i32 >> 16) & 0xFFFF) as i16 as i32;
                let mut wr = RECT::default();
                let _ = GetWindowRect(hwnd, &mut wr);
                let cx = mx - wr.left;
                let cy = my - wr.top;
                let w = wr.right - wr.left;
                let h = wr.bottom - wr.top;
                let b = HIT_BORDER_PX;
                if w > 2 * b && h > 2 * b && cx >= b && cx < w - b && cy >= b && cy < h - b {
                    LRESULT(HTTRANSPARENT as isize)
                } else {
                    LRESULT(HTCLIENT as isize)
                }
            }
            WM_SETCURSOR => {
                if let Ok(c) = LoadCursorW(None, IDC_SIZEALL) {
                    let _ = SetCursor(Some(c));
                }
                LRESULT(1)
            }
            WM_LBUTTONDOWN => {
                let mut wr = RECT::default();
                let _ = GetWindowRect(hwnd, &mut wr);
                let mut p = POINT::default();
                let _ = GetCursorPos(&mut p);
                DRAG_STATE.with(|s| {
                    *s.borrow_mut() = Some(DragState {
                        start_mouse_screen: p,
                        start_source_top_left: (wr.left, wr.top),
                    });
                });
                let _ = SetCapture(hwnd);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let buttons = wparam.0 as u32;
                if (buttons & MK_LBUTTON) == 0 || GetCapture() != hwnd {
                    return LRESULT(0);
                }
                let mut p = POINT::default();
                let _ = GetCursorPos(&mut p);
                let target = DRAG_STATE.with(|s| {
                    s.borrow().as_ref().map(|d| {
                        let dx = p.x - d.start_mouse_screen.x;
                        let dy = p.y - d.start_mouse_screen.y;
                        (
                            d.start_source_top_left.0 + dx,
                            d.start_source_top_left.1 + dy,
                        )
                    })
                });
                if let Some((nx, ny)) = target {
                    let host_isize = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
                    if host_isize != 0 {
                        let host = HWND(host_isize as *mut _);
                        let _ = PostMessageW(
                            Some(host),
                            WM_APP_SOURCE_MOVE_TO,
                            WPARAM(nx as usize),
                            LPARAM(ny as isize),
                        );
                    }
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                if GetCapture() == hwnd {
                    let _ = ReleaseCapture();
                }
                DRAG_STATE.with(|s| {
                    *s.borrow_mut() = None;
                });
                LRESULT(0)
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut cr = RECT::default();
                let _ = GetClientRect(hwnd, &mut cr);
                let w = cr.right - cr.left;
                let h = cr.bottom - cr.top;

                // Fill with the colour key so the inside renders fully transparent.
                let bg = CreateSolidBrush(KEY_COLOR);
                let _ = FillRect(hdc, &cr, bg);
                let _ = DeleteObject(HGDIOBJ(bg.0));

                // Four solid-red strips form the border (cheaper than `Rectangle`
                // and avoids dealing with the current pen / stock objects).
                let border_brush = CreateSolidBrush(BORDER_COLOR);
                let strips = [
                    RECT {
                        left: 0,
                        top: 0,
                        right: w,
                        bottom: BORDER_PX.min(h),
                    },
                    RECT {
                        left: 0,
                        top: (h - BORDER_PX).max(0),
                        right: w,
                        bottom: h,
                    },
                    RECT {
                        left: 0,
                        top: 0,
                        right: BORDER_PX.min(w),
                        bottom: h,
                    },
                    RECT {
                        left: (w - BORDER_PX).max(0),
                        top: 0,
                        right: w,
                        bottom: h,
                    },
                ];
                for s in &strips {
                    let _ = FillRect(hdc, s, border_brush);
                }
                let _ = DeleteObject(HGDIOBJ(border_brush.0));

                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_DESTROY => LRESULT(0),
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
