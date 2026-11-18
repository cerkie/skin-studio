//! The interface: a big live gameplay view with lazer-style settings overlaid on the left. Every control edits the
//! settings (or the playback options) and the view updates at once.
use egui::{Align2, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use engine::presets;
use serde_json::Value;

use crate::state::{self, PROPS};
use crate::studio::{PickKind, Studio, Tab, TABS};
use crate::theme::{self as th, pal::*, Kind};

pub const PANEL_W: f32 = 570.0;
const SIDEBAR_W: f32 = 170.0;

/// What the session looks like right now.
#[derive(Default, Clone)]
pub struct SessionView {
    pub loaded: bool,
    pub clock: f32,
    pub start: f32,
    pub end: f32,
    pub playing: bool,
    pub info: String,
    pub volume: f32,
}

/// What the interface asks the app to do to the session.
#[derive(Default)]
pub struct UiOut {
    /// Where the gameplay view goes, in points.
    pub viewport: Option<Rect>,
    pub toggle_play: bool,
    pub seek: Option<f32>,
    pub volume: Option<f32>,
    pub offset: Option<f32>,
    pub cursor: Option<f32>,
    pub hit_lighting: Option<bool>,
    pub snaking: Option<(bool, bool)>,
    pub speed: Option<f32>,
}

/// A rounded bubble with a value, above `anchor` (bottom centre).
fn bubble(ui: &Ui, anchor: Pos2, text: &str) {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::new(14.0, FontFamily::Proportional), CONTENT1);
    let size = galley.size() + Vec2::new(16.0, 8.0);
    let rect = Rect::from_min_size(Pos2::new(anchor.x - size.x * 0.5, anchor.y - size.y), size);
    ui.painter().rect_filled(rect.translate(Vec2::new(0.0, 2.0)).expand(1.0), CornerRadius::same(6), Color32::from_black_alpha(90));
    ui.painter().rect_filled(rect, CornerRadius::same(6), BACKGROUND6);
    ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, LIGHT4), egui::StrokeKind::Inside);
    ui.painter().galley(rect.min + Vec2::new(8.0, 4.0), galley, CONTENT1);
}

fn fmt_speed(v: f32) -> String {
    let s = format!("{:.2}", v);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A slim slider with a nub and a value bubble while it is dragged. Returns true when `f` (0..1) changed.
fn mini_slider(ui: &mut Ui, id: &str, area: Rect, f: &mut f32, tip: impl Fn(f32) -> String) -> bool {
    let resp = ui.interact(area, ui.id().with(id), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let mut changed = false;
    if resp.dragged() || resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            *f = ((p.x - area.left() - 5.0) / (area.width() - 10.0)).clamp(0.0, 1.0);
            changed = true;
        }
    }
    let line = Rect::from_center_size(area.center(), Vec2::new(area.width(), 6.0));
    ui.painter().rect_filled(line, CornerRadius::same(3), BACKGROUND5);
    let x = area.left() + 5.0 + (area.width() - 10.0) * *f;
    ui.painter().rect_filled(Rect::from_min_max(line.min, Pos2::new(x, line.bottom())), CornerRadius::same(3), LIGHT4);
    ui.painter().rect_filled(Rect::from_center_size(Pos2::new(x, area.center().y), Vec2::new(8.0, 20.0)), CornerRadius::same(4), if resp.hovered() || resp.dragged() { HIGHLIGHT1 } else { LIGHT3 });
    if resp.dragged() {
        bubble(ui, Pos2::new(x, area.top() - 2.0), &tip(*f));
    }
    changed
}

/// Messages from the app (export done, errors) in a card over the view: wrapped, never cut short, gone after a few seconds.
fn toast(ui: &mut Ui, st: &mut Studio, view: Rect) {
    if st.status != st.status_shown {
        st.status_shown = st.status.clone();
        st.status_at = std::time::Instant::now();
    }
    let age = st.status_at.elapsed().as_secs_f32();
    if st.status.is_empty() || age > 12.0 {
        return;
    }
    let a = ((age * 6.0).min(1.0) * ((12.0 - age) * 2.0).min(1.0)).clamp(0.0, 1.0);
    let w = (view.width() - 48.0).clamp(220.0, 620.0);
    let galley = ui.painter().layout(st.status.clone(), FontId::new(14.0, FontFamily::Proportional), CONTENT1.gamma_multiply(a), w - 32.0);
    let size = galley.size() + Vec2::new(32.0, 24.0);
    let rect = Rect::from_min_size(Pos2::new(view.center().x - size.x * 0.5, view.bottom() - 54.0 - 32.0 - size.y), size);
    th::shadow(ui.painter(), rect, 10, (110.0 * a) as u8);
    ui.painter().rect_filled(rect, CornerRadius::same(10), Color32::from_rgba_unmultiplied(0x1c, 0x1a, 0x22, (240.0 * a) as u8));
    ui.painter().rect_stroke(rect, CornerRadius::same(10), Stroke::new(1.0, Color32::from_white_alpha((30.0 * a) as u8)), egui::StrokeKind::Inside);
    ui.painter().galley(rect.min + Vec2::new(16.0, 12.0), galley, CONTENT1);
    if ui.interact(rect, ui.id().with("toast"), Sense::click()).clicked() {
        st.status_at = std::time::Instant::now() - std::time::Duration::from_secs(12);
    }
}

fn fmt_time(ms: f32) -> String {
    let s = (ms.max(0.0) / 1000.0) as u32;
    format!("{}:{:02}", s / 60, s % 60)
}

