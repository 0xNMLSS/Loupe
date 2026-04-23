// Classic Win32 `WC_MAGNIFIER` + transparent hit-test overlay.
// Extracted from the original single-file `magnifier` module for parity
// with the optional GPU path.

use std::sync::OnceLock;

use windows::Win32::Foundation::{
    COLORREF, ERROR_CLASS_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, RECT,
    WPARAM,
};
use windows::Win32::UI::Magnification::{
    MAGTRANSFORM, MW_FILTERMODE_EXCLUDE, MagInitialize, MagSetWindowFilterList, MagSetWindowSource,
    MagSetWindowTransform, MagUninitialize, WC_MAGNIFIERW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CreateWindowExW, DefWindowProcW, GWL_EXSTYLE, GWLP_USERDATA, GetWindowLongPtrW,
    HMENU, HWND_TOP, IDC_ARROW, LWA_ALPHA, LoadCursorW, MoveWindow, PostMessageW, RegisterClassW,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetLayeredWindowAttributes,
    SetWindowLongPtrW, SetWindowPos, WINDOW_EX_STYLE, WM_ERASEBKGND, WM_LBUTTONDBLCLK,
    WM_MOUSEWHEEL, WNDCLASSW, WS_CHILD, WS_EX_LAYERED, WS_VISIBLE,
};

use windows::core::PCWSTR;

use crate::wstr;

/// Child window id assigned to the magnifier control inside the host window.
const MAG_CHILD_ID: isize = 0x1001;

/// Invisible layered child on top of the magnifier — receives double-clicks.
const HIT_OVERLAY_CHILD_ID: isize = 0x1002;

const HIT_OVERLAY_CLASS: &str = "loupe.hit";

static HIT_OVERLAY_CLASS_NAME: OnceLock<usize> = OnceLock::new();

/// Posted to the host (main) window so it can toggle borderless fullscreen.
pub const WM_APP_TOGGLE_FULLSCREEN: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 4;

/// Mouse wheel zoom: `WPARAM` unused; `LPARAM` = signed wheel delta (same units as `WM_MOUSEWHEEL`,
/// typically ±120 per notch).
pub const WM_APP_WHEEL_ZOOM: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 6;

const HIT_OVERLAY_ALPHA: u8 = 6;

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

/// Initialise the Win32 Magnification runtime (required before any `WC_MAGNIFIER` window).
pub fn init() -> bool {
    unsafe { MagInitialize().as_bool() }
}

/// Tear down the Magnification runtime on process exit.
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

/// Exclude `host` (and any other windows) from `child`'s magnification source.
///
/// Without this, when the magnified source rectangle is large enough to contain
/// the host's screen position (e.g. extreme zoom-out covering the whole desktop),
/// `WC_MAGNIFIER` captures the loupe window itself → its own image is fed back
/// into the next frame → visible flicker / black noise. Excluding the host makes
/// that area come out empty in the magnified view, which is the expected
/// behaviour (same idea as OBS "exclude window from display capture").
pub fn exclude_from_source(child: HWND, host: HWND) {
    let mut hosts = [host];
    unsafe {
        let _ = MagSetWindowFilterList(child, MW_FILTERMODE_EXCLUDE, 1, hosts.as_mut_ptr());
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
        let host_isize = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        let host_opt = (host_isize != 0).then_some(HWND(host_isize as *mut _));

        match msg {
            WM_LBUTTONDBLCLK => {
                if let Some(host) = host_opt {
                    let _ =
                        PostMessageW(Some(host), WM_APP_TOGGLE_FULLSCREEN, WPARAM(0), LPARAM(0));
                }
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                if let Some(host) = host_opt {
                    let delta = ((wparam.0 as u32 >> 16) as i16) as i32;
                    if delta != 0 {
                        let _ = PostMessageW(
                            Some(host),
                            WM_APP_WHEEL_ZOOM,
                            WPARAM(0),
                            LPARAM(delta as isize),
                        );
                    }
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

/// Low-level: only updates the source rect. Prefer [`fit_source`] (or
/// `Renderer::set_source`, which re-fits) whenever a custom transform is in use.
pub fn set_source(child: HWND, src: RECT) {
    unsafe {
        let _ = MagSetWindowSource(child, src);
    }
}

/// Uniform scale used by `fit_source` (`client / source`, min axis).
fn uniform_scale_for_fit(child_w: i32, child_h: i32, src: RECT) -> f32 {
    let src_w = (src.right - src.left).max(1) as f32;
    let src_h = (src.bottom - src.top).max(1) as f32;
    let cw = child_w.max(1) as f32;
    let ch = child_h.max(1) as f32;
    (cw / src_w).min(ch / src_h).max(0.01)
}

/// Compute and apply a uniform-scale transform so `src` exactly fills a
/// `child_w x child_h` magnifier child, then set the source rect. The aspect
/// ratio of the source is preserved; if it differs from the child's, the
/// shorter axis dictates the scale (no stretching, no clipping).
pub fn fit_source(child: HWND, child_w: i32, child_h: i32, src: RECT) {
    let src_w = (src.right - src.left).max(1) as f32;
    let src_h = (src.bottom - src.top).max(1) as f32;
    let cw = child_w.max(1) as f32;
    let ch = child_h.max(1) as f32;
    let scale = uniform_scale_for_fit(child_w, child_h, src);

    // Uniform scale leaves letterboxing; center in the client (same math as GPU `build_ps_cbuf`).
    let scaled_w = src_w * scale;
    let scaled_h = src_h * scale;
    let ox = (cw - scaled_w) * 0.5;
    let oy = (ch - scaled_h) * 0.5;

    // `MAGTRANSFORM.v` is `v[row][col]` row-major: indices row*3+col. Windows uses the usual
    // column-vector affine `p' = M * p` with translation in the third column (m02, m12) → v[2], v[5].
    let mut transform = MAGTRANSFORM::default();
    transform.v[0] = scale;
    transform.v[1] = 0.0;
    transform.v[2] = ox;
    transform.v[3] = 0.0;
    transform.v[4] = scale;
    transform.v[5] = oy;
    transform.v[6] = 0.0;
    transform.v[7] = 0.0;
    transform.v[8] = 1.0;

    unsafe {
        let _ = MagSetWindowTransform(child, &mut transform);
        let _ = MagSetWindowSource(child, src);
    }
}
