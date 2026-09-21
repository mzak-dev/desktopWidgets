//! wgpu renderer: consumes a `DrawList`, nothing else (the seam in `draw.rs`).
//! One `Instance`/`Adapter`/`Device`/`Queue` is shared by every window
//! (decision 2); each window owns a `Target` (surface + per-window buffers).
//!
//! DX12 + DirectComposition by default (ADR-001), `Mailbox` presentation
//! (ADR-005), premultiplied alpha everywhere.

use std::collections::HashMap;
use std::sync::Arc;

use glyphon::{Cache as GlyphCache, Color as TextColor, Resolution, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport};
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::draw::*;
use crate::text::TextEngine;

const SHADER: &str = include_str!("shader.wgsl");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power {
    High,
    Low,
    /// The CPU rasteriser ("Microsoft Basic Render Driver"). Slow but it never
    /// touches the vendor GPU driver, so automated tests run on it.
    Software,
}

impl Power {
    /// Anything but an explicit "high"/"software" is Low (ADR-005: High can pin a core).
    pub fn parse(s: &str) -> Power {
        match s.to_ascii_lowercase().as_str() {
            "high" => Power::High,
            "software" | "warp" | "cpu" => Power::Software,
            _ => Power::Low,
        }
    }
    fn wgpu(self) -> wgpu::PowerPreference {
        match self {
            Power::High => wgpu::PowerPreference::HighPerformance,
            Power::Low | Power::Software => wgpu::PowerPreference::LowPower,
        }
    }
    fn fallback(self) -> bool {
        self == Power::Software
    }
}

struct GpuImage {
    bind: wgpu::BindGroup,
    w: u32,
    h: u32,
}

struct Buf {
    buf: wgpu::Buffer,
    cap: u64,
}

impl Buf {
    fn new(device: &wgpu::Device) -> Self {
        Self { buf: Self::make(device, 4096), cap: 4096 }
    }
    fn make(device: &wgpu::Device, cap: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: cap,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }
    fn write(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[u8]) {
        if data.len() as u64 > self.cap {
            self.cap = (data.len() as u64).next_power_of_two();
            self.buf = Self::make(device, self.cap);
        }
        if !data.is_empty() {
            queue.write_buffer(&self.buf, 0, data);
        }
    }
}

/// Swapchain sizes are rounded up to this and given this much slack, so a live
/// drag reconfigures a handful of times instead of once per frame. wgpu resizes
/// a DirectComposition swapchain with `ResizeBuffers`, which its own source
/// calls fragile and which failed under per-frame resizing (see ADR-006).
const BUCKET: u32 = 128;
const MAX_SIDE: u32 = 8192;

fn bucketed(v: u32) -> u32 {
    (v.div_ceil(BUCKET) * BUCKET + BUCKET).min(MAX_SIDE)
}

#[derive(Debug)]
pub enum RenderError {
    /// Skip this frame; nothing is wrong with the device.
    Skip(String),
    /// The surface could not be recovered: rebuild the GPU.
    Lost(String),
}

/// Per-window GPU state.
pub struct Target {
    surface: Option<wgpu::Surface<'static>>,
    window: Option<Arc<Window>>,
    /// The window's drawable size; the swapchain (`cfg`) is at least this big.
    view: (u32, u32),
    failures: u32,
    last_configure: std::time::Instant,
    cfg: wgpu::SurfaceConfiguration,
    globals: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    viewport: Viewport,
    shapes: [Buf; 2],
    imgs: [Buf; 2],
}

impl Target {
    /// Swapchain size (>= `view`).
    pub fn size(&self) -> (u32, u32) {
        (self.cfg.width, self.cfg.height)
    }

    pub fn view(&self) -> (u32, u32) {
        self.view
    }
}

pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub format: wgpu::TextureFormat,
    pub alpha_mode: wgpu::CompositeAlphaMode,
    pub present_mode: wgpu::PresentMode,
    /// Human-readable "adapter / backend / alpha / present" for logs and Settings.
    pub info: String,
    shape_pipe: wgpu::RenderPipeline,
    img_pipe: wgpu::RenderPipeline,
    globals_layout: wgpu::BindGroupLayout,
    img_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    glyph_cache: GlyphCache,
    atlas: TextAtlas,
    text_r: [TextRenderer; 2],
    images: HashMap<String, GpuImage>,
    lost: Arc<std::sync::atomic::AtomicBool>,
    /// How many times any swapchain was reconfigured (a regression counter for tests).
    pub configures: std::cell::Cell<u32>,
}

