# A Skia renderer beside wgpu, switchable in Settings

Everything above `src/gfx/` reaches the screen through one flat `DrawList` (`src/draw.rs`), so a second renderer is a second painter for that list, not a second engine. wgpu stays the default. Skia is an opt-in beside it, chosen in **Settings → General → Renderer** (live, no restart) or with `--renderer wgpu|skia`, and built only with `--features skia`. The plan is for wgpu to go once Skia has proven itself on real hardware; until then both ship.

**What is built.** `Gpu` and `Target` are enums over the backends (`src/gfx/mod.rs`). The Skia backend (`src/gfx/skia/`) draws the draw list on the CPU and shows it through the same DirectComposition path as before:

- `paint.rs` paints a `DrawList` onto any Skia canvas with the shader's semantics: shapes, then images, then text per layer, layer 1 above layer 0, a hard clip rectangle per item, straight colours in and premultiplied out. Rects, capsules, arcs, soft shadows (a Gaussian whose sigma matches the shader's smoothstep slope), tinted and feathered images and animation frames are all covered by tests that read pixels back.
- `glyphs.rs` draws text from the cosmic-text layout `text.rs` already produces, at the positions glyphon used, so shaping, measuring and wrapping did not change and no layout moved.
- `present.rs` holds a D3D11 device on the integrated or dedicated adapter (WARP for `software`), a `CreateSwapChainForComposition` swapchain (premultiplied BGRA, flip model, non-blocking present) and the DirectComposition visual on the window (ADR-0001, ADR-0005). The swapchain is sized in buckets as in ADR-0006 (`sizing.rs`).
- A renderer that cannot start, or is lost three times in 90 s, falls back to wgpu for the session and says so in the log and in Settings. A live switch rebuilds every window's target on the other backend; a window that will not take its new target is recreated.

**Why Skia draws on the CPU, and why not Graphite.** The goal was Skia's Graphite backend. As of `skia-safe` 0.153.3 (the newest release) that is not usable here:

- Graphite is exposed for Metal and Vulkan only; there is no Dawn, so no Graphite on D3D12. On Windows that means Vulkan, and a Vulkan swapchain gets no per-pixel window alpha on this setup (ADR-0001), so its pixels would have to reach a DirectComposition swapchain by another route.
- The only routes out of Graphite on Vulkan are a per-frame `read_pixels` readback, or sharing the image with D3D. Sharing needs `BackendTextures::MakeVulkan` or a way to get the image behind a Graphite surface; the released bindings have neither (only Metal has `backend_textures`).
- rust-skia has no prebuilt binary for `graphite` + `vulkan` on Windows at 0.153.3 (its download 404s, while the default feature set downloads fine), so building Skia would need its source tree, LLVM and Python on every machine and CI run.

A GPU-backed Skia would therefore draw on the GPU only to read every frame back to the CPU. The CPU raster surface does the same work without a Vulkan driver, a second GPU API, or a source build, and it also serves the tests, `--render-widget` and the screenshots, which have no GPU. Graphite stays open as a later step: the painter takes any `Canvas`, so a Graphite surface (plus a readback, or shared textures if the bindings gain them) replaces only where the canvas comes from. Not measured: whether CPU raster is cheap enough with many animating windows. That, idle CPU and stability are what the real-hardware check decides.

**Look.** Skia-native shapes, blur and clips replace the hand-written SDF math, so edge anti-aliasing and shadow falloff differ slightly from wgpu; geometry, layout and colours do not. Image feathering is the one small SkSL effect, as there is no native equivalent. Animation frames are sampled strictly inside their cell of the atlas, so a neighbouring frame cannot bleed in at an edge.

**Not verified.** The painter, sizing and option logic are exercised by tests and run on CI. The Windows-only presenter, the live switch and the fallbacks compile but have not run on a real desktop: composition swapchain alpha and corners, idle CPU with several windows, drag and resize, and switching back and forth are what to check first.
