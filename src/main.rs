// loupe — a tiny live screen-region magnifier for Windows.
//
// Architecture: a single hidden-on-startup main window owns an optional global
// hotkey and the tray icon. Triggering "New lens" (tray left-click, menu item, or hotkey)
// shows a fullscreen overlay, lets the user drag a rectangle, then opens (or
// reuses) a floating always-on-top host window with a magnifier child: either
// `WC_MAGNIFIER` (Classic) or a D3D11 swapchain child (GPU / WGC + Lanczos).

#![cfg(windows)]
// Default: console subsystem so `eprintln!` / local dev logs show in the terminal.
// Release artifacts (GitHub Actions) build with `--features hide_console`.
#![cfg_attr(feature = "hide_console", windows_subsystem = "windows")]

mod config;
mod dpi;
mod hotkey;
mod hotkey_bind;
mod magnifier;
mod region;
mod source_frame;
mod tray;

use std::cell::RefCell;

use std::mem::size_of;

use windows::Win32::Foundation::COLORREF;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::HOT_KEY_MODIFIERS;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, GWL_EXSTYLE, GWL_STYLE, GetClientRect, GetMessageW, GetSystemMetrics,
    GetWindowLongPtrW, GetWindowPlacement, GetWindowRect, HICON, HMENU, HWND_TOP, IDC_ARROW,
    KillTimer, LWA_ALPHA, LoadCursorW, LoadIconW, MSG, PostQuitMessage, RegisterClassW,
    SET_WINDOW_POS_FLAGS, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, SW_HIDE, SW_SHOW, SetLayeredWindowAttributes, SetTimer, SetWindowLongPtrW,
    SetWindowPlacement, SetWindowPos, ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE,
    WINDOWPLACEMENT, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_HOTKEY, WM_LBUTTONUP, WM_RBUTTONUP,
    WM_SIZE, WM_TIMER, WNDCLASSW, WS_CLIPCHILDREN, WS_EX_LAYERED, WS_EX_TOPMOST,
    WS_OVERLAPPEDWINDOW,
};

use windows::core::PCWSTR;

use crate::hotkey_bind::WM_APP_HOTKEY_BOUND;
use crate::magnifier::Renderer;
use crate::magnifier::RendererKind;
use crate::magnifier::{WM_APP_TOGGLE_FULLSCREEN, WM_APP_WHEEL_ZOOM};
use crate::region::WM_APP_REGION_DONE;
use crate::source_frame::WM_APP_SOURCE_MOVE_TO;
use crate::tray::{
    IDM_BIND_HOTKEY, IDM_NEW_LENS, IDM_QUIT, IDM_RENDERER_CLASSIC, IDM_RENDERER_GPU,
    IDM_TOGGLE_SOURCE_FRAME, WM_APP_TRAY,
};