pub fn draw(root: &mut Ui, st: &mut Studio, sv: &SessionView, out: &mut UiOut) {
    let ctx = root.ctx().clone();
    // ---- toolbar
    egui::Panel::top("toolbar").exact_size(56.0).frame(egui::Frame::new().fill(BACKGROUND6).inner_margin(egui::Margin::ZERO)).show(root, |ui| {
        let bar = ui.max_rect();
        ui.painter().line_segment([bar.left_bottom(), bar.right_bottom()], Stroke::new(1.0, Color32::from_white_alpha(10)));
        ui.horizontal_centered(|ui| {
            // lazer's settings button: a crimson tile the height of the bar
            let (r, _) = ui.allocate_exact_size(Vec2::new(56.0, 56.0), Sense::hover());
            ui.painter().rect_filled(r, 0.0, RED);
            ui.painter().circle_stroke(r.center(), 10.0, Stroke::new(2.6, Color32::WHITE));
            ui.painter().circle_filled(r.center(), 3.6, Color32::WHITE);
            ui.add_space(16.0);
            ui.label(th::text("skin studio", 17.0, CONTENT1).family(th::bold()));
            ui.add_space(22.0);
            if th::button(ui, "New", Kind::Ghost, true).clicked() {
                st.new_settings();
            }
            if th::button(ui, "Open", Kind::Ghost, true).clicked() {
                st.pick(PickKind::OpenSettings);
            }
            if th::button(ui, "Save", Kind::Ghost, true).clicked() {
                st.save();
            }
            if st.dirty {
                // unsaved changes
                let (d, _) = ui.allocate_exact_size(Vec2::new(14.0, 14.0), Sense::hover());
                ui.painter().circle_filled(d.center(), 3.5, Color32::from_rgb(0xff, 0xad, 0x4d));
            }
            ui.add_space(10.0);
            if th::button(ui, "Export images", Kind::Accent, true).clicked() {
                st.export();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(16.0);
                let msg = if let Some(p) = &st.path {
                    p.to_string_lossy().to_string()
                } else {
                    "Unsaved settings".to_string()
                };
                let msg = if msg.chars().count() > 90 { format!("…{}", msg.chars().skip(msg.chars().count() - 88).collect::<String>()) } else { msg };
                ui.label(th::text(msg, 13.0, LIGHT3).family(th::light()));
            });
        });
    });

    // ---- settings overlay
    egui::Panel::left("settings").exact_size(PANEL_W).resizable(false).frame(egui::Frame::new().fill(BACKGROUND4).inner_margin(egui::Margin::ZERO)).show(root, |ui| {
        let full = ui.max_rect();
        let side = Rect::from_min_max(full.min, Pos2::new(full.left() + SIDEBAR_W, full.bottom()));
        let body = Rect::from_min_max(Pos2::new(full.left() + SIDEBAR_W, full.top()), full.max);
        th::vgradient(ui.painter(), body, th::lighten(BACKGROUND4, 0.05), th::darken(BACKGROUND4, 0.08));
        th::vgradient(ui.painter(), side, th::lighten(BACKGROUND5, 0.03), th::darken(BACKGROUND5, 0.12));
        // the panel casts a soft shadow onto the view
        for i in 0..14 {
            let x = full.right() + i as f32 * 1.6;
            ui.painter().rect_filled(Rect::from_min_max(Pos2::new(x, full.top()), Pos2::new(x + 1.7, full.bottom())), 0.0, Color32::from_black_alpha((70.0 * (1.0 - i as f32 / 14.0).powi(2)) as u8));
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(side), |ui| {
            ui.add_space(10.0);
            for (tab, name) in TABS {
                let icon = match tab { Tab::Animation => 0, Tab::Timing => 2, Tab::Motion => 3, Tab::Look => 4, Tab::Preview => 6, Tab::Export => 8 };
                if th::sidebar_item(ui, name, icon, st.tab == tab && st.search.trim().is_empty()).clicked() {
                    st.tab = tab;
                    st.search.clear();
                }
            }
        });
        let search_rect = Rect::from_min_size(body.min, Vec2::new(body.width(), 56.0));
        th::search_box(ui, search_rect, &mut st.search);
        let body = Rect::from_min_max(Pos2::new(body.left(), body.top() + 56.0), body.max);
        ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.set_width(body.width());
                ui.spacing_mut().item_spacing = Vec2::new(0.0, 0.0);
                if !st.search.trim().is_empty() {
                    search_results(ui, st);
                    return;
                }
                th::section_header(ui, TABS.iter().find(|(t, _)| *t == st.tab).map(|(_, n)| *n).unwrap_or(""));
                match st.tab {
                    Tab::Animation => tab_animation(ui, st),
                    Tab::Timing => tab_timing(ui, st),
                    Tab::Motion => tab_motion(ui, st),
                    Tab::Look => tab_look(ui, st),
                    Tab::Preview => tab_preview(ui, st, out),
                    Tab::Export => tab_export(ui, st),
                }
                ui.add_space(40.0);
            });
        });
    });

    // ---- the gameplay view fills the rest
    egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::TRANSPARENT)).show(root, |ui| {
        let rect = ui.max_rect();
        out.viewport = Some(rect);
        toast(ui, st, rect);
        let clicked = ui.interact(rect, ui.id().with("view"), Sense::click()).clicked();
        let transport_h = 54.0;
        let tw = (rect.width() - 40.0).clamp(200.0, 760.0);
        let tr = Rect::from_center_size(Pos2::new(rect.center().x, rect.bottom() - transport_h / 2.0 - 16.0), Vec2::new(tw, transport_h));
        let over_transport = ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| tr.contains(p));
        if clicked && !over_transport && sv.loaded {
            out.toggle_play = true;
        }
        if st.lazer_active() {
            let msg = if st.loading { "Loading the map...".to_string() } else if sv.loaded { format!("Playing in the osu!lazer window\n{}", sv.info) } else { "Starting the osu!lazer preview...".to_string() };
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, msg, FontId::new(20.0, th::light()), CONTENT2);
        } else if st.loading {
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, "Loading the map...", FontId::new(22.0, th::light()), CONTENT2);
        } else if !st.session_error.is_empty() {
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, &st.session_error, FontId::new(18.0, th::light()), CONTENT2);
        }
        transport(ui, tr, st, sv, out);
        anim_card(&ctx, ui, st, rect);
        if sv.loaded && !sv.info.is_empty() {
            // keep clear of the animation card in the corner
            let max_chars = ((rect.width() * 0.6) / 6.2) as usize;
            let info: String = if sv.info.chars().count() > max_chars { format!("{}…", sv.info.chars().take(max_chars.saturating_sub(1)).collect::<String>()) } else { sv.info.clone() };
            ui.painter().text(Pos2::new(rect.left() + 16.0, rect.top() + 12.0), Align2::LEFT_TOP, info, FontId::new(12.0, th::light()), Color32::from_white_alpha(130));
        }
    });
}

