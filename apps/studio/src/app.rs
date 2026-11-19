//! The window: one winit + wgpu surface. The map is drawn into it at the display's refresh rate (one new frame per
//! refresh, paced to the compositor), the interface is egui drawn over it, and both share the same frame.
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::ViewportId;
use preview_window::session::{Prepared, Session};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::studio::Studio;
use crate::theme;
use crate::ui::{self, SessionView, UiOut, PANEL_W};

struct Live {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    studio: Studio,
    session: Option<Session>,
    /// A skin sheet being rasterized for the current window size.
    rescale: Option<std::sync::mpsc::Receiver<(game::skin::Sheet, f32)>>,
    last_resize: Instant,
    last: Instant,
    atlas_seen: u64,
    cs_ar_seen: (f32, f32),
    fps_count: u32,
    fps_since: Instant,
    /// The skin editor was open on the last frame, and the gameplay was playing when it was opened.
    was_in_skin: bool,
    resume_after_skin: bool,
}

pub struct App {
    live: Option<Live>,
}

impl App {
    pub fn new() -> App {
        App { live: None }
    }
}

fn wait_for_refresh() {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::Graphics::Dwm::DwmFlush();
    }
}

/// The interface is laid out in points; this is how many pixels one point is, so the settings panel (570 points wide
/// in lazer's own proportions) keeps a sensible size on a 1080p screen and on a 4K one.
/// The application icon (the title bar, the taskbar and Alt-Tab); the same picture is built into the Windows executable.
fn app_icon() -> Option<winit::window::Icon> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(&include_bytes!("../assets/icon-256.png")[..]));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()?];
    let info = r.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba {
        return None;
    }
    winit::window::Icon::from_rgba(buf[..(info.width * info.height * 4) as usize].to_vec(), info.width, info.height).ok()
}

/// Points per pixel of the interface: the system's scaling times the chosen size, or (automatic) a little more in a tall window.
fn ui_scale(window: &Window, chosen: f32) -> f32 {
    let sf = window.scale_factor() as f32;
    if chosen >= 0.5 {
        return sf * chosen;
    }
    let h = window.inner_size().height as f32;
    (sf * (h / (sf * 900.0)).clamp(1.0, 1.6)).max(1.0)
}

