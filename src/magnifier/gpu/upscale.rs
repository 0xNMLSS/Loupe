// Lanczos-3 fullscreen pass: compile HLSL at runtime, draw a 3-vertex triangle.

use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D::D3D_SRV_DIMENSION_TEXTURE2D;
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D::ID3DInclude;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BUFFER_DESC, D3D11_COMPARISON_ALWAYS, D3D11_CPU_ACCESS_WRITE,
    D3D11_CULL_NONE, D3D11_DEPTH_STENCIL_DESC, D3D11_DEPTH_STENCILOP_DESC,
    D3D11_DEPTH_WRITE_MASK_ZERO, D3D11_FILL_SOLID, D3D11_FILTER_MIN_MAG_MIP_POINT,
    D3D11_MAP_WRITE_DISCARD, D3D11_MAPPED_SUBRESOURCE, D3D11_RASTERIZER_DESC, D3D11_SAMPLER_DESC,
    D3D11_SHADER_RESOURCE_VIEW_DESC, D3D11_SHADER_RESOURCE_VIEW_DESC_0, D3D11_STENCIL_OP_KEEP,
    D3D11_TEX2D_SRV, D3D11_TEXTURE_ADDRESS_BORDER, D3D11_USAGE_DYNAMIC, D3D11_VIEWPORT,
    ID3D11Buffer, ID3D11DepthStencilState, ID3D11Device, ID3D11DeviceContext, ID3D11InputLayout,
    ID3D11PixelShader, ID3D11RasterizerState, ID3D11RenderTargetView, ID3D11Resource,
    ID3D11SamplerState, ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VertexShader,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

use windows::core::Interface;
use windows::core::s;

/// Constant buffer layout (matches HLSL `PsCb`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PsCbuf {
    pub tex_rect: [f32; 4],
    pub view: [f32; 4],
    pub extra: [f32; 4],
}

const PS_CB_SIZE: u32 = 48;

const SHADERS: &str = r#"
cbuffer PsCb : register(b0) {
  float4 texRect;
  float4 view;
  float4 extra;
}
Texture2D screen : register(t0);
SamplerState s0 : register(s0);

struct VsOut { float4 pos : SV_POSITION; };

VsOut vs_main(uint id : SV_VertexID) {
  float2 uv = float2((id << 1) & 2, id & 2);
  VsOut o;
  o.pos = float4(uv * float2(2, -2) + float2(-1, 1), 0, 1);
  return o;
}

float sinc_fn(float x) {
  if (abs(x) < 1e-5) return 1.0;
  float a = 3.1415926535 * x;
  return sin(a) / a;
}
float lanczos_w(float d, float a) {
  if (d <= -a || d >= a) return 0.0;
  return sinc_fn(d) * sinc_fn(d / a);
}

float3 sample_lanczos(float2 tc, int tw, int th) {
  if (tc.x < 0 || tc.y < 0 || tc.x > (float)(tw - 1) || tc.y > (float)(th - 1)) return float3(0,0,0);
  int ic = (int)floor(tc.x);
  int ir = (int)floor(tc.y);
  float fx = tc.x - (float)ic;
  float fy = tc.y - (float)ir;
  float3 acc = 0.0;
  float wsum = 0.0;
  for (int j = -5; j <= 5; j++) {
    for (int i = -5; i <= 5; i++) {
      int xi = ic + i;
      int yj = ir + j;
      if (xi < 0 || yj < 0 || xi >= tw || yj >= th) continue;
      float wx = lanczos_w((float)i - fx, 3.0);
      float wy = lanczos_w((float)j - fy, 3.0);
      float w = wx * wy;
      acc += screen.Load(int3(xi, yj, 0)).rgb * w;
      wsum += w;
    }
  }
  if (wsum < 1e-4) return float3(0,0,0);
  return acc / wsum;
}

float4 ps_main(VsOut i) : SV_Target {
  float2 cp = i.pos.xy;
  float child_w = extra.x;
  float child_h = extra.y;
  int tw = (int)extra.z;
  int th = (int)extra.w;
  if (cp.x < 0 || cp.y < 0 || cp.x >= child_w || cp.y >= child_h) return float4(0,0,0,1);
  if (cp.x < view.x || cp.y < view.y) return float4(0,0,0,1);
  if (cp.x >= view.x + view.z || cp.y >= view.y + view.w) return float4(0,0,0,1);
  float uu = (cp.x - view.x) / view.z;
  float vv = (cp.y - view.y) / view.w;
  float tx = lerp(texRect.x, texRect.z, uu);
  float ty = lerp(texRect.y, texRect.w, vv);
  float3 c = sample_lanczos(float2(tx, ty), tw, th);
  return float4(c, 1.0);
}
"#;