fn transport(ui: &mut Ui, r: Rect, st: &mut Studio, sv: &SessionView, out: &mut UiOut) {
    if !sv.loaded {
        return;
    }
    th::shadow(ui.painter(), r, 12, 120);
    ui.painter().rect_filled(r, CornerRadius::same(14), Color32::from_rgba_unmultiplied(0x1c, 0x1a, 0x22, 238));
    ui.painter().rect_stroke(r, CornerRadius::same(14), Stroke::new(1.0, Color32::from_white_alpha(18)), egui::StrokeKind::Inside);
    let btn = Rect::from_center_size(Pos2::new(r.left() + 36.0, r.center().y), Vec2::new(44.0, 34.0));
    let bresp = ui.interact(btn, ui.id().with("play"), Sense::click());
    ui.painter().rect_filled(btn, CornerRadius::same(5), if bresp.hovered() { th::lighten(COLOR3, 0.2) } else { COLOR3 });
    if sv.playing {
        for dx in [-5.0, 5.0] {
            ui.painter().rect_filled(Rect::from_center_size(btn.center() + Vec2::new(dx, 0.0), Vec2::new(4.0, 14.0)), 1.0, Color32::WHITE);
        }
    } else {
        let c = btn.center();
        ui.painter().add(egui::Shape::convex_polygon(vec![c + Vec2::new(-5.0, -8.0), c + Vec2::new(-5.0, 8.0), c + Vec2::new(8.0, 0.0)], Color32::WHITE, Stroke::NONE));
    }
    if bresp.clicked() {
        out.toggle_play = true;
    }
    // a new random map, next to play (the die)
    let nm = Rect::from_center_size(Pos2::new(btn.right() + 24.0, r.center().y), Vec2::new(36.0, 34.0));
    let nresp = ui.interact(nm, ui.id().with("newmap"), Sense::click());
    ui.painter().rect_filled(nm, CornerRadius::same(5), if nresp.hovered() { DARK3 } else { DARK2 });
    {
        let c = nm.center();
        let sq = Rect::from_center_size(c, Vec2::new(16.0, 16.0));
        ui.painter().rect_stroke(sq, CornerRadius::same(3), Stroke::new(1.8, CONTENT1), egui::StrokeKind::Middle);
        for (dx, dy) in [(-4.0, -4.0), (4.0, 4.0), (0.0, 0.0), (4.0, -4.0), (-4.0, 4.0)] {
            ui.painter().circle_filled(c + Vec2::new(dx, dy), 1.6, CONTENT1);
        }
    }
    let nresp = nresp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Play a different random map");
    if nresp.clicked() {
        st.new_map();
    }
    // playback speed: a chip that opens a slider
    let sp = Rect::from_center_size(Pos2::new(nm.right() + 34.0, r.center().y), Vec2::new(52.0, 34.0));
    let spresp = ui.interact(sp, ui.id().with("speed"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Playback speed");
    ui.painter().rect_filled(sp, CornerRadius::same(5), if spresp.hovered() { DARK3 } else { DARK2 });
    ui.painter().text(sp.center(), Align2::CENTER_CENTER, format!("{}x", fmt_speed(st.cfg.speed)), FontId::new(14.0, FontFamily::Proportional), if (st.cfg.speed - 1.0).abs() < 0.001 { CONTENT2 } else { HIGHLIGHT1 });
    egui::Popup::menu(&spresp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).gap(8.0).show(|ui| {
        ui.set_width(250.0);
        ui.add_space(4.0);
        ui.label(egui::RichText::new("Playback speed").color(CONTENT2).size(14.0));
        let (area, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::hover());
        let mut f = (st.cfg.speed - 0.1) / 1.9;
        if mini_slider(ui, "speedbar", area, &mut f, |f| format!("{}x", fmt_speed(0.1 + f * 1.9))) {
            st.cfg.speed = (((0.1 + f * 1.9) * 20.0).round() / 20.0).clamp(0.1, 2.0);
            st.cfg.save();
            out.speed = Some(st.cfg.speed);
        }
        ui.horizontal(|ui| {
            for v in [0.25f32, 0.5, 0.75, 1.0, 1.5] {
                if ui.add(egui::Button::new(egui::RichText::new(format!("{}x", fmt_speed(v))).size(13.0)).fill(DARK2)).clicked() {
                    st.cfg.speed = v;
                    st.cfg.save();
                    out.speed = Some(v);
                }
            }
        });
        ui.add_space(4.0);
    });
    // right side, from the edge inwards: time, volume slider, mute button
    let time = format!("{} / {}", fmt_time(sv.clock - sv.start), fmt_time(sv.end - sv.start));
    ui.painter().text(Pos2::new(r.right() - 14.0, r.center().y), Align2::RIGHT_CENTER, time, FontId::new(14.0, FontFamily::Proportional), CONTENT2);
    let vol = Rect::from_min_max(Pos2::new(r.right() - 14.0 - 96.0 - 96.0, r.center().y - 12.0), Pos2::new(r.right() - 14.0 - 96.0 - 8.0, r.center().y + 12.0));
    let mute = Rect::from_center_size(Pos2::new(vol.left() - 22.0, r.center().y), Vec2::new(32.0, 32.0));
    let mresp = ui.interact(mute, ui.id().with("mute"), Sense::click());
    let col = if mresp.hovered() { CONTENT1 } else { LIGHT1 };
    let c = mute.center();
    ui.painter().add(egui::Shape::convex_polygon(vec![c + Vec2::new(-9.0, -3.5), c + Vec2::new(-4.0, -3.5), c + Vec2::new(1.0, -8.0), c + Vec2::new(1.0, 8.0), c + Vec2::new(-4.0, 3.5), c + Vec2::new(-9.0, 3.5)], col, Stroke::NONE));
    if st.cfg.muted || st.cfg.volume <= 0.0 {
        ui.painter().line_segment([c + Vec2::new(4.0, -4.0), c + Vec2::new(11.0, 4.0)], Stroke::new(2.0, col));
        ui.painter().line_segment([c + Vec2::new(4.0, 4.0), c + Vec2::new(11.0, -4.0)], Stroke::new(2.0, col));
    } else {
        for (i, rad) in [5.0f32, 9.0].iter().enumerate() {
            if st.cfg.volume > 0.0 + 0.45 * i as f32 {
                let pts: Vec<Pos2> = (-6..=6).map(|k| { let a = k as f32 / 6.0 * 0.9; c + Vec2::new(2.0 + rad * a.cos(), rad * a.sin()) }).collect();
                ui.painter().add(egui::Shape::line(pts, Stroke::new(1.8, col)));
            }
        }
    }
    if mresp.clicked() {
        st.cfg.muted = !st.cfg.muted;
        if !st.cfg.muted && st.cfg.volume <= 0.0 {
            st.cfg.volume = 0.1;
        }
        st.cfg.save();
        out.volume = Some(st.effective_volume());
    }
    let vresp = ui.interact(vol, ui.id().with("vol"), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::PointingHand);
    if vresp.dragged() || vresp.clicked() {
        if let Some(p) = vresp.interact_pointer_pos() {
            let v = ((p.x - vol.left() - 5.0) / (vol.width() - 10.0)).clamp(0.0, 1.0);
            st.cfg.volume = v;
            st.cfg.muted = v <= 0.0;
            st.cfg.save();
            out.volume = Some(st.effective_volume());
        }
    }
    let shown = if st.cfg.muted { 0.0 } else { st.cfg.volume };
    let line = Rect::from_center_size(vol.center(), Vec2::new(vol.width(), 6.0));
    ui.painter().rect_filled(line, CornerRadius::same(3), BACKGROUND5);
    let vx = vol.left() + 5.0 + (vol.width() - 10.0) * shown;
    ui.painter().rect_filled(Rect::from_min_max(line.min, Pos2::new(vx, line.bottom())), CornerRadius::same(3), LIGHT4);
    ui.painter().rect_filled(Rect::from_center_size(Pos2::new(vx, vol.center().y), Vec2::new(8.0, 20.0)), CornerRadius::same(4), if vresp.hovered() || vresp.dragged() { HIGHLIGHT1 } else { LIGHT3 });
    if vresp.dragged() {
        bubble(ui, Pos2::new(vx, vol.top() - 6.0), &format!("{:.0}%", shown * 100.0));
    }
    let bar = Rect::from_min_max(Pos2::new(nm.right() + 80.0, r.center().y - 14.0), Pos2::new(mute.left() - 22.0, r.center().y + 14.0));
    let resp = ui.interact(bar, ui.id().with("seek"), Sense::click_and_drag());
    let f = if sv.end > sv.start { ((sv.clock - sv.start) / (sv.end - sv.start)).clamp(0.0, 1.0) } else { 0.0 };
    let line = Rect::from_center_size(bar.center(), Vec2::new(bar.width(), 6.0));
    ui.painter().rect_filled(line, CornerRadius::same(3), BACKGROUND5);
    let nx = bar.left() + 5.0 + (bar.width() - 10.0) * f;
    ui.painter().rect_filled(Rect::from_min_max(line.min, Pos2::new(nx, line.bottom())), CornerRadius::same(3), th::lerp_c(LIGHT4, HIGHLIGHT1, 0.6));
    ui.painter().rect_filled(Rect::from_center_size(Pos2::new(nx, bar.center().y), Vec2::new(10.0, 24.0)), CornerRadius::same(5), HIGHLIGHT1);
    if resp.dragged() || resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            out.seek = Some(((p.x - bar.left() - 5.0) / (bar.width() - 10.0)).clamp(0.0, 1.0));
        }
    }
}