/// The interface sizes to step through with the keyboard and to pick from the settings (as factors; 0 is automatic).
pub const UI_SIZES: [f32; 11] = [0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

/// The next size up or down from `current` (the factor in use now).
pub fn next_ui_size(current: f32, up: bool) -> f32 {
    if up {
        UI_SIZES.iter().copied().find(|s| *s > current + 0.01).unwrap_or(UI_SIZES[UI_SIZES.len() - 1])
    } else {
        UI_SIZES.iter().rev().copied().find(|s| *s < current - 0.01).unwrap_or(UI_SIZES[0])
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.live.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes().with_title("skin studio").with_inner_size(LogicalSize::new(1600.0, 940.0)).with_min_inner_size(LogicalSize::new(1280.0, 720.0));
        if let Some(icon) = app_icon() {
            attrs = attrs.with_window_icon(Some(icon.clone()));
            #[cfg(windows)]
            {
                use winit::platform::windows::WindowAttributesExtWindows;
                attrs = attrs.with_taskbar_icon(Some(icon));
            }
        }
        let window = Arc::new(el.create_window(attrs).expect("window"));
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone()).expect("surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, compatible_surface: Some(&surface), ..Default::default() })).expect("adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { required_limits: adapter.limits(), ..Default::default() })).expect("device");
        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        let size = window.inner_size();
        let mut config = surface.get_default_config(&adapter, size.width.max(1), size.height.max(1)).expect("config");
        config.format = format;
        config.present_mode = wgpu::PresentMode::Fifo;
        config.desired_maximum_frame_latency = 1;
        surface.configure(&device, &config);

        let ctx = egui::Context::default();
        // the interface size is ours (config, Ctrl + and Ctrl -): egui must not zoom on its own as well
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        let studio = Studio::new(ctx.clone());
        theme::install(&ctx);
        let egui_state = egui_winit::State::new(ctx.clone(), ViewportId::ROOT, &*window, Some(window.scale_factor() as f32), window.theme(), Some(device.limits().max_texture_dimension_2d as usize));
        let egui_renderer = egui_wgpu::Renderer::new(&device, format, egui_wgpu::RendererOptions::default());
        self.live = Some(Live { window, surface, config, device, queue, format, ctx, egui_state, egui_renderer, studio, session: None, rescale: None, last_resize: Instant::now(), last: Instant::now(), atlas_seen: 0, cs_ar_seen: (0.0, 0.0), fps_count: 0, fps_since: Instant::now(), was_in_skin: false, resume_after_skin: false });
        el.set_control_flow(ControlFlow::Poll);
        self.live.as_ref().unwrap().window.request_redraw();
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, ev: WindowEvent) {
        let Some(l) = self.live.as_mut() else { return };
        let resp = l.egui_state.on_window_event(&l.window, &ev);
        match ev {
            WindowEvent::CloseRequested => {
                l.session = None;
                el.exit();
            }
            WindowEvent::Resized(sz) if sz.width > 0 && sz.height > 0 => {
                l.config.width = sz.width;
                l.config.height = sz.height;
                l.surface.configure(&l.device, &l.config);
                if let Some(s) = &mut l.session {
                    s.resize((sz.width, sz.height));
                }
                l.last_resize = Instant::now();
            }
            WindowEvent::KeyboardInput { event: KeyEvent { logical_key, state: ElementState::Pressed, repeat: false, .. }, .. } if !resp.consumed && !l.ctx.egui_wants_keyboard_input() => {
                if let Some(s) = &mut l.session {
                    match logical_key {
                        Key::Named(NamedKey::Space) => s.toggle_play(),
                        Key::Named(k @ (NamedKey::ArrowUp | NamedKey::ArrowDown)) => {
                            let v = (l.studio.cfg.volume + if k == NamedKey::ArrowUp { 0.05 } else { -0.05 }).clamp(0.0, 1.0);
                            l.studio.cfg.volume = v;
                            l.studio.cfg.muted = v <= 0.0;
                            l.studio.cfg.save();
                            s.set_volume(l.studio.effective_volume());
                        }
                        Key::Named(k @ (NamedKey::ArrowLeft | NamedKey::ArrowRight)) => s.seek_by(if k == NamedKey::ArrowLeft { -5000.0 } else { 5000.0 }),
                        _ => {}
                    }
                }
            }
            WindowEvent::RedrawRequested => self.frame(el),
            _ => {}
        }
    }
}

