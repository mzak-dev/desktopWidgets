//! WARNING: do not run this on the development machine. Runs of this and of the
//! app on the real GPU crashed the AMD driver (see ADR-005 and ADR-006). It is kept
//! only as the record of the Phase 0 measurements.
//!
//! Phase 0 gate (plan): does "one window per Instance" hold?
//!
//! N borderless, non-activating, per-pixel-alpha windows, one shared wgpu
//! Device, each with its own DirectComposition swapchain. Prints a report:
//! adapter + alpha modes, idle CPU, per-frame cost, z-order, memory.
//!
//!   cargo run --release --example phase0_spike -- --n 12 --z desktop --hold 40
//!
//! Flags: --n N  --z desktop|abovehost|bottom|normal|topmost
//!        --power high|low  --backend dx12|vulkan|gl  --hold SECS

use std::sync::Arc;
use std::time::{Duration, Instant};

use wayfinder::platform::win32::{self as w32, ZMode};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::{Window, WindowAttributes, WindowId};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    res: [f32; 2],
    half: [f32; 2],
    radius: f32,
    border: f32,
    _p: [f32; 2],
    fill: [f32; 4],
    border_color: [f32; 4],
}

struct Cfg {
    n: usize,
    z: String,
    power: wgpu::PowerPreference,
    backend: wgpu::Backends,
    hold: f32,
    quick: bool,
    latency: u32,
    present: wgpu::PresentMode,
    hwnd: bool,
    nopresent: bool,
}

fn parse_args() -> Cfg {
    let mut cfg = Cfg {
        n: 12,
        z: "desktop".into(),
        power: wgpu::PowerPreference::HighPerformance,
        backend: wgpu::Backends::DX12,
        hold: 0.0,
        quick: false,
        latency: 2,
        present: wgpu::PresentMode::AutoVsync,
        hwnd: false,
        nopresent: false,
    };
    let a: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < a.len() {
        let v = a.get(i + 1).cloned().unwrap_or_default();
        match a[i].as_str() {
            "--n" => cfg.n = v.parse().unwrap_or(12),
            "--z" => cfg.z = v,
            "--power" => {
                cfg.power = if v == "low" {
                    wgpu::PowerPreference::LowPower
                } else {
                    wgpu::PowerPreference::HighPerformance
                }
            }
            "--backend" => {
                cfg.backend = match v.as_str() {
                    "vulkan" => wgpu::Backends::VULKAN,
                    "gl" => wgpu::Backends::GL,
                    _ => wgpu::Backends::DX12,
                }
            }
            "--hold" => cfg.hold = v.parse().unwrap_or(0.0),
            "--latency" => cfg.latency = v.parse().unwrap_or(2),
            "--present" => {
                cfg.present = match v.as_str() {
                    "fifo" => wgpu::PresentMode::Fifo,
                    "mailbox" => wgpu::PresentMode::Mailbox,
                    "immediate" => wgpu::PresentMode::Immediate,
                    "novsync" => wgpu::PresentMode::AutoNoVsync,
                    _ => wgpu::PresentMode::AutoVsync,
                }
            }
            "--nopresent" => {
                cfg.nopresent = true;
                i -= 1;
            }
            "--hwnd" => {
                cfg.hwnd = true;
                i -= 1;
            }
            "--quick" => {
                cfg.quick = true;
                i -= 1; // flag without a value
            }
            _ => {}
        }
        i += 2;
    }
    cfg
}

struct Win {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    surface_cfg: wgpu::SurfaceConfiguration,
    uniform: wgpu::Buffer,
    bind: wgpu::BindGroup,
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

#[derive(PartialEq, Clone, Copy)]
enum Phase {
    Settle,
    Idle,
    AllRedraw,
    OneRedraw,
    Hold,
    Done,
}

struct Spike {
    cfg: Cfg,
    start: Instant,
    gpu: Option<Gpu>,
    wins: Vec<Win>,
    phase: Phase,
    phase_start: Instant,
    cpu_at_phase_start: Duration,
    frames: u64,
    frame_time: Duration,
    report: Vec<String>,
    wakeups: u64,
    events: u64,
}

fn process_cpu() -> Duration {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let (mut c, mut e, mut k, mut u) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    unsafe {
        let _ = GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
    }
    let t = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    Duration::from_nanos((t(k) + t(u)) * 100)
}

fn private_mb() -> f64 {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows::Win32::System::Threading::GetCurrentProcess;
    let mut m = PROCESS_MEMORY_COUNTERS_EX::default();
    m.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    unsafe {
        let _ = GetProcessMemoryInfo(GetCurrentProcess(), &mut m as *mut _ as *mut _, m.cb);
    }
    m.PrivateUsage as f64 / 1048576.0
}

impl Spike {
    fn say(&mut self, s: String) {
        println!("{s}");
        self.report.push(s);
    }

