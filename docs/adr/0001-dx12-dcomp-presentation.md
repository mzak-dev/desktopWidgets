# DX12 with DirectComposition for transparent presentation

Per-pixel window alpha is the headline feature, and on Windows it is only reliably available through DirectComposition on DX12. wgpu deliberately reports `Opaque` only for plain HWND swapchains (gfx-rs/wgpu#7117), and Vulkan's alpha support is per-driver. Measured on this machine (RX 9070 XT + AMD iGPU, Phase 0 spike, `examples/phase0_spike.rs`):

- DX12 + `DxgiFromVisual`: `[Auto, Inherit, Opaque, PostMultiplied, PreMultiplied]` on both adapters. 12 windows sharing one `Device` render correctly with real blending.
- Vulkan on the discrete GPU with a plain HWND swapchain: `[Opaque]` only. Transparency is impossible there. sideQM's earlier ADR also records an access violation ~2s after first present on this driver.

Consequences:

- `with_no_redirection_bitmap(true)` is required, and it makes winit skip its `DwmEnableBlurBehindWindow` call, so `with_transparent(true)` is a no-op on this path. Do not "fix" it.
- Premultiplied alpha is a contract: every shader returns `rgb * a`, every blend state is `(One, OneMinusSrcAlpha)`.
- Enumerate `alpha_modes` at runtime and fail loudly, never silently opaque.
- Vulkan and GL stay selectable with `WAYFINDER_BACKEND=vulkan|gl` for re-testing after driver or wgpu updates.