/// The animation on its own, in the corner of the view.
fn anim_card(ctx: &egui::Context, ui: &mut Ui, st: &mut Studio, view: Rect) {
    let n = st.frames.len();
    let (mw, mh) = st.frames.iter().flatten().fold((1usize, 1usize), |a, f| (a.0.max(f.w), a.1.max(f.h)));
    let now = std::time::Instant::now();
    let dt = now.duration_since(st.anim_last).as_secs_f32() * 1000.0;
    st.anim_last = now;
    if st.anim_playing && !st.anim_scrub {
        st.anim_pos += dt.min(100.0) * st.anim_speed;
    }
    let loop_ms = if n > 0 { n as f32 * 1000.0 / st.fps.max(1.0) } else { 1.0 };
    st.anim_pos = st.anim_pos.rem_euclid(loop_ms);
    let idx = if n > 0 { ((st.anim_pos * st.fps / 1000.0) as usize).min(n - 1) } else { 0 };
    if st.anim_tex.is_none() || st.anim_idx != idx || st.anim_gen != st.atlas_gen {
        let bg = [0x18, 0x17, 0x1c, 255];
        let mut px = vec![0u8; mw * mh * 4];
        for p in px.chunks_mut(4) {
            p.copy_from_slice(&bg);
        }
        if let Some(Some(f)) = st.frames.get(idx) {
            let (ox, oy) = ((mw - f.w) / 2, (mh - f.h) / 2);
            for y in 0..f.h {
                for x in 0..f.w {
                    let q = f.px[y * f.w + x];
                    let o = ((oy + y) * mw + ox + x) * 4;
                    for k in 0..3 {
                        px[o + k] = (q[k] * 255.0 + px[o + k] as f32 * (1.0 - q[3])).min(255.0) as u8;
                    }
                }
            }
        }
        // the canvas outline, always drawn
        let edge = [0x50, 0x47, 0x6b, 255];
        for x in 0..mw {
            px[x * 4..x * 4 + 4].copy_from_slice(&edge);
            px[((mh - 1) * mw + x) * 4..((mh - 1) * mw + x) * 4 + 4].copy_from_slice(&edge);
        }
        for y in 0..mh {
            px[(y * mw) * 4..(y * mw) * 4 + 4].copy_from_slice(&edge);
            px[(y * mw + mw - 1) * 4..(y * mw + mw - 1) * 4 + 4].copy_from_slice(&edge);
        }
        let img = egui::ColorImage::from_rgba_unmultiplied([mw, mh], &px);
        match &mut st.anim_tex {
            Some(tex) => tex.set(img, egui::TextureOptions::NEAREST),
            None => st.anim_tex = Some(ctx.load_texture("anim", img, egui::TextureOptions::NEAREST)),
        }
        st.anim_idx = idx;
        st.anim_gen = st.atlas_gen;
    }
    let Some(tex) = st.anim_tex.clone() else { return };
    // draw each source pixel as a whole number of screen pixels so edges stay sharp
    let ppp = ctx.pixels_per_point();
    let max_w = (view.width() * 0.26).clamp(150.0, 300.0) * ppp;
    let k = ((max_w / mw as f32).floor()).clamp(1.0, 12.0);
    let (iw, ih) = (mw as f32 * k / ppp, mh as f32 * k / ppp);
    let w = iw + 16.0;
    let h = ih;
    let card = Rect::from_min_size(Pos2::new(view.right() - w - 16.0, view.top() + 14.0), Vec2::new(w, h + 74.0));
    th::shadow(ui.painter(), card, 10, 120);
    ui.painter().rect_filled(card, CornerRadius::same(12), Color32::from_rgba_unmultiplied(0x1c, 0x1a, 0x22, 238));
    ui.painter().rect_stroke(card, CornerRadius::same(12), Stroke::new(1.0, Color32::from_white_alpha(18)), egui::StrokeKind::Inside);
    let fit = Rect::from_min_size(card.min + Vec2::new(8.0, 8.0), Vec2::new(iw, ih));
    // snap to the pixel grid
    let snap = |v: f32| (v * ppp).round() / ppp;
    let fit = Rect::from_min_size(Pos2::new(snap(fit.min.x), snap(fit.min.y)), fit.size());
    ui.painter().image(tex.id(), fit, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
    // a scrub bar over the frames of the animation
    let bar = Rect::from_min_size(Pos2::new(card.left() + 12.0, card.bottom() - 64.0), Vec2::new(w - 24.0, 24.0));
    let sresp = ui.interact(bar, ui.id().with("animseek"), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::PointingHand);
    st.anim_scrub = sresp.dragged();
    if sresp.dragged() || sresp.clicked() {
        if let Some(p) = sresp.interact_pointer_pos() {
            let f = ((p.x - bar.left() - 5.0) / (bar.width() - 10.0)).clamp(0.0, 0.999);
            st.anim_pos = f * loop_ms;
        }
    }
    let line = Rect::from_center_size(bar.center(), Vec2::new(bar.width(), 5.0));
    ui.painter().rect_filled(line, CornerRadius::same(3), BACKGROUND5);
    let fx = bar.left() + 5.0 + (bar.width() - 10.0) * (st.anim_pos / loop_ms);
    ui.painter().rect_filled(Rect::from_min_max(line.min, Pos2::new(fx, line.bottom())), CornerRadius::same(3), th::lerp_c(LIGHT4, HIGHLIGHT1, 0.6));
    ui.painter().rect_filled(Rect::from_center_size(Pos2::new(fx, bar.center().y), Vec2::new(8.0, 18.0)), CornerRadius::same(4), if sresp.hovered() || sresp.dragged() { HIGHLIGHT1 } else { LIGHT3 });
    // play / pause, the frame count, and the speed
    let ctrl = Rect::from_min_size(Pos2::new(card.left() + 12.0, card.bottom() - 34.0), Vec2::new(w - 24.0, 26.0));
    let play = Rect::from_min_size(ctrl.min, Vec2::new(ctrl.width() - 52.0, ctrl.height()));
    let resp = ui.interact(play, ui.id().with("animplay"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let col = if resp.hovered() { CONTENT1 } else { LIGHT1 };
    let c = play.left_center() + Vec2::new(7.0, 0.0);
    if st.anim_playing {
        for dx in [-3.0, 3.0] {
            ui.painter().rect_filled(Rect::from_center_size(c + Vec2::new(dx, 0.0), Vec2::new(3.0, 11.0)), 1.0, col);
        }
    } else {
        ui.painter().add(egui::Shape::convex_polygon(vec![c + Vec2::new(-4.0, -6.0), c + Vec2::new(-4.0, 6.0), c + Vec2::new(5.0, 0.0)], col, Stroke::NONE));
    }
    let label = format!("{}/{}  {} fps", idx + 1, n.max(1), st.fps);
    ui.painter().text(play.left_center() + Vec2::new(20.0, 0.0), Align2::LEFT_CENTER, label, FontId::new(12.0, FontFamily::Proportional), col);
    if resp.clicked() {
        st.anim_playing = !st.anim_playing;
    }
    let chip = Rect::from_min_size(Pos2::new(ctrl.right() - 46.0, ctrl.top()), Vec2::new(46.0, ctrl.height()));
    let cresp = ui.interact(chip, ui.id().with("animspeed"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Animation playback speed");
    ui.painter().rect_filled(chip, CornerRadius::same(6), if cresp.hovered() { DARK3 } else { DARK2 });
    ui.painter().text(chip.center(), Align2::CENTER_CENTER, format!("{}x", fmt_speed(st.anim_speed)), FontId::new(13.0, FontFamily::Proportional), if (st.anim_speed - 1.0).abs() < 0.001 { CONTENT2 } else { HIGHLIGHT1 });
    egui::Popup::menu(&cresp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).gap(8.0).show(|ui| {
        ui.set_width(240.0);
        ui.add_space(4.0);
        ui.label(egui::RichText::new("Animation speed").color(CONTENT2).size(14.0));
        let (area, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::hover());
        let mut f = (st.anim_speed - 0.05) / 1.95;
        if mini_slider(ui, "animspeedbar", area, &mut f, |f| format!("{}x", fmt_speed(0.05 + f * 1.95))) {
            st.anim_speed = (((0.05 + f * 1.95) * 20.0).round() / 20.0).clamp(0.05, 2.0);
        }
        ui.horizontal(|ui| {
            for v in [0.1f32, 0.25, 0.5, 1.0, 2.0] {
                if ui.add(egui::Button::new(egui::RichText::new(format!("{}x", fmt_speed(v))).size(13.0)).fill(DARK2)).clicked() {
                    st.anim_speed = v;
                }
            }
        });
        ui.add_space(4.0);
    });
}

// ------------------------------------------------------------------------------------------------ form helpers
fn num(ui: &mut Ui, st: &mut Studio, id: &str, label: &str) {
    let cur = state::fmt_num(&st.settings[id]);
    let buf = st.bufs.entry(id.to_string()).or_insert_with(|| cur.clone());
    let resp = th::text_row(ui, id, label, buf);
    let text = buf.clone();
    if resp.changed() {
        st.set_field(id, &text);
    } else if !resp.has_focus() && text != cur {
        st.bufs.insert(id.to_string(), cur.clone());
    }
    if id.ends_with("_color") {
        let rgb = state::hex_to_rgb(&text).or_else(|| state::hex_to_rgb(&cur)).unwrap_or([255, 255, 255]);
        let sw = Rect::from_center_size(Pos2::new(resp.rect.right() - 12.0, resp.rect.center().y), Vec2::new(22.0, 22.0));
        if let Some(c) = swatch_picker(ui, id, sw, rgb) {
            let hex = format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
            st.set_field(id, &hex);
            st.bufs.insert(id.to_string(), hex);
        }
    }
}

/// A color swatch that opens a color picker; returns the new color when it changes.
fn swatch_picker(ui: &mut Ui, id: &str, rect: Rect, rgb: [u8; 3]) -> Option<[u8; 3]> {
    let resp = ui.interact(rect, ui.id().with(("swatch", id)), Sense::click());
    ui.painter().rect_filled(rect, CornerRadius::same(4), Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
    ui.painter().rect_stroke(rect, CornerRadius::same(4), Stroke::new(if resp.hovered() { 2.0 } else { 1.0 }, if resp.hovered() { LIGHT4 } else { DARK3 }), egui::StrokeKind::Inside);
    let mut out = None;
    egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).gap(4.0).show(|ui| {
        let mut c = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
        if egui::color_picker::color_picker_color32(ui, &mut c, egui::color_picker::Alpha::Opaque) {
            out = Some([c.r(), c.g(), c.b()]);
        }
    });
    out
}

fn toggle(ui: &mut Ui, st: &mut Studio, id: &str, label: &str) {
    let mut on = st.settings[id].as_bool().unwrap_or(false);
    if th::switch_row(ui, id, label, &mut on) {
        st.set_field(id, if on { "1" } else { "0" });
        st.bufs.clear();
    }
}

fn choice(ui: &mut Ui, st: &mut Studio, id: &str, label: &str, items: &[&str]) {
    let cur = st.settings[id].as_str().unwrap_or("").to_string();
    let list: Vec<String> = items.iter().map(|s| s.to_string()).collect();
    if let Some(i) = th::dropdown_row(ui, id, label, &cur, &list) {
        st.set_field(id, &list[i]);
    }
}

// ------------------------------------------------------------------------------------------------ tabs
/// Every setting a search can find, and the tab it lives on.
const SEARCHABLE: &[(Tab, &str)] = &[
    (Tab::Animation, "Start from an effect preset"), (Tab::Animation, "Source image"), (Tab::Animation, "Custom image"), (Tab::Animation, "Canvas width"), (Tab::Animation, "Canvas height"),
    (Tab::Animation, "Seamless line"), (Tab::Animation, "Circle size CS"), (Tab::Animation, "Approach rate AR"), (Tab::Animation, "Soften"), (Tab::Animation, "Tightness"), (Tab::Animation, "Thickness"), (Tab::Animation, "Tile overlap"),
    (Tab::Timing, "Frame rate fps"), (Tab::Timing, "Speed"), (Tab::Timing, "Time shift"), (Tab::Timing, "Fit the speed to the window"), (Tab::Timing, "Appear after"), (Tab::Timing, "Disappear at"), (Tab::Timing, "Blank at the end"),
    (Tab::Timing, "Fade in"), (Tab::Timing, "Fade out"), (Tab::Timing, "Copies in a row"), (Tab::Timing, "Play forward and back"), (Tab::Timing, "Play once"),
    (Tab::Motion, "Position keyframes"), (Tab::Motion, "Scale keyframes"), (Tab::Motion, "Rotation keyframes"), (Tab::Motion, "Opacity keyframes"), (Tab::Motion, "Blur keyframes"), (Tab::Motion, "Easing"),
    (Tab::Look, "Color keys tint"), (Tab::Look, "Add a color"), (Tab::Look, "Outline"), (Tab::Look, "Outline color"), (Tab::Look, "Drop shadow"), (Tab::Look, "Shadow blur"), (Tab::Look, "Debanding"), (Tab::Look, "Grain dither"),
    (Tab::Preview, "Skin"), (Tab::Preview, "Hit sounds"), (Tab::Preview, "Map"), (Tab::Preview, "Random maps lazer stable library"), (Tab::Preview, "Snaking sliders in out"), (Tab::Preview, "Hit lighting"), (Tab::Preview, "Sound"), (Tab::Preview, "Volume"), (Tab::Preview, "Music offset"), (Tab::Preview, "Playback speed"), (Tab::Preview, "Cursor size"),
    (Tab::Export, "Export sizes 1x 2x"), (Tab::Export, "Export images"), (Tab::Export, "History saved versions"), (Tab::Export, "Import followpoint images"),
];

fn search_results(ui: &mut Ui, st: &mut Studio) {
    let q = st.search.trim().to_lowercase();
    th::section_header(ui, "Search");
    let mut hits = 0;
    for (tab, label) in SEARCHABLE {
        if !label.to_lowercase().contains(&q) {
            continue;
        }
        hits += 1;
        let tab_name = TABS.iter().find(|(t, _)| t == tab).map(|(_, n)| *n).unwrap_or("");
        let w = ui.available_width() - 22.0;
        let (r, resp) = ui.allocate_exact_size(Vec2::new(w, 46.0 + th::ROW_GAP), Sense::click());
        let r = Rect::from_min_size(r.min + Vec2::new(12.0, 0.0), Vec2::new(r.width(), r.height() - th::ROW_GAP));
        ui.painter().rect_filled(r, CornerRadius::same(5), if resp.hovered() { DARK3 } else { CONTROL });
        ui.painter().text(r.left_center() + Vec2::new(10.0, -8.0), Align2::LEFT_CENTER, label, FontId::new(15.0, FontFamily::Proportional), CONTENT1);
        ui.painter().text(r.left_center() + Vec2::new(10.0, 10.0), Align2::LEFT_CENTER, tab_name, FontId::new(12.0, th::light()), LIGHT3);
        if resp.clicked() {
            st.tab = *tab;
            st.search.clear();
        }
    }
    if hits == 0 {
        th::note(ui, "Nothing matches that.");
    }
}

fn tab_animation(ui: &mut Ui, st: &mut Studio) {
    th::note(ui, "Pick a ready-made animation to start from. Each one brings its own timing, and everything after it is yours to change.");
    let list = presets::effect_names();
    if let Some(i) = th::dropdown_row(ui, "effect", "Start from an effect", "Choose an effect...", &list) {
        st.apply_effect(&list[i].clone());
    }
    th::subsection_header(ui, "Source");
    let sources = presets::source_names();
    let cur = st.settings["preset"].as_str().unwrap_or("Chevron (bold)").to_string();
    if let Some(i) = th::dropdown_row(ui, "source", "Source image", &cur, &sources) {
        st.set_source(&sources[i].clone());
    }
    if cur == "Custom image..." {
        let img = st.settings["image"].as_str().unwrap_or("").to_string();
        th::note(ui, if img.is_empty() { "No image chosen yet." } else { &img });
        if th::button_wide(ui, "Choose image...", Kind::Purple, true).clicked() {
            st.pick(PickKind::CustomImage);
        }
    }
    num(ui, st, "width", "Canvas width (px)");
    num(ui, st, "height", "Canvas height (px)");
    if st.is_seamless() {
        th::subsection_header(ui, "Seamless line");
        th::note(ui, "The live view uses the CS and AR of the map being played, so the line always joins up; these values are what gets exported.");
        for (id, l) in [("sl_cs", "Circle size (CS)"), ("sl_ar", "Approach rate (AR)"), ("sl_soft", "Soften (ms)"), ("sl_tight", "Tightness (0 - 1)"), ("sl_thick", "Thickness"), ("sl_alpha", "Opacity"), ("sl_overlap", "Tile overlap (0 - 1)")] {
            num(ui, st, id, l);
        }
    }
}

fn tab_timing(ui: &mut Ui, st: &mut Studio) {
    th::subsection_header(ui, "Playback");
    num(ui, st, "fps", "Frame rate (fps, at most 1000)");
    num(ui, st, "speed", "Speed multiplier (1 = as designed)");
    duration_row(ui, st);
    num(ui, st, "shift", "Time shift (ms; - waits, + skips ahead)");
    if th::button_wide(ui, "Set speed to fit the window", Kind::Dark, true).clicked() {
        // the whole animation between "Appear after" and "Disappear at"
        let s = engine::settings::Settings::from_json(&st.settings);
        let span = if s.clamp_mode { s.disappear } else { s.disappear - s.lead };
        if span <= 0.0 {
            st.status = "Set \"Disappear at\" (later than \"Appear after\") first, then fit the speed to that window.".into();
        } else {
            let speed = format!("{:.3}", s.design_length() / span);
            st.set_field("speed", &speed);
            st.bufs.remove("speed");
        }
    }
    th::subsection_header(ui, "Window");
    for (id, l) in [("lead", "Appear after (ms)"), ("disappear", "Disappear at (ms, 0 = never)"), ("tail", "Blank at the end (ms)"), ("fade_in", "Fade in (ms)"), ("fade_out", "Fade out (ms)")] {
        num(ui, st, id, l);
    }
    choice(ui, st, "window_mode", "Appear / disappear window", &["shift", "clamp"]);
    th::subsection_header(ui, "Repeat");
    num(ui, st, "copies", "Copies in a row");
    toggle(ui, st, "pingpong", "Play forward and back");
    toggle(ui, st, "no_loop", "Play once (pad to a point's life)");
}

/// The animation's total length in ms. It is the designed length divided by the speed multiplier, so typing a length sets the
/// speed multiplier that gives it.
fn duration_row(ui: &mut Ui, st: &mut Studio) {
    let s = engine::settings::Settings::from_json(&st.settings);
    let cur = format!("{}", (s.real_length() * 10.0).round() / 10.0);
    let buf = st.bufs.entry("total_duration".to_string()).or_insert_with(|| cur.clone());
    let resp = th::text_row(ui, "total_duration", "Total duration (ms)", buf);
    let text = buf.clone();
    if resp.changed() {
        if let Ok(v) = text.trim().parse::<f32>() {
            if v > 0.0 {
                let speed = format!("{:.4}", s.design_length() / v);
                st.set_field("speed", &speed);
                st.bufs.remove("speed");
            }
        }
    } else if !resp.has_focus() && text != cur {
        st.bufs.insert("total_duration".to_string(), cur);
    }
}

fn tab_motion(ui: &mut Ui, st: &mut Studio) {
    th::note(ui, "Drag the dots to shape each property over time. Double-click empty space to add a key, right-click a dot to remove it. The rows below set exact times, values and easings. The line that sweeps across is where the animation preview is.");
    for (pid, plabel) in PROPS {
        th::subsection_header(ui, plabel);
        crate::motion::graph(ui, st, pid);
        let keys = presets::keys_of(&st.settings["props"][pid]);
        for (i, k) in keys.iter().enumerate() {
            key_row(ui, st, pid, i, k);
        }
        if th::button_wide(ui, "Add a key", Kind::Dark, true).clicked() {
            state::add_key(&mut st.settings, pid);
            st.bufs.clear();
            st.edited();
        }
    }
}

fn key_row(ui: &mut Ui, st: &mut Studio, pid: &str, i: usize, k: &Value) {
    let rid = ui.id().with((pid, i));
    let width = ui.available_width() - 22.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 38.0 + th::ROW_GAP), Sense::hover());
    let rect = Rect::from_min_size(rect.min + Vec2::new(12.0, 0.0), Vec2::new(rect.width(), rect.height() - th::ROW_GAP));
    ui.painter().rect_filled(rect, CornerRadius::same(5), CONTROL);
    let cur = [state::fmt_num(&k[0]), state::fmt_num(&k[1])];
    let mut x = rect.left() + 8.0;
    for (f, w) in [(0usize, 62.0f32), (1, 70.0)] {
        let key = format!("key:{pid}:{i}:{f}");
        let buf = st.bufs.entry(key.clone()).or_insert_with(|| cur[f].clone());
        let r = Rect::from_min_size(Pos2::new(x, rect.top() + 9.0), Vec2::new(w, 20.0));
        let resp = th::put_free(ui, r, egui::TextEdit::singleline(buf).id(rid.with(f)).frame(egui::Frame::NONE).desired_width(w).font(FontId::new(15.0, FontFamily::Proportional)).text_color(CONTENT1).margin(egui::Margin::ZERO));
        let text = buf.clone();
        if resp.changed() {
            state::edit_key(&mut st.settings, pid, i, f as i32, &text);
            st.edited();
        } else if !resp.has_focus() && text != cur[f] {
            st.bufs.insert(key, cur[f].clone());
        }
        ui.painter().line_segment([Pos2::new(x + w + 4.0, rect.top() + 8.0), Pos2::new(x + w + 4.0, rect.bottom() - 8.0)], Stroke::new(1.0, DARK1));
        x += w + 12.0;
    }
    // easing
    let easings = presets::easings();
    let ease = k[2].as_str().unwrap_or("linear").to_string();
    let er = Rect::from_min_max(Pos2::new(x, rect.top() + 4.0), Pos2::new(rect.right() - 40.0, rect.bottom() - 4.0));
    let resp = ui.interact(er, rid.with("ease"), Sense::click());
    ui.painter().text(er.left_center() + Vec2::new(2.0, 0.0), Align2::LEFT_CENTER, &ease, FontId::new(14.0, FontFamily::Proportional), if resp.hovered() { CONTENT1 } else { CONTENT2 });
    let mut picked = None;
    egui::Popup::menu(&resp).width(200.0).gap(2.0).show(|ui| {
        egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
            for (j, e) in easings.iter().enumerate() {
                let (r, ir) = ui.allocate_exact_size(Vec2::new(ui.available_width().max(190.0), 30.0), Sense::click());
                if ir.hovered() {
                    ui.painter().rect_filled(r, CornerRadius::same(4), DARK3);
                }
                ui.painter().text(r.left_center() + Vec2::new(10.0, 0.0), Align2::LEFT_CENTER, e, FontId::new(15.0, FontFamily::Proportional), if *e == ease { HIGHLIGHT1 } else { CONTENT1 });
                if ir.clicked() {
                    picked = Some(j);
                    ui.close();
                }
            }
        });
    });
    if let Some(j) = picked {
        state::edit_key(&mut st.settings, pid, i, 2, &easings[j]);
        st.edited();
    }
    // remove
    let xr = Rect::from_center_size(Pos2::new(rect.right() - 20.0, rect.center().y), Vec2::new(26.0, 26.0));
    let xresp = ui.interact(xr, rid.with("rm"), Sense::click());
    let c = xr.center();
    let col = if xresp.hovered() { CONTENT1 } else { LIGHT3 };
    ui.painter().line_segment([c + Vec2::new(-5.0, -5.0), c + Vec2::new(5.0, 5.0)], Stroke::new(2.0, col));
    ui.painter().line_segment([c + Vec2::new(-5.0, 5.0), c + Vec2::new(5.0, -5.0)], Stroke::new(2.0, col));
    if xresp.clicked() {
        state::remove_key(&mut st.settings, pid, i);
        st.bufs.clear();
        st.edited();
    }
}

fn tab_look(ui: &mut Ui, st: &mut Studio) {
    th::subsection_header(ui, "Color");
    tab_color(ui, st);
    tab_finish(ui, st);
}

fn tab_color(ui: &mut Ui, st: &mut Studio) {
    let mut on = st.settings["color_on"].as_bool().unwrap_or(false);
    if th::switch_row(ui, "color_on", "Tint the animation", &mut on) {
        st.settings["color_on"] = serde_json::json!(on);
        if on && st.settings.get("color_keys").is_none_or(|k| k.as_array().is_none_or(|a| a.is_empty())) {
            let keys = presets::color_keys_of(&st.settings);
            st.settings["color_keys"] = Value::Array(keys);
        }
        st.bufs.clear();
        st.edited();
    }
    if !on {
        return;
    }
    num(ui, st, "color_strength", "Strength");
    th::subsection_header(ui, "Color keys");
    let keys = presets::color_keys_of(&st.settings);
    for (i, k) in keys.iter().enumerate() {
        color_row(ui, st, i, k);
    }
    if th::button_wide(ui, "Add a color", Kind::Dark, true).clicked() {
        state::add_color(&mut st.settings);
        st.bufs.clear();
        st.edited();
    }
}

fn color_row(ui: &mut Ui, st: &mut Studio, i: usize, k: &Value) {
    let rid = ui.id().with(("col", i));
    let width = ui.available_width() - 22.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 38.0 + th::ROW_GAP), Sense::hover());
    let rect = Rect::from_min_size(rect.min + Vec2::new(12.0, 0.0), Vec2::new(rect.width(), rect.height() - th::ROW_GAP));
    ui.painter().rect_filled(rect, CornerRadius::same(5), CONTROL);
    let hex = k[1].as_str().unwrap_or("#ffffff").to_string();
    let rgb = state::hex_to_rgb(&hex).unwrap_or([255, 255, 255]);
    if let Some(c) = swatch_picker(ui, &format!("col{i}"), Rect::from_min_size(Pos2::new(rect.left() + 8.0, rect.top() + 8.0), Vec2::new(22.0, 22.0)), rgb) {
        let hex = format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
        state::edit_color(&mut st.settings, i, 1, &hex);
        st.bufs.insert(format!("col:{i}:1"), hex);
        st.edited();
    }
    let cur = [state::fmt_num(&k[0]), hex.clone()];
    let mut x = rect.left() + 40.0;
    for (f, w) in [(0usize, 56.0f32), (1, 84.0)] {
        let key = format!("col:{i}:{f}");
        let buf = st.bufs.entry(key.clone()).or_insert_with(|| cur[f].clone());
        let r = Rect::from_min_size(Pos2::new(x, rect.top() + 9.0), Vec2::new(w, 20.0));
        let resp = th::put_free(ui, r, egui::TextEdit::singleline(buf).id(rid.with(f)).frame(egui::Frame::NONE).desired_width(w).font(FontId::new(15.0, FontFamily::Proportional)).text_color(CONTENT1).margin(egui::Margin::ZERO));
        let text = buf.clone();
        if resp.changed() {
            state::edit_color(&mut st.settings, i, f as i32, &text);
            st.edited();
        } else if !resp.has_focus() && text != cur[f] {
            st.bufs.insert(key, cur[f].clone());
        }
        ui.painter().line_segment([Pos2::new(x + w + 4.0, rect.top() + 8.0), Pos2::new(x + w + 4.0, rect.bottom() - 8.0)], Stroke::new(1.0, DARK1));
        x += w + 12.0;
    }
    let easings = presets::easings();
    let ease = k[2].as_str().unwrap_or("linear").to_string();
    let er = Rect::from_min_max(Pos2::new(x, rect.top() + 4.0), Pos2::new(rect.right() - 40.0, rect.bottom() - 4.0));
    let resp = ui.interact(er, rid.with("ease"), Sense::click());
    ui.painter().text(er.left_center() + Vec2::new(2.0, 0.0), Align2::LEFT_CENTER, &ease, FontId::new(14.0, FontFamily::Proportional), if resp.hovered() { CONTENT1 } else { CONTENT2 });
    let mut picked = None;
    egui::Popup::menu(&resp).width(200.0).gap(2.0).show(|ui| {
        egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
            for (j, e) in easings.iter().enumerate() {
                let (r, ir) = ui.allocate_exact_size(Vec2::new(ui.available_width().max(190.0), 30.0), Sense::click());
                if ir.hovered() {
                    ui.painter().rect_filled(r, CornerRadius::same(4), DARK3);
                }
                ui.painter().text(r.left_center() + Vec2::new(10.0, 0.0), Align2::LEFT_CENTER, e, FontId::new(15.0, FontFamily::Proportional), if *e == ease { HIGHLIGHT1 } else { CONTENT1 });
                if ir.clicked() {
                    picked = Some(j);
                    ui.close();
                }
            }
        });
    });
    if let Some(j) = picked {
        state::edit_color(&mut st.settings, i, 2, &easings[j]);
        st.edited();
    }
    let xr = Rect::from_center_size(Pos2::new(rect.right() - 20.0, rect.center().y), Vec2::new(26.0, 26.0));
    let xresp = ui.interact(xr, rid.with("rm"), Sense::click());
    let c = xr.center();
    let col = if xresp.hovered() { CONTENT1 } else { LIGHT3 };
    ui.painter().line_segment([c + Vec2::new(-5.0, -5.0), c + Vec2::new(5.0, 5.0)], Stroke::new(2.0, col));
    ui.painter().line_segment([c + Vec2::new(-5.0, 5.0), c + Vec2::new(5.0, -5.0)], Stroke::new(2.0, col));
    if xresp.clicked() {
        state::remove_color(&mut st.settings, i);
        st.bufs.clear();
        st.edited();
    }
}