    fn render(&mut self, idx: usize) {
        let (Some(gpu), Some(w)) = (self.gpu.as_ref(), self.wins.get(idx)) else {
            return;
        };
        let t0 = Instant::now();
        use wgpu::CurrentSurfaceTexture as Cst;
        let frame = match w.surface.get_current_texture() {
            Cst::Success(f) | Cst::Suboptimal(f) => f,
            Cst::Outdated | Cst::Lost => {
                w.surface.configure(&gpu.device, &w.surface_cfg);
                return;
            }
            _ => return,
        };
        let view = frame.texture.create_view(&Default::default());
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&gpu.pipeline);
            pass.set_bind_group(0, &w.bind, &[]);
            pass.draw(0..6, 0..1);
        }
        gpu.queue.submit([enc.finish()]);
        gpu.queue.present(frame);
        self.frames += 1;
        self.frame_time += t0.elapsed();
    }

    fn enter(&mut self, p: Phase) {
        self.phase = p;
        self.phase_start = Instant::now();
        self.cpu_at_phase_start = process_cpu();
        self.frames = 0;
        self.frame_time = Duration::ZERO;
        self.wakeups = 0;
        self.events = 0;
    }

    fn cpu_pct(&self) -> f64 {
        let wall = self.phase_start.elapsed().as_secs_f64().max(1e-6);
        (process_cpu() - self.cpu_at_phase_start).as_secs_f64() / wall * 100.0
    }

    fn z_report(&mut self) {
        let ours: Vec<isize> = self
            .wins
            .iter()
            .filter_map(|w| w32::hwnd_of(&w.window))
            .map(|h| h.0 as isize)
            .collect();
        let host = w32::desktop_icon_host().map(|h| h.0 as isize);
        let z = w32::z_order();
        let vis: Vec<_> = z.iter().filter(|e| e.visible).collect();
        let top_ours = vis.iter().position(|e| ours.contains(&e.hwnd));
        let bot_ours = vis.iter().rposition(|e| ours.contains(&e.hwnd));
        let host_pos = vis.iter().position(|e| Some(e.hwnd) == host);
        self.say(format!(
            "z: visible top-level windows={} ours: first@{:?} last@{:?} desktop-icon-host@{:?}",
            vis.len(),
            top_ours,
            bot_ours,
            host_pos
        ));
        if let Some(b) = bot_ours {
            let below: Vec<_> = vis[b + 1..].iter().take(3).map(|e| e.class.clone()).collect();
            self.say(format!("z: directly below our lowest: {below:?}"));
        }
        if let Some(t) = top_ours {
            let above: Vec<_> = vis[..t].iter().rev().take(3).map(|e| e.class.clone()).collect();
            self.say(format!("z: directly above our highest: {above:?}"));
        }
    }

    fn apply_z(&self) {
        let host = w32::desktop_icon_host();
        for w in &self.wins {
            let Some(h) = w32::hwnd_of(&w.window) else { continue };
            match (self.cfg.z.as_str(), host) {
                ("abovehost", Some(host)) => w32::place_above(h, host),
                (z, _) => w32::set_zmode(h, ZMode::parse(z).unwrap_or(ZMode::Desktop)),
            }
        }
    }
}

impl ApplicationHandler for Spike {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if !self.wins.is_empty() {
            return;
        }
        let monitors: Vec<_> = el.available_monitors().collect();
        for m in &monitors {
            let (p, s) = (m.position(), m.size());
            self.say(format!(
                "monitor {:?}: {}x{} at ({},{}) scale {}",
                m.name(),
                s.width,
                s.height,
                p.x,
                p.y,
                m.scale_factor()
            ));
        }
        let mon = monitors.first().expect("no monitor");
        let scale = mon.scale_factor();
        let (cw, ch) = ((200.0 * scale) as u32, (120.0 * scale) as u32);
        let (gap, cols) = ((16.0 * scale) as i32, 6usize);

