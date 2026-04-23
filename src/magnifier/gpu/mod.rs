//! WGC (Windows.Graphics.Capture) + D3D11 + Lanczos upscale path.

mod device;
mod upscale;

use std::sync::{Arc, Mutex, Once};

use device::D3dWindow;
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::Direct3D11CaptureFrame;
use windows::Graphics::Capture::Direct3D11CaptureFramePool;
use windows::Graphics::Capture::GraphicsCaptureItem;
use windows::Graphics::Capture::GraphicsCaptureSession;
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::Direct3D11::IDirect3DSurface;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{E_FAIL, HINSTANCE, HWND, POINT, RECT};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_VIEWPORT,
    ID3D11Resource, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use windows::Win32::Graphics::Dxgi::DXGI_PRESENT;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::GetMonitorInfoW;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST;
use windows::Win32::Graphics::Gdi::MONITORINFO;
use windows::Win32::Graphics::Gdi::MonitorFromPoint;
use windows::Win32::System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::RO_INIT_MULTITHREADED;
use windows::Win32::System::WinRT::RoInitialize;
use windows::core::Interface;
use windows::core::Ref;

use super::RendererParams;
use upscale::{UpscalePass, build_ps_cbuf};

static RO_INIT: Once = Once::new();
static mut RO_OK: bool = true;

fn ro_init() -> bool {
    RO_INIT.call_once(|| {
        if unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.is_err() {
            eprintln!("loupe: RoInitialize failed; GPU capture unavailable");
            unsafe {
                RO_OK = false;
            }
        }
    });
    unsafe { RO_OK }
}

struct Capture {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    _item: GraphicsCaptureItem,
    frame_token: i64,
    hmon: HMONITOR,
    size: SizeInt32,
    /// `Box<Arc<Mutex<GpuCtx>>>` for `FrameArrived` (raw ptr is `Send`); see `on_frame` / `Drop`.
    frame_ctx: *mut Arc<Mutex<GpuCtx>>,
}

struct GpuCtx {
    d3d: D3dWindow,
    d_win: IDirect3DDevice,
    pass: UpscalePass,
    params: RendererParams,
    cap: Option<Capture>,
    /// Self-owned copy of the most recent captured frame. We can't keep the
    /// `Direct3D11CaptureFrame` texture across frames (the WGC buffer pool
    /// recycles N=2 textures), so each `on_frame` does a `CopyResource` into
    /// `cached_tex` and we draw from there. This also lets pan / zoom /
    /// resize trigger an immediate `redraw()` without waiting for WGC to
    /// push another frame — critical when the captured screen is static.
    cached_tex: Option<ID3D11Texture2D>,
    cached_size: SizeInt32,
}

pub struct GpuRenderer {
    inner: Arc<Mutex<GpuCtx>>,
}