/// Convert a Rust `&str` into a NUL-terminated UTF-16 buffer suitable for
/// passing as `PCWSTR` to Win32 functions.
pub fn wstr(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Resource id of the embedded application icon (see `app.rc`). Used with
/// `LoadIconW` + the `MAKEINTRESOURCE`-style integer-ptr convention.
pub const IDI_APP_ICON: u16 = 1;

/// Load the embedded application icon, falling back to a default system icon
/// if the resource is missing for some reason.
pub fn load_app_icon() -> HICON {
    unsafe {
        let hinstance = GetModuleHandleW(None).ok();
        if let Some(h) = hinstance
            && let Ok(icon) = LoadIconW(Some(h.into()), PCWSTR(IDI_APP_ICON as usize as *const u16))
        {
            return icon;
        }
        LoadIconW(
            None,
            windows::Win32::UI::WindowsAndMessaging::IDI_APPLICATION,
        )
        .unwrap_or_default()
    }
}

/// Per-process state. The main window's `WndProc` reaches into this via the
/// `STATE` thread-local because Win32 callbacks are bare `extern "system"`
/// functions with no closure capture.
struct AppState {
    main: HWND,
    /// Active magnifier: classic or GPU. `None` only before `run_message_loop` runs.
    mag: Option<Renderer>,
    /// Current pipeline (must match the active `Renderer` variant where `mag` is `Some`).
    renderer_kind: RendererKind,
    /// Transparent layered child above the magnifier; captures double-clicks.
    hit_overlay: Option<HWND>,
    /// `WM_TIMER` for classic `MagSetWindowSource` refresh. `None` for GPU.
    timer_id: Option<usize>,
    current_source: Option<RECT>,
    /// Whether a global hotkey is currently registered.
    hotkey_registered: bool,
    /// The currently bound hotkey combo, if any.
    current_hotkey: Option<(HOT_KEY_MODIFIERS, u32)>,
    /// True while the window is in borderless-fullscreen mode.
    is_fullscreen: bool,
    /// Window style and placement saved before entering fullscreen.
    pre_fullscreen: Option<(isize, WINDOWPLACEMENT)>,
    /// Persistent on-screen red outline of the current source rectangle.
    /// Lazily created on the first `apply_source` and reused thereafter; the
    /// host owns its `HWND` lifetime (`DestroyWindow` in `WM_DESTROY`).
    source_frame: Option<HWND>,
    /// Whether the source frame should be visible. Toggled via the tray menu.
    /// Defaults to `true`; not persisted across runs.
    source_frame_visible: bool,
}

thread_local! {
    static STATE: RefCell<Option<AppState>> = const { RefCell::new(None) };
}

const MAIN_CLASS: &str = "loupe.main";
const REFRESH_TIMER_ID: usize = 1;
const REFRESH_TIMER_MS: u32 = 16; // ~60 Hz

fn main() {
    dpi::enable_per_monitor_v2();

    if !magnifier::init() {
        eprintln!("lens: MagInitialize failed");
        return;
    }

    let cfg = config::load_config();

    let main = match create_main_window(cfg.renderer) {
        Some(h) => h,
        None => {
            eprintln!("lens: failed to create main window");
            magnifier::shutdown();
            return;
        }
    };

    STATE.with(|s| {
        *s.borrow_mut() = Some(AppState {
            main,
            mag: None,
            renderer_kind: RendererKind::Classic,
            hit_overlay: None,
            timer_id: None,
            current_source: None,
            hotkey_registered: false,
            current_hotkey: None,
            is_fullscreen: false,
            pre_fullscreen: None,
            source_frame: None,
            source_frame_visible: true,
        });
    });

    // Restore saved hotkey from %APPDATA%\loupe\config.toml (if any).
    if let Some(hk) = cfg.hotkey {
        let mods = HOT_KEY_MODIFIERS(hk.mods);
        let vkey = hk.vkey;
        let ok = hotkey::register(main, mods, vkey);
        STATE.with(|s| {
            if let Some(st) = s.borrow_mut().as_mut() {
                st.hotkey_registered = ok;
                if ok {
                    st.current_hotkey = Some((mods, vkey));
                }
            }
        });
    }

    if !tray::add(main) {
        eprintln!("lens: failed to add tray icon");
    }

    run_message_loop();

    // Teardown — best-effort, in reverse order of creation.
    tray::remove(main);
    let state = STATE.with(|s| s.borrow_mut().take());
    if let Some(st) = state {
        if st.hotkey_registered {
            hotkey::unregister(st.main);
        }
        if let Some(id) = st.timer_id {
            unsafe {
                let _ = KillTimer(Some(st.main), id);
            }
        }
        if let Some(f) = st.source_frame {
            unsafe {
                let _ = DestroyWindow(f);
            }
        }
    }
    magnifier::shutdown();
}

fn create_main_window(preferred_renderer: RendererKind) -> Option<HWND> {
    let class = wstr(MAIN_CLASS);
    let title = wstr("Loupe");
    unsafe {
        let hmodule = GetModuleHandleW(None).ok()?;
        let instance: HINSTANCE = hmodule.into();
        let icon = load_app_icon();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(main_wnd_proc),
            hInstance: instance,
            lpszClassName: PCWSTR(class.as_ptr()),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: icon,
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);

        // WS_EX_LAYERED is required by `WC_MAGNIFIER` on its host window.
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_LAYERED,
            PCWSTR(class.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            640,
            480,
            None,
            Some(HMENU::default()),
            Some(instance),
            None,
        )
        .ok()?;
        if hwnd.is_invalid() {
            return None;
        }

        // Fully opaque — the layered attribute only needs to exist for
        // the magnifier child to render correctly.
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);

        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);

        let (renderer, _effective_kind) = {
            let mut k = preferred_renderer;
            loop {
                if let Some(r) = Renderer::new(k, hwnd, client, instance) {
                    let eff = if matches!(&r, Renderer::Gpu(_)) {
                        RendererKind::Gpu
                    } else {
                        RendererKind::Classic
                    };
                    break (r, eff);
                }
                if k == RendererKind::Gpu {
                    eprintln!(
                        "lens: GPU renderer (WGC/D3D11) init failed, falling back to classic magnifier"
                    );
                    k = RendererKind::Classic;
                    continue;
                }
                eprintln!("lens: could not create magnifier child");
                return None;
            }
        };

        let overlay = magnifier::create_hit_overlay(hwnd, client, instance);
        if overlay.is_none() {
            eprintln!("loupe: warning: hit overlay unavailable; double-click fullscreen disabled");
        }

        let timer = if matches!(&renderer, Renderer::Classic(_)) {
            let t = SetTimer(Some(hwnd), REFRESH_TIMER_ID, REFRESH_TIMER_MS, None);
            (t != 0).then_some(t)
        } else {
            None
        };
        if matches!(&renderer, Renderer::Classic(_)) && timer.is_none() {
            eprintln!("lens: SetTimer(REFRESH) failed; classic path may stutter");
        }

        set_post_create(renderer, overlay, timer);

        Some(hwnd)
    }
}

