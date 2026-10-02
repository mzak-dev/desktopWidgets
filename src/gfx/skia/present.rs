//! Puts CPU pixels in a window with real per-pixel alpha (ADR-0001): a DXGI swapchain
//! made for composition, hung on a DirectComposition visual over the window, and
//! filled by a D3D11 upload. Premultiplied BGRA8, flip model, a non-blocking present
//! so the engine owns the frame clock (ADR-0005).

use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::DirectComposition::{DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_CREATE_FACTORY_FLAGS, DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET,
    DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE, DXGI_GPU_PREFERENCE_MINIMUM_POWER, DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIAdapter, IDXGIAdapter1, IDXGIDevice, IDXGIFactory2,
    IDXGIFactory6, IDXGISwapChain1,
};
use windows::core::Interface;

use crate::gfx::Power;

/// The GPU (or WARP) that presents. Pixels are drawn on the CPU, so it only has to
/// take an upload and show it.
pub struct Device {
    d3d: ID3D11Device,
    ctx: ID3D11DeviceContext,
    factory: IDXGIFactory2,
    dcomp: IDCompositionDevice,
    /// The adapter's name, for the log and Settings.
    pub adapter: String,
    /// WARP, the "Microsoft Basic Render Driver": no GPU is doing the presenting.
    pub software: bool,
}

/// One window's swapchain and the visual that shows it. Dropping it takes the
/// window's content away.
pub struct Swap {
    chain: IDXGISwapChain1,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
}

#[derive(Debug)]
pub enum PresentError {
    /// The device was removed or reset: it has to be rebuilt.
    Lost(String),
    Failed(String),
}

fn name_of(a: &IDXGIAdapter1) -> (String, bool) {
    match unsafe { a.GetDesc1() } {
        Ok(d) => {
            let name = String::from_utf16_lossy(&d.Description);
            (name.trim_end_matches('\0').to_string(), d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0)
        }
        Err(_) => ("unknown adapter".into(), false),
    }
}

impl Device {
    /// `Power::Software` presents on WARP; the others take the integrated or the
    /// dedicated GPU, as the adapter setting says (ADR-0005).
    pub fn new(power: Power) -> Result<Device, String> {
        unsafe {
            let factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).map_err(|e| format!("dxgi factory: {e}"))?;
            let adapter = (power != Power::Software).then(|| pick(&factory, power)).flatten();
            let (adapter, name, software) = match adapter {
                Some(a) => {
                    let (name, software) = name_of(&a);
                    (a.cast::<IDXGIAdapter>().ok(), name, software)
                }
                None => (None, "Microsoft Basic Render Driver".to_string(), true),
            };
            let driver = if adapter.is_some() { D3D_DRIVER_TYPE_UNKNOWN } else { D3D_DRIVER_TYPE_WARP };
            let (mut d3d, mut ctx) = (None, None);
            D3D11CreateDevice(
                adapter.as_ref(),
                driver,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut d3d),
                None,
                Some(&mut ctx),
            )
            .map_err(|e| format!("d3d11 device on {name}: {e}"))?;
            let (d3d, ctx) = d3d.zip(ctx).ok_or("d3d11 returned no device")?;
            let dxgi: IDXGIDevice = d3d.cast().map_err(|e| format!("dxgi device: {e}"))?;
            let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi).map_err(|e| format!("dcomp device: {e}"))?;
            Ok(Device { d3d, ctx, factory, dcomp, adapter: name, software })
        }
    }

    /// A swapchain of `w` x `h` for `hwnd`, shown by a visual on it. The window must
    /// have no redirection bitmap, or DirectComposition draws beneath it (ADR-0001).
    pub fn swap(&self, hwnd: HWND, w: u32, h: u32) -> Result<Swap, String> {
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: w.max(1),
            Height: h.max(1),
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            Flags: 0,
        };
        unsafe {
            let chain = self.factory.CreateSwapChainForComposition(&self.d3d, &desc, None).map_err(|e| format!("swapchain: {e}"))?;
            let target = self.dcomp.CreateTargetForHwnd(hwnd, true).map_err(|e| format!("dcomp target: {e}"))?;
            let visual = self.dcomp.CreateVisual().map_err(|e| format!("dcomp visual: {e}"))?;
            visual.SetContent(&chain).map_err(|e| format!("dcomp content: {e}"))?;
            target.SetRoot(&visual).map_err(|e| format!("dcomp root: {e}"))?;
            self.dcomp.Commit().map_err(|e| format!("dcomp commit: {e}"))?;
            Ok(Swap { chain, _target: target, _visual: visual })
        }
    }

    pub fn resize(&self, swap: &Swap, w: u32, h: u32) -> Result<(), String> {
        unsafe {
            // nothing may hold the old buffers while they are replaced
            self.ctx.ClearState();
            self.ctx.Flush();
            swap.chain.ResizeBuffers(0, w.max(1), h.max(1), DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0)).map_err(|e| format!("resize {w}x{h}: {e}"))
        }
    }

    /// Shows the top-left `w` x `h` of `px`, premultiplied BGRA8 with `stride` bytes a row.
    pub fn present(&self, swap: &Swap, px: &[u8], stride: u32, w: u32, h: u32) -> Result<(), PresentError> {
        debug_assert!(px.len() >= stride as usize * (h as usize).saturating_sub(1) + w as usize * 4);
        unsafe {
            let back: ID3D11Texture2D = swap.chain.GetBuffer(0).map_err(|e| PresentError::Failed(format!("back buffer: {e}")))?;
            let region = D3D11_BOX { left: 0, top: 0, front: 0, right: w, bottom: h, back: 1 };
            self.ctx.UpdateSubresource(&back, 0, Some(&region), px.as_ptr().cast(), stride, 0);
            drop(back);
            let hr = swap.chain.Present(0, DXGI_PRESENT(0));
            if hr == DXGI_ERROR_DEVICE_REMOVED || hr == DXGI_ERROR_DEVICE_RESET {
                return Err(PresentError::Lost(format!("present: {hr:?}")));
            }
            hr.ok().map_err(|e| PresentError::Failed(format!("present: {e}")))
        }
    }

    /// Whether the device has been removed (a driver reset, an unplugged adapter).
    pub fn removed(&self) -> bool {
        unsafe { self.d3d.GetDeviceRemovedReason().is_err() }
    }
}

/// The integrated or the dedicated adapter, falling back to the first one on a
/// Windows too old to rank them.
fn pick(factory: &IDXGIFactory2, power: Power) -> Option<IDXGIAdapter1> {
    let pref = if power == Power::High { DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE } else { DXGI_GPU_PREFERENCE_MINIMUM_POWER };
    unsafe {
        factory
            .cast::<IDXGIFactory6>()
            .ok()
            .and_then(|f| f.EnumAdapterByGpuPreference::<IDXGIAdapter1>(0, pref).ok())
            .or_else(|| factory.EnumAdapters1(0).ok())
    }
}