pub struct UpscalePass {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    il: ID3D11InputLayout,
    cb: ID3D11Buffer,
    rs: ID3D11RasterizerState,
    ds: ID3D11DepthStencilState,
    sam: ID3D11SamplerState,
}

impl UpscalePass {
    pub fn new(dev: &ID3D11Device) -> Option<Self> {
        let mut vblob: Option<ID3DBlob> = None;
        let mut pblob: Option<ID3DBlob> = None;
        unsafe {
            D3DCompile(
                SHADERS.as_ptr() as *const _,
                SHADERS.len(),
                s!("loupe.hlsl"),
                None,
                None::<&ID3DInclude>,
                s!("vs_main"),
                s!("vs_5_0"),
                0,
                0,
                &mut vblob,
                None,
            )
            .ok()?;
            D3DCompile(
                SHADERS.as_ptr() as *const _,
                SHADERS.len(),
                s!("loupe.hlsl"),
                None,
                None::<&ID3DInclude>,
                s!("ps_main"),
                s!("ps_5_0"),
                0,
                0,
                &mut pblob,
                None,
            )
            .ok()?;
        }
        let vb = vblob?;
        let pb = pblob?;
        let vcode = unsafe {
            std::slice::from_raw_parts(vb.GetBufferPointer() as *const u8, vb.GetBufferSize())
        };
        let pcode = unsafe {
            std::slice::from_raw_parts(pb.GetBufferPointer() as *const u8, pb.GetBufferSize())
        };

        let mut vs: Option<ID3D11VertexShader> = None;
        let mut ps: Option<ID3D11PixelShader> = None;
        unsafe {
            dev.CreateVertexShader(vcode, None, Some(&mut vs)).ok()?;
            dev.CreatePixelShader(pcode, None, Some(&mut ps)).ok()?;
        }
        let vs = vs?;
        let ps = ps?;

        let mut il: Option<ID3D11InputLayout> = None;
        unsafe { dev.CreateInputLayout(&[], vcode, Some(&mut il)) }.ok()?;
        let il = il?;

        let bd = D3D11_BUFFER_DESC {
            ByteWidth: PS_CB_SIZE,
            Usage: D3D11_USAGE_DYNAMIC,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            ..Default::default()
        };
        let mut cb: Option<ID3D11Buffer> = None;
        unsafe { dev.CreateBuffer(&bd, None, Some(&mut cb)) }.ok()?;
        let cb = cb?;

        let r_desc = D3D11_RASTERIZER_DESC {
            FillMode: D3D11_FILL_SOLID,
            CullMode: D3D11_CULL_NONE,
            ..Default::default()
        };
        let mut rs: Option<ID3D11RasterizerState> = None;
        unsafe { dev.CreateRasterizerState(&r_desc, Some(&mut rs)) }.ok()?;
        let rs = rs?;

        let ds_desc = D3D11_DEPTH_STENCIL_DESC {
            DepthEnable: false.into(),
            DepthWriteMask: D3D11_DEPTH_WRITE_MASK_ZERO,
            DepthFunc: D3D11_COMPARISON_ALWAYS,
            StencilEnable: false.into(),
            StencilReadMask: 0xFF,
            StencilWriteMask: 0xFF,
            FrontFace: D3D11_DEPTH_STENCILOP_DESC {
                StencilFailOp: D3D11_STENCIL_OP_KEEP,
                StencilDepthFailOp: D3D11_STENCIL_OP_KEEP,
                StencilPassOp: D3D11_STENCIL_OP_KEEP,
                StencilFunc: D3D11_COMPARISON_ALWAYS,
            },
            BackFace: D3D11_DEPTH_STENCILOP_DESC {
                StencilFailOp: D3D11_STENCIL_OP_KEEP,
                StencilDepthFailOp: D3D11_STENCIL_OP_KEEP,
                StencilPassOp: D3D11_STENCIL_OP_KEEP,
                StencilFunc: D3D11_COMPARISON_ALWAYS,
            },
        };
        let mut ds: Option<ID3D11DepthStencilState> = None;
        unsafe { dev.CreateDepthStencilState(&ds_desc, Some(&mut ds)) }.ok()?;
        let ds = ds?;

        let sdesc = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_POINT,
            AddressU: D3D11_TEXTURE_ADDRESS_BORDER,
            AddressV: D3D11_TEXTURE_ADDRESS_BORDER,
            AddressW: D3D11_TEXTURE_ADDRESS_BORDER,
            BorderColor: [0.0, 0.0, 0.0, 0.0],
            ..Default::default()
        };
        let mut sam: Option<ID3D11SamplerState> = None;
        unsafe { dev.CreateSamplerState(&sdesc, Some(&mut sam)) }.ok()?;
        let sam = sam?;