        let mut windows = Vec::new();
        for i in 0..self.cfg.n {
            let m = &monitors[i % monitors.len()];
            let (col, row) = ((i / monitors.len()) % cols, (i / monitors.len()) / cols);
            let pos = PhysicalPosition::new(
                m.position().x + gap * 2 + col as i32 * (cw as i32 + gap),
                m.position().y + gap * 2 + row as i32 * (ch as i32 + gap),
            );
            let attrs = WindowAttributes::default()
                .with_title(format!("wayfinder spike {i}"))
                .with_decorations(false)
                .with_transparent(true)
                .with_resizable(false)
                .with_visible(false)
                .with_active(false)
                .with_position(pos)
                .with_inner_size(PhysicalSize::new(cw, ch))
                .with_skip_taskbar(true)
                // ADR-001: DComp presents beneath the GDI redirection bitmap.
                .with_no_redirection_bitmap(!self.cfg.hwnd);
            let window = Arc::new(el.create_window(attrs).expect("window"));
            w32::apply_widget_styles(&window);
            windows.push(window);
        }

        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = self.cfg.backend;
        desc.backend_options.dx12.presentation_system = if self.cfg.hwnd {
            wgpu::Dx12SwapchainKind::DxgiFromHwnd
        } else {
            wgpu::Dx12SwapchainKind::DxgiFromVisual
        };
        let instance = wgpu::Instance::new(desc);

        let surfaces: Vec<_> = windows
            .iter()
            .map(|w| instance.create_surface(w.clone()).expect("surface"))
            .collect();

