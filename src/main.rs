// lens — a tiny live screen-region magnifier for Windows.
//
// Architecture: a single hidden-on-startup main window owns an optional global
// hotkey and the tray icon. Triggering "New lens" (via tray menu or hotkey)
// shows a fullscreen overlay, lets the user drag a rectangle, then opens (or
// reuses) a floating always-on-top host window with a `WC_MAGNIFIER` child
// that mirrors the selected source rect at 60 Hz.

#![cfg(windows)]

mod dpi;
mod hotkey;
mod hotkey_bind;
mod magnifier;
mod region;
mod tray;

use std::cell::RefCell;

use windows::Win32::Foundation::COLORREF;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
    GetMessageW, HICON, HMENU, IDC_ARROW, KillTimer, LWA_ALPHA, LoadCursorW, LoadIconW, MSG,
    PostQuitMessage, RegisterClassW, SW_SHOW, SetLayeredWindowAttributes, SetTimer, ShowWindow,
    TranslateMessage, WM_COMMAND, WM_DESTROY, WM_HOTKEY, WM_LBUTTONUP, WM_RBUTTONUP, WM_SIZE,
    WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_TOPMOST, WS_OVERLAPPEDWINDOW,
};
use windows::core::PCWSTR;

use crate::hotkey_bind::WM_APP_HOTKEY_BOUND;
use crate::region::WM_APP_REGION_DONE;
use crate::tray::{IDM_BIND_HOTKEY, IDM_NEW_LENS, IDM_QUIT, WM_APP_TRAY};

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
    mag_child: Option<HWND>,
    timer_id: Option<usize>,
    current_source: Option<RECT>,
    /// Whether a global hotkey is currently registered.
    hotkey_registered: bool,
}

thread_local! {
    static STATE: RefCell<Option<AppState>> = const { RefCell::new(None) };
}

const MAIN_CLASS: &str = "lens.main";
const REFRESH_TIMER_ID: usize = 1;
const REFRESH_TIMER_MS: u32 = 16; // ~60 Hz

fn main() {
    dpi::enable_per_monitor_v2();

    if !magnifier::init() {
        eprintln!("lens: MagInitialize failed");
        return;
    }

    let main = match create_main_window() {
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
            mag_child: None,
            timer_id: None,
            current_source: None,
            hotkey_registered: false,
        });
    });

    // No default hotkey — user binds one via tray menu ▸ "Bind hotkey…".
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
    }
    magnifier::shutdown();
}

fn create_main_window() -> Option<HWND> {
    let class = wstr(MAIN_CLASS);
    let title = wstr("lens");
    unsafe {
        let hinstance = GetModuleHandleW(None).ok()?;
        let icon = load_app_icon();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(main_wnd_proc),
            hInstance: hinstance.into(),
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
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            640,
            480,
            None,
            Some(HMENU::default()),
            Some(hinstance.into()),
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
        let child = magnifier::create_child(hwnd, client)?;

        let timer = SetTimer(Some(hwnd), REFRESH_TIMER_ID, REFRESH_TIMER_MS, None);
        set_post_create(child, timer);

        Some(hwnd)
    }
}

fn set_post_create(child: HWND, timer: usize) {
    POST_CREATE.with(|c| {
        *c.borrow_mut() = Some((child, timer));
    });
}

thread_local! {
    static POST_CREATE: RefCell<Option<(HWND, usize)>> = const { RefCell::new(None) };
}

fn run_message_loop() {
    if let Some((child, timer)) = POST_CREATE.with(|c| c.borrow_mut().take()) {
        STATE.with(|s| {
            if let Some(st) = s.borrow_mut().as_mut() {
                st.mag_child = Some(child);
                st.timer_id = Some(timer);
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
                WM_RBUTTONUP | WM_LBUTTONUP => {
                    tray::show_menu(hwnd);
                    LRESULT(0)
                }
                _ => LRESULT(0),
            },
            WM_COMMAND => {
                let id = (wparam.0 as u32) & 0xFFFF;
                match id {
                    IDM_NEW_LENS => region::show(hwnd),
                    IDM_BIND_HOTKEY => hotkey_bind::show(hwnd),
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
                            if !st.hotkey_registered {
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
            WM_SIZE => {
                let mut client = RECT::default();
                let _ = GetClientRect(hwnd, &mut client);
                let cw = client.right - client.left;
                let ch = client.bottom - client.top;
                STATE.with(|s| {
                    if let Some(st) = s.borrow().as_ref()
                        && let Some(c) = st.mag_child
                    {
                        magnifier::resize_to(c, client);
                        if let Some(src) = st.current_source {
                            magnifier::fit_source(c, cw, ch, src);
                        }
                    }
                });
                LRESULT(0)
            }
            WM_TIMER => {
                if wparam.0 == REFRESH_TIMER_ID {
                    STATE.with(|s| {
                        if let Some(st) = s.borrow().as_ref()
                            && let (Some(c), Some(src)) = (st.mag_child, st.current_source)
                        {
                            magnifier::set_source(c, src);
                        }
                    });
                }
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

fn apply_source(hwnd: HWND, src: RECT) {
    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
    }
    let cw = client.right - client.left;
    let ch = client.bottom - client.top;

    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            st.current_source = Some(src);
            if let Some(c) = st.mag_child {
                magnifier::fit_source(c, cw, ch, src);
            }
        }
    });
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
}