        Some(Self {
            vs,
            ps,
            il,
            cb,
            rs,
            ds,
            sam,
        })
    }

    /// Full-screen draw: binds capture texture as t0, updates CB, sets RT/VP from caller.
    pub fn draw(
        &self,
        ctx: &ID3D11DeviceContext,
        dev: &ID3D11Device,
        rtv: &ID3D11RenderTargetView,
        cap_tex: &ID3D11Texture2D,
        cbuf: &PsCbuf,
        vp: &D3D11_VIEWPORT,
    ) -> windows::core::Result<()> {
        let res: ID3D11Resource = cap_tex.cast()?;
        let srv_desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            ViewDimension: D3D_SRV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_SRV {
                    MostDetailedMip: 0,
                    MipLevels: 0xFFFFFFFF,
                },
            },
        };
        let mut srv: Option<ID3D11ShaderResourceView> = None;
        unsafe { dev.CreateShaderResourceView(&res, Some(&srv_desc), Some(&mut srv)) }?;
        let srv = srv.ok_or_else(|| {
            windows::core::Error::new(
                windows::Win32::Foundation::E_FAIL,
                "CreateShaderResourceView",
            )
        })?;

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            ctx.Map(&self.cb, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped))?;
            std::ptr::copy_nonoverlapping(
                cbuf as *const _ as *const u8,
                mapped.pData as *mut u8,
                PS_CB_SIZE as usize,
            );
            ctx.Unmap(&self.cb, 0);
        }

        let cb = [Some(self.cb.clone())];
        let srvs = [Some(srv.clone())];
        let sams = [Some(self.sam.clone())];
        let rt = [Some(rtv.clone())];
        unsafe {
            ctx.OMSetRenderTargets(Some(&rt), None);
            ctx.RSSetViewports(Some(std::slice::from_ref(vp)));
            ctx.OMSetDepthStencilState(&self.ds, 0);
            ctx.RSSetState(&self.rs);
            ctx.IASetInputLayout(&self.il);
            ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.VSSetShader(&self.vs, None);
            ctx.PSSetShader(&self.ps, None);
            ctx.PSSetConstantBuffers(0, Some(&cb));
            ctx.PSSetShaderResources(0, Some(&srvs));
            ctx.PSSetSamplers(0, Some(&sams));
            ctx.Draw(3, 0);
            // Unbind SRV
            let null: [Option<ID3D11ShaderResourceView>; 1] = [None];
            ctx.PSSetShaderResources(0, Some(&null));
        }
        Ok(())
    }
}

/// Map virtual-screen `src` + child into shader constants (v1: clamp to `mon` for texture).
pub fn build_ps_cbuf(
    src: windows::Win32::Foundation::RECT,
    mon: windows::Win32::Foundation::RECT,
    child_w: i32,
    child_h: i32,
    tex_w: i32,
    tex_h: i32,
) -> PsCbuf {
    let tw = tex_w as f32;
    let th = tex_h as f32;
    let mon_w = (mon.right - mon.left).max(1) as f32;
    let mon_h = (mon.bottom - mon.top).max(1) as f32;

    let ix0 = src.left.max(mon.left);
    let iy0 = src.top.max(mon.top);
    let ix1 = src.right.min(mon.right);
    let iy1 = src.bottom.min(mon.bottom);

    let (sw, sh) = if ix1 > ix0 && iy1 > iy0 {
        (ix1 - ix0, iy1 - iy0)
    } else {
        (mon.right - mon.left, mon.bottom - mon.top)
    };
    let sw = sw.max(1) as f32;
    let sh = sh.max(1) as f32;

    let dx0 = (ix0 - mon.left) as f32;
    let dy0 = (iy0 - mon.top) as f32;
    let dx1 = (ix1 - mon.left) as f32;
    let dy1 = (iy1 - mon.top) as f32;
    let tex_l = dx0 / mon_w * tw;
    let tex_t = dy0 / mon_h * th;
    let tex_r = dx1 / mon_w * tw;
    let tex_b = dy1 / mon_h * th;

    let cw = child_w.max(1) as f32;
    let ch = child_h.max(1) as f32;
    let sc = (cw / sw).min(ch / sh);
    let ctw = sw * sc;
    let cth = sh * sc;
    let off_x = (cw - ctw) * 0.5;
    let off_y = (ch - cth) * 0.5;

    PsCbuf {
        tex_rect: [tex_l, tex_t, tex_r, tex_b],
        view: [off_x, off_y, ctw, cth],
        extra: [cw, ch, tw, th],
    }
}