impl GpuRenderer {
    pub fn new(host: HWND, client: RECT, instance: HINSTANCE) -> Option<Self> {
        if !ro_init() {
            return None;
        }
        let d3d = D3dWindow::new(host, client, instance)?;
        let dev = d3d.device().clone();
        let dxgi: IDXGIDevice = dev.cast().ok()?;
        let d_win: IDirect3DDevice = {
            let insp = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }.ok()?;
            insp.cast().ok()?
        };
        let pass = UpscalePass::new(&dev)?;
        let (cw, ch) = d3d.size_px();
        // Clippy: WGC + D3D are used from a pool thread with a `usize` cookie; not all Win32 types are `Send`.
        #[allow(clippy::arc_with_non_send_sync)]
        let inner = Arc::new(Mutex::new(GpuCtx {
            d3d,
            d_win,
            pass,
            params: RendererParams {
                src: RECT::default(),
                child_w: cw.max(1) as i32,
                child_h: ch.max(1) as i32,
            },
            cap: None,
            cached_tex: None,
            cached_size: SizeInt32 {
                Width: 0,
                Height: 0,
            },
        }));
        Some(Self { inner })
    }

    #[allow(dead_code)] // Same HWND as `Renderer::child_hwnd` for the GPU path.
    pub fn child_hwnd(&self) -> HWND {
        if let Ok(g) = self.inner.lock() {
            g.d3d.hwnd
        } else {
            HWND::default()
        }
    }

    pub fn resize_to(&self, client: RECT) {
        if let Ok(mut g) = self.inner.lock() {
            g.d3d.resize(client);
            g.params.child_w = (client.right - client.left).max(1);
            g.params.child_h = (client.bottom - client.top).max(1);
        }
        self.redraw();
    }

    pub fn fit_source(&self, child_w: i32, child_h: i32, src: RECT) {
        if let Ok(mut g) = self.inner.lock() {
            g.params.src = src;
            g.params.child_w = child_w;
            g.params.child_h = child_h;
        }
        if let Err(e) = Self::rebuild(self.inner.clone()) {
            eprintln!("loupe: GPU rebuild_capture: {e:?}");
        }
        self.redraw();
    }

    /// For parity with `MagSetWindowSource` at 60Hz — for GPU, only updates `src`
    /// and recreates the capture if the target monitor changes.
    pub fn set_source(&self, src: RECT) {
        let h1 = hmon_of(src);
        let need = if let Ok(mut g) = self.inner.lock() {
            g.params.src = src;
            let valid = src.right - src.left > 1 && src.bottom - src.top > 1;
            if !valid {
                false
            } else {
                g.cap.as_ref().is_none_or(|c| c.hmon.0 != h1.0)
            }
        } else {
            false
        };
        if need && let Err(e) = Self::rebuild(self.inner.clone()) {
            eprintln!("loupe: GPU set_source rebuild: {e:?}");
        }
        self.redraw();
    }

    /// Re-render using `cached_tex` (last frame `CopyResource`d in `on_frame`).
    /// Called when the user pans / zooms / resizes / toggles fullscreen — these
    /// change the destination viewport or source rect mapping but do not
    /// themselves trigger a new WGC frame, so without an explicit redraw the
    /// picture would stay frozen on a static desktop.
    pub fn redraw(&self) {
        let Ok(g) = self.inner.lock() else {
            return;
        };
        let Some(tex) = g.cached_tex.clone() else {
            return;
        };
        let size = g.cached_size;
        if size.Width <= 0 || size.Height <= 0 {
            return;
        }
        let mrect = mon_rect(hmon_of(g.params.src));
        let cbuf = build_ps_cbuf(
            g.params.src,
            mrect,
            g.params.child_w,
            g.params.child_h,
            size.Width,
            size.Height,
        );
        let (w, h) = g.d3d.size_px();
        let vp = D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: w as f32,
            Height: h as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        g.d3d.clear_to([0.0, 0.0, 0.0, 1.0]);
        if let Err(e) = g.pass.draw(
            g.d3d.context(),
            g.d3d.device(),
            g.d3d.rtv(),
            &tex,
            &cbuf,
            &vp,
        ) {
            eprintln!("loupe: GPU redraw pass.draw: {e:?}");
        }
        let _ = unsafe { g.d3d.swapchain().Present(1, DXGI_PRESENT(0)) };
    }

    fn rebuild(shared: Arc<Mutex<GpuCtx>>) -> windows::core::Result<()> {
        let (src, child_w, child_h) = {
            let g = shared
                .lock()
                .map_err(|_| windows::core::Error::new(E_FAIL, "GpuCtx lock poisoned"))?;
            (g.params.src, g.params.child_w, g.params.child_h)
        };
        if src.right - src.left <= 1 || src.bottom - src.top <= 1 {
            return Ok(());
        }
        let hmon = hmon_of(src);
        let iop: IGraphicsCaptureItemInterop =
            windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem = unsafe { iop.CreateForMonitor(hmon) }?;
        let size = item.Size()?;

        {
            let mut g = shared
                .lock()
                .map_err(|_| windows::core::Error::new(E_FAIL, "GpuCtx lock poisoned"))?;
            if let Some(c) = &g.cap
                && c.hmon.0 as isize == hmon.0 as isize
                && c.size.Width == size.Width
                && c.size.Height == size.Height
            {
                return Ok(());
            }
            if let Some(prev) = g.cap.take() {
                let _ = prev.pool.RemoveFrameArrived(prev.frame_token);
                let _ = prev.session.Close();
                let _ = prev.pool.Close();
                if !prev.frame_ctx.is_null() {
                    unsafe {
                        let _ = Box::from_raw(prev.frame_ctx);
                    }
                }
            }
            // The cached frame was for the previous monitor / size. Drop it so
            // that any `redraw()` between now and the first `on_frame` of the
            // new capture doesn't show the wrong content.
            g.cached_tex = None;
            g.cached_size = SizeInt32 {
                Width: 0,
                Height: 0,
            };
        }

        let d_w = {
            let g = shared
                .lock()
                .map_err(|_| windows::core::Error::new(E_FAIL, "GpuCtx lock poisoned"))?;
            g.d_win.clone()
        };
        let pool = Direct3D11CaptureFramePool::Create(
            &d_w,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )?;
        let session = pool.CreateCaptureSession(&item)?;
        let _ = session.SetIsCursorCaptureEnabled(true);
        let _ = session.SetIsBorderRequired(false);
        session.StartCapture()?;

        // `FrameArrived` must be `Send`. In current Rust, `*mut Arc<...>` is only `Send` if the
        // pointee is `Send`; use a `usize` token for the `Box<Arc<...>>` we allocate below.
        let frame_ctx = Box::into_raw(Box::new(shared.clone()));
        let frame_addr = frame_ctx as usize;
        let token = pool.FrameArrived(&TypedEventHandler::new(
            move |p: Ref<Direct3D11CaptureFramePool>, _| on_frame(p, frame_addr),
        ))?;

        {
            let mut g = shared
                .lock()
                .map_err(|_| windows::core::Error::new(E_FAIL, "GpuCtx lock poisoned"))?;
            g.cap = Some(Capture {
                pool,
                session,
                _item: item,
                frame_token: token,
                hmon,
                size,
                frame_ctx,
            });
        }
        let _ = (src, child_w, child_h);
        Ok(())
    }
}

