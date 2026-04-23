// D3D11 device, DXGI flip swap chain on a child HWND, and back-buffer RTV.

use std::sync::OnceLock;

use windows::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HINSTANCE, HMODULE, HWND, LRESULT, RECT,
};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_FLAG, D3D11_RENDER_TARGET_VIEW_DESC,
    D3D11_RENDER_TARGET_VIEW_DESC_0, D3D11_RTV_DIMENSION_TEXTURE2D, D3D11_SDK_VERSION,
    D3D11_TEX2D_RTV, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11RenderTargetView,
    ID3D11Resource, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_ALPHA_MODE_IGNORE;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_SWAP_CHAIN_FLAG,
    DXGI_SWAP_EFFECT_FLIP_DISCARD, IDXGIFactory2, IDXGISwapChain1,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, HMENU, IDC_ARROW, LoadCursorW, MoveWindow,
    RegisterClassW, WINDOW_EX_STYLE, WM_DESTROY, WNDCLASSW, WS_CHILD, WS_VISIBLE,
};

use windows::core::IUnknown;
use windows::core::Interface;
use windows::core::PCWSTR;

use crate::wstr;

const GPU_CLASS: &str = "loupe.gpuview";
const GPU_CHILD_ID: isize = 0x1003;

static GPU_CLASS_ATOM: OnceLock<usize> = OnceLock::new();

struct D3dResources {
    rtv: ID3D11RenderTargetView,
    swapchain: IDXGISwapChain1,
    context: ID3D11DeviceContext,
    device: ID3D11Device,
}

/// Child `WS_CHILD` + D3D11 swap chain. On drop: COM objects release first, then `DestroyWindow`.
pub struct D3dWindow {
    pub hwnd: HWND,
    client_w: u32,
    client_h: u32,
    d3d: Option<D3dResources>,
}

impl D3dWindow {
    /// Creates a D3D11 device, a child window, and a flip-model swap chain (BGRA) for `client`.
    pub fn new(host: HWND, client: RECT, instance: HINSTANCE) -> Option<Self> {
        let w = (client.right - client.left).max(1) as u32;
        let h = (client.bottom - client.top).max(1) as u32;

        let class_name = register_gpu_class(instance)?;
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name,
                PCWSTR::null(),
                WS_CHILD | WS_VISIBLE,
                0,
                0,
                w as i32,
                h as i32,
                Some(host),
                Some(HMENU(GPU_CHILD_ID as *mut _)),
                Some(instance),
                None,
            )
        }
        .ok()?;
        if hwnd.is_invalid() {
            return None;
        }

        let feature_levels = [D3D_FEATURE_LEVEL_11_0];
        let mut dev: Option<ID3D11Device> = None;
        let mut fl = windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL(0);
        let mut ctx: Option<ID3D11DeviceContext> = None;
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_FLAG(D3D11_CREATE_DEVICE_BGRA_SUPPORT.0),
                Some(&feature_levels),
                D3D11_SDK_VERSION,
                Some(&mut dev),
                Some(&mut fl),
                Some(&mut ctx),
            )
            .ok()?;
        }
        let device = dev?;
        let context = ctx?;

        let factory: IDXGIFactory2 =
            unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)) }.ok()?;

        let sc_desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: w,
            Height: h,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: windows::core::BOOL(0),
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: 0,
        };

        let d: ID3D11Device = device.clone();
        let u: IUnknown = d.cast().ok()?;
        let swapchain =
            unsafe { factory.CreateSwapChainForHwnd(&u, hwnd, &sc_desc, None, None) }.ok()?;

        let rtv = build_rtv(&device, &swapchain).ok()?;

        let d3d = Some(D3dResources {
            rtv,
            swapchain,
            context,
            device,
        });

        Some(Self {
            hwnd,
            client_w: w,
            client_h: h,
            d3d,
        })
    }

    /// Resize the child, swap chain, and RTV.
    pub fn resize(&mut self, client: RECT) {
        let w = (client.right - client.left).max(1) as u32;
        let h = (client.bottom - client.top).max(1) as u32;
        if w == self.client_w && h == self.client_h {
            return;
        }
        self.client_w = w;
        self.client_h = h;
        unsafe {
            let _ = MoveWindow(self.hwnd, 0, 0, w as i32, h as i32, true);
        }
        let Some(d3d) = &mut self.d3d else {
            return;
        };
        unsafe {
            d3d.context.ClearState();
        }
        if unsafe {
            d3d.swapchain.ResizeBuffers(
                0,
                w,
                h,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                DXGI_SWAP_CHAIN_FLAG(0),
            )
        }
        .is_err()
        {
            return;
        }
        if let Ok(r) = build_rtv(&d3d.device, &d3d.swapchain) {
            d3d.rtv = r;
        }
    }

    pub fn device(&self) -> &ID3D11Device {
        &self.d3d.as_ref().unwrap().device
    }

    pub fn context(&self) -> &ID3D11DeviceContext {
        &self.d3d.as_ref().unwrap().context
    }

    pub fn swapchain(&self) -> &IDXGISwapChain1 {
        &self.d3d.as_ref().unwrap().swapchain
    }

    pub fn rtv(&self) -> &ID3D11RenderTargetView {
        &self.d3d.as_ref().unwrap().rtv
    }

    pub fn size_px(&self) -> (u32, u32) {
        (self.client_w, self.client_h)
    }

    /// Clear the entire swap-chain RTV to a solid (linear) color.
    pub fn clear_to(&self, rgba: [f32; 4]) {
        if let Some(d) = &self.d3d {
            unsafe {
                d.context
                    .OMSetRenderTargets(Some(&[Some(d.rtv.clone())]), None);
            }
            unsafe { d.context.ClearRenderTargetView(&d.rtv, &rgba) };
        }
    }
}