type PostCreateBundle = (Renderer, Option<HWND>, Option<usize>);

fn set_post_create(mag: Renderer, overlay: Option<HWND>, refresh_timer: Option<usize>) {
    POST_CREATE.with(|c| {
        *c.borrow_mut() = Some((mag, overlay, refresh_timer));
    });
}

thread_local! {
    static POST_CREATE: RefCell<Option<PostCreateBundle>> = const { RefCell::new(None) };
}

fn run_message_loop() {
    if let Some((mag, overlay, timer)) = POST_CREATE.with(|c| c.borrow_mut().take()) {
        let rk = match &mag {
            Renderer::Classic(_) => RendererKind::Classic,
            Renderer::Gpu(_) => RendererKind::Gpu,
        };
        STATE.with(|s| {
            if let Some(st) = s.borrow_mut().as_mut() {
                st.mag = Some(mag);
                st.renderer_kind = rk;
                st.hit_overlay = overlay;
                st.timer_id = timer;
            }
        });
    }

    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn main_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_HOTKEY => {
                if wparam.0 as i32 == hotkey::HOTKEY_ID_NEW_LENS {
                    region::show(hwnd);
                }
                LRESULT(0)
            }
            WM_APP_TRAY => match (lparam.0 as u32) & 0xFFFF {
                WM_LBUTTONUP => {
                    region::show(hwnd);
                    LRESULT(0)
                }
                WM_RBUTTONUP => {
                    let (label, renderer, frame_visible) = STATE.with(|s| {
                        s.borrow()
                            .as_ref()
                            .map_or((None, RendererKind::Classic, true), |st| {
                                let lab = st
                                    .current_hotkey
                                    .map(|(mods, vkey)| hotkey_bind::format_binding(mods, vkey));
                                (lab, st.renderer_kind, st.source_frame_visible)
                            })
                    });
                    tray::show_menu(hwnd, label.as_deref(), renderer, frame_visible);
                    LRESULT(0)
                }
                _ => LRESULT(0),
            },
            WM_COMMAND => {
                let id = (wparam.0 as u32) & 0xFFFF;
                match id {
                    IDM_NEW_LENS => region::show(hwnd),
                    IDM_BIND_HOTKEY => {
                        let current =
                            STATE.with(|s| s.borrow().as_ref().and_then(|st| st.current_hotkey));
                        hotkey_bind::show(hwnd, current);
                    }
                    IDM_RENDERER_CLASSIC => {
                        switch_renderer(hwnd, RendererKind::Classic);
                    }
                    IDM_RENDERER_GPU => {
                        switch_renderer(hwnd, RendererKind::Gpu);
                    }
                    IDM_TOGGLE_SOURCE_FRAME => {
                        toggle_source_frame_visibility();
                    }
                    IDM_QUIT => {
                        let _ = DestroyWindow(hwnd);
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            x if x == WM_APP_HOTKEY_BOUND => {
                if wparam.0 == 1 {
                    let (mods, vkey) = hotkey_bind::unpack(lparam);
                    STATE.with(|s| {
                        if let Some(st) = s.borrow_mut().as_mut() {
                            if st.hotkey_registered {
                                hotkey::unregister(st.main);
                            }
                            st.hotkey_registered = hotkey::register(st.main, mods, vkey);
                            if st.hotkey_registered {
                                st.current_hotkey = Some((mods, vkey));
                                config::save_hotkey(mods.0, vkey);
                            } else {
                                eprintln!("lens: hotkey already in use by another app");
                            }
                        }
                    });
                }
                LRESULT(0)
            }
            x if x == WM_APP_REGION_DONE => {
                if wparam.0 == 1
                    && let Some(rect) = region::take_last()
                {
                    apply_source(hwnd, rect);
                }
                LRESULT(0)
            }
            x if x == WM_APP_TOGGLE_FULLSCREEN => {
                toggle_fullscreen(hwnd);
                LRESULT(0)
            }
            x if x == WM_APP_SOURCE_MOVE_TO => {
                let nx = wparam.0 as isize as i32;
                let ny = lparam.0 as i32;
                move_source_to(hwnd, nx, ny);
                LRESULT(0)
            }
            x if x == WM_APP_WHEEL_ZOOM => {
                wheel_zoom_source(hwnd, lparam);
                LRESULT(0)
            }
            WM_SIZE => {
                let mut client = RECT::default();
                let _ = GetClientRect(hwnd, &mut client);
                let cw = client.right - client.left;
                let ch = client.bottom - client.top;
                STATE.with(|s| {
                    if let Some(st) = s.borrow_mut().as_mut() {
                        if let Some(m) = st.mag.as_ref() {
                            m.resize_to(client);
                            if let Some(src) = st.current_source {
                                // Keep `current_source` and the on-desktop red
                                // frame literally identical to the user's
                                // selection. `cap_source_for_renderer` is
                                // intentionally NOT applied here — letting the
                                // host shrink without dragging the source
                                // along is the price for that guarantee.
                                m.fit_source(cw, ch, src);
                            }
                        }
                        if let Some(o) = st.hit_overlay {
                            magnifier::resize_to(o, client);
                            magnifier::elevate_above_siblings(o);
                        }
                    }
                });
                LRESULT(0)
            }
            WM_TIMER => {
                if wparam.0 == REFRESH_TIMER_ID {
                    STATE.with(|s| {
                        if let Some(st) = s.borrow().as_ref() {
                            if st.renderer_kind != RendererKind::Classic {
                                return;
                            }
                            if let (Some(m), Some(src)) = (st.mag.as_ref(), st.current_source) {
                                m.set_source(src);
                            }
                        }
                    });
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                // Hide the magnifier window but keep the process alive (tray remains).
                // The user can reopen it via "New loupe". Real exit is via tray → Quit.
                // Also hide the red source frame so it doesn't linger on the desktop.
                STATE.with(|s| {
                    if let Some(st) = s.borrow().as_ref()
                        && let Some(f) = st.source_frame
                    {
                        source_frame::hide(f);
                    }
                });
                let _ = ShowWindow(hwnd, SW_HIDE);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// Toggle borderless fullscreen on `hwnd`.
///
/// Entering: saves the current window style and placement, strips
/// `WS_OVERLAPPEDWINDOW` (title bar + borders), then calls `SetWindowPos`
/// to cover the entire monitor rectangle.
///
/// Exiting: restores the saved style via `SetWindowLongPtrW` with
/// `SWP_FRAMECHANGED`, then calls `SetWindowPlacement` to return to the
/// previous size and position.
fn toggle_fullscreen(hwnd: HWND) {
    let currently = STATE.with(|s| {
        s.borrow()
            .as_ref()
            .map(|st| (st.is_fullscreen, st.pre_fullscreen))
    });
    let Some((is_fullscreen, pre)) = currently else {
        return;
    };

    unsafe {
        if is_fullscreen {
            // Restore windowed mode.
            if let Some((saved_style, saved_wp)) = pre {
                SetWindowLongPtrW(hwnd, GWL_STYLE, saved_style);
                let _ = SetWindowPlacement(hwnd, &saved_wp);
                // SWP_FRAMECHANGED forces re-evaluation of the new style.
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOP),
                    0,
                    0,
                    0,
                    0,
                    SET_WINDOW_POS_FLAGS(0x0020 | 0x0200 | 0x0001 | 0x0002),
                    // SWP_FRAMECHANGED | SWP_NOOWNERZORDER | SWP_NOSIZE | SWP_NOMOVE
                );
            }
            STATE.with(|s| {
                if let Some(st) = s.borrow_mut().as_mut() {
                    st.is_fullscreen = false;
                    st.pre_fullscreen = None;
                }
            });
        } else {
            // Enter borderless fullscreen.
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            let mut wp = WINDOWPLACEMENT {
                length: size_of::<WINDOWPLACEMENT>() as u32,
                ..Default::default()
            };
            let _ = GetWindowPlacement(hwnd, &mut wp);

            // Identify which monitor the window lives on.
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let _ = GetMonitorInfoW(monitor, &mut mi);
            let r = mi.rcMonitor;

            // Strip the frame style so SetWindowPos gives us a bare surface.
            SetWindowLongPtrW(hwnd, GWL_STYLE, style & !(WS_OVERLAPPEDWINDOW.0 as isize));
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOP),
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SET_WINDOW_POS_FLAGS(0x0020 | 0x0200), // SWP_FRAMECHANGED | SWP_NOOWNERZORDER
            );

            STATE.with(|s| {
                if let Some(st) = s.borrow_mut().as_mut() {
                    st.is_fullscreen = true;
                    st.pre_fullscreen = Some((style, wp));
                }
            });
        }
    }
}

/// Teardown the active magnifier, build the requested one, and re-apply
/// `current_source` if set. On GPU init failure, falls back to classic and
/// updates config.
fn switch_renderer(hwnd: HWND, requested: RendererKind) {
    let instance: HINSTANCE = match unsafe { GetModuleHandleW(None) } {
        Ok(m) => m.into(),
        Err(_) => return,
    };
    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
    }
    let cw = client.right - client.left;
    let ch = client.bottom - client.top;

    let effective = STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            if st.renderer_kind == requested {
                return None;
            }
            if let Some(id) = st.timer_id.take() {
                unsafe {
                    let _ = KillTimer(Some(st.main), id);
                }
            }
            if let Some(o) = st.hit_overlay.take() {
                unsafe {
                    let _ = DestroyWindow(o);
                }
            }
            st.mag.take();

            let mut k = requested;
            let mag_opt: Option<Renderer> = loop {
                if let Some(r) = Renderer::new(k, st.main, client, instance) {
                    break Some(r);
                }
                if k == RendererKind::Gpu {
                    eprintln!(
                        "lens: GPU renderer (WGC/D3D11) init failed, falling back to classic magnifier"
                    );
                    k = RendererKind::Classic;
                    continue;
                }
                eprintln!("lens: could not create magnifier child after renderer switch");
                break None;
            };

            let mag = mag_opt?;

            st.renderer_kind = match &mag {
                Renderer::Classic(_) => RendererKind::Classic,
                Renderer::Gpu(_) => RendererKind::Gpu,
            };
            st.mag = Some(mag);
            st.hit_overlay = magnifier::create_hit_overlay(st.main, client, instance);
            if st.hit_overlay.is_none() {
                eprintln!("loupe: warning: hit overlay unavailable; double-click fullscreen disabled");
            }
            if st.renderer_kind == RendererKind::Classic {
                let t = unsafe { SetTimer(Some(st.main), REFRESH_TIMER_ID, REFRESH_TIMER_MS, None) };
                st.timer_id = (t != 0).then_some(t);
            } else {
                st.timer_id = None;
            }
            if let Some(src) = st.current_source
                && let Some(m) = st.mag.as_ref()
            {
                m.fit_source(cw, ch, src);
            }
            return Some(st.renderer_kind);
        }
        None
    });

    if let Some(kind) = effective {
        config::save_renderer(kind);
    }
}

