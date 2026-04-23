use std::cell::RefCell;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, HDC, InvalidateRect,
    PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetSystemMetrics,
    GetWindowLongPtrW, HCURSOR, HMENU, IDC_CROSS, LWA_ALPHA, LoadCursorW, PostMessageW,
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

/// Selection-rectangle border thickness in device pixels.
const BORDER_THICKNESS: i32 = 4;

/// Pixels per rainbow colour segment along the border perimeter.
/// Smaller = smoother gradient, more GDI brush operations per paint.
const RAINBOW_SEG: i32 = 3;

/// Alpha value for the transparent overlay window (0 = fully transparent,
/// 255 = opaque). 60 ≈ 24% opacity — enough to dim the desktop without
/// hiding it completely.
const OVERLAY_ALPHA: u8 = 60;

/// Convert an HSV hue (0–359°, S=1, V=1) to a Win32 `COLORREF` (0x00BBGGRR).
fn hue_to_colorref(hue: u16) -> COLORREF {
    let h = (hue % 360) as f32;
    let c = 1.0f32;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let (r, g, b): (f32, f32, f32) = match (h / 60.0) as u8 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let r = (r * 255.0) as u32;
    let g = (g * 255.0) as u32;
    let b = (b * 255.0) as u32;
    COLORREF(r | (g << 8) | (b << 16))
}

/// Draw a rainbow border by walking the perimeter clockwise and assigning each
/// `RAINBOW_SEG`-pixel segment a hue proportional to its position / perimeter.
/// The four corner squares are painted by whichever band reaches them first
/// (top/bottom take priority over left/right since they are painted first).
unsafe fn draw_rainbow_border(hdc: HDC, lx: i32, ly: i32, rx: i32, ry: i32, b: i32) {
    let w = (rx - lx).max(1);
    let h = (ry - ly).max(1);
    // Perimeter of the centre-line: corners shared, so use full side lengths.
    let perim = 2 * (w + h);

    // `pos` tracks how far along the perimeter we are (in pixels).
    let mut pos = 0i32;

    macro_rules! seg_color {
        () => {
            hue_to_colorref(((pos as i64 * 360 / perim as i64) as u16) % 360)
        };
    }
    macro_rules! fill {
        ($r:expr) => {{
            let br = unsafe { CreateSolidBrush(seg_color!()) };
            let _ = unsafe { FillRect(hdc, &$r, br) };
            let _ = unsafe { DeleteObject(br.into()) };
        }};
    }

    // Top band: left → right
    let mut x = lx;
    while x < rx {
        let xe = (x + RAINBOW_SEG).min(rx);
        fill!(RECT {
            left: x,
            top: ly,
            right: xe,
            bottom: (ly + b).min(ry)
        });
        pos += xe - x;
        x = xe;
    }
    // Right band: top → bottom (skip top-right corner already painted)
    let mut y = ly + b;
    while y < ry - b {
        let ye = (y + RAINBOW_SEG).min(ry - b);
        fill!(RECT {
            left: (rx - b).max(lx),
            top: y,
            right: rx,
            bottom: ye
        });
        pos += ye - y;
        y = ye;
    }
    // Bottom band: right → left (so hue continues flowing clockwise)
    let mut x = rx;
    while x > lx {
        let xs = (x - RAINBOW_SEG).max(lx);
        fill!(RECT {
            left: xs,
            top: (ry - b).max(ly),
            right: x,
            bottom: ry
        });
        pos += x - xs;
        x = xs;
    }
    // Left band: bottom → top (skip corners already painted)
    let mut y = ry - b;
    while y > ly + b {
        let ys = (y - RAINBOW_SEG).max(ly + b);
        fill!(RECT {
            left: lx,
            top: ys,
            right: (lx + b).min(rx),
            bottom: y
        });
        pos += y - ys;
        y = ys;
    }
}

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

/// Show the fullscreen opaque overlay and let the user drag a region.
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

        // Semi-transparent overlay — desktop shows through, border still pops.
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), OVERLAY_ALPHA, LWA_ALPHA);
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

                // Dim the entire screen. Because the window has LWA_ALPHA, every
                // pixel is uniformly blended — painting black darkens the desktop.
                let bg = CreateSolidBrush(COLORREF(0x00_00_00_00));
                FillRect(hdc, &ps.rcPaint, bg);
                let _ = DeleteObject(bg.into());

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

                    // Near-black fill inside the selection: at 24 % overlay alpha
                    // this lets the desktop show through more clearly than the
                    // dimmed surround, giving a natural "spotlight" effect.
                    let sel_fill = CreateSolidBrush(COLORREF(0x00_08_08_08));
                    FillRect(hdc, &sel, sel_fill);
                    let _ = DeleteObject(sel_fill.into());

                    // Rainbow border: hue is proportional to perimeter position,
                    // so all spectrum colours appear simultaneously around the rect.
                    draw_rainbow_border(hdc, lx, ly, rx, ry, BORDER_THICKNESS);
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