impl Drop for D3dWindow {
    fn drop(&mut self) {
        self.d3d = None;
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn build_rtv(
    dev: &ID3D11Device,
    sc: &IDXGISwapChain1,
) -> windows::core::Result<ID3D11RenderTargetView> {
    let back_buffer: ID3D11Texture2D = unsafe { sc.GetBuffer(0)? };
    let res: ID3D11Resource = back_buffer.cast()?;
    let desc = D3D11_RENDER_TARGET_VIEW_DESC {
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_RENDER_TARGET_VIEW_DESC_0 {
            Texture2D: D3D11_TEX2D_RTV { MipSlice: 0 },
        },
    };
    let mut rtv: Option<ID3D11RenderTargetView> = None;
    unsafe { dev.CreateRenderTargetView(&res, Some(&desc), Some(&mut rtv)) }?;
    rtv.ok_or_else(|| {
        windows::core::Error::new(windows::Win32::Foundation::E_FAIL, "CreateRenderTargetView")
    })
}

fn register_gpu_class(instance: HINSTANCE) -> Option<PCWSTR> {
    let p = *GPU_CLASS_ATOM.get_or_init(|| {
        let name: &'static mut [u16] = Box::leak(wstr(GPU_CLASS).into_boxed_slice());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(gpu_host_wnd_proc),
            hInstance: instance,
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() },
            lpszClassName: PCWSTR(name.as_ptr()),
            ..Default::default()
        };
        let atom = unsafe { RegisterClassW(&wc) };
        if atom == 0 {
            let e = unsafe { GetLastError() };
            if e != ERROR_CLASS_ALREADY_EXISTS {
                eprintln!("loupe: RegisterClassW({GPU_CLASS}) failed: {e:?}");
                return 0;
            }
        }
        name.as_ptr() as usize
    });
    if p == 0 {
        return None;
    }
    Some(PCWSTR(p as *const u16))
}

unsafe extern "system" fn gpu_host_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> LRESULT {
    unsafe {
        if msg == WM_DESTROY {
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}