/// Virtual desktop bounds (union of all monitors). Used to keep the magnified
/// source rectangle inside the visible screen area; once `MagSetWindowSource` is
/// asked for a region that extends past the desktop, it intermittently fails to
/// deliver a frame and the user sees black flashes.
fn virtual_screen_rect() -> RECT {
    unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let w = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1);
        let h = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1);
        RECT {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        }
    }
}

/// Minimum allowed magnification factor for the Classic (`WC_MAGNIFIER`)
/// renderer. The legacy Magnification API becomes visibly jittery (intermittent
/// dropped frames / black specks) somewhere just above 1.0x and the exact
/// breakdown threshold drifts between systems and Windows builds. We pick a
/// conservative floor of 1.5x rather than chase the API's edge cases. Users
/// who want a sub-1.5x view should switch to the GPU renderer.
const MIN_CLASSIC_SCALE: f32 = 1.5;

/// For the Classic renderer, cap the source `RECT` so that the resulting
/// magnification scale is at least [`MIN_CLASSIC_SCALE`]. The cap shrinks
/// `src` around its center; the GPU renderer is returned unchanged.
fn cap_source_for_renderer(src: RECT, cw: i32, ch: i32, kind: RendererKind) -> RECT {
    if kind != RendererKind::Classic {
        return src;
    }
    let max_w = ((cw as f32 / MIN_CLASSIC_SCALE).floor() as i32).max(1);
    let max_h = ((ch as f32 / MIN_CLASSIC_SCALE).floor() as i32).max(1);
    let w = src.right - src.left;
    let h = src.bottom - src.top;
    if w <= max_w && h <= max_h {
        return src;
    }
    let cx = src.left + w / 2;
    let cy = src.top + h / 2;
    let w_new = w.min(max_w);
    let h_new = h.min(max_h);
    RECT {
        left: cx - w_new / 2,
        top: cy - h_new / 2,
        right: cx - w_new / 2 + w_new,
        bottom: cy - h_new / 2 + h_new,
    }
}