impl Drop for GpuRenderer {
    fn drop(&mut self) {
        if let Ok(mut g) = self.inner.lock()
            && let Some(c) = g.cap.take()
        {
            let _ = c.pool.RemoveFrameArrived(c.frame_token);
            let _ = c.session.Close();
            let _ = c.pool.Close();
            if !c.frame_ctx.is_null() {
                unsafe {
                    let _ = Box::from_raw(c.frame_ctx);
                }
            }
        }
    }
}

fn hmon_of(r: RECT) -> HMONITOR {
    let pt = POINT {
        x: (r.left + r.right) / 2,
        y: (r.top + r.bottom) / 2,
    };
    unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) }
}

fn mon_rect(h: HMONITOR) -> RECT {
    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(h, &mut mi) }.as_bool() {
        mi.rcMonitor
    } else {
        RECT::default()
    }
}

/// `ctx_addr` = `Box::into_raw(Box::new(arc)) as usize` (same `Arc<Mutex<GpuCtx>>` as `GpuRenderer.inner`).
fn on_frame(s: Ref<Direct3D11CaptureFramePool>, ctx_addr: usize) -> windows::core::Result<()> {
    let ctx = ctx_addr as *mut Arc<Mutex<GpuCtx>>;
    let shared: Arc<Mutex<GpuCtx>> = unsafe { &*ctx }.clone();
    let pool = s.as_ref().ok_or(windows::core::Error::new(
        E_FAIL,
        "WGC: null pool in FrameArrived",
    ))?;
    let frame: Direct3D11CaptureFrame = pool.TryGetNextFrame()?;
    let size = match frame.ContentSize() {
        Ok(s) => s,
        Err(e) => {
            let _ = frame.Close();
            return Err(e);
        }
    };
    let surf: IDirect3DSurface = match frame.Surface() {
        Ok(s) => s,
        Err(e) => {
            let _ = frame.Close();
            return Err(e);
        }
    };
    let acc: IDirect3DDxgiInterfaceAccess = surf.cast()?;
    let cap_tex: ID3D11Texture2D = unsafe { acc.GetInterface::<ID3D11Texture2D>()? };

    let mut g = if let Ok(l) = shared.try_lock() {
        l
    } else {
        let _ = frame.Close();
        return Ok(());
    };

    // Copy the WGC pool texture into our self-owned cache so we can keep
    // drawing from it across pan / zoom / resize without waiting for the
    // next frame to arrive.
    if let Err(e) = ensure_cached_tex(&mut g, size) {
        eprintln!("loupe: GPU ensure_cached_tex: {e:?}");
        let _ = frame.Close();
        return Ok(());
    }
    let cached = match g.cached_tex.clone() {
        Some(t) => t,
        None => {
            let _ = frame.Close();
            return Ok(());
        }
    };
    let copy_res: windows::core::Result<()> = (|| {
        let dst: ID3D11Resource = cached.cast()?;
        let src: ID3D11Resource = cap_tex.cast()?;
        unsafe { g.d3d.context().CopyResource(&dst, &src) };
        Ok(())
    })();
    if let Err(e) = copy_res {
        eprintln!("loupe: GPU CopyResource: {e:?}");
        let _ = frame.Close();
        return Ok(());
    }
    g.cached_size = size;

    let mrect = mon_rect(hmon_of(g.params.src));
    let cbuf = build_ps_cbuf(
        g.params.src,
        mrect,
        g.params.child_w,
        g.params.child_h,
        size.Width,
        size.Height,
    );
    let (w, h) = g.d3d.size_px();
    let vp = D3D11_VIEWPORT {
        TopLeftX: 0.0,
        TopLeftY: 0.0,
        Width: w as f32,
        Height: h as f32,
        MinDepth: 0.0,
        MaxDepth: 1.0,
    };
    g.d3d.clear_to([0.0, 0.0, 0.0, 1.0]);
    if let Err(e) = g.pass.draw(
        g.d3d.context(),
        g.d3d.device(),
        g.d3d.rtv(),
        &cached,
        &cbuf,
        &vp,
    ) {
        eprintln!("loupe: GPU pass.draw: {e:?}");
    }
    let _ = unsafe { g.d3d.swapchain().Present(1, DXGI_PRESENT(0)) };
    drop(g);
    let _ = frame.Close();
    Ok(())
}

/// Ensure `g.cached_tex` exists with at least `size` dimensions. (Re)created on
/// first frame after capture rebuild or whenever the captured monitor's size
/// changes (e.g. DPI / resolution change). Format is fixed BGRA8 to match
/// the WGC pool and the upscale shader's SRV.
fn ensure_cached_tex(g: &mut GpuCtx, size: SizeInt32) -> windows::core::Result<()> {
    let need = match &g.cached_tex {
        Some(_) if g.cached_size.Width == size.Width && g.cached_size.Height == size.Height => {
            false
        }
        _ => true,
    };
    if !need {
        return Ok(());
    }
    let desc = D3D11_TEXTURE2D_DESC {
        Width: size.Width.max(1) as u32,
        Height: size.Height.max(1) as u32,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        ..Default::default()
    };
    let mut tex: Option<ID3D11Texture2D> = None;
    unsafe { g.d3d.device().CreateTexture2D(&desc, None, Some(&mut tex)) }?;
    g.cached_tex = tex;
    g.cached_size = size;
    Ok(())
}
