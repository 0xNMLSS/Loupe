use std::sync::OnceLock;

use windows::Win32::Foundation::{
    COLORREF, ERROR_CLASS_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, RECT,
    WPARAM,
};
use windows::Win32::UI::Magnification::{
    MAGTRANSFORM, MagInitialize, MagSetWindowSource, MagSetWindowTransform, MagUninitialize,
    WC_MAGNIFIERW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CreateWindowExW, DefWindowProcW, GWL_EXSTYLE, GWLP_USERDATA, GetWindowLongPtrW,
    HMENU, HWND_TOP, IDC_ARROW, LWA_ALPHA, LoadCursorW, MoveWindow, PostMessageW, RegisterClassW,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetLayeredWindowAttributes,
    SetWindowLongPtrW, SetWindowPos, WINDOW_EX_STYLE, WM_ERASEBKGND, WM_LBUTTONDBLCLK, WNDCLASSW,
    WS_CHILD, WS_EX_LAYERED, WS_VISIBLE,
};

use windows::core::PCWSTR;

use crate::wstr;

/// Child window id assigned to the magnifier control inside the host window.
const MAG_CHILD_ID: isize = 0x1001;

/// Invisible layered child on top of the magnifier — receives double-clicks.
/// `WC_MAGNIFIER` does not reliably participate in hit-testing; input passes
/// through to the desktop, so we cannot subclass it for click gestures.
const HIT_OVERLAY_CHILD_ID: isize = 0x1002;

const HIT_OVERLAY_CLASS: &str = "loupe.hit";

/// Leaked wide name + one-time `RegisterClassW`. `WNDCLASSW::lpszClassName` must
/// point to memory that stays valid for the process lifetime.
/// Stored as `usize` because raw pointers are not `Send`/`Sync` in `static`s on
/// this edition.
static HIT_OVERLAY_CLASS_NAME: OnceLock<usize> = OnceLock::new();

/// Posted to the host (main) window so it can toggle borderless fullscreen.
/// Must match the handler in `main.rs`.
pub const WM_APP_TOGGLE_FULLSCREEN: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 4;

/// Nearly invisible (alpha) so the magnified bitmap remains visible while the
/// overlay still receives mouse input.
const HIT_OVERLAY_ALPHA: u8 = 6;

/// Registers `loupe.hit` once; `instance` must be the module handle used for `CreateWindowExW`.
fn hit_overlay_class_name(instance: HINSTANCE) -> PCWSTR {
    let bits = *HIT_OVERLAY_CLASS_NAME.get_or_init(|| unsafe {
        let name: &'static mut [u16] = Box::leak(wstr(HIT_OVERLAY_CLASS).into_boxed_slice());
        let wc = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(hit_overlay_wnd_proc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: PCWSTR(name.as_ptr()),
            ..Default::default()
        };
        let atom = RegisterClassW(&wc);
        if atom == 0 {
            let err = GetLastError();
            if err != ERROR_CLASS_ALREADY_EXISTS {
                eprintln!("loupe: RegisterClassW({HIT_OVERLAY_CLASS}) failed: {err:?}");
                return 0;
            }
        }
        name.as_ptr() as usize
    });
    PCWSTR(bits as *const u16)
}

/// Initialise the Magnification runtime. Must be called once before creating
/// any magnifier window.
pub fn init() -> bool {
    unsafe { MagInitialize().as_bool() }
}

/// Tear down the Magnification runtime during shutdown.
pub fn shutdown() {
    unsafe {
        let _ = MagUninitialize();
    }
}

/// Create the `WC_MAGNIFIER` child filling the host's client area.
pub fn create_child(host: HWND, client: RECT, instance: HINSTANCE) -> Option<HWND> {
    let width = (client.right - client.left).max(1);
    let height = (client.bottom - client.top).max(1);
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(WC_MAGNIFIERW.as_ptr()),
            PCWSTR::null(),
            WS_CHILD | WS_VISIBLE,
            0,
            0,
            width,
            height,
            Some(host),
            Some(HMENU(MAG_CHILD_ID as *mut _)),
            Some(instance),
            None,
        )
    };
    match hwnd {
        Ok(h) if !h.is_invalid() => Some(h),
        _ => {
            let err = unsafe { GetLastError() };
            eprintln!("loupe: CreateWindowExW(WC_MAGNIFIER) failed: {err:?}");
            None
        }
    }
}