/// Clamp `src` to `bounds`. If `src` is larger than `bounds` on an axis, that
/// axis is shrunk to match (no over-cover). Otherwise `src` is translated so it
/// stays fully inside `bounds`, preserving its size.
fn clamp_rect_to_bounds(src: RECT, bounds: RECT) -> RECT {
    let mut s = src;
    let bw = bounds.right - bounds.left;
    let bh = bounds.bottom - bounds.top;
    let sw = s.right - s.left;
    let sh = s.bottom - s.top;

    if sw >= bw {
        s.left = bounds.left;
        s.right = bounds.right;
    } else {
        if s.left < bounds.left {
            let shift = bounds.left - s.left;
            s.left += shift;
            s.right += shift;
        }
        if s.right > bounds.right {
            let shift = s.right - bounds.right;
            s.left -= shift;
            s.right -= shift;
        }
    }
    if sh >= bh {
        s.top = bounds.top;
        s.bottom = bounds.bottom;
    } else {
        if s.top < bounds.top {
            let shift = bounds.top - s.top;
            s.top += shift;
            s.bottom += shift;
        }
        if s.bottom > bounds.bottom {
            let shift = s.bottom - bounds.bottom;
            s.top -= shift;
            s.bottom -= shift;
        }
    }
    s
}

/// Mouse wheel over the hit overlay: scale the source `RECT` about its center
/// (smaller rect = zoom in). `lparam` is the signed wheel delta (±120 per notch).
fn wheel_zoom_source(hwnd: HWND, lparam: LPARAM) {
    let delta = lparam.0 as i32;
    if delta == 0 {
        return;
    }

    // Per WHEEL_DELTA (120): source width/height *= ZOOM_PER_NOTCH^(-delta/120).
    const ZOOM_PER_NOTCH: f32 = 1.1;
    const MIN_SIDE: i32 = 16;
    const MAX_SIDE: i32 = 262_144;

    let scale_factor = ZOOM_PER_NOTCH.powf(-(delta as f32) / 120.0);

    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
    }
    let cw = client.right - client.left;
    let ch = client.bottom - client.top;

    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            let Some(mut src) = st.current_source else {
                return;
            };
            let Some(m) = st.mag.as_ref() else {
                return;
            };

            let w = src.right - src.left;
            let h = src.bottom - src.top;
            if w <= 0 || h <= 0 {
                return;
            }

            let cx = src.left + w / 2;
            let cy = src.top + h / 2;

            let w_new = (w as f32 * scale_factor)
                .round()
                .clamp(MIN_SIDE as f32, MAX_SIDE as f32) as i32;
            let h_new = (h as f32 * scale_factor)
                .round()
                .clamp(MIN_SIDE as f32, MAX_SIDE as f32) as i32;

            if w_new == w && h_new == h {
                return;
            }

            src.left = cx - w_new / 2;
            src.right = cx + w_new / 2;
            src.top = cy - h_new / 2;
            src.bottom = cy + h_new / 2;

            // On Classic, refuse zoom-out steps that would trip the 1.5× floor.
            // We could silently cap the rect (the previous behaviour) but then
            // the on-desktop red frame would no longer match the source the
            // user is asking for. Stopping the wheel keeps `current_source`
            // and the visible frame in lock-step.
            let capped = cap_source_for_renderer(src, cw, ch, st.renderer_kind);
            let would_cap = (capped.right - capped.left) != (src.right - src.left)
                || (capped.bottom - capped.top) != (src.bottom - src.top);
            if would_cap && st.renderer_kind == RendererKind::Classic {
                return;
            }
            src = capped;
            src = clamp_rect_to_bounds(src, virtual_screen_rect());
            st.current_source = Some(src);
            m.fit_source(cw, ch, src);
            if let Some(f) = st.source_frame {
                source_frame::move_to(f, src);
            }
        }
    });
}