        for a in pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all())) {
            let i = a.get_info();
            self.say(format!("adapter: {} ({:?}, {:?})", i.name, i.backend, i.device_type));
        }
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: self.cfg.power,
            compatible_surface: Some(&surfaces[0]),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .expect("adapter");
        let info = adapter.get_info();
        self.say(format!(
            "CHOSEN adapter ({:?}): {} / {:?} / {:?}",
            self.cfg.power, info.name, info.backend, info.device_type
        ));
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .expect("device");

        let caps = surfaces[0].get_capabilities(&adapter);
        self.say(format!("alpha_modes={:?} present_modes={:?} formats={:?}", caps.alpha_modes, caps.present_modes, caps.formats));
        let alpha = [
            wgpu::CompositeAlphaMode::PreMultiplied,
            wgpu::CompositeAlphaMode::PostMultiplied,
        ]
        .into_iter()
        .find(|m| caps.alpha_modes.contains(m));
        let alpha_mode = match alpha {
            Some(a) => a,
            None if self.cfg.hwnd => wgpu::CompositeAlphaMode::Opaque, // control experiment only
            None => {
                self.say("FAIL: no transparent alpha mode".into());
                el.exit();
                return;
            }
        };
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);

        let shader = device.create_shader_module(wgpu::include_wgsl!("phase0_spike.wgsl"));
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let premul = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState { color: premul, alpha: premul }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        use wgpu::util::DeviceExt;
        let n = self.cfg.n;
        for (i, (window, surface)) in windows.into_iter().zip(surfaces).enumerate() {
            let size = window.inner_size();
            let surface_cfg = wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                color_space: wgpu::SurfaceColorSpace::Auto,
                width: size.width.max(1),
                height: size.height.max(1),
                present_mode: self.cfg.present,
                desired_maximum_frame_latency: self.cfg.latency,
                alpha_mode,
                view_formats: vec![],
            };
            surface.configure(&device, &surface_cfg);
            // Distinct hue per window so a screenshot proves each swapchain is live.
            let h = i as f32 / n as f32 * 6.0;
            let rgb = |o: f32| ((h + o).rem_euclid(6.0) - 3.0).abs().clamp(1.0, 2.0) - 1.0;
            let (r, g, b) = (rgb(0.0), rgb(4.0), rgb(2.0));
            let a = 0.55;
            let margin = 14.0 * window.scale_factor() as f32;
            let u = Uniforms {
                res: [size.width as f32, size.height as f32],
                half: [size.width as f32 * 0.5 - margin, size.height as f32 * 0.5 - margin],
                radius: 18.0 * window.scale_factor() as f32,
                border: 2.0,
                _p: [0.0; 2],
                fill: [r, g, b, a],
                border_color: [1.0, 1.0, 1.0, 0.9],
            };
            let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::bytes_of(&u),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
            });
            self.wins.push(Win { window, surface, surface_cfg, uniform, bind });
        }
        self.gpu = Some(Gpu { device, queue, pipeline, layout });

        for i in 0..self.wins.len() {
            if !self.cfg.nopresent {
                self.render(i);
            }
        }
        for w in &self.wins {
            w.window.set_visible(true);
        }
        self.apply_z();
        self.say(format!("windows up: {} | private MB {:.1}", self.wins.len(), private_mb()));
        self.start = Instant::now();
        self.enter(Phase::Settle);
        el.set_control_flow(ControlFlow::Poll);
    }

    fn window_event(&mut self, _: &ActiveEventLoop, id: WindowId, ev: WindowEvent) {
        self.events += 1;
        if let WindowEvent::RedrawRequested = ev {
            if let Some(i) = self.wins.iter().position(|w| w.window.id() == id) {
                self.render(i);
            }
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.wakeups += 1;
        let e = self.phase_start.elapsed().as_secs_f32();
        match self.phase {
            Phase::Settle if e > 2.0 => {
                self.apply_z();
                self.z_report();
                self.enter(Phase::Idle);
                el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(500)));
            }
            Phase::Idle => {
                el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(500)));
                if e > 5.0 {
                    let cpu = self.cpu_pct();
                    self.say(format!("IDLE 5s (no redraws): cpu {cpu:.3}% wakeups={} window_events={} | private MB {:.1}", self.wakeups, self.events, private_mb()));
                    if self.cfg.quick {
                        self.phase = Phase::Done;
                        el.exit();
                        return;
                    }
                    self.enter(Phase::AllRedraw);
                    el.set_control_flow(ControlFlow::Poll);
                }
            }
            Phase::AllRedraw => {
                for i in 0..self.wins.len() {
                    self.wins[i].window.request_redraw();
                }
                if e > 5.0 {
                    let cpu = self.cpu_pct();
                    let (f, ft) = (self.frames.max(1), self.frame_time);
                    self.say(format!(
                        "ALL {} windows redrawing 5s: {} frames, {:.3} ms/frame avg (CPU-side), process cpu {cpu:.1}% | private MB {:.1}",
                        self.wins.len(), self.frames, ft.as_secs_f64() * 1000.0 / f as f64, private_mb()
                    ));
                    self.enter(Phase::OneRedraw);
                }
            }
            Phase::OneRedraw => {
                self.wins[0].window.request_redraw();
                if e > 5.0 {
                    let cpu = self.cpu_pct();
                    let (f, ft) = (self.frames.max(1), self.frame_time);
                    self.say(format!(
                        "ONE window redrawing 5s: {} frames, {:.3} ms/frame avg, process cpu {cpu:.2}%",
                        self.frames, ft.as_secs_f64() * 1000.0 / f as f64
                    ));
                    self.enter(Phase::Hold);
                    el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(250)));
                }
            }
            Phase::Hold => {
                // Re-assert z-order on a timer: the spike's stand-in for the
                // WM_WINDOWPOSCHANGING/shell-event driven loop of Phase 1.
                self.apply_z();
                el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(250)));
                if e > self.cfg.hold {
                    self.z_report();
                    self.enter(Phase::Done);
                    el.exit();
                }
            }
            _ => {}
        }
    }
}

fn main() {
    let cfg = parse_args();
    println!(
        "phase0_spike: n={} z={} power={:?} backend={:?} hold={}s",
        cfg.n, cfg.z, cfg.power, cfg.backend, cfg.hold
    );
    let el = EventLoop::new().expect("event loop");
    let mut app = Spike {
        cfg,
        start: Instant::now(),
        gpu: None,
        wins: Vec::new(),
        phase: Phase::Settle,
        phase_start: Instant::now(),
        cpu_at_phase_start: Duration::ZERO,
        frames: 0,
        frame_time: Duration::ZERO,
        report: Vec::new(),
        wakeups: 0,
        events: 0,
    };
    el.run_app(&mut app).expect("run");
}