impl App {
    fn frame(&mut self, el: &ActiveEventLoop) {
        let Some(l) = self.live.as_mut() else { return };
        let t0 = Instant::now();
        let dt = t0.duration_since(l.last).as_secs_f32() * 1000.0;
        l.last = t0;

        // worker results
        if let Some(p) = l.studio.drain() {
            install_session(l, p);
        }
        l.studio.pump_regen();
        if l.studio.atlas_gen != l.atlas_seen {
            l.atlas_seen = l.studio.atlas_gen;
            if let Some(s) = &mut l.session {
                s.set_follow(l.studio.atlas.clone());
            }
        }
        // a rescaled skin sheet that finished rasterizing
        if let Some(rx) = &l.rescale {
            if let Ok((sheet, ss)) = rx.try_recv() {
                if let Some(s) = &mut l.session {
                    s.apply_base(sheet, ss);
                }
                l.rescale = None;
            }
        }

        // the interface
        // Ctrl + and Ctrl - make the interface bigger and smaller, Ctrl 0 goes back to automatic
        {
            let sf = l.window.scale_factor() as f32;
            let now = ui_scale(&l.window, l.studio.cfg.ui_scale) / sf;
            let (up, down, reset) = l.ctx.input_mut(|i| {
                let m = egui::Modifiers::COMMAND;
                (i.consume_key(m, egui::Key::Plus) || i.consume_key(m, egui::Key::Equals), i.consume_key(m, egui::Key::Minus), i.consume_key(m, egui::Key::Num0))
            });
            let new = if up { Some(next_ui_size(now, true)) } else if down { Some(next_ui_size(now, false)) } else if reset { Some(0.0) } else { None };
            if let Some(v) = new {
                l.studio.cfg.ui_scale = v;
                l.studio.cfg.save();
            }
        }
        let ppp = ui_scale(&l.window, l.studio.cfg.ui_scale);
        l.studio.ui_now = ppp / l.window.scale_factor() as f32;
        l.ctx.set_pixels_per_point(ppp);
        let raw = l.egui_state.take_egui_input(&l.window);
        let sv = match &l.session {
            Some(s) => SessionView { loaded: true, clock: s.clock_ms, start: s.start_ms, end: s.end_ms, playing: s.playing, info: s.info.clone() },
            None => SessionView::default(),
        };
        let mut out = UiOut::default();
        let studio = &mut l.studio;
        let full = l.ctx.run_ui(raw, |ui| ui::draw(ui, studio, &sv, &mut out));
        l.egui_state.handle_platform_output(&l.window, full.platform_output);

        // what the interface asked of the session
        if let Some(s) = &mut l.session {
            if out.toggle_play {
                s.toggle_play();
            }
            if let Some(f) = out.seek {
                s.seek_to(s.start_ms + (s.end_ms - s.start_ms) * f);
            }
            if let Some(v) = out.volume {
                s.set_volume(v);
            }
            if let Some(v) = out.offset {
                s.set_offset(v);
            }
            if let Some(v) = out.cursor {
                s.set_cursor_scale(v);
            }
            if let Some(v) = out.speed {
                s.set_speed(v);
            }
            if let Some(v) = out.hit_lighting {
                s.set_hit_lighting(v);
            }
            if let Some((a, b)) = out.snaking {
                s.set_snaking(a, b);
            }
        }
        let size_px = (l.config.width as f32, l.config.height as f32);
        let vp_px = out.viewport.map(|r| [r.min.x * ppp, r.min.y * ppp, r.width() * ppp, r.height() * ppp]).unwrap_or([0.0, 0.0, size_px.0, size_px.1]);
        // what the view needs, from the window and the settings panel (the first frame has no layout yet)
        let from_window = ((size_px.0 - PANEL_W * ppp) / 640.0).min((size_px.1 - (46.0 + 360.0) * ppp) / 480.0);
        let from_layout = (vp_px[2] / 640.0).min(vp_px[3] / 480.0);
        let want_ss = (if from_layout.is_finite() && from_layout > 0.2 && from_layout < 8.0 { from_layout } else { from_window }).clamp(0.4, 4.0);

        if l.studio.load_session && l.rescale.is_none() {
            // a skin change carries on from the moment being played
            l.studio.resume = l.session.as_ref().map(|s| (s.clock_ms, s.playing));
            l.studio.start_session_load(want_ss);
        }
        // keep the skin sharp when the window is resized: rasterize again once the size has settled
        if let Some(s) = &l.session {
            let off = (want_ss / s.built_ss - 1.0).abs();
            if off > 0.08 && l.rescale.is_none() && l.last_resize.elapsed() > Duration::from_millis(350) && !l.studio.loading {
                let job = s.rescale_job(want_ss);
                let (tx, rx) = std::sync::mpsc::channel();
                let ctx = l.ctx.clone();
                std::thread::spawn(move || {
                    let r = job();
                    let _ = tx.send(r);
                    ctx.request_repaint();
                });
                l.rescale = Some(rx);
            }
        }
        // the seamless line follows the CS and AR of the map being played
        if let Some(s) = &l.session {
            let cs_ar = (s.map.cs, s.map.ar);
            if cs_ar != l.cs_ar_seen {
                l.cs_ar_seen = cs_ar;
                l.studio.map_cs_ar = Some(cs_ar);
                if l.studio.is_seamless() {
                    l.studio.regen_pending = true;
                }
            }
        }

        // draw
        let frame = match l.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            _ => {
                l.surface.configure(&l.device, &l.config);
                l.window.request_redraw();
                return;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        // the skin editor has no gameplay: it is held (and silent) while that half of the application is open, and goes on when it is left
        let in_skin = l.studio.mode == crate::studio::Mode::Skin;
        if in_skin != l.was_in_skin {
            l.was_in_skin = in_skin;
            if let Some(s) = &mut l.session {
                if in_skin {
                    l.resume_after_skin = s.playing;
                    s.set_playing(false);
                } else if l.resume_after_skin {
                    s.set_playing(true);
                }
            }
        }
        if let Some(s) = l.session.as_mut().filter(|_| !in_skin) {
            s.advance(dt);
            if s.ended {
                // the map has played out: a random one goes on to the next, a chosen one plays again
                if l.studio.cfg.map == "random" {
                    if !l.studio.load_session && !l.studio.loading {
                        l.studio.new_map();
                    }
                } else {
                    s.restart();
                }
            }
            s.set_viewport(vp_px[0], vp_px[1], vp_px[2], vp_px[3]);
            s.render(&view);
        } else {
            // nothing loaded yet: just the background
            let mut enc = l.device.create_command_encoder(&Default::default());
            drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            }));
            l.queue.submit(Some(enc.finish()));
        }
        // the interface on top
        let prims = l.ctx.tessellate(full.shapes, full.pixels_per_point);
        let desc = egui_wgpu::ScreenDescriptor { size_in_pixels: [l.config.width, l.config.height], pixels_per_point: full.pixels_per_point };
        for (id, deltas) in &full.textures_delta.set {
            for d in deltas.iter() {
                l.egui_renderer.update_texture(&l.device, &l.queue, *id, d);
            }
        }
        let mut enc = l.device.create_command_encoder(&Default::default());
        let extra = l.egui_renderer.update_buffers(&l.device, &l.queue, &mut enc, &prims, &desc);
        {
            let pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            l.egui_renderer.render(&mut pass, &prims, &desc);
        }
        l.queue.submit(extra.into_iter().chain(Some(enc.finish())));
        for id in &full.textures_delta.free {
            l.egui_renderer.free_texture(id);
        }
        l.window.pre_present_notify();
        l.queue.present(frame);
        let _ = l.format;
        wait_for_refresh();
        l.fps_count += 1;
        if l.fps_since.elapsed() >= Duration::from_secs(2) {
            if std::env::var("STUDIO_FPS").is_ok() {
                eprintln!("{:.0} frames/s, {:.2} ms of work", l.fps_count as f64 / l.fps_since.elapsed().as_secs_f64(), t0.elapsed().as_secs_f64() * 1000.0);
            }
            l.fps_count = 0;
            l.fps_since = Instant::now();
        }
        let _ = el;
        l.window.request_redraw();
    }
}