/// Move the source rectangle to a new top-left in virtual-screen coordinates,
/// preserving its current size. Triggered by dragging the on-desktop red
/// frame; the resulting rect is clamped to the virtual desktop and the frame
/// is repositioned to match the clamped rect.
fn move_source_to(hwnd: HWND, new_left: i32, new_top: i32) {
    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
    }
    let cw = client.right - client.left;
    let ch = client.bottom - client.top;

    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            let Some(src) = st.current_source else {
                return;
            };
            let Some(m) = st.mag.as_ref() else {
                return;
            };
            let w = src.right - src.left;
            let h = src.bottom - src.top;
            let mut moved = RECT {
                left: new_left,
                top: new_top,
                right: new_left + w,
                bottom: new_top + h,
            };
            moved = clamp_rect_to_bounds(moved, virtual_screen_rect());
            if moved.left == src.left && moved.top == src.top {
                return;
            }
            st.current_source = Some(moved);
            m.fit_source(cw, ch, moved);
            if let Some(f) = st.source_frame {
                source_frame::move_to(f, moved);
            }
        }
    });
}

fn apply_source(hwnd: HWND, src: RECT) {
    // Reshape the loupe so its client area matches the selection's aspect
    // ratio (preserving total area). Without this, a tall selection viewed
    // through a wide loupe would be letterboxed AND silently capped on the
    // Classic renderer, leaving the on-desktop red frame visibly different
    // from what the user dragged. Skipped while in fullscreen.
    match_loupe_aspect_to_selection(hwnd, src);

    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
    }
    let cw = client.right - client.left;
    let ch = client.bottom - client.top;

    let instance: HINSTANCE = match unsafe { GetModuleHandleW(None) } {
        Ok(m) => m.into(),
        Err(_) => HINSTANCE::default(),
    };

    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            // P0 UX guarantee: `current_source` and the on-desktop red frame
            // are *exactly* the rectangle the user dragged — no center-shrink
            // cap applied here. `match_loupe_aspect_to_selection` above has
            // already reshaped the loupe so the natural fit is well above
            // the Classic 1.5× floor for any reasonable selection; if a user
            // does pick something extreme on Classic the picture may flicker
            // but the red frame still tracks the source 1:1.
            st.current_source = Some(src);
            if let Some(m) = st.mag.as_ref() {
                m.fit_source(cw, ch, src);
            }
            if st.source_frame.is_none() && !instance.is_invalid() {
                st.source_frame = source_frame::create(st.main, instance);
            }
            if let Some(f) = st.source_frame {
                source_frame::move_to(f, src);
                if st.source_frame_visible {
                    source_frame::show(f);
                } else {
                    source_frame::hide(f);
                }
            }
        }
    });
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
}