fn tab_finish(ui: &mut Ui, st: &mut Studio) {
    th::subsection_header(ui, "Outline");
    toggle(ui, st, "outline_on", "Outline");
    for (id, l) in [("outline_w", "Width (px)"), ("outline_color", "Color (#rrggbb)"), ("outline_opacity", "Opacity")] {
        num(ui, st, id, l);
    }
    th::subsection_header(ui, "Drop shadow");
    th::note(ui, "The shadow is cast by the source image alone, so it does not change when the outline does.");
    toggle(ui, st, "shadow_on", "Drop shadow");
    for (id, l) in [("shadow_dx", "Offset X (px)"), ("shadow_dy", "Offset Y (px)"), ("shadow_blur", "Blur (px)"), ("shadow_color", "Color (#rrggbb)"), ("shadow_opacity", "Opacity")] {
        num(ui, st, id, l);
    }
    th::subsection_header(ui, "Debanding");
    toggle(ui, st, "deband_on", "Remove banding and dither");
    for (id, l) in [("deband_radius", "Radius (px)"), ("deband_iter", "Passes"), ("deband_thr_rgb", "Color threshold"), ("deband_thr_a", "Alpha threshold"), ("deband_grain", "Extra grain")] {
        num(ui, st, id, l);
    }
}

fn tab_preview(ui: &mut Ui, st: &mut Studio, out: &mut UiOut) {
    th::subsection_header(ui, "Renderer");
    let renderers = ["Built-in".to_string(), "osu!lazer (a separate window)".to_string()];
    let cur = if st.lazer_active() { 1 } else { 0 };
    if let Some(i) = th::dropdown_row(ui, "renderer", "Draw the preview with", &renderers[cur], &renderers) {
        st.set_renderer(i == 1);
    }
    if st.lazer_active() {
        th::note(ui, "lazer draws the map with its own code and assets, so it looks exactly as in the game. It needs a skin that is not Argon Pro to show your animation. The window can be placed next to this one.");
    } else if crate::lazer::find_host().is_none() {
        th::note(ui, "The osu!lazer renderer is an optional download (lazer-preview, from the releases page). Unzip it into a folder called lazer-preview next to this program.");
    }
    th::subsection_header(ui, "Skin");
    let names: Vec<String> = std::iter::once("Argon Pro".to_string()).chain(st.skins.iter().map(|(l, _)| l.clone())).collect();
    let cur = st.skin_label();
    if let Some(i) = th::dropdown_row(ui, "skin", "Skin", &cur, &names) {
        st.cfg.skin = if i == 0 { "argon".into() } else { st.skins[i - 1].1.clone() };
        st.cfg.save();
        st.load_session = true;
    }
    if th::button_wide(ui, "Browse for a skin...", Kind::Purple, true).clicked() {
        st.pick(PickKind::Skin);
    }
    th::note(ui, &format!("Hit sounds come from: {}", if st.cfg.sound_skin.is_empty() { "the skin itself (Argon Pro has none of its own)".to_string() } else { crate::studio::label(&st.cfg.sound_skin) }));
    if th::button_wide(ui, "Choose a skin for hit sounds...", Kind::Dark, true).clicked() {
        st.pick(PickKind::SoundSkin);
    }
    th::subsection_header(ui, "Map");
    let libs = ["Automatic (lazer if found)".to_string(), "osu!lazer".to_string(), "osu!stable (Songs folder)".to_string()];
    let cur_lib = match st.cfg.library.as_str() { "lazer" => 1, "stable" => 2, _ => 0 };
    if let Some(i) = th::dropdown_row(ui, "library", "Random maps come from", &libs[cur_lib], &libs) {
        st.cfg.library = ["auto", "lazer", "stable"][i].to_string();
        st.new_map();
    }
    let lazer = game::realm::lazer_dir();
    th::note(ui, &if st.cfg.map != "random" { st.cfg.map.clone() } else if st.cfg.library != "stable" && lazer.is_some() { format!("A random map from osu!lazer ({})", lazer.unwrap().display()) } else { format!("A random map from {}", if st.cfg.songs.is_empty() { "your Songs folder".to_string() } else { st.cfg.songs.clone() }) });
    if th::button_wide(ui, "Pick another random map", Kind::Purple, true).clicked() {
        st.new_map();
    }
    if th::button_wide(ui, "Choose a .osu, .osz or .olz file...", Kind::Dark, true).clicked() {
        st.pick(PickKind::Map);
    }
    if th::button_wide(ui, "Choose the Songs folder...", Kind::Dark, true).clicked() {
        st.pick(PickKind::Songs);
    }
    th::subsection_header(ui, "Sliders");
    let (mut si, mut so) = (st.cfg.snake_in, st.cfg.snake_out);
    let c1 = th::switch_row(ui, "snake_in", "Snaking in sliders", &mut si);
    let c2 = th::switch_row(ui, "snake_out", "Snaking out sliders", &mut so);
    if c1 || c2 {
        st.cfg.snake_in = si;
        st.cfg.snake_out = so;
        st.cfg.save();
        out.snaking = Some((si, so));
    }
    th::subsection_header(ui, "Effects");
    let mut hl = st.cfg.hit_lighting;
    if th::switch_row(ui, "hitlight", "Hit lighting", &mut hl) {
        st.cfg.hit_lighting = hl;
        st.cfg.save();
        out.hit_lighting = Some(hl);
    }
    th::subsection_header(ui, "Audio and cursor");
    let mut on = !st.cfg.muted;
    if th::switch_row(ui, "sound", "Sound (off until you turn it on)", &mut on) {
        st.cfg.muted = !on;
        st.cfg.save();
        out.volume = Some(st.effective_volume());
    }
    let mut v = st.cfg.volume;
    if th::slider_row(ui, "volume", "Volume", &format!("{:.0}%", st.cfg.volume * 100.0), &mut v) {
        st.cfg.volume = v;
        st.cfg.muted = v <= 0.0;
        st.cfg.save();
        out.volume = Some(st.effective_volume());
    }
    // the music offset: exact value in a box, plus 1 ms and 10 ms nudges (the slider alone is too coarse)
    let cur = format!("{}", (st.cfg.offset_ms * 10.0).round() / 10.0);
    let buf = st.bufs.entry("offset_ms".to_string()).or_insert_with(|| cur.clone());
    let resp = th::text_row(ui, "offset_ms", "Music offset in ms (positive plays the song later)", buf);
    let text = buf.clone();
    if resp.changed() {
        if let Ok(x) = text.trim().parse::<f32>() {
            st.cfg.offset_ms = x.clamp(-1000.0, 1000.0);
            st.cfg.save();
            out.offset = Some(st.cfg.offset_ms);
        }
    } else if !resp.has_focus() && text != cur {
        st.bufs.insert("offset_ms".to_string(), cur);
    }
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        for (label, d) in [("-10", -10.0f32), ("-1", -1.0), ("+1", 1.0), ("+10", 10.0)] {
            if th::button(ui, label, Kind::Dark, true).clicked() {
                st.cfg.offset_ms = (st.cfg.offset_ms + d).clamp(-1000.0, 1000.0);
                st.cfg.save();
                st.bufs.remove("offset_ms");
                out.offset = Some(st.cfg.offset_ms);
            }
        }
        if th::button(ui, "Reset", Kind::Dark, true).clicked() {
            st.cfg.offset_ms = 0.0;
            st.cfg.save();
            st.bufs.remove("offset_ms");
            out.offset = Some(0.0);
        }
    });
    let mut sp = (st.cfg.speed - 0.1) / 1.9;
    if th::slider_row(ui, "speedrow", "Playback speed", &format!("{}x", fmt_speed(st.cfg.speed)), &mut sp) {
        st.cfg.speed = (((0.1 + sp * 1.9) * 20.0).round() / 20.0).clamp(0.1, 2.0);
        st.cfg.save();
        out.speed = Some(st.cfg.speed);
    }
    let mut c = (st.cfg.cursor_scale - 0.3) / 2.7;
    if th::slider_row(ui, "cursor", "Cursor size", &format!("{:.0}%", st.cfg.cursor_scale * 100.0), &mut c) {
        st.cfg.cursor_scale = ((c * 2.7 + 0.3) * 20.0).round() / 20.0;
        st.cfg.save();
        out.cursor = Some(st.cfg.cursor_scale);
    }
    th::note(ui, "The music offset moves the song against the map; hit sounds stay on the objects. Space pauses, left and right seek 5 seconds, up and down change the volume. Click the view to pause.");
}