fn instance_desc(dcomp: bool) -> wgpu::InstanceDescriptor {
    let mut d = wgpu::InstanceDescriptor::new_without_display_handle();
    d.backends = match std::env::var("WAYFINDER_BACKEND").as_deref() {
        Ok("vulkan") => wgpu::Backends::VULKAN,
        Ok("gl") => wgpu::Backends::GL,
        _ => wgpu::Backends::DX12,
    };
    if dcomp {
        d.backend_options.dx12.presentation_system = wgpu::Dx12SwapchainKind::DxgiFromVisual;
    }
    d
}

impl Gpu {
    /// Create the shared device using `window`'s surface to pick a compatible adapter.
    pub fn new(window: &Arc<Window>, power: Power) -> Result<(Gpu, Target), String> {
        let instance = wgpu::Instance::new(instance_desc(true));
        let surface = instance.create_surface(window.clone()).map_err(|e| format!("create surface: {e}"))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: power.wgpu(),
            compatible_surface: Some(&surface),
            force_fallback_adapter: power.fallback(),
            apply_limit_buckets: false,
        }))
        .map_err(|e| format!("no adapter: {e}"))?;
        let caps = surface.get_capabilities(&adapter);
        let alpha_mode = [wgpu::CompositeAlphaMode::PreMultiplied, wgpu::CompositeAlphaMode::PostMultiplied]
            .into_iter()
            .find(|m| caps.alpha_modes.contains(m))
            .ok_or_else(|| format!("no transparent alpha mode on {} ({:?}); see ADR-001", adapter.get_info().name, caps.alpha_modes))?;
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        // ADR-005: Fifo blocks every Present on vblank; Mailbox lets us own the frame clock.
        let present_mode = [wgpu::PresentMode::Mailbox, wgpu::PresentMode::Immediate]
            .into_iter()
            .find(|m| caps.present_modes.contains(m))
            .unwrap_or(wgpu::PresentMode::Fifo);
        let mut gpu = Self::build(instance, adapter, format, alpha_mode, present_mode)?;
        let target = gpu.target_with(surface, window);
        Ok((gpu, target))
    }

    /// Offscreen device for `--render`, tests and screenshots (no window).
    pub fn new_headless(power: Power) -> Result<Gpu, String> {
        let instance = wgpu::Instance::new(instance_desc(false));
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: power.wgpu(),
            compatible_surface: None,
            force_fallback_adapter: power.fallback(),
            apply_limit_buckets: false,
        }))
        .map_err(|e| format!("no adapter: {e}"))?;
        Self::build(instance, adapter, wgpu::TextureFormat::Bgra8Unorm, wgpu::CompositeAlphaMode::Auto, wgpu::PresentMode::Fifo)
    }

    fn build(
        instance: wgpu::Instance,
        adapter: wgpu::Adapter,
        format: wgpu::TextureFormat,
        alpha_mode: wgpu::CompositeAlphaMode,
        present_mode: wgpu::PresentMode,
    ) -> Result<Gpu, String> {
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .map_err(|e| format!("no device: {e}"))?;
        device.on_uncaptured_error(std::sync::Arc::new(|e: wgpu::Error| eprintln!("wayfinder: wgpu error: {e}")));
        let lost = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = lost.clone();
        device.set_device_lost_callback(move |reason, msg| {
            eprintln!("wayfinder: GPU device lost ({reason:?}): {msg}");
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        let ai = adapter.get_info();
        let info = format!("{} / {:?} / {:?} / alpha {:?} / present {:?}", ai.name, ai.backend, ai.device_type, alpha_mode, present_mode);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("wayfinder"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let img_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("image"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // Premultiplied blending: (One, OneMinusSrcAlpha) for colour and alpha.
        let premul = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let targets = [Some(wgpu::ColorTargetState {
            format,
            blend: Some(wgpu::BlendState { color: premul, alpha: premul }),
            write_mask: wgpu::ColorWrites::ALL,
        })];

        let attr = |location, offset, format| wgpu::VertexAttribute { format, offset, shader_location: location };
        use wgpu::VertexFormat::{Float32, Float32x2, Float32x4};
        let shape_attrs = [
            attr(0, 0, Float32x2),
            attr(1, 8, Float32x2),
            attr(2, 16, Float32),
            attr(3, 20, Float32),
            attr(4, 24, Float32),
            attr(5, 28, Float32),
            attr(6, 32, Float32x4),
            attr(7, 48, Float32x4),
            attr(8, 64, Float32x4),
            attr(9, 80, Float32x4),
        ];
        let img_attrs = [
            attr(0, 0, Float32x2),
            attr(1, 8, Float32x2),
            attr(2, 16, Float32),
            attr(3, 20, Float32),
            attr(4, 32, Float32x4),
            attr(5, 48, Float32x4),
        ];
        let mk = |label: &str, layouts: &[Option<&wgpu::BindGroupLayout>], vs: &str, fs: &str, stride: u64, attrs: &[wgpu::VertexAttribute]| {
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(label), bind_group_layouts: layouts, immediate_size: 0 });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout { array_stride: stride, step_mode: wgpu::VertexStepMode::Instance, attributes: attrs })],
                },
                fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(fs), compilation_options: Default::default(), targets: &targets }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let shape_pipe = mk("shapes", &[Some(&globals_layout)], "vs_shape", "fs_shape", size_of::<Inst>() as u64, &shape_attrs);
        let img_pipe = mk("images", &[Some(&globals_layout), Some(&img_layout)], "vs_img", "fs_img", size_of::<ImgInst>() as u64, &img_attrs);

        let glyph_cache = GlyphCache::new(&device);
        let mut atlas = TextAtlas::new(&device, &queue, &glyph_cache, format);
        let text_r = [
            TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None),
            TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None),
        ];
        Ok(Gpu {
            instance,
            adapter,
            device,
            queue,
            format,
            alpha_mode,
            present_mode,
            info,
            shape_pipe,
            img_pipe,
            globals_layout,
            img_layout,
            sampler,
            glyph_cache,
            atlas,
            text_r,
            images: HashMap::new(),
            lost,
            configures: std::cell::Cell::new(0),
        })
    }

    /// True once the driver has reported the device lost (a TDR, say).
    pub fn is_lost(&self) -> bool {
        self.lost.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn target_inner(&self, surface: Option<wgpu::Surface<'static>>, window: Option<Arc<Window>>, w: u32, h: u32) -> Target {
        let (w, h) = if surface.is_some() { (bucketed(w.max(1)), bucketed(h.max(1))) } else { (w, h) };
        let cfg = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: self.format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: w.max(1),
            height: h.max(1),
            present_mode: self.present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: self.alpha_mode,
            view_formats: vec![],
        };
        if let Some(s) = &surface {
            s.configure(&self.device, &cfg);
        }
        let globals = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("globals"),
            contents: bytemuck::cast_slice(&[w as f32, h as f32, 0.0, 0.0]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let globals_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &self.globals_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });
        Target {
            surface,
            window,
            view: (w, h),
            failures: 0,
            last_configure: std::time::Instant::now(),
            cfg,
            globals,
            globals_bind,
            viewport: Viewport::new(&self.device, &self.glyph_cache),
            shapes: [Buf::new(&self.device), Buf::new(&self.device)],
            imgs: [Buf::new(&self.device), Buf::new(&self.device)],
        }
    }

    fn target_with(&mut self, surface: wgpu::Surface<'static>, window: &Arc<Window>) -> Target {
        let s = window.inner_size();
        let mut t = self.target_inner(Some(surface), Some(window.clone()), s.width, s.height);
        t.view = (s.width.max(1), s.height.max(1));
        t
    }

    /// A target for an additional window sharing this device.
    pub fn target_for(&mut self, window: &Arc<Window>) -> Result<Target, String> {
        let surface = self.instance.create_surface(window.clone()).map_err(|e| format!("create surface: {e}"))?;
        let caps = surface.get_capabilities(&self.adapter);
        if !caps.alpha_modes.contains(&self.alpha_mode) {
            return Err(format!("surface lacks alpha mode {:?} (has {:?})", self.alpha_mode, caps.alpha_modes));
        }
        Ok(self.target_with(surface, window))
    }

    /// Reconfigure the swapchain, under an error scope so a failure is reported
    /// rather than left as a silently unconfigured surface.
    fn reconfigure(&self, t: &mut Target, w: u32, h: u32) -> bool {
        t.cfg.width = w.clamp(1, MAX_SIDE);
        t.cfg.height = h.clamp(1, MAX_SIDE);
        t.last_configure = std::time::Instant::now();
        self.configures.set(self.configures.get() + 1);
        let Some(s) = &t.surface else { return true };
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        s.configure(&self.device, &t.cfg);
        match pollster::block_on(scope.pop()) {
            Some(e) => {
                eprintln!("wayfinder: configure {}x{} failed: {e}", t.cfg.width, t.cfg.height);
                false
            }
            None => true,
        }
    }

    /// Replace a broken surface with a fresh one for the same window.
    fn recover_surface(&self, t: &mut Target) -> Result<(), String> {
        let window = t.window.clone().ok_or("no window to recover")?;
        let surface = self.instance.create_surface(window).map_err(|e| format!("create surface: {e}"))?;
        t.surface = Some(surface);
        if self.reconfigure(t, t.cfg.width, t.cfg.height) { Ok(()) } else { Err("configure failed on a fresh surface".into()) }
    }

    /// Tell the target how big its window is. The swapchain only changes when
    /// the window outgrows it (immediately, with slack) or is far smaller than
    /// it for a while, never once per frame.
    pub fn fit(&self, t: &mut Target, w: u32, h: u32) {
        let (w, h) = (w.max(1), h.max(1));
        t.view = (w, h);
        let (cw, ch) = (t.cfg.width, t.cfg.height);
        let grow = w > cw || h > ch;
        let shrink = cw >= w + 4 * BUCKET && ch >= h + 4 * BUCKET && t.last_configure.elapsed() > std::time::Duration::from_millis(600);
        if !(grow || shrink) {
            return;
        }
        let nw = if w > cw || shrink { bucketed(w) } else { cw };
        let nh = if h > ch || shrink { bucketed(h) } else { ch };
        if !self.reconfigure(t, nw, nh) {
            t.failures += 1;
            if self.recover_surface(t).is_ok() {
                t.failures = 0;
            }
        }
    }

    // ---- images ------------------------------------------------------------

    pub fn has_image(&self, id: &str) -> bool {
        self.images.contains_key(id)
    }

    pub fn image_size(&self, id: &str) -> Option<(u32, u32)> {
        self.images.get(id).map(|i| (i.w, i.h))
    }

    /// Upload straight-alpha RGBA8.
    pub fn upload_image(&mut self, id: &str, rgba: &[u8], w: u32, h: u32) {
        let tex = self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some(id),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            rgba,
        );
        let view = tex.create_view(&Default::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(id),
            layout: &self.img_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        self.images.insert(id.to_string(), GpuImage { bind, w, h });
    }

    pub fn drop_image(&mut self, id: &str) {
        self.images.remove(id);
    }

    // ---- drawing -----------------------------------------------------------

    fn encode(&mut self, t: &mut Target, list: &DrawList, text: &mut TextEngine, view: &wgpu::TextureView) -> Result<wgpu::CommandBuffer, String> {
        let (w, h) = (t.cfg.width, t.cfg.height);
        self.queue.write_buffer(&t.globals, 0, bytemuck::cast_slice(&[w as f32, h as f32, 0.0, 0.0]));
        t.viewport.update(&self.queue, Resolution { width: w, height: h });

        let TextEngine { fs, swash, slots, .. } = text;
        for i in 0..2 {
            let layer = &list.layers[i];
            let areas: Vec<TextArea> = layer
                .texts
                .iter()
                .filter_map(|it| {
                    let buffer = &slots.get(&it.key)?.buf;
                    let c = it.color;
                    Some(TextArea {
                        buffer,
                        left: it.x,
                        top: it.y,
                        scale: it.scale,
                        bounds: TextBounds { left: it.clip[0] as i32, top: it.clip[1] as i32, right: it.clip[2] as i32, bottom: it.clip[3] as i32 },
                        default_color: TextColor::rgba(c[0], c[1], c[2], c[3]),
                        custom_glyphs: &[],
                    })
                })
                .collect();
            self.text_r[i]
                .prepare(&self.device, &self.queue, fs, &mut self.atlas, &t.viewport, areas, swash)
                .map_err(|e| format!("text prepare: {e}"))?;
            t.shapes[i].write(&self.device, &self.queue, bytemuck::cast_slice(&layer.shapes));
            let imgs: Vec<ImgInst> = layer.images.iter().map(|d| d.inst).collect();
            t.imgs[i].write(&self.device, &self.queue, bytemuck::cast_slice(&imgs));
        }

        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            for i in 0..2 {
                let layer = &list.layers[i];
                if !layer.shapes.is_empty() {
                    pass.set_pipeline(&self.shape_pipe);
                    pass.set_bind_group(0, &t.globals_bind, &[]);
                    pass.set_vertex_buffer(0, t.shapes[i].buf.slice(..));
                    pass.draw(0..6, 0..layer.shapes.len() as u32);
                }
                if !layer.images.is_empty() {
                    pass.set_pipeline(&self.img_pipe);
                    pass.set_bind_group(0, &t.globals_bind, &[]);
                    pass.set_vertex_buffer(0, t.imgs[i].buf.slice(..));
                    for (k, d) in layer.images.iter().enumerate() {
                        if let Some(img) = self.images.get(&d.tex) {
                            pass.set_bind_group(1, &img.bind, &[]);
                            pass.draw(0..6, k as u32..k as u32 + 1);
                        }
                    }
                }
                if !layer.texts.is_empty() {
                    self.text_r[i].render(&self.atlas, &t.viewport, &mut pass).map_err(|e| format!("text render: {e}"))?;
                }
            }
        }
        Ok(enc.finish())
    }

    /// Draw to the window's swapchain and present.
    pub fn render(&mut self, t: &mut Target, list: &DrawList, text: &mut TextEngine) -> Result<(), RenderError> {
        use wgpu::CurrentSurfaceTexture as Cst;
        if self.is_lost() {
            return Err(RenderError::Lost("device lost".into()));
        }
        let acquire = |t: &Target| t.surface.as_ref().map(|s| s.get_current_texture());
        let frame = match acquire(t).ok_or_else(|| RenderError::Skip("target has no surface".into()))? {
            Cst::Success(f) | Cst::Suboptimal(f) => f,
            other => {
                // Outdated / Lost / Validation ("not configured"): put the surface right and retry once.
                let (w, h) = (t.cfg.width, t.cfg.height);
                let fixed = self.reconfigure(t, w, h) || self.recover_surface(t).is_ok();
                let retry = if fixed { acquire(t) } else { None };
                match retry {
                    Some(Cst::Success(f)) | Some(Cst::Suboptimal(f)) => {
                        t.failures = 0;
                        f
                    }
                    _ => {
                        t.failures += 1;
                        let msg = format!("surface unavailable: {other:?}");
                        return Err(if t.failures >= 3 { RenderError::Lost(msg) } else { RenderError::Skip(msg) });
                    }
                }
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let cmd = self.encode(t, list, text, &view).map_err(RenderError::Skip)?;
        self.queue.submit([cmd]);
        self.queue.present(frame);
        self.atlas.trim();
        Ok(())
    }

    /// Render to an RGBA8 image (headless): used by `--render`, tests and docs.
    pub fn render_offscreen(&mut self, w: u32, h: u32, list: &DrawList, text: &mut TextEngine) -> Result<Vec<u8>, String> {
        let mut t = self.target_inner(None, None, w, h);
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let cmd = self.encode(&mut t, list, text, &view)?;
        let row = (w * 4).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit([cmd, enc.finish()]);
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| format!("poll: {e:?}"))?;
        let data = slice.get_mapped_range().map_err(|e| format!("map: {e:?}"))?;
        let bgra = matches!(self.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h as usize {
            let line = &data[y * row as usize..y * row as usize + (w * 4) as usize];
            for px in line.chunks_exact(4) {
                if bgra {
                    out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                } else {
                    out.extend_from_slice(px);
                }
            }
        }
        Ok(out)
    }
}
