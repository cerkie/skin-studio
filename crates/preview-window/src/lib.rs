//! A self-paced preview window. It owns its swapchain and its own event loop (on whatever thread calls `run_blocking`),
//! so it is not at the mercy of a UI toolkit's frame scheduling. One present per display refresh, each frame started
//! right after the compositor took the previous one (PresentMon: 480 displayed frames/s at 480 Hz, 0.0% missed).
//!
//! The editor talks to it through [`Shared`]: swap in a new atlas / map, change settings, play, pause, seek.
pub mod session;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use engine::atlas::Atlas;
use engine::lazer::{self, Obj};
use preview::{build_scene, Instance, Renderer, Settings};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

/// State shared between the editor and the preview thread.
pub struct Shared {
    atlas: Mutex<Arc<Atlas>>,
    atlas_gen: AtomicU64,
    objs: Mutex<Arc<Vec<Obj>>>,
    settings: Mutex<Settings>,
    /// Map-time range the playback loops over (ms).
    section: Mutex<(f32, f32)>,
    playing: AtomicBool,
    /// Playback speed multiplier, as f32 bits.
    speed: AtomicU32,
    seek: Mutex<Option<f32>>,
    stats: Mutex<String>,
    /// Set by the editor to close the window.
    pub close: AtomicBool,
    /// Set by the window when it has closed.
    pub closed: AtomicBool,
}

impl Shared {
    pub fn new(atlas: Atlas, objs: Vec<Obj>) -> Arc<Self> {
        let first = objs.first().map(|o| o.time).unwrap_or(0.0);
        let last = objs.last().map(|o| o.end_time).unwrap_or(first + 1000.0);
        Arc::new(Shared {
            atlas: Mutex::new(Arc::new(atlas)),
            atlas_gen: AtomicU64::new(1),
            objs: Mutex::new(Arc::new(objs)),
            settings: Mutex::new(Settings::default()),
            section: Mutex::new(((first - 1200.0).max(0.0), last + 1500.0 - (first - 1200.0).max(0.0))),
            playing: AtomicBool::new(true),
            speed: AtomicU32::new(1.0f32.to_bits()),
            seek: Mutex::new(None),
            stats: Mutex::new(String::new()),
            close: AtomicBool::new(false),
            closed: AtomicBool::new(true),
        })
    }
    pub fn set_atlas(&self, atlas: Atlas) {
        *self.atlas.lock().unwrap() = Arc::new(atlas);
        self.atlas_gen.fetch_add(1, Ordering::SeqCst);
    }
    pub fn set_objs(&self, objs: Vec<Obj>) {
        *self.objs.lock().unwrap() = Arc::new(objs);
    }
    pub fn set_settings(&self, s: Settings) {
        *self.settings.lock().unwrap() = s;
    }
    pub fn set_section(&self, start: f32, len: f32) {
        *self.section.lock().unwrap() = (start, len.max(100.0));
    }
    pub fn set_playing(&self, p: bool) {
        self.playing.store(p, Ordering::SeqCst);
    }
    pub fn playing(&self) -> bool {
        self.playing.load(Ordering::SeqCst)
    }
    pub fn set_speed(&self, v: f32) {
        self.speed.store(v.to_bits(), Ordering::SeqCst);
    }
    pub fn seek(&self, ms_in_section: f32) {
        *self.seek.lock().unwrap() = Some(ms_in_section);
    }
    pub fn stats(&self) -> String {
        self.stats.lock().unwrap().clone()
    }
}

pub struct Options {
    pub title: String,
    /// Window (and render) size in physical pixels; the scene is laid out for 1280x840.
    pub size: (u32, u32),
    /// Present mode name: "fifo" (default), "mailbox", "immediate".
    pub present: String,
    /// Wait for the compositor after each present (Windows). Strongly recommended with fifo.
    pub pace: bool,
    /// Close by itself after this long (for benchmarks).
    pub exit_after: Option<Duration>,
    /// Software pacing at this rate instead of following the compositor: tests a target such as 1000 Hz on a slower
    /// display (use present "immediate" or "mailbox"). Spin-waits, so it burns a core: a benchmark, not a default.
    pub pace_hz: Option<f32>,
}