fn tab_export(ui: &mut Ui, st: &mut Studio) {
    th::subsection_header(ui, "Export");
    choice(ui, st, "export", "Export sizes", &["1x", "2x", "both"]);
    if th::button_wide(ui, "Export images...", Kind::Purple, true).clicked() {
        st.export();
    }
    th::subsection_header(ui, "History");
    tab_history(ui, st);
    th::subsection_header(ui, "Import");
    tab_import(ui, st);
}

fn tab_history(ui: &mut Ui, st: &mut Studio) {
    th::note(ui, "Every save and export is remembered, so the settings behind a set of images are never lost.");
    if th::button_wide(ui, "Save the current settings to history", Kind::Purple, true).clicked() {
        st.add_history();
    }
    let items = st.history.clone();
    for h in items {
        let width = ui.available_width() - 22.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 56.0 + th::ROW_GAP), Sense::hover());
        let rect = Rect::from_min_size(rect.min + Vec2::new(12.0, 0.0), Vec2::new(rect.width(), rect.height() - th::ROW_GAP));
        ui.painter().rect_filled(rect, CornerRadius::same(5), CONTROL);
        let name = if h.name.chars().count() > 30 { format!("{}…", h.name.chars().take(29).collect::<String>()) } else { h.name.clone() };
        ui.painter().text(rect.min + Vec2::new(9.0, 9.0), Align2::LEFT_TOP, name, FontId::new(15.0, th::semibold()), CONTENT1);
        ui.painter().text(rect.min + Vec2::new(9.0, 31.0), Align2::LEFT_TOP, &h.when, FontId::new(13.0, th::light()), CONTENT2);
        let load = Rect::from_center_size(Pos2::new(rect.right() - 92.0, rect.center().y), Vec2::new(64.0, 30.0));
        let del = Rect::from_center_size(Pos2::new(rect.right() - 30.0, rect.center().y), Vec2::new(54.0, 30.0));
        for (r, label, kind, which) in [(load, "Load", Kind::Purple, 0), (del, "Delete", Kind::Dark, 1)] {
            let resp = ui.interact(r, ui.id().with((&h.id, which)), Sense::click());
            let base = if kind == Kind::Purple { COLOR3 } else { DARK2 };
            ui.painter().rect_filled(r, CornerRadius::same(5), if resp.hovered() { th::lighten(base, 0.2) } else { base });
            ui.painter().text(r.center(), Align2::CENTER_CENTER, label, FontId::new(13.0, th::semibold()), CONTENT1);
            if resp.clicked() {
                if which == 0 {
                    st.load_history(&h.id);
                } else {
                    st.delete_history(&h.id);
                }
            }
        }
    }
}

fn tab_import(ui: &mut Ui, st: &mut Studio) {
    th::note(ui, "Point this at a folder of followpoint-N.png files (from any skin) and the settings that most likely made them are worked out.");
    th::note(ui, &if st.import.folder.is_empty() { "No folder chosen yet.".to_string() } else { st.import.folder.clone() });
    if th::button_wide(ui, "Choose the folder...", Kind::Purple, true).clicked() {
        st.pick(PickKind::ImportDir);
    }
    let mut fps = std::mem::take(&mut st.import.fps);
    th::text_row(ui, "import_fps", "Frame rate (leave empty to work it out)", &mut fps);
    st.import.fps = fps;
    if th::button_wide(ui, "Analyze", Kind::Pink, !st.import.folder.is_empty() && !st.import.running).clicked() {
        st.run_import();
    }
    if !st.import.report.is_empty() {
        let r = st.import.report.clone();
        th::note(ui, &r);
    }
    if th::button_wide(ui, "Use these settings in the editor", Kind::Purple, st.import.result.is_some()).clicked() {
        st.use_import();
    }
}