/// Layered, almost-transparent child above the magnifier; captures double-clicks.
pub fn create_hit_overlay(host: HWND, client: RECT, instance: HINSTANCE) -> Option<HWND> {
    let width = (client.right - client.left).max(1);
    let height = (client.bottom - client.top).max(1);
    let class = hit_overlay_class_name(instance);
    if class.is_null() {
        eprintln!("loupe: hit overlay class not registered; double-click fullscreen disabled");
        return None;
    }
    // Do not pass WS_EX_LAYERED / WS_EX_NOACTIVATE at creation: combined extended
    // styles on a child can make `CreateWindowExW` fail with ERROR_INVALID_HANDLE (6).
    // Apply WS_EX_LAYERED after create, then layered attributes (standard pattern).
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            PCWSTR::null(),
            WS_CHILD | WS_VISIBLE,
            0,
            0,
            width,
            height,
            Some(host),
            Some(HMENU(HIT_OVERLAY_CHILD_ID as *mut _)),
            Some(instance),
            None,
        )
    };
    let h = match hwnd {
        Ok(w) if !w.is_invalid() => w,
        _ => {
            let err = unsafe { GetLastError() };
            eprintln!(
                "loupe: CreateWindowExW(hit overlay) failed: {err:?} (see Win32 ERROR_* for code)"
            );
            return None;
        }
    };
    unsafe {
        let _ = SetWindowLongPtrW(h, GWLP_USERDATA, host.0 as isize);
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        SetWindowLongPtrW(h, GWL_EXSTYLE, ex | (WS_EX_LAYERED.0 as isize));
        let _ = SetLayeredWindowAttributes(h, COLORREF(0), HIT_OVERLAY_ALPHA, LWA_ALPHA);
        let _ = SetWindowPos(
            h,
            Some(HWND_TOP),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
    Some(h)
}

unsafe extern "system" fn hit_overlay_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_LBUTTONDBLCLK => {
                let host_isize = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
                if host_isize != 0 {
                    let host = HWND(host_isize as *mut _);
                    let _ =
                        PostMessageW(Some(host), WM_APP_TOGGLE_FULLSCREEN, WPARAM(0), LPARAM(0));
                }
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// Resize a child to fully cover its host's client area.
pub fn resize_to(child: HWND, client: RECT) {
    let width = client.right - client.left;
    let height = client.bottom - client.top;
    unsafe {
        let _ = MoveWindow(child, 0, 0, width, height, true);
    }
}

/// Keep the hit overlay above the magnifier after `WM_SIZE` (sibling z-order).
pub fn elevate_above_siblings(hwnd: HWND) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOP),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

/// Point the magnifier at a screen rectangle (virtual-screen coordinates).
pub fn set_source(child: HWND, src: RECT) {
    unsafe {
        let _ = MagSetWindowSource(child, src);
    }
}

/// Compute and apply a uniform-scale transform so `src` exactly fills a
/// `child_w x child_h` magnifier child, then set the source rect. The aspect
/// ratio of the source is preserved; if it differs from the child's, the
/// shorter axis dictates the scale (no stretching, no clipping).
pub fn fit_source(child: HWND, child_w: i32, child_h: i32, src: RECT) {
    let src_w = (src.right - src.left).max(1) as f32;
    let src_h = (src.bottom - src.top).max(1) as f32;
    let scale_x = child_w as f32 / src_w;
    let scale_y = child_h as f32 / src_h;
    let scale = scale_x.min(scale_y).max(0.01);

    // MAGTRANSFORM.v is a row-major 3x3 matrix flattened as [f32; 9].
    // Index = row * 3 + col. We set m11 (scale x), m22 (scale y), and m33 (1).
    let mut transform = MAGTRANSFORM::default();
    transform.v[0] = scale;
    transform.v[4] = scale;
    transform.v[8] = 1.0;

    unsafe {
        let _ = MagSetWindowTransform(child, &mut transform);
        let _ = MagSetWindowSource(child, src);
    }
}
