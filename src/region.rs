// Region-selection overlay. Uses per-pixel alpha via UpdateLayeredWindow so
// the rainbow border is drawn at alpha=255 (fully opaque) while the dimmed
// background is at alpha=60 (~24% — desktop clearly visible through it).

use std::cell::RefCell;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetSystemMetrics,
    GetWindowLongPtrW, HCURSOR, HMENU, IDC_CROSS, LoadCursorW, PostMessageW, RegisterClassW,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_SHOW,
    SetWindowLongPtrW, ShowWindow, ULW_ALPHA, UpdateLayeredWindow, WM_APP, WM_DESTROY, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::PCWSTR;

use crate::wstr;

/// Posted to the main window when region selection finishes or is cancelled.
pub const WM_APP_REGION_DONE: u32 = WM_APP + 2;

const BORDER_THICKNESS: i32 = 4;

/// Premultiplied-black pixel for the dim background (alpha ≈ 24%).
/// Format in DIB memory (LE): byte[0]=B, byte[1]=G, byte[2]=R, byte[3]=A
/// As u32: (A<<24)|(R<<16)|(G<<8)|B → black premultiplied = just A in bits 24–31.
const DIM_PIXEL: u32 = 60 << 24; // A=60, RGB premultiplied = 0

/// Premultiplied-black pixel for the selection interior (very faint, ≈ 6%).
const SEL_PIXEL: u32 = 15 << 24;

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

/// Pack a `COLORREF` (0x00BBGGRR) into an opaque DIB pixel (A=255).
/// DIB u32 layout (LE): byte[0]=B, byte[1]=G, byte[2]=R, byte[3]=A
///   → u32 = (A<<24)|(R<<16)|(G<<8)|B
fn colorref_to_opaque_pixel(c: COLORREF) -> u32 {
    let v = c.0;
    let r = v & 0xFF;
    let g = (v >> 8) & 0xFF;
    let b = (v >> 16) & 0xFF;
    0xFF_00_00_00 | (r << 16) | (g << 8) | b
}

struct Overlay {
    main: HWND,
    dragging: bool,
    start: POINT,
    current: POINT,
    /// Selection rect in virtual-screen coords (for the magnifier source).
    rect: RECT,
    /// Overlay window pixel dimensions and virtual-screen origin.
    win_w: i32,
    win_h: i32,
    virt_x: i32,
    virt_y: i32,
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
        let _ = RegisterClassW(&wc);
    }
}

/// Show the fullscreen overlay. `main_hwnd` receives `WM_APP_REGION_DONE`
/// when the user releases the mouse (`wparam == 1`) or presses Esc (`wparam == 0`).
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

        // WS_EX_LAYERED without SetLayeredWindowAttributes → use UpdateLayeredWindow
        // for per-pixel alpha (opaque border, semi-transparent dim background).
        let hwnd = match CreateWindowExW(
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
        ) {
            Ok(h) if !h.is_invalid() => h,
            _ => return,
        };

        let state = Box::new(Overlay {
            main: main_hwnd,
            dragging: false,
            start: POINT::default(),
            current: POINT::default(),
            rect: RECT::default(),
            win_w: w,
            win_h: h,
            virt_x: x,
            virt_y: y,
        });
        let ptr = Box::into_raw(state);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);

        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetCapture(hwnd);
        update_layered(hwnd, &*ptr);
    }
}

/// Most-recently committed selection, in virtual-screen coordinates.
pub fn take_last() -> Option<RECT> {
    SELECTED_RECT.with(|c| c.borrow_mut().take())
}

/// Repaint the overlay surface via `UpdateLayeredWindow`.
///
/// - Background pixels: `DIM_PIXEL` (premultiplied black, alpha ≈ 24%)
/// - Selection interior: `SEL_PIXEL` (even fainter — desktop shows through more)
/// - Border pixels: fully opaque rainbow (alpha = 255), hue proportional to
///   clockwise perimeter position so all spectrum colours appear at once.
unsafe fn update_layered(hwnd: HWND, st: &Overlay) {
    let w = st.win_w;
    let h = st.win_h;
    if w <= 0 || h <= 0 {
        return;
    }

    unsafe {
        update_layered_inner(hwnd, st, w, h);
    }
}

