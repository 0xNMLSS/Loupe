//! Magnifier: classic `WC_MAGNIFIER` or GPU WGC + D3D11 + Lanczos.

pub mod gpu;
mod legacy;

use windows::Win32::Foundation::HINSTANCE;
use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::RECT;
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, GetParent};

pub use legacy::{
    WM_APP_TOGGLE_FULLSCREEN, WM_APP_WHEEL_ZOOM, create_child, create_hit_overlay,
    elevate_above_siblings, exclude_from_source, resize_to,
};

// Params shared with the GPU path (mutex in `gpu::GpuRenderer`).
pub struct RendererParams {
    pub src: RECT,
    pub child_w: i32,
    pub child_h: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RendererKind {
    Classic,
    Gpu,
}

impl RendererKind {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "classic" | "wc" | "magnification" => Some(Self::Classic),
            "gpu" | "wgc" | "lanczos" => Some(Self::Gpu),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Gpu => "gpu",
        }
    }
}

/// Active magnifier: classic child HWND or the GPU child / pipeline.
pub enum Renderer {
    Classic(HWND),
    Gpu(gpu::GpuRenderer),
}

impl Renderer {
    pub fn new(kind: RendererKind, host: HWND, client: RECT, instance: HINSTANCE) -> Option<Self> {
        match kind {
            RendererKind::Classic => {
                let h = create_child(host, client, instance)?;
                exclude_from_source(h, host);
                Some(Renderer::Classic(h))
            }
            RendererKind::Gpu => gpu::GpuRenderer::new(host, client, instance).map(Renderer::Gpu),
        }
    }

    #[allow(dead_code)] // Public facade; main uses `Renderer` methods directly.
    pub fn child_hwnd(&self) -> HWND {
        match self {
            Self::Classic(h) => *h,
            Self::Gpu(g) => g.child_hwnd(),
        }
    }

    pub fn resize_to(&self, client: RECT) {
        match self {
            Self::Classic(h) => legacy::resize_to(*h, client),
            Self::Gpu(g) => g.resize_to(client),
        }
    }

    /// Classic: re-apply [`legacy::fit_source`] (transform + source). Calling
    /// `MagSetWindowSource` alone on a timer resets the magnification transform on
    /// many systems, which stretches the source to the control and breaks aspect ratio.
    pub fn set_source(&self, src: RECT) {
        match self {
            Self::Classic(h) => unsafe {
                match GetParent(*h) {
                    Ok(parent) if !parent.is_invalid() => {
                        let mut client = RECT::default();
                        let _ = GetClientRect(parent, &mut client);
                        let cw = client.right - client.left;
                        let ch = client.bottom - client.top;
                        legacy::fit_source(*h, cw, ch, src);
                    }
                    _ => legacy::set_source(*h, src),
                }
            },
            Self::Gpu(g) => g.set_source(src),
        }
    }

    pub fn fit_source(&self, child_w: i32, child_h: i32, src: RECT) {
        match self {
            Self::Classic(h) => legacy::fit_source(*h, child_w, child_h, src),
            Self::Gpu(g) => g.fit_source(child_w, child_h, src),
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        use windows::Win32::UI::WindowsAndMessaging::DestroyWindow;
        if let Self::Classic(h) = self {
            unsafe {
                let _ = DestroyWindow(*h);
            }
        }
        // `Gpu` tears down the child HWND in `D3dWindow` + WGC in `GpuRenderer` drop.
    }
}

/// Initialise Win32 Magnification API (no-op for pure GPU, but we always call it).
pub fn init() -> bool {
    legacy::init()
}

pub fn shutdown() {
    legacy::shutdown();
}