impl Default for Options {
    fn default() -> Self {
        Options { title: "Preview".into(), size: (1280, 960), present: "fifo".into(), pace: true, exit_after: None, pace_hz: None }
    }
}

struct Live {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    renderer: Renderer,
    atlas: Arc<Atlas>,
    atlas_gen: u64,
    scratch: Vec<lazer::PointSprite>,
    inst: Vec<Instance>,
    clock_ms: f32,
    last: Instant,
    gaps: Vec<f32>,
    cpu_us: f64,
    frames: u32,
    last_report: Instant,
    end_at: Option<Instant>,
    next_deadline: Instant,
}

struct App {
    shared: Arc<Shared>,
    opts: Options,
    live: Option<Live>,
}

/// Block until the compositor has finished its next pass (Windows). Elsewhere a blocking Fifo present does this.
fn wait_for_refresh() {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::Graphics::Dwm::DwmFlush();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.live.is_some() {
            return;
        }
        let window = Arc::new(
            el.create_window(
                Window::default_attributes()
                    .with_title(self.opts.title.clone())
                    .with_inner_size(PhysicalSize::new(self.opts.size.0, self.opts.size.1)),
            )
            .expect("window"),
        );
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone()).expect("surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .expect("adapter");
        let (device, queue) = pollster::block_on(
            adapter.request_device(&wgpu::DeviceDescriptor { required_limits: adapter.limits(), ..Default::default() }),
        )
        .expect("device");
        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        let size = window.inner_size();
        let mut config = surface.get_default_config(&adapter, size.width.max(1), size.height.max(1)).expect("config");
        config.format = format;
        config.present_mode = match self.opts.present.as_str() {
            "immediate" => wgpu::PresentMode::Immediate,
            "mailbox" => wgpu::PresentMode::Mailbox,
            _ => wgpu::PresentMode::Fifo,
        };
        config.desired_maximum_frame_latency = 1;
        surface.configure(&device, &config);
        let atlas = self.shared.atlas.lock().unwrap().clone();
        let atlas_gen = self.shared.atlas_gen.load(Ordering::SeqCst);
        let renderer = Renderer::with_format(device.clone(), queue.clone(), &atlas, (config.width, config.height), format);
        let section = *self.shared.section.lock().unwrap();
        self.shared.closed.store(false, Ordering::SeqCst);
        self.live = Some(Live {
            window,
            surface,
            config,
            device,
            queue,
            format,
            renderer,
            atlas,
            atlas_gen,
            scratch: vec![],
            inst: vec![],
            clock_ms: section.0,
            last: Instant::now(),
            gaps: vec![],
            cpu_us: 0.0,
            frames: 0,
            last_report: Instant::now(),
            end_at: self.opts.exit_after.map(|d| Instant::now() + d),
            next_deadline: Instant::now(),
        });
        el.set_control_flow(ControlFlow::Poll);
        self.live.as_ref().unwrap().window.request_redraw();
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, ev: WindowEvent) {
        let Some(l) = self.live.as_mut() else { return };
        match ev {
            WindowEvent::CloseRequested => {
                self.shared.closed.store(true, Ordering::SeqCst);
                el.exit();
            }
            WindowEvent::KeyboardInput { event: KeyEvent { logical_key: Key::Named(NamedKey::Space), state: ElementState::Pressed, repeat: false, .. }, .. } => {
                let p = self.shared.playing();
                self.shared.set_playing(!p);
            }
            WindowEvent::Resized(sz) if sz.width > 0 && sz.height > 0 => {
                l.config.width = sz.width;
                l.config.height = sz.height;
                l.surface.configure(&l.device, &l.config);
            }
            WindowEvent::RedrawRequested => {
                if self.shared.close.load(Ordering::SeqCst) {
                    self.shared.closed.store(true, Ordering::SeqCst);
                    el.exit();
                    return;
                }
                let t0 = Instant::now();
                l.gaps.push(t0.duration_since(l.last).as_secs_f32() * 1000.0);
                let dt = t0.duration_since(l.last).as_secs_f32() * 1000.0;
                l.last = t0;
                // a new atlas (the animation changed): re-upload
                let g = self.shared.atlas_gen.load(Ordering::SeqCst);
                if g != l.atlas_gen {
                    l.atlas = self.shared.atlas.lock().unwrap().clone();
                    l.atlas_gen = g;
                    l.renderer = Renderer::with_format(l.device.clone(), l.queue.clone(), &l.atlas, (l.config.width, l.config.height), l.format);
                }
                let (start, len) = *self.shared.section.lock().unwrap();
                if let Some(ms) = self.shared.seek.lock().unwrap().take() {
                    l.clock_ms = start + ms;
                } else if self.shared.playing() {
                    let sp = f32::from_bits(self.shared.speed.load(Ordering::SeqCst));
                    l.clock_ms += dt.min(50.0) * sp;
                }
                if l.clock_ms >= start + len || l.clock_ms < start {
                    l.clock_ms = start + (l.clock_ms - start).rem_euclid(len);
                }
                let frame = match l.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
                    _ => {
                        l.surface.configure(&l.device, &l.config);
                        l.window.request_redraw();
                        return;
                    }
                };
                let view = frame.texture.create_view(&Default::default());
                let objs = self.shared.objs.lock().unwrap().clone();
                let settings = self.shared.settings.lock().unwrap().clone();
                build_scene(&objs, &l.atlas, l.clock_ms, &settings, 2.0, &mut l.scratch, &mut l.inst);
                l.renderer.render_to_view(&view, &l.inst, [22.0 / 255.0, 24.0 / 255.0, 30.0 / 255.0, 1.0]);
                l.window.pre_present_notify();
                l.queue.present(frame);
                let busy = t0.elapsed().as_secs_f64() * 1e6;
                if let Some(hz) = self.opts.pace_hz {
                    l.next_deadline += Duration::from_secs_f64(1.0 / hz as f64);
                    let now = Instant::now();
                    if l.next_deadline < now {
                        l.next_deadline = now; // fell behind: do not try to catch up with a burst
                    }
                    while Instant::now() < l.next_deadline {
                        std::hint::spin_loop();
                    }
                } else if self.opts.pace {
                    wait_for_refresh();
                }
                l.cpu_us += busy;
                l.frames += 1;
                let el_ = l.last_report.elapsed();
                if el_ >= Duration::from_secs(1) {
                    l.gaps.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    let q = |p: f32| l.gaps[((l.gaps.len() - 1) as f32 * p) as usize];
                    let msg = format!(
                        "{:.0} frames/s | {:.0} us work per frame | gap ms median {:.2} p99 {:.2} max {:.2} | {} sprites | map {:.2} s",
                        l.frames as f64 / el_.as_secs_f64(),
                        l.cpu_us / l.frames as f64,
                        q(0.5),
                        q(0.99),
                        q(1.0),
                        l.inst.len(),
                        l.clock_ms / 1000.0
                    );
                    *self.shared.stats.lock().unwrap() = msg;
                    l.gaps.clear();
                    l.frames = 0;
                    l.cpu_us = 0.0;
                    l.last_report = Instant::now();
                }
                if l.end_at.is_some_and(|t| Instant::now() >= t) {
                    self.shared.closed.store(true, Ordering::SeqCst);
                    el.exit();
                    return;
                }
                l.window.request_redraw();
            }
            _ => {}
        }
    }
}

/// Create the window and run its event loop on the calling thread until it closes. May be called from a spawned thread.
pub fn run_blocking(shared: Arc<Shared>, opts: Options) {
    let mut builder = EventLoop::builder();
    #[cfg(windows)]
    {
        use winit::platform::windows::EventLoopBuilderExtWindows;
        builder.with_any_thread(true);
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use winit::platform::wayland::EventLoopBuilderExtWayland;
        use winit::platform::x11::EventLoopBuilderExtX11;
        EventLoopBuilderExtX11::with_any_thread(&mut builder, true);
        EventLoopBuilderExtWayland::with_any_thread(&mut builder, true);
    }
    let event_loop = builder.build().expect("event loop");
    let mut app = App { shared, opts, live: None };
    event_loop.run_app(&mut app).expect("run");
}