fn install_session(l: &mut Live, p: Prepared) {
    let size = (l.config.width, l.config.height);
    let mut s = Session::finish(p, l.device.clone(), l.queue.clone(), l.format, size);
    // the map the worker picked decides how the seamless line is spaced
    l.cs_ar_seen = (0.0, 0.0);
    s.set_follow(l.studio.atlas.clone());
    // after a skin change the same map carries on from where the old session has got to, not from where it was when loading began
    if let (Some(old), Some(_)) = (&l.session, l.studio.resume) {
        if old.map_path_eq(&s) {
            s.seek_to(old.clock_ms);
            s.set_playing(old.playing);
        }
    }
    l.session = Some(s);
    l.studio.loading = false;
    l.studio.session_error.clear();
}

pub fn run() {
    let mut builder = EventLoop::builder();
    #[cfg(windows)]
    {
        use winit::platform::windows::EventLoopBuilderExtWindows;
        builder.with_any_thread(true);
    }
    let event_loop = builder.build().expect("event loop");
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("run");
}

#[cfg(test)]
mod scale_tests {
    use super::*;

    #[test]
    fn the_interface_size_steps_up_and_down_through_the_list() {
        assert_eq!(next_ui_size(1.0, true), 1.1);
        assert_eq!(next_ui_size(1.0, false), 0.9);
        assert_eq!(next_ui_size(1.37, true), 1.5, "from an in-between size (automatic) to the next one");
        assert_eq!(next_ui_size(1.37, false), 1.25);
        assert_eq!(next_ui_size(2.0, true), 2.0);
        assert_eq!(next_ui_size(0.5, false), 0.5);
        assert_eq!(next_ui_size(0.7, false), 0.6);
        assert!(UI_SIZES.windows(2).all(|w| w[0] < w[1]));
    }
}