/// Resize the loupe so its client area matches the selection's aspect ratio
/// while **preserving the total client area** (`new_w * new_h ≈ old_w *
/// old_h`). The window is anchored at its current top-left and clamped to
/// the work area of the monitor it currently lives on. Skipped when the
/// host is in borderless fullscreen (the geometry is locked to the monitor
/// rect there). The subsequent `WM_SIZE` will refit the magnifier child
/// using the new dimensions.
fn match_loupe_aspect_to_selection(hwnd: HWND, src: RECT) {
    let is_fullscreen = STATE.with(|s| s.borrow().as_ref().is_some_and(|st| st.is_fullscreen));
    if is_fullscreen {
        return;
    }

    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
    }
    let old_cw = (client.right - client.left).max(1);
    let old_ch = (client.bottom - client.top).max(1);
    let area = old_cw as f64 * old_ch as f64;

    let sw = (src.right - src.left).max(1) as f64;
    let sh = (src.bottom - src.top).max(1) as f64;
    let aspect = sw / sh;

    let new_cw = (area * aspect).sqrt().round().max(64.0) as i32;
    let new_ch = (area / aspect).sqrt().round().max(64.0) as i32;
    if new_cw == old_cw && new_ch == old_ch {
        return;
    }

    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
    let exstyle = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    let mut wr = RECT {
        left: 0,
        top: 0,
        right: new_cw,
        bottom: new_ch,
    };
    unsafe {
        let _ = AdjustWindowRectEx(
            &mut wr,
            WINDOW_STYLE(style),
            false,
            WINDOW_EX_STYLE(exstyle),
        );
    }
    let mut new_w = wr.right - wr.left;
    let mut new_h = wr.bottom - wr.top;

    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut mi = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        let _ = GetMonitorInfoW(monitor, &mut mi);
    }
    let max_w = (mi.rcWork.right - mi.rcWork.left).max(1);
    let max_h = (mi.rcWork.bottom - mi.rcWork.top).max(1);
    if new_w > max_w || new_h > max_h {
        let s = (max_w as f64 / new_w as f64).min(max_h as f64 / new_h as f64);
        new_w = (new_w as f64 * s) as i32;
        new_h = (new_h as f64 * s) as i32;
    }

    let mut wnd = RECT::default();
    unsafe {
        let _ = GetWindowRect(hwnd, &mut wnd);
    }
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            wnd.left,
            wnd.top,
            new_w,
            new_h,
            SET_WINDOW_POS_FLAGS(0x0004 | 0x0010), // SWP_NOZORDER | SWP_NOACTIVATE
        );
    }
}

/// Tray menu handler: flip the persistent flag, then show or hide the frame
/// (re-creating it if it has not yet been instantiated). When toggling on
/// without a `current_source` the frame stays hidden — there is nothing to
/// outline yet.
fn toggle_source_frame_visibility() {
    let instance: HINSTANCE = match unsafe { GetModuleHandleW(None) } {
        Ok(m) => m.into(),
        Err(_) => HINSTANCE::default(),
    };
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            st.source_frame_visible = !st.source_frame_visible;
            if st.source_frame_visible {
                if st.source_frame.is_none() && !instance.is_invalid() {
                    st.source_frame = source_frame::create(st.main, instance);
                }
                if let (Some(f), Some(rect)) = (st.source_frame, st.current_source) {
                    source_frame::move_to(f, rect);
                    source_frame::show(f);
                }
            } else if let Some(f) = st.source_frame {
                source_frame::hide(f);
            }
        }
    });
}
