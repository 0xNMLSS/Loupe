use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::Magnification::{
    MagInitialize, MagSetWindowSource, MagUninitialize, WC_MAGNIFIERW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, MoveWindow, WINDOW_EX_STYLE, WS_CHILD, WS_VISIBLE,
};
use windows::core::PCWSTR;

/// Child window id assigned to the magnifier control inside the host window.
const MAG_CHILD_ID: isize = 0x1001;

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
pub fn create_child(host: HWND, client: RECT) -> Option<HWND> {
    let width = client.right - client.left;
    let height = client.bottom - client.top;
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
            Some(windows::Win32::UI::WindowsAndMessaging::HMENU(
                MAG_CHILD_ID as *mut _,
            )),
            None,
            None,
        )
    };
    match hwnd {
        Ok(h) if !h.is_invalid() => Some(h),
        _ => None,
    }
}

/// Resize the magnifier child to fully cover its host's client area.
pub fn resize_to(child: HWND, client: RECT) {
    let width = client.right - client.left;
    let height = client.bottom - client.top;
    unsafe {
        let _ = MoveWindow(child, 0, 0, width, height, true);
    }
}

/// Point the magnifier at a screen rectangle (virtual-screen coordinates).
pub fn set_source(child: HWND, src: RECT) {
    unsafe {
        let _ = MagSetWindowSource(child, src);
    }
}