unsafe fn update_layered_inner(hwnd: HWND, st: &Overlay, w: i32, h: i32) {
    unsafe {
        let hdc_screen = GetDC(None);
        let hdc_mem = CreateCompatibleDC(Some(hdc_screen));

        // 32-bit top-down DIB; BI_RGB = 0; bmiColors unused for 32bpp.
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // negative → top-down row order
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits_raw = core::ptr::null_mut::<core::ffi::c_void>();
        let hbmp =
            match CreateDIBSection(Some(hdc_mem), &bmi, DIB_RGB_COLORS, &mut bits_raw, None, 0) {
                Ok(b) if !b.is_invalid() => b,
                _ => {
                    let _ = DeleteDC(hdc_mem);
                    ReleaseDC(None, hdc_screen);
                    return;
                }
            };
        let old = SelectObject(hdc_mem, hbmp.into());
        let pixels = core::slice::from_raw_parts_mut(bits_raw as *mut u32, (w * h) as usize);

        // 1. Uniform dim background.
        pixels.fill(DIM_PIXEL);

        // 2. Selection area (if any).
        if st.dragging || st.rect.right > st.rect.left {
            let lx = st.start.x.min(st.current.x).max(0).min(w);
            let ly = st.start.y.min(st.current.y).max(0).min(h);
            let rx = st.start.x.max(st.current.x).max(0).min(w);
            let ry = st.start.y.max(st.current.y).max(0).min(h);

            if rx > lx && ry > ly {
                // Interior — slightly less dim than surroundings.
                for row in ly..ry {
                    let base = (row * w) as usize;
                    pixels[base + lx as usize..base + rx as usize].fill(SEL_PIXEL);
                }

                // Rainbow border: walk clockwise, hue = pos / perim * 360°.
                let b = BORDER_THICKNESS;
                let pw = rx - lx;
                let ph = ry - ly;
                let perim = (2 * (pw + ph)).max(1) as u32;

                macro_rules! paint_band {
                    ($row:expr, $col:expr, $pos:expr) => {
                        let hue = ($pos * 360 / perim) as u16;
                        pixels[($row * w + $col) as usize] =
                            colorref_to_opaque_pixel(hue_to_colorref(hue));
                    };
                }

                // Top band (L→R, includes TL and TR corners).
                for col in lx..rx {
                    let pos = (col - lx) as u32;
                    for row in ly..(ly + b).min(ry) {
                        paint_band!(row, col, pos);
                    }
                }
                // Right band (T→B, corners will be repainted by top/bottom).
                for row in ly..ry {
                    let pos = (pw + (row - ly)) as u32;
                    for col in (rx - b).max(lx)..rx {
                        paint_band!(row, col, pos);
                    }
                }
                // Bottom band (R→L, overwrites BR and BL corners from right band).
                for col in (lx..rx).rev() {
                    let pos = (pw + ph + (rx - 1 - col)) as u32;
                    for row in (ry - b).max(ly)..ry {
                        paint_band!(row, col, pos);
                    }
                }
                // Left band (B→T, corners already settled by top/bottom bands).
                for row in (ly..ry).rev() {
                    let pos = (2 * pw + ph + (ry - 1 - row)) as u32;
                    for col in lx..(lx + b).min(rx) {
                        paint_band!(row, col, pos);
                    }
                }
            }
        }

        // 3. Commit to the layered window.
        let blend = BLENDFUNCTION {
            BlendOp: 0, // AC_SRC_OVER
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: 1, // AC_SRC_ALPHA — use per-pixel alpha from DIB
        };
        let size = SIZE { cx: w, cy: h };
        let src_pt = POINT::default();
        let _ = UpdateLayeredWindow(
            hwnd,
            Some(hdc_screen),
            None, // keep window position from CreateWindowExW
            Some(&size),
            Some(hdc_mem),
            Some(&src_pt),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );

        let _ = SelectObject(hdc_mem, old);
        let _ = DeleteObject(hbmp.into());
        let _ = DeleteDC(hdc_mem);
        ReleaseDC(None, hdc_screen);
    } // end unsafe block
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

        let virt_x = st.virt_x;
        let virt_y = st.virt_y;

        match msg {
            WM_LBUTTONDOWN => {
                let (lx, ly) = lo_hi(lparam);
                st.dragging = true;
                st.start = POINT { x: lx, y: ly };
                st.current = st.start;
                update_layered(hwnd, st);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if st.dragging {
                    let (lx, ly) = lo_hi(lparam);
                    st.current = POINT { x: lx, y: ly };
                    update_layered(hwnd, st);
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
