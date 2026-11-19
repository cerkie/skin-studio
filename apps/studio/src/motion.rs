//! The motion editor: a dock under the view with a curve editor and a dope sheet for every property of the animation.
//!
//! Everything is on the keyboard and the mouse at once:
//!   - click a key to select it, shift-click to add to the selection, drag on empty space to box-select;
//!   - drag a key to move it (every selected key moves with it); shift locks the drag to one axis, alt turns time snapping off,
//!     ctrl snaps values to the grid;
//!   - double-click empty space to add a key on the curve, click the pill in the middle of a segment to pick its easing;
//!   - click or drag the ruler to scrub the animation card; wheel zooms time, shift+wheel zooms values, middle-drag pans;
//!   - Delete, Ctrl+A, Ctrl+C / Ctrl+V, K (key at the playhead), arrows to nudge, Space to play, F to fit, Ctrl+Z / Ctrl+Y.
use egui::{Align2, Color32, CornerRadius, FontFamily, FontId, Key, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};
use engine::keys;
use engine::settings::Settings;
use serde_json::{json, Value};

use crate::state::PROPS;
use crate::studio::Studio;
use crate::theme::pal::*;

const PROP_COLORS: [Color32; 7] = [
    Color32::from_rgb(0x8c, 0x66, 0xff),
    Color32::from_rgb(0xff, 0x66, 0xab),
    Color32::from_rgb(0x66, 0xcc, 0xff),
    Color32::from_rgb(0x66, 0xff, 0xd8),
    Color32::from_rgb(0xff, 0xc8, 0x66),
    Color32::from_rgb(0xb8, 0xff, 0x66),
    Color32::from_rgb(0xff, 0x8a, 0x66),
];

/// One keyframe as the editor handles it: a time in ms (`None` = the end of the animation), a value and the easing of the step to the next key.
#[derive(Clone, Debug, PartialEq)]
pub struct EKey {
    pub t: Option<f64>,
    pub v: f64,
    pub e: String,
}

impl EKey {
    fn from_json(k: &Value) -> EKey {
        EKey { t: k[0].as_f64(), v: k[1].as_f64().unwrap_or(0.0), e: k[2].as_str().unwrap_or("linear").to_string() }
    }
    fn to_json(&self) -> Value {
        json!([self.t.map(|t| json!(t)).unwrap_or(json!("end")), (self.v * 1000.0).round() / 1000.0, self.e])
    }
    fn at(&self, total: f32) -> f32 {
        self.t.map(|t| t as f32).unwrap_or(total)
    }
}

fn read(settings: &Value, pid: &str) -> Vec<EKey> {
    engine::presets::keys_of(&settings["props"][pid]).iter().map(EKey::from_json).collect()
}

fn write(settings: &mut Value, pid: &str, keys: &[EKey]) {
    settings["props"][pid] = Value::Array(keys.iter().map(EKey::to_json).collect());
}

fn sort(keys: &mut [EKey]) {
    keys.sort_by(|a, b| a.t.unwrap_or(f64::MAX).partial_cmp(&b.t.unwrap_or(f64::MAX)).unwrap_or(std::cmp::Ordering::Equal));
}

struct Drag {
    /// The keys of every property that is being dragged, as they were when the drag began.
    orig: Vec<(usize, Vec<EKey>)>,
    start: Pos2,
    moved: bool,
}

/// What the dock remembers between frames.
pub struct MotionState {
    pub prop: usize,
    pub curves: bool,
    /// Whether the dock is open (it starts open) and how tall it is.
    pub open: bool,
    pub height: f32,
    /// Where a scrub put the playhead: (the card's position, the exact time asked for), so the tag shows 75 ms and not the 77 ms
    /// of the nearest frame, for as long as the card stays on that frame.
    pinned: Option<(f32, f32)>,
    /// Snap resolution in ms for keys and the playhead, 0 = off.
    pub snap_ms: f32,
    /// The last motion time the card showed (kept while it shows the blank part of its loop).
    last_tau: f32,
    /// Selected keys as (property, index).
    pub sel: Vec<(usize, usize)>,
    /// The visible stretch of time, `None` = all of it.
    view: Option<(f32, f32)>,
    /// A value range chosen by zooming, `None` = fitted to the curve.
    vrange: Option<(f32, f32)>,
    undo: Vec<Value>,
    redo: Vec<Value>,
    clip: Vec<EKey>,
    drag: Option<Drag>,
    boxsel: Option<Pos2>,
    scrub_resume: Option<bool>,
}

impl Default for MotionState {
    fn default() -> Self {
        MotionState { prop: 0, curves: true, open: true, height: 360.0, pinned: None, snap_ms: 10.0, last_tau: 0.0, sel: vec![], view: None, vrange: None, undo: vec![], redo: vec![], clip: vec![], drag: None, boxsel: None, scrub_resume: None }
    }
}

impl MotionState {
    pub fn from_config(c: &(bool, f32)) -> MotionState {
        MotionState { open: c.0, height: c.1, ..Default::default() }
    }

    fn remember(&mut self, settings: &Value) {
        let snap = snapshot(settings);
        if self.undo.last() != Some(&snap) {
            self.undo.push(snap);
            if self.undo.len() > 120 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
    }
    pub fn selected_in(&self, p: usize) -> Vec<usize> {
        let mut v: Vec<usize> = self.sel.iter().filter(|s| s.0 == p).map(|s| s.1).collect();
        v.sort();
        v
    }
}

fn nice(v: f32) -> String {
    let s = format!("{:.3}", v);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A round step at least `min` big: 1, 2, 2.5 or 5 times a power of ten.
fn nice_step(min: f32) -> f32 {
    let min = min.max(1e-6);
    let p = 10f32.powf(min.log10().floor());
    for m in [1.0, 2.0, 2.5, 5.0, 10.0] {
        if p * m >= min {
            return p * m;
        }
    }
    p * 10.0
}

fn font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

fn val_at(st: &Studio, pid: &str, tau: f32) -> f32 {
    let parsed = Settings::from_json(&st.settings);
    keys::eval(parsed.prop(pid), tau, parsed.design_length())
}

/// The motion time the animation card is showing, `None` while it shows the blank part of its loop.
pub fn playhead(st: &Studio) -> Option<f32> {
    if let Some((pos, t)) = st.motion.pinned {
        if (st.anim_pos - pos).abs() < 1e-3 {
            return Some(t);
        }
    }
    let idx = (st.anim_pos * st.fps / 1000.0).max(0.0) as usize;
    engine::gen::frame_tau(&Settings::from_json(&st.settings), idx)
}

/// Put the animation card on the frame that shows motion time `tau`.
fn seek_tau(st: &mut Studio, tau: f32) {
    let s = Settings::from_json(&st.settings);
    let count = if st.frames.is_empty() { ((s.lead + s.real_length() + s.tail) * s.fps / 1000.0) as usize + 2 } else { st.frames.len() };
    let mut best: Option<(usize, f32)> = None;
    for i in 0..count {
        if let Some(t) = engine::gen::frame_tau(&s, i) {
            let d = (t - tau).abs();
            if best.is_none_or(|b| d < b.1) {
                best = Some((i, d));
            }
        }
    }
    if let Some((i, _)) = best {
        st.anim_pos = (i as f32 + 0.5) * 1000.0 / st.fps.max(1.0);
    }
}

fn snapped(st: &Studio, t: f32) -> f32 {
    let r = st.motion.snap_ms;
    if r > 0.0 { (t / r).round() * r } else { t }
}

/// The thumbnail of an easing: its curve in a small box.
pub fn ease_thumb(p: &egui::Painter, r: Rect, name: &str, col: Color32) {
    let pts: Vec<Pos2> = (0..=24).map(|i| {
        let u = i as f32 / 24.0;
        Pos2::new(r.left() + u * r.width(), r.bottom() - keys::ease(name, u).clamp(-0.4, 1.4) * r.height())
    }).collect();
    p.add(Shape::line(pts, Stroke::new(1.6, col)));
}

/// A grid of easings to choose from; returns the name that was clicked.
pub fn ease_picker(ui: &mut Ui, current: &str) -> Option<String> {
    let mut picked = None;
    let names = engine::presets::easings();
    let cols = 4;
    ui.set_min_width(4.0 * 74.0 + 10.0);
    egui::Grid::new("ease_grid").spacing(Vec2::new(6.0, 6.0)).show(ui, |ui| {
        for (i, name) in names.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(Vec2::new(68.0, 64.0), Sense::click());
            let on = name == current;
            ui.painter().rect_filled(r, CornerRadius::same(6), if resp.hovered() { DARK3 } else { BACKGROUND5 });
            if on {
                ui.painter().rect_stroke(r, CornerRadius::same(6), Stroke::new(1.5, HIGHLIGHT1), StrokeKind::Inside);
            }
            let plot = Rect::from_min_max(r.min + Vec2::new(10.0, 10.0), Pos2::new(r.right() - 10.0, r.bottom() - 24.0));
            ui.painter().rect_stroke(plot, CornerRadius::ZERO, Stroke::new(1.0, Color32::from_white_alpha(12)), StrokeKind::Inside);
            ease_thumb(ui.painter(), plot, name, if on { HIGHLIGHT1 } else { CONTENT1 });
            ui.painter().text(Pos2::new(r.center().x, r.bottom() - 11.0), Align2::CENTER_CENTER, name, font(10.5), if on { HIGHLIGHT1 } else { LIGHT1 });
            if resp.clicked() {
                picked = Some(name.clone());
                ui.close();
            }
            if (i + 1) % cols == 0 {
                ui.end_row();
            }
        }
    });
    picked
}

/// The dock itself; `root` is the window's top-level `Ui`. Call it between the transport bar and the central panel.
pub fn dock(root: &mut Ui, st: &mut Studio) {
    let window_h = root.ctx().content_rect().height();
    // a window too short for the editor shows the folded bar whatever was chosen
    let too_short = window_h < 560.0;
    if !st.motion.open || too_short {
        egui::Panel::bottom("motion_dock_folded").exact_size(40.0).frame(egui::Frame::new().fill(BACKGROUND5).inner_margin(egui::Margin::ZERO)).show(root, |ui| {
            let full = ui.max_rect();
            ui.painter().line_segment([full.left_top(), full.right_top()], Stroke::new(1.0, Color32::from_white_alpha(14)));
            ui.painter().text(Pos2::new(full.left() + 16.0, full.center().y), Align2::LEFT_CENTER, "Motion", FontId::new(17.0, crate::theme::bold()), CONTENT1);
            ui.painter().text(Pos2::new(full.left() + 100.0, full.center().y), Align2::LEFT_CENTER, "curves, keys and the animation preview", font(12.5), LIGHT3);
            if fold_button(ui, Rect::from_center_size(Pos2::new(full.right() - 28.0, full.center().y), Vec2::new(34.0, 28.0)), false) && !too_short {
                st.motion.open = true;
                st.cfg.dock_open = true;
                st.cfg.save();
            }
        });
        return;
    }
    // the dock is as tall as `height` says; its top edge is a handle that changes it (and the choice is remembered)
    let max_h = (window_h - 240.0).max(300.0);
    let height = st.motion.height.clamp(300.0, max_h);
    egui::Panel::bottom("motion_dock")
        .resizable(false)
        .exact_size(height)
        .frame(egui::Frame::new().fill(BACKGROUND5).inner_margin(egui::Margin::ZERO))
        .show(root, |ui| {
            let full = ui.max_rect();
            ui.painter().line_segment([full.left_top(), full.right_top()], Stroke::new(1.0, Color32::from_white_alpha(14)));
            let handle = Rect::from_min_size(full.left_top(), Vec2::new(full.width(), 7.0));
            let hresp = ui.interact(handle, ui.scope_id().with("motion-handle"), Sense::drag()).on_hover_cursor(egui::CursorIcon::ResizeVertical);
            if hresp.dragged() {
                st.motion.height = (height - hresp.drag_delta().y).clamp(300.0, max_h);
            }
            if hresp.drag_stopped() {
                st.cfg.dock_height = st.motion.height;
                st.cfg.save();
            }
            // nothing is drawn until the pointer is on the edge: then a thin line shows that it can be dragged
            if hresp.hovered() || hresp.dragged() {
                ui.painter().line_segment([full.left_top() + Vec2::new(0.0, 1.0), full.right_top() + Vec2::new(0.0, 1.0)], Stroke::new(2.0, Color32::from_rgba_unmultiplied(0x8c, 0x66, 0xff, 200)));
            }
            editor(ui, st, full);
        });
}

/// The arrow that folds the dock away (pointing down) or brings it back (pointing up).
fn fold_button(ui: &mut Ui, r: Rect, open: bool) -> bool {
    let resp = ui.interact(r, ui.scope_id().with(("mfold", open)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(if open { "Fold the motion editor away" } else { "Open the motion editor" });
    if resp.hovered() {
        ui.painter().rect_filled(r, CornerRadius::same(6), DARK2);
    }
    let (c, d) = (r.center(), if open { 1.0 } else { -1.0 });
    ui.painter().add(Shape::line(vec![c + Vec2::new(-6.0, -3.0 * d), c + Vec2::new(0.0, 3.0 * d), c + Vec2::new(6.0, -3.0 * d)], Stroke::new(2.2, if resp.hovered() { CONTENT1 } else { LIGHT1 })));
    resp.clicked()
}

fn editor(ui: &mut Ui, st: &mut Studio, full: Rect) {
    let parsed = Settings::from_json(&st.settings);
    let parsed_now = parsed.clone();
    let total = parsed.design_length().max(1.0);
    let n = st.frames.len();
    let loop_ms = if n > 0 { n as f32 * 1000.0 / st.fps.max(1.0) } else { 1000.0 };

    // ---- the animation preview lives in the dock, on the right (drawn first: it moves the playhead)
    let ctx_e = ui.ctx().clone();
    let mut right = full.right();
    // the graph keeps at least 300 points next to the property list; the preview gets the rest, up to 45% of the dock
    let spare = full.width() - 214.0 - 300.0 - 24.0;
    st.anim_limit = (spare.min(full.width() * 0.45), full.height() - 16.0);
    if n > 0 && spare >= crate::ui::card_min_width(st) {
        let size = crate::ui::anim_card_size(&ctx_e, st);
        let origin = Pos2::new(full.right() - size.x - 8.0, full.top() + 8.0);
        // a column of the dock, not a card: its own shade and a divider, flush with the dock's edges
        let col = Rect::from_min_max(Pos2::new(origin.x - 8.0, full.top() + 1.0), full.max);
        ui.painter().rect_filled(col, CornerRadius::ZERO, BACKGROUND6);
        ui.painter().line_segment([col.left_top(), col.left_bottom()], Stroke::new(1.0, Color32::from_white_alpha(10)));
        crate::ui::anim_card(&ctx_e, ui, st, origin);
        right = col.left();
    }
    let head_tau = playhead(st);
    if let Some(t) = head_tau {
        st.motion.last_tau = t;
    }
    let tau_now = st.motion.last_tau;

    // ---- layout
    let head = Rect::from_min_size(full.min + Vec2::new(0.0, 1.0), Vec2::new(right - full.left(), 40.0));
    let list = Rect::from_min_max(Pos2::new(full.left(), head.bottom()), Pos2::new(full.left() + 214.0, full.bottom()));
    let main = Rect::from_min_max(Pos2::new(list.right(), head.bottom()), Pos2::new(right, full.bottom()));
    let ruler = Rect::from_min_size(main.min + Vec2::new(0.0, 0.0), Vec2::new(main.width(), 24.0));
    let strip = Rect::from_min_size(Pos2::new(main.left() + 8.0, main.bottom() - 42.0), Vec2::new(main.width() - 16.0, 36.0));
    let plot = Rect::from_min_max(Pos2::new(main.left() + 52.0, ruler.bottom() + 6.0), Pos2::new(main.right() - 18.0, strip.top() - 6.0));
    let p = ui.painter().clone();
    p.rect_filled(list, CornerRadius::ZERO, BACKGROUND6);
    p.line_segment([list.right_top(), list.right_bottom()], Stroke::new(1.0, Color32::from_white_alpha(10)));

    if main.width() < 320.0 || full.height() < 220.0 {
        header(ui, st, head, total, (0.0, total));
        p.text(main.center(), Align2::CENTER_CENTER, "Make the window larger to edit the motion", font(14.0), LIGHT3);
        return;
    }

    // ---- time axis (zoomed and panned)
    let (t0, t1) = st.motion.view.unwrap_or((0.0, total));
    let (t0, t1) = if t1 - t0 < 5.0 { (t0, t0 + 5.0) } else { (t0, t1) };
    let to_x = |t: f32| plot.left() + (t - t0) / (t1 - t0) * plot.width();
    let from_x = |x: f32| t0 + (x - plot.left()) / plot.width() * (t1 - t0);
    let step_t = nice_step(70.0 / plot.width().max(1.0) * (t1 - t0));
    // the grid keys and the playhead snap to: the chosen resolution, or (snapping off) none at all
    let snap_step = st.motion.snap_ms;
    let nudge_step = if snap_step > 0.0 { snap_step } else { nice_step(10.0 / plot.width() * (t1 - t0)).max(1.0) };

    header(ui, st, head, total, (t0, t1));

    // ---- keyboard (only when the dock has the focus or the pointer is over it, and no text box is being typed in)
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let over = pointer.is_some_and(|q| full.contains(q));
    let plot_id = ui.scope_id().with("motion-plot");
    let focused = ui.ctx().memory(|m| m.has_focus(plot_id));
    let typing = ui.ctx().memory(|m| m.focused().is_some()) && !focused;
    if (over || focused) && !typing {
        keyboard(ui, st, total, nudge_step, loop_ms);
    }

    // ---- the property list
    property_list(ui, st, list, tau_now);

    // ---- ruler
    let rresp = ui.interact(ruler, ui.scope_id().with("motion-ruler"), Sense::click_and_drag());
    p.rect_filled(ruler, CornerRadius::ZERO, BACKGROUND6);
    let mut tick = (t0 / step_t).ceil() * step_t;
    let mut guard = 0;
    while tick <= t1 && guard < 400 {
        guard += 1;
        let x = to_x(tick);
        p.line_segment([Pos2::new(x, ruler.bottom() - 7.0), Pos2::new(x, ruler.bottom())], Stroke::new(1.0, Color32::from_white_alpha(40)));
        p.text(Pos2::new(x + 4.0, ruler.center().y - 1.0), Align2::LEFT_CENTER, format!("{} ms", nice(tick)), font(11.0), LIGHT3);
        tick += step_t;
    }
    if rresp.dragged() || rresp.clicked() {
        if let Some(m) = rresp.interact_pointer_pos() {
            if st.motion.scrub_resume.is_none() {
                st.motion.scrub_resume = Some(st.anim_playing);
            }
            st.anim_playing = false;
            let t = snapped(st, from_x(m.x)).clamp(0.0, total);
            seek_tau(st, t);
            st.motion.pinned = Some((st.anim_pos, t));
        }
    }
    if !rresp.dragged() && !rresp.is_pointer_button_down_on() {
        if let Some(was) = st.motion.scrub_resume.take() {
            st.anim_playing = was;
        }
    }

    // ---- the plot area and its input
    let bg = ui.interact(plot.expand2(Vec2::new(10.0, 4.0)), plot_id, Sense::click_and_drag());
    p.rect_filled(plot, CornerRadius::same(4), BACKGROUND6);
    zoom_and_pan(ui, st, &bg, plot, (t0, t1), total, to_x);

    // vertical grid
    let mut tick = (t0 / step_t).ceil() * step_t;
    let mut guard = 0;
    while tick <= t1 && guard < 400 {
        guard += 1;
        let x = to_x(tick);
        p.line_segment([Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())], Stroke::new(1.0, Color32::from_white_alpha(if (tick / step_t).round() as i64 % 5 == 0 { 14 } else { 7 })));
        tick += step_t;
    }
    // the part of the motion the animation never shows (the window shift moves what the frames cover): dimmed
    {
        let s_ = parsed_now.clone();
        let count = if st.frames.is_empty() { ((s_.lead + s_.real_length() + s_.tail) * s_.fps / 1000.0) as usize + 2 } else { st.frames.len() };
        let shown: Vec<f32> = (0..count).filter_map(|i| engine::gen::frame_tau(&s_, i)).collect();
        if let (Some(lo), Some(hi)) = (shown.iter().cloned().reduce(f32::min), shown.iter().cloned().reduce(f32::max)) {
            let dim = Color32::from_white_alpha(9);
            let clip = p.with_clip_rect(plot);
            if lo > 0.5 {
                let r = Rect::from_min_max(Pos2::new(to_x(0.0), plot.top()), Pos2::new(to_x(lo), plot.bottom()));
                clip.rect_filled(r, CornerRadius::ZERO, dim);
            }
            if hi < total - 0.5 {
                let r = Rect::from_min_max(Pos2::new(to_x(hi), plot.top()), Pos2::new(to_x(total), plot.bottom()));
                clip.rect_filled(r, CornerRadius::ZERO, dim);
                if r.width() > 190.0 {
                    clip.text(Pos2::new(r.center().x, plot.bottom() - 10.0), Align2::CENTER_CENTER, format!("not shown: the animation ends at {} ms", nice(hi.round())), font(11.5), LIGHT3);
                } else if r.width() > 60.0 {
                    clip.text(Pos2::new(r.center().x, plot.bottom() - 10.0), Align2::CENTER_CENTER, "not shown", font(11.5), LIGHT3);
                }
            }
        }
    }
    // the ends of the animation
    for t in [0.0, total] {
        if t >= t0 && t <= t1 {
            let x = to_x(t);
            p.line_segment([Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())], Stroke::new(1.5, Color32::from_white_alpha(38)));
        }
    }

    let mut ctx = Ctx { st, plot, total, t0, t1, snap_step };
    if ctx.st.motion.curves {
        curves(ui, &mut ctx, &bg, tau_now);
    } else {
        dope(ui, &mut ctx, &bg, tau_now);
    }

    // where a click on the ruler would put the playhead (snapped), with its time
    if let Some(m) = pointer.filter(|q| ruler.contains(*q) && !rresp.dragged()) {
        let t = snapped(ctx.st, from_x(m.x)).clamp(0.0, total);
        let x = to_x(t);
        p.line_segment([Pos2::new(x, ruler.top() + 12.0), Pos2::new(x, ruler.bottom())], Stroke::new(1.0, Color32::from_white_alpha(60)));
        stamp(&p, ruler, x, &format!("{} ms", nice(t)), Color32::from_white_alpha(120), BACKGROUND5);
    }
    key_strip(ui, ctx.st, strip, total);

    // the playhead, over everything, with its time on the ruler
    if let Some(t) = head_tau.filter(|t| *t >= t0 && *t <= t1) {
        let x = to_x(t);
        let pink = Color32::from_rgb(0xff, 0x66, 0x9d);
        p.line_segment([Pos2::new(x, ruler.top()), Pos2::new(x, plot.bottom())], Stroke::new(1.5, pink));
        stamp(&p, ruler, x, &format!("{} ms", nice(t.round())), Color32::WHITE, pink);
    }
}

/// A small timestamp tag hanging from the top of the ruler at `x`.
fn stamp(p: &egui::Painter, ruler: Rect, x: f32, text: &str, fg: Color32, bg: Color32) {
    let g = p.layout_no_wrap(text.to_string(), FontId::new(11.5, crate::theme::semibold()), fg);
    let size = g.size() + Vec2::new(12.0, 4.0);
    let mut r = Rect::from_min_size(Pos2::new(x - size.x * 0.5, ruler.top() + 1.0), size);
    if r.left() < ruler.left() + 2.0 {
        r = r.translate(Vec2::new(ruler.left() + 2.0 - r.left(), 0.0));
    }
    if r.right() > ruler.right() - 2.0 {
        r = r.translate(Vec2::new(ruler.right() - 2.0 - r.right(), 0.0));
    }
    p.rect_filled(r, CornerRadius::same(4), bg);
    p.galley(r.min + Vec2::new(6.0, 2.0), g, fg);
}

/// Under the graph: time, value and easing of the selected key to type or pick, a delete button, and the tools for the property.
fn key_strip(ui: &mut Ui, st: &mut Studio, strip: Rect, total: f32) {
    let p = ui.painter().clone();
    p.rect_filled(strip, CornerRadius::same(8), BACKGROUND6);
    let prop = st.motion.prop.min(PROPS.len() - 1);
    // the key being edited: the last one clicked, in the property being shown
    let target = st.motion.sel.iter().rev().find(|s| s.0 == prop).copied().or_else(|| st.motion.sel.last().copied());
    let mut x = strip.left() + 12.0;
    let cy = strip.center().y;
    match target {
        Some((tp, ti)) if read(&st.settings, PROPS[tp].0).get(ti).is_some() => {
            let pid = PROPS[tp].0;
            let k = read(&st.settings, pid)[ti].clone();
            p.text(Pos2::new(x, cy), Align2::LEFT_CENTER, format!("{} · key {}", PROPS[tp].1.split(' ').next().unwrap_or(""), ti + 1), FontId::new(12.5, crate::theme::semibold()), PROP_COLORS[tp]);
            x += 120.0;
            let cur = [k.t.map(|t| nice(t as f32)).unwrap_or_else(|| "end".to_string()), nice(k.v as f32)];
            for (f, (label, w)) in [("Time (ms)", 66.0f32), ("Value", 76.0)].into_iter().enumerate() {
                p.text(Pos2::new(x, cy), Align2::LEFT_CENTER, label, font(12.0), LIGHT3);
                x += if f == 0 { 64.0 } else { 44.0 };
                let key = format!("mkey:{tp}:{ti}:{f}");
                let buf = st.bufs.entry(key.clone()).or_insert_with(|| cur[f].clone());
                let field = Rect::from_min_size(Pos2::new(x, cy - 12.0), Vec2::new(w, 24.0));
                p.rect_filled(field, CornerRadius::same(5), CONTROL);
                let resp = crate::theme::put_free(ui, field.shrink2(Vec2::new(6.0, 2.0)), egui::TextEdit::singleline(buf).id(ui.scope_id().with(("mkeyedit", tp, ti, f))).frame(egui::Frame::NONE).desired_width(w - 12.0).font(font(14.0)).text_color(CONTENT1).margin(egui::Margin::ZERO));
                let text = buf.clone();
                if resp.changed() {
                    st.motion.remember(&st.settings);
                    crate::state::edit_key(&mut st.settings, pid, ti, f as i32, &text);
                    st.edited();
                } else if !resp.has_focus() && text != cur[f] {
                    st.bufs.insert(key, cur[f].clone());
                }
                x += w + 14.0;
            }
            // the easing of the step from this key
            p.text(Pos2::new(x, cy), Align2::LEFT_CENTER, "Easing", font(12.0), LIGHT3);
            x += 46.0;
            let er = Rect::from_min_size(Pos2::new(x, cy - 13.0), Vec2::new(132.0, 26.0));
            let eresp = ui.interact(er, ui.scope_id().with("mstrip-ease"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("The easing of the step from this key to the next");
            p.rect_filled(er, CornerRadius::same(5), if eresp.hovered() { DARK3 } else { CONTROL });
            ease_thumb(&p, Rect::from_min_size(er.min + Vec2::new(8.0, 6.0), Vec2::new(22.0, 14.0)), &k.e, CONTENT1);
            p.text(er.left_center() + Vec2::new(38.0, 0.0), Align2::LEFT_CENTER, &k.e, font(13.0), CONTENT1);
            let cur_e = k.e.clone();
            let mut picked = None;
            egui::Popup::menu(&eresp).gap(4.0).show(|ui| {
                picked = ease_picker(ui, &cur_e);
            });
            if let Some(e) = picked {
                st.motion.remember(&st.settings);
                crate::state::edit_key(&mut st.settings, pid, ti, 2, &e);
                commit(st);
            }
            x += 132.0 + 12.0;
            let dr = Rect::from_min_size(Pos2::new(x, cy - 13.0), Vec2::new(60.0, 26.0));
            if pill(ui, dr, "Delete", false, "Remove the selected keys (Delete)") {
                delete_selected(st);
            }
        }
        _ => {
            p.text(Pos2::new(x, cy), Align2::LEFT_CENTER, "Select a key to type its time and value or to change its easing", font(13.0), LIGHT3);
        }
    }
    // the tools of the property, on the right
    let tr = Rect::from_min_size(Pos2::new(strip.right() - 90.0, cy - 13.0), Vec2::new(80.0, 26.0));
    let tresp = pill_resp(ui, tr, "Tools", false, "Reverse, flip, smooth or reset the property being shown");
    {
        let c = Pos2::new(tr.right() - 14.0, tr.center().y + 0.5);
        p.add(Shape::line(vec![c + Vec2::new(-4.0, -2.0), c + Vec2::new(0.0, 2.0), c + Vec2::new(4.0, -2.0)], Stroke::new(1.8, LIGHT1)));
    }
    egui::Popup::menu(&tresp).gap(4.0).show(|ui| {
        ui.set_min_width(190.0);
        let items: [(&str, u8); 6] = [("Reverse in time", 0), ("Flip the values", 1), ("Smooth every step", 2), ("Make every step linear", 3), ("Reset to a plain line", 4), ("Make the time shift the base", 5)];
        for (name, id) in items {
            let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width().max(180.0), 30.0), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(r, CornerRadius::same(4), DARK3);
            }
            ui.painter().text(r.left_center() + Vec2::new(10.0, 0.0), Align2::LEFT_CENTER, name, font(14.5), CONTENT1);
            if resp.clicked() {
                match id {
                    0 => reverse(st),
                    1 => flip(st),
                    2 => ease_all(st, "smoother"),
                    3 => ease_all(st, "linear"),
                    5 => {
                        bake_shift(st);
                    }
                    _ => reset(st),
                }
                ui.close();
            }
        }
    });
    let _ = total;
}

struct Ctx<'a> {
    st: &'a mut Studio,
    plot: Rect,
    total: f32,
    t0: f32,
    t1: f32,
    snap_step: f32,
}

impl Ctx<'_> {
    fn to_x(&self, t: f32) -> f32 {
        self.plot.left() + (t - self.t0) / (self.t1 - self.t0) * self.plot.width()
    }
    fn from_x(&self, x: f32) -> f32 {
        self.t0 + (x - self.plot.left()) / self.plot.width() * (self.t1 - self.t0)
    }
}

// ----------------------------------------------------------------------------------------------------------- header

fn pill(ui: &mut Ui, r: Rect, label: &str, on: bool, tip: &str) -> bool {
    pill_resp(ui, r, label, on, tip).clicked()
}

fn pill_resp(ui: &mut Ui, r: Rect, label: &str, on: bool, tip: &str) -> egui::Response {
    let resp = ui.interact(r, ui.scope_id().with(("mpill", label)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip);
    let fill = if on { th_purple() } else if resp.hovered() { DARK3 } else { BACKGROUND6 };
    ui.painter().rect_filled(r, CornerRadius::same(6), fill);
    ui.painter().text(r.center(), Align2::CENTER_CENTER, label, FontId::new(13.5, crate::theme::semibold()), if on || resp.hovered() { CONTENT1 } else { LIGHT1 });
    resp
}

fn th_purple() -> Color32 {
    Color32::from_rgb(0x66, 0x3d, 0xd4)
}

fn header(ui: &mut Ui, st: &mut Studio, head: Rect, total: f32, view: (f32, f32)) {
    let p = ui.painter().clone();
    p.text(Pos2::new(head.left() + 16.0, head.center().y), Align2::LEFT_CENTER, "Motion", FontId::new(17.0, crate::theme::bold()), CONTENT1);

    // on the right: the fold arrow, and the current time (type a number to move the playhead there)
    if fold_button(ui, Rect::from_center_size(Pos2::new(head.right() - 24.0, head.center().y), Vec2::new(34.0, 28.0)), true) {
        st.motion.open = false;
        st.cfg.dock_open = false;
        st.cfg.save();
    }
    let time_w = 132.0;
    let time_left = head.right() - 48.0 - time_w;
    time_field(ui, st, Rect::from_min_size(Pos2::new(time_left, head.top() + 6.0), Vec2::new(time_w, 28.0)), total);

    // the buttons, left to right, as many as there is room for (the least important go first when it is tight)
    let x = std::cell::Cell::new(head.left() + 104.0);
    let limit = time_left - 8.0;
    let place = |w: f32| -> Option<Rect> {
        if x.get() + w > limit {
            return None;
        }
        let r = Rect::from_min_size(Pos2::new(x.get(), head.top() + 6.0), Vec2::new(w, 28.0));
        x.set(x.get() + w + 6.0);
        Some(r)
    };
    if let Some(r) = place(72.0) {
        if pill(ui, r, "Curves", st.motion.curves, "Edit one property as a curve") {
            st.motion.curves = true;
        }
    }
    if let Some(r) = place(92.0) {
        if pill(ui, r, "Dope sheet", !st.motion.curves, "See the keys of every property on one timeline") {
            st.motion.curves = false;
        }
    }
    x.set(x.get() + 8.0);
    if let Some(sr) = place(106.0) {
        let label = if st.motion.snap_ms > 0.0 { format!("Snap: {} ms", nice(st.motion.snap_ms)) } else { "Snap: off".to_string() };
        let sresp = pill_resp(ui, sr, &label, st.motion.snap_ms > 0.0, "Snap resolution for keys and the playhead (hold Alt while dragging to skip it)");
        egui::Popup::menu(&sresp).gap(4.0).show(|ui| {
            ui.set_min_width(120.0);
            for (v, name) in [(0.0, "Off"), (5.0, "5 ms"), (10.0, "10 ms"), (25.0, "25 ms"), (50.0, "50 ms"), (100.0, "100 ms")] {
                let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width().max(110.0), 30.0), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(r, CornerRadius::same(4), DARK3);
                }
                let on = (st.motion.snap_ms - v).abs() < 1e-3;
                ui.painter().text(r.left_center() + Vec2::new(10.0, 0.0), Align2::LEFT_CENTER, name, font(14.5), if on { HIGHLIGHT1 } else { CONTENT1 });
                if resp.clicked() {
                    st.motion.snap_ms = v;
                    ui.close();
                }
            }
        });
    }
    if let Some(r) = place(66.0) {
        if pill(ui, r, "+ Key", false, "Add a key to the selected property at the playhead (K)") {
            add_key_at_playhead(st, total);
        }
    }
    if let Some(r) = place(56.0) {
        if pill(ui, r, "Undo", false, "Undo (Ctrl+Z)") {
            undo(st);
        }
    }
    if let Some(r) = place(56.0) {
        if pill(ui, r, "Redo", false, "Redo (Ctrl+Y)") {
            redo(st);
        }
    }
    if let Some(r) = place(44.0) {
        if pill(ui, r, "Fit", false, "Show the whole animation (F)") {
            st.motion.view = None;
            st.motion.vrange = None;
        }
    }
    if st.motion.view.is_some() {
        let g = p.layout_no_wrap(format!("{:.0} - {:.0} ms", view.0, view.1), font(12.0), LIGHT3);
        if limit - g.size().x > x.get() + 8.0 {
            p.galley(Pos2::new(limit - g.size().x, head.center().y - g.size().y * 0.5), g, LIGHT3);
        }
    }
}

/// "Time [ 423 ] ms": the time of the playhead in the motion, which can be typed over to put the playhead exactly there.
fn time_field(ui: &mut Ui, st: &mut Studio, r: Rect, total: f32) {
    let p = ui.painter().clone();
    p.text(Pos2::new(r.left(), r.center().y), Align2::LEFT_CENTER, "Time", font(13.0), LIGHT3);
    let field = Rect::from_min_size(Pos2::new(r.left() + 38.0, r.top()), Vec2::new(66.0, r.height()));
    p.rect_filled(field, CornerRadius::same(6), CONTROL);
    let cur = nice(playhead(st).unwrap_or(st.motion.last_tau).round());
    let key = "motion-time".to_string();
    let buf = st.bufs.entry(key.clone()).or_insert_with(|| cur.clone());
    let id = ui.scope_id().with("motion-time-edit");
    let resp = crate::theme::put_free(ui, field.shrink2(Vec2::new(7.0, 3.0)), egui::TextEdit::singleline(buf).id(id).frame(egui::Frame::NONE).desired_width(52.0).font(font(14.5)).align(egui::Align2::LEFT_CENTER).text_color(CONTENT1).margin(egui::Margin::ZERO));
    let text = buf.clone();
    if resp.changed() {
        if let Ok(t) = text.trim().parse::<f32>() {
            let t = t.clamp(0.0, total);
            st.anim_playing = false;
            seek_tau(st, t);
            st.motion.pinned = Some((st.anim_pos, t));
        }
    } else if !resp.has_focus() && text != cur {
        // not being typed in: follow the playhead
        st.bufs.insert(key, cur);
    }
    p.text(Pos2::new(field.right() + 6.0, r.center().y), Align2::LEFT_CENTER, "ms", font(13.0), LIGHT3);
}

// -------------------------------------------------------------------------------------------------- property list

fn property_list(ui: &mut Ui, st: &mut Studio, list: Rect, tau: f32) {
    let row_h = ((list.height() - 8.0) / PROPS.len() as f32).clamp(28.0, 40.0);
    for (i, (pid, label)) in PROPS.iter().enumerate() {
        let r = Rect::from_min_size(list.min + Vec2::new(6.0, 6.0 + i as f32 * row_h), Vec2::new(list.width() - 12.0, row_h - 2.0));
        let resp = ui.interact(r, ui.scope_id().with(("mprop", i)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        let on = st.motion.prop == i;
        if on {
            ui.painter().rect_filled(r, CornerRadius::same(6), Color32::from_rgba_unmultiplied(0x8c, 0x66, 0xff, 46));
            ui.painter().rect_filled(Rect::from_min_size(r.min, Vec2::new(3.0, r.height())), CornerRadius::same(2), PROP_COLORS[i]);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, CornerRadius::same(6), DARK2);
        }
        ui.painter().circle_filled(Pos2::new(r.left() + 16.0, r.center().y), 4.5, PROP_COLORS[i]);
        ui.painter().text(Pos2::new(r.left() + 30.0, r.center().y), Align2::LEFT_CENTER, label, font(13.5), if on { CONTENT1 } else { CONTENT2 });
        let v = val_at(st, pid, tau);
        ui.painter().text(Pos2::new(r.right() - 8.0, r.center().y), Align2::RIGHT_CENTER, nice(v), font(12.5), if on { LIGHT1 } else { LIGHT3 });
        if resp.clicked() {
            select_prop(st, i);
        }
    }
}

fn select_prop(st: &mut Studio, i: usize) {
    if st.motion.prop != i {
        st.motion.prop = i;
        if st.motion.curves {
            st.motion.sel.clear();
        }
        st.motion.vrange = None;
    }
}

// ------------------------------------------------------------------------------------------------------- operations

fn commit(st: &mut Studio) {
    st.bufs.clear();
    st.edited();
}

/// What undo keeps: the motion of every property, the colour keys and the time shift.
fn snapshot(settings: &Value) -> Value {
    json!({ "props": settings["props"].clone(), "color_keys": settings["color_keys"].clone(), "shift": settings["shift"].clone() })
}

fn restore(settings: &mut Value, snap: Value) {
    settings["props"] = snap["props"].clone();
    if snap["color_keys"].is_null() {
        if let Some(o) = settings.as_object_mut() {
            o.remove("color_keys");
        }
    } else {
        settings["color_keys"] = snap["color_keys"].clone();
    }
    if !snap["shift"].is_null() {
        settings["shift"] = snap["shift"].clone();
    }
}

pub fn undo(st: &mut Studio) {
    if let Some(prev) = st.motion.undo.pop() {
        st.motion.redo.push(snapshot(&st.settings));
        restore(&mut st.settings, prev);
        st.motion.sel.clear();
        commit(st);
    }
}

pub fn redo(st: &mut Studio) {
    if let Some(next) = st.motion.redo.pop() {
        st.motion.undo.push(snapshot(&st.settings));
        restore(&mut st.settings, next);
        st.motion.sel.clear();
        commit(st);
    }
}

fn add_key_at_playhead(st: &mut Studio, total: f32) {
    let tau = playhead(st).unwrap_or(st.motion.last_tau).clamp(0.0, total);
    let pid = PROPS[st.motion.prop].0;
    let v = val_at(st, pid, tau) as f64;
    st.motion.remember(&st.settings);
    let mut keys = read(&st.settings, pid);
    let t = (tau as f64).round();
    if keys.iter().any(|k| k.t == Some(t)) {
        return;
    }
    let new = EKey { t: Some(t), v, e: "linear".into() };
    keys.push(new.clone());
    sort(&mut keys);
    let idx = keys.iter().position(|k| *k == new).unwrap_or(0);
    write(&mut st.settings, pid, &keys);
    st.motion.sel = vec![(st.motion.prop, idx)];
    commit(st);
}

fn delete_selected(st: &mut Studio) {
    if st.motion.sel.is_empty() {
        return;
    }
    st.motion.remember(&st.settings);
    for p in 0..PROPS.len() {
        let sel = st.motion.selected_in(p);
        if sel.is_empty() {
            continue;
        }
        let mut keys = read(&st.settings, PROPS[p].0);
        let keep: Vec<EKey> = keys.iter().enumerate().filter(|(i, _)| !sel.contains(i)).map(|(_, k)| k.clone()).collect();
        // a property always keeps at least one key
        keys = if keep.is_empty() { vec![keys[0].clone()] } else { keep };
        write(&mut st.settings, PROPS[p].0, &keys);
    }
    st.motion.sel.clear();
    commit(st);
}

/// Move the selected keys by `dt` ms and `dv` in value, within the room their neighbours leave.
fn nudge(st: &mut Studio, dt: f32, dv: f64, total: f32) {
    if st.motion.sel.is_empty() {
        return;
    }
    st.motion.remember(&st.settings);
    apply_move(st, None, dt, dv, total);
    commit(st);
}

/// Apply a move to the selection relative to `orig` (the keys when a drag began) or to the current keys.
fn apply_move(st: &mut Studio, orig: Option<&Vec<(usize, Vec<EKey>)>>, mut dt: f32, dv: f64, total: f32) {
    let props: Vec<usize> = (0..PROPS.len()).filter(|p| !st.motion.selected_in(*p).is_empty()).collect();
    let base: Vec<(usize, Vec<EKey>)> = match orig {
        Some(o) => o.clone(),
        None => props.iter().map(|p| (*p, read(&st.settings, PROPS[*p].0))).collect(),
    };
    // the room: no key may pass an unselected neighbour (or the two ends of the animation)
    let (mut lo, mut hi) = (f32::MIN, f32::MAX);
    for (p, keys) in &base {
        let sel = st.motion.selected_in(*p);
        for &i in &sel {
            let k = &keys[i];
            if k.t.is_none() || (i == 0 && k.t.unwrap_or(0.0) <= 0.0) {
                continue; // the end key and a first key at zero stay where they are
            }
            let t = k.at(total);
            lo = lo.max(0.0 - t);
            hi = hi.min(total - t);
            if i > 0 && !sel.contains(&(i - 1)) {
                lo = lo.max(keys[i - 1].at(total) + 1.0 - t);
            }
            if i + 1 < keys.len() && !sel.contains(&(i + 1)) {
                hi = hi.min(keys[i + 1].at(total) - 1.0 - t);
            }
        }
    }
    dt = dt.clamp(lo.min(0.0), hi.max(0.0));
    for (p, keys) in &base {
        let sel = st.motion.selected_in(*p);
        let mut out = keys.clone();
        for &i in &sel {
            let k = &mut out[i];
            if let Some(t) = k.t {
                if !(i == 0 && t <= 0.0) {
                    k.t = Some((t + dt as f64).round());
                }
            }
            k.v += dv;
        }
        write(&mut st.settings, PROPS[*p].0, &out);
    }
}

fn keyboard(ui: &mut Ui, st: &mut Studio, total: f32, step: f32, loop_ms: f32) {
    let tau = playhead(st).unwrap_or(st.motion.last_tau);
    let (cmd, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
    let pressed = |k: Key| ui.input(|i| i.key_pressed(k));
    let consume = |k: Key| ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, k));
    if cmd && pressed(Key::Z) && !shift {
        undo(st);
    } else if cmd && (pressed(Key::Y) || (pressed(Key::Z) && shift)) {
        redo(st);
    }
    if consume(Key::Delete) || consume(Key::Backspace) {
        delete_selected(st);
    }
    if consume(Key::Escape) {
        st.motion.sel.clear();
    }
    if cmd && pressed(Key::A) {
        let pr = st.motion.prop;
        st.motion.sel.clear();
        let props: Vec<usize> = if st.motion.curves { vec![pr] } else { (0..PROPS.len()).collect() };
        for p in props {
            for i in 0..read(&st.settings, PROPS[p].0).len() {
                st.motion.sel.push((p, i));
            }
        }
    }
    if cmd && pressed(Key::C) {
        let sel = st.motion.selected_in(st.motion.prop);
        let keys = read(&st.settings, PROPS[st.motion.prop].0);
        st.motion.clip = sel.iter().filter_map(|i| keys.get(*i).cloned()).collect();
    }
    if cmd && pressed(Key::V) && !st.motion.clip.is_empty() {
        let pid = PROPS[st.motion.prop].0;
        st.motion.remember(&st.settings);
        let mut keys = read(&st.settings, pid);
        let first = st.motion.clip[0].t.unwrap_or(0.0);
        let at = tau.round() as f64;
        let mut added = vec![];
        for k in st.motion.clip.clone() {
            let t = (k.t.unwrap_or(total as f64) - first + at).clamp(0.0, total as f64);
            if keys.iter().any(|e| e.t == Some(t)) {
                continue;
            }
            let nk = EKey { t: Some(t), v: k.v, e: k.e };
            keys.push(nk.clone());
            added.push(nk);
        }
        sort(&mut keys);
        st.motion.sel = added.iter().filter_map(|a| keys.iter().position(|k| k == a)).map(|i| (st.motion.prop, i)).collect();
        write(&mut st.settings, pid, &keys);
        commit(st);
    }
    if !cmd && pressed(Key::K) {
        add_key_at_playhead(st, total);
    }
    if !cmd && pressed(Key::F) {
        st.motion.view = None;
        st.motion.vrange = None;
    }
    if !cmd && pressed(Key::Space) {
        st.anim_playing = !st.anim_playing;
        let _ = loop_ms;
    }
    let k = if shift { 10.0 } else { 1.0 };
    if consume_with(ui, Key::ArrowLeft) {
        nudge(st, -step * k, 0.0, total);
    }
    if consume_with(ui, Key::ArrowRight) {
        nudge(st, step * k, 0.0, total);
    }
    if st.motion.curves {
        let (lo, hi) = st.motion.vrange.unwrap_or_else(|| value_range(st, PROPS[st.motion.prop].0, total));
        let vs = nice_step((hi - lo) / 200.0) as f64 * k as f64;
        if consume_with(ui, Key::ArrowUp) {
            nudge(st, 0.0, vs, total);
        }
        if consume_with(ui, Key::ArrowDown) {
            nudge(st, 0.0, -vs, total);
        }
    }
}

fn consume_with(ui: &mut Ui, k: Key) -> bool {
    ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, k) || i.consume_key(egui::Modifiers::SHIFT, k))
}

fn zoom_and_pan(ui: &mut Ui, st: &mut Studio, bg: &egui::Response, plot: Rect, view: (f32, f32), total: f32, to_x: impl Fn(f32) -> f32) {
    let _ = to_x;
    let (t0, t1) = view;
    if bg.hovered() {
        let (scroll, shift, ptr) = ui.input(|i| (i.smooth_scroll_delta.y, i.modifiers.shift, i.pointer.hover_pos()));
        if scroll.abs() > 0.1 {
            let f = (-scroll * 0.004).exp();
            if shift && st.motion.curves {
                // zoom the values around the pointer
                let (lo, hi) = st.motion.vrange.unwrap_or_else(|| value_range(st, PROPS[st.motion.prop].0, total));
                if let Some(m) = ptr {
                    let at = lo + (plot.bottom() - m.y) / plot.height() * (hi - lo);
                    st.motion.vrange = Some((at - (at - lo) * f, at + (hi - at) * f));
                }
            } else if let Some(m) = ptr {
                let at = t0 + (m.x - plot.left()) / plot.width() * (t1 - t0);
                let (n0, n1) = (at - (at - t0) * f, at + (t1 - at) * f);
                let (n0, n1) = (n0.max(-total * 0.05), n1.min(total * 1.05));
                st.motion.view = if n1 - n0 >= total * 1.0 { None } else { Some((n0, n1.max(n0 + 5.0))) };
            }
        }
    }
    // middle-drag pans
    let (mid, delta, down) = ui.input(|i| (i.pointer.middle_down(), i.pointer.delta(), i.pointer.hover_pos()));
    if mid && down.is_some_and(|q| plot.expand(8.0).contains(q)) {
        let dt = -delta.x / plot.width() * (t1 - t0);
        st.motion.view = Some((t0 + dt, t1 + dt));
        if st.motion.curves {
            let (lo, hi) = st.motion.vrange.unwrap_or_else(|| value_range(st, PROPS[st.motion.prop].0, total));
            let dv = delta.y / plot.height() * (hi - lo);
            st.motion.vrange = Some((lo + dv, hi + dv));
        }
    }
}

fn value_range(st: &Studio, pid: &str, total: f32) -> (f32, f32) {
    let parsed = Settings::from_json(&st.settings);
    let nk = parsed.prop(pid).clone();
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for i in 0..=80 {
        let v = keys::eval(&nk, i as f32 / 80.0 * total, total);
        lo = lo.min(v);
        hi = hi.max(v);
    }
    for k in &nk {
        lo = lo.min(k.v);
        hi = hi.max(k.v);
    }
    if hi - lo < 1e-3 {
        lo -= 0.5;
        hi += 0.5;
    }
    let pad = (hi - lo) * 0.2;
    (lo - pad, hi + pad)
}

// ------------------------------------------------------------------------------------------------- shared gestures

/// Begin, continue and end a key drag; `curves` lets the value move too. Returns true while a drag is going on.
fn key_drag(ui: &mut Ui, c: &mut Ctx, r: &egui::Response, prop: usize, idx: usize, value_per_px: Option<f64>) -> bool {
    let additive = ui.input(|i| i.modifiers.shift || i.modifiers.command);
    if r.clicked() || r.drag_started() {
        let here = (prop, idx);
        let on = c.st.motion.sel.contains(&here);
        if additive && r.clicked() {
            if on {
                c.st.motion.sel.retain(|s| *s != here);
            } else {
                c.st.motion.sel.push(here);
            }
        } else if !on {
            if c.st.motion.curves && prop != c.st.motion.prop {
                select_prop(c.st, prop);
            }
            c.st.motion.sel = vec![here];
            c.st.motion.prop = prop;
        }
    }
    if r.drag_started() {
        c.st.motion.remember(&c.st.settings);
        let props: Vec<usize> = (0..PROPS.len()).filter(|p| !c.st.motion.selected_in(*p).is_empty()).collect();
        let orig = props.iter().map(|p| (*p, read(&c.st.settings, PROPS[*p].0))).collect();
        // measured from where the button went down, so the key does not lag behind the pointer by the drag threshold
        let start = ui.input(|i| i.pointer.press_origin()).or(r.interact_pointer_pos()).unwrap_or_default();
        c.st.motion.drag = Some(Drag { orig, start, moved: false });
    }
    let mut active = false;
    if r.dragged() {
        active = true;
        if let (Some(d), Some(m)) = (c.st.motion.drag.as_ref(), r.interact_pointer_pos()) {
            let (shift, alt, ctrl) = ui.input(|i| (i.modifiers.shift, i.modifiers.alt, i.modifiers.command));
            let mut dx = m.x - d.start.x;
            let mut dy = m.y - d.start.y;
            if shift {
                // lock to the axis the pointer has gone furthest along
                if dx.abs() >= dy.abs() { dy = 0.0 } else { dx = 0.0 }
            }
            if value_per_px.is_none() {
                dy = 0.0;
            }
            let mut dt = dx / c.plot.width() * (c.t1 - c.t0);
            let orig = d.orig.clone();
            // snap the dragged key's own time to the grid
            if c.snap_step > 0.0 && !alt {
                if let Some((_, keys)) = orig.iter().find(|(p, _)| *p == prop) {
                    let t = keys[idx].at(c.total);
                    dt = ((t + dt) / c.snap_step).round() * c.snap_step - t;
                }
            }
            let mut dv = -(dy as f64) * value_per_px.unwrap_or(0.0);
            if ctrl {
                if let Some((_, keys)) = orig.iter().find(|(p, _)| *p == prop) {
                    let step = (nice_step(20.0 * value_per_px.unwrap_or(0.01) as f32)) as f64;
                    dv = ((keys[idx].v + dv) / step).round() * step - keys[idx].v;
                }
            }
            if let Some(d) = c.st.motion.drag.as_mut() {
                d.moved = dx.abs() + dy.abs() > 1.0;
            }
            apply_move(c.st, Some(&orig), dt, dv, c.total);
            commit(c.st);
        }
    }
    if r.drag_stopped() {
        c.st.motion.drag = None;
    }
    active
}

/// Drag on empty space to box-select; click on empty space to clear; double-click to add. Returns the box while it is being drawn.
fn background(ui: &mut Ui, c: &mut Ctx, bg: &egui::Response, hit: impl Fn(Rect) -> Vec<(usize, usize)>) -> Option<Rect> {
    if bg.drag_started_by(egui::PointerButton::Primary) && c.st.motion.drag.is_none() {
        // the box starts where the button went down, not where the pointer was when egui noticed the drag
        c.st.motion.boxsel = ui.input(|i| i.pointer.press_origin()).or(bg.interact_pointer_pos());
    }
    let mut rect = None;
    if let (Some(a), Some(b)) = (c.st.motion.boxsel, ui.input(|i| i.pointer.interact_pos())) {
        rect = Some(Rect::from_two_pos(a, b));
    }
    if bg.drag_stopped() {
        if let Some(r) = rect.take() {
            let add = ui.input(|i| i.modifiers.shift || i.modifiers.command);
            let found = hit(r);
            if !add {
                c.st.motion.sel.clear();
            }
            for f in found {
                if !c.st.motion.sel.contains(&f) {
                    c.st.motion.sel.push(f);
                }
            }
        }
        c.st.motion.boxsel = None;
    }
    if bg.clicked() && c.st.motion.drag.is_none() {
        c.st.motion.sel.clear();
    }
    if let Some(r) = rect {
        ui.painter().rect_filled(r, CornerRadius::same(2), Color32::from_rgba_unmultiplied(0x8c, 0x66, 0xff, 36));
        ui.painter().rect_stroke(r, CornerRadius::same(2), Stroke::new(1.0, HIGHLIGHT1), StrokeKind::Inside);
    }
    rect
}

// ------------------------------------------------------------------------------------------------------- curve mode

fn curves(ui: &mut Ui, c: &mut Ctx, bg: &egui::Response, tau: f32) {
    let prop = c.st.motion.prop;
    let (pid, label) = PROPS[prop];
    let col = PROP_COLORS[prop];
    let total = c.total;
    let parsed = Settings::from_json(&c.st.settings);
    let nk = parsed.prop(pid).clone();
    let ekeys = read(&c.st.settings, pid);
    let p = ui.painter().clone();
    let plot = c.plot;

    // the value range: fitted to the curve unless zoomed by hand, and held still while a key is dragged
    let rid = ui.scope_id().with(("motion-range", pid));
    let dragging = c.st.motion.drag.is_some();
    let mut range = c.st.motion.vrange.unwrap_or_else(|| value_range(c.st, pid, total));
    if dragging {
        if let Some(h) = ui.memory(|m| m.data.get_temp::<(f32, f32)>(rid)) {
            range = h;
        }
    }
    ui.memory_mut(|m| {
        if dragging {
            m.data.insert_temp(rid, range);
        } else {
            m.data.remove::<(f32, f32)>(rid);
        }
    });
    let (lo, hi) = range;
    let to_y = |v: f32| plot.bottom() - (v - lo) / (hi - lo) * plot.height();
    let from_y = |y: f32| lo + (plot.bottom() - y) / plot.height() * (hi - lo);

    // horizontal grid with labels
    let vstep = nice_step(34.0 / plot.height() * (hi - lo));
    let mut v = (lo / vstep).ceil() * vstep;
    let mut vguard = 0;
    while v <= hi && vguard < 200 {
        vguard += 1;
        let y = to_y(v);
        p.line_segment([Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)], Stroke::new(1.0, Color32::from_white_alpha(if v.abs() < vstep * 0.01 { 30 } else { 10 })));
        p.text(Pos2::new(plot.left() - 8.0, y), Align2::RIGHT_CENTER, nice(v), font(11.0), LIGHT3);
        v += vstep;
    }
    p.text(Pos2::new(plot.right() - 8.0, plot.top() + 12.0), Align2::RIGHT_CENTER, label, FontId::new(13.0, crate::theme::semibold()), Color32::from_white_alpha(70));

    // the curve, filled underneath
    let steps = (plot.width() / 3.0) as usize;
    let pts: Vec<Pos2> = (0..=steps).map(|i| {
        let t = c.t0 + i as f32 / steps as f32 * (c.t1 - c.t0);
        Pos2::new(plot.left() + i as f32 / steps as f32 * plot.width(), to_y(keys::eval(&nk, t.clamp(0.0, total), total)).clamp(plot.top() - 40.0, plot.bottom() + 40.0))
    }).collect();
    let mut fill = pts.clone();
    fill.push(Pos2::new(plot.right(), plot.bottom()));
    fill.push(Pos2::new(plot.left(), plot.bottom()));
    let clipped = p.with_clip_rect(plot);
    let _ = fill;
    clipped.add(Shape::line(pts, Stroke::new(2.6, col)));

    // box select
    let (t0, t1) = (c.t0, c.t1);
    let boxed = background(ui, c, bg, |r| {
        ekeys.iter().enumerate().filter(|(_, k)| r.contains(Pos2::new(plot.left() + (k.at(total) - t0) / (t1 - t0) * plot.width(), to_y(k.v as f32)))).map(|(i, _)| (prop, i)).collect()
    });
    let _ = boxed;
    if bg.double_clicked() {
        if let Some(m) = bg.interact_pointer_pos() {
            if plot.contains(m) {
                c.st.motion.remember(&c.st.settings);
                let t = c.from_x(m.x);
                let t = if c.snap_step > 0.0 { (t / c.snap_step).round() * c.snap_step } else { t.round() };
                let mut keys = ekeys.clone();
                let nk2 = EKey { t: Some(t.clamp(0.0, total) as f64), v: ((from_y(m.y) * 1000.0).round() / 1000.0) as f64, e: "linear".into() };
                keys.push(nk2.clone());
                sort(&mut keys);
                write(&mut c.st.settings, pid, &keys);
                c.st.motion.sel = vec![(prop, keys.iter().position(|k| *k == nk2).unwrap_or(0))];
                commit(c.st);
            }
        }
    }

    // segment pills: click to choose the easing of that step
    for i in 0..ekeys.len().saturating_sub(1) {
        let (a, b) = (&ekeys[i], &ekeys[i + 1]);
        let (ta, tb) = (a.at(total), b.at(total));
        let w = (tb - ta) / (c.t1 - c.t0) * plot.width();
        if w < 66.0 {
            continue;
        }
        let tm = (ta + tb) * 0.5;
        let pos = Pos2::new(c.to_x(tm), to_y(keys::eval(&nk, tm, total)));
        if !plot.contains(pos) {
            continue;
        }
        let r = Rect::from_center_size(pos, Vec2::new(34.0, 22.0));
        let resp = ui.interact(r, ui.scope_id().with(("mseg", prop, i)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(format!("Easing: {} (click to change)", a.e));
        p.rect_filled(r, CornerRadius::same(5), if resp.hovered() { DARK3 } else { BACKGROUND5 });
        p.rect_stroke(r, CornerRadius::same(5), Stroke::new(1.0, if resp.hovered() { col } else { Color32::from_white_alpha(28) }), StrokeKind::Inside);
        ease_thumb(&p, Rect::from_min_max(r.min + Vec2::new(8.0, 5.0), r.max - Vec2::new(8.0, 5.0)), &a.e, CONTENT1);
        let cur = a.e.clone();
        let mut picked = None;
        egui::Popup::menu(&resp).gap(4.0).show(|ui| {
            picked = ease_picker(ui, &cur);
        });
        if let Some(e) = picked {
            c.st.motion.remember(&c.st.settings);
            let mut keys2 = ekeys.clone();
            let sel = c.st.motion.selected_in(prop);
            // the chosen easing goes to every selected step, or just this one when this step's key is not selected
            if sel.contains(&i) {
                for s in sel {
                    keys2[s].e = e.clone();
                }
            } else {
                keys2[i].e = e;
            }
            write(&mut c.st.settings, pid, &keys2);
            commit(c.st);
        }
    }

    // the playhead's value
    if tau >= c.t0 && tau <= c.t1 {
        let y = to_y(keys::eval(&nk, tau, total));
        let x = c.to_x(tau);
        p.circle_filled(Pos2::new(x, y), 4.0, Color32::from_rgb(0xff, 0x66, 0x9d));
        p.text(Pos2::new(x + 8.0, y - 10.0), Align2::LEFT_CENTER, nice(keys::eval(&nk, tau, total)), font(12.0), Color32::from_rgb(0xff, 0xa8, 0xc6));
    }

    // the keys
    let vpp = ((hi - lo) / plot.height()) as f64;
    for (i, k) in ekeys.iter().enumerate() {
        let t = k.at(total);
        let pos = Pos2::new(c.to_x(t), to_y(k.v as f32));
        if pos.x < plot.left() - 10.0 || pos.x > plot.right() + 10.0 {
            continue;
        }
        let r = ui.interact(Rect::from_center_size(pos, Vec2::splat(22.0)), ui.scope_id().with(("mkey", prop, i)), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::Grab);
        key_drag(ui, c, &r, prop, i, Some(vpp));
        let selected = c.st.motion.sel.contains(&(prop, i));
        let active = r.hovered() || r.dragged();
        if active || selected {
            p.circle_filled(pos, 12.0, Color32::from_rgba_unmultiplied(0x8c, 0x66, 0xff, if selected { 64 } else { 40 }));
        }
        p.circle_filled(pos, 6.5, if selected { Color32::WHITE } else { CONTENT1 });
        p.circle_stroke(pos, 6.5, Stroke::new(2.2, col));
        if active {
            tooltip(&p, plot, pos, &format!("{} ms, {}", t.round(), nice(k.v as f32)));
        }
    }
}

fn tooltip(p: &egui::Painter, bounds: Rect, pos: Pos2, text: &str) {
    let g = p.layout_no_wrap(text.to_string(), font(12.0), CONTENT1);
    let size = g.size() + Vec2::new(14.0, 8.0);
    let mut rect = Rect::from_min_size(Pos2::new(pos.x - size.x * 0.5, pos.y - 18.0 - size.y), size);
    if rect.left() < bounds.left() {
        rect = rect.translate(Vec2::new(bounds.left() - rect.left(), 0.0));
    }
    if rect.right() > bounds.right() {
        rect = rect.translate(Vec2::new(bounds.right() - rect.right(), 0.0));
    }
    if rect.top() < bounds.top() {
        rect = rect.translate(Vec2::new(0.0, pos.y + 18.0 - rect.top()));
    }
    p.rect_filled(rect, CornerRadius::same(5), BACKGROUND6);
    p.rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, LIGHT4), StrokeKind::Inside);
    p.galley(rect.min + Vec2::new(7.0, 4.0), g, CONTENT1);
}

// -------------------------------------------------------------------------------------------------------- dope sheet

fn dope(ui: &mut Ui, c: &mut Ctx, bg: &egui::Response, tau: f32) {
    let p = ui.painter().clone();
    let plot = c.plot;
    let total = c.total;
    let rows = PROPS.len();
    let row_h = plot.height() / rows as f32;
    let all: Vec<Vec<EKey>> = PROPS.iter().map(|(pid, _)| read(&c.st.settings, pid)).collect();
    let clipped = p.with_clip_rect(plot);

    // row stripes and labels
    for (i, (_, label)) in PROPS.iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(plot.left(), plot.top() + i as f32 * row_h), Vec2::new(plot.width(), row_h));
        if c.st.motion.prop == i {
            clipped.rect_filled(r, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(0x8c, 0x66, 0xff, 18));
        } else if i % 2 == 1 {
            clipped.rect_filled(r, CornerRadius::ZERO, Color32::from_white_alpha(4));
        }
        p.text(Pos2::new(plot.left() - 8.0, r.center().y), Align2::RIGHT_CENTER, label.split(' ').next().unwrap_or(""), font(11.0), LIGHT3);
    }

    // bars between keys: brighter where the value changes
    for (i, keys) in all.iter().enumerate() {
        let y = plot.top() + (i as f32 + 0.5) * row_h;
        for w in keys.windows(2) {
            let (xa, xb) = (c.to_x(w[0].at(total)), c.to_x(w[1].at(total)));
            let changes = (w[0].v - w[1].v).abs() > 1e-6;
            let r = Rect::from_min_max(Pos2::new(xa, y - 3.0), Pos2::new(xb, y + 3.0));
            clipped.rect_filled(r, CornerRadius::same(3), Color32::from_rgba_unmultiplied(PROP_COLORS[i].r(), PROP_COLORS[i].g(), PROP_COLORS[i].b(), if changes { 90 } else { 34 }));
        }
    }

    let (t0, t1) = (c.t0, c.t1);
    background(ui, c, bg, |r| {
        let mut found = vec![];
        for (i, keys) in all.iter().enumerate() {
            let y = plot.top() + (i as f32 + 0.5) * row_h;
            for (j, k) in keys.iter().enumerate() {
                if r.contains(Pos2::new(plot.left() + (k.at(total) - t0) / (t1 - t0) * plot.width(), y)) {
                    found.push((i, j));
                }
            }
        }
        found
    });
    // click on a row (not on a key) to choose that property
    if bg.clicked() {
        if let Some(m) = bg.interact_pointer_pos() {
            let i = (((m.y - plot.top()) / row_h) as usize).min(rows - 1);
            select_prop(c.st, i);
        }
    }

    for (i, keys) in all.iter().enumerate() {
        let y = plot.top() + (i as f32 + 0.5) * row_h;
        for (j, k) in keys.iter().enumerate() {
            let t = k.at(total);
            let pos = Pos2::new(c.to_x(t), y);
            if pos.x < plot.left() - 10.0 || pos.x > plot.right() + 10.0 {
                continue;
            }
            let r = ui.interact(Rect::from_center_size(pos, Vec2::new(20.0, row_h.min(26.0))), ui.scope_id().with(("mdkey", i, j)), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::Grab);
            key_drag(ui, c, &r, i, j, None);
            let selected = c.st.motion.sel.contains(&(i, j));
            let active = r.hovered() || r.dragged();
            let rad = if active { 7.5 } else { 6.5 };
            let diamond = vec![Pos2::new(pos.x, pos.y - rad), Pos2::new(pos.x + rad, pos.y), Pos2::new(pos.x, pos.y + rad), Pos2::new(pos.x - rad, pos.y)];
            p.add(Shape::convex_polygon(diamond, if selected { Color32::WHITE } else { PROP_COLORS[i] }, Stroke::new(1.6, if selected { PROP_COLORS[i] } else { Color32::from_black_alpha(120) })));
            if active {
                tooltip(&p, plot, pos, &format!("{}: {} ms, {}", PROPS[i].1, t.round(), nice(k.v as f32)));
            }
        }
    }
    let _ = tau;
}

// ------------------------------------------------------------------------------------------------------------ tools

/// Whole-property operations for the settings page.
pub fn reverse(st: &mut Studio) {
    let pid = PROPS[st.motion.prop].0;
    let total = Settings::from_json(&st.settings).design_length();
    st.motion.remember(&st.settings);
    let keys = read(&st.settings, pid);
    let mut out: Vec<EKey> = Vec::new();
    for (i, k) in keys.iter().enumerate().rev() {
        // the easing of a step is stored on the key it starts from, so it moves to the key now in front of it
        let e = if i > 0 { reverse_ease(&keys[i - 1].e) } else { "linear".to_string() };
        out.push(EKey { t: Some(if k.t.is_none() { 0.0 } else { (total as f64 - k.t.unwrap_or(0.0)).max(0.0) }), v: k.v, e });
    }
    if let Some(l) = out.last_mut() {
        l.e = "linear".into();
    }
    sort(&mut out);
    write(&mut st.settings, pid, &out);
    st.motion.sel.clear();
    commit(st);
}

/// An easing played backwards in time is its mirror; the names only cover the ones that have a counterpart.
fn reverse_ease(e: &str) -> String {
    match e {
        "ease-in" => "ease-out",
        "ease-out" => "ease-in",
        "ease-in cubic" => "ease-out cubic",
        "ease-out cubic" => "ease-in cubic",
        other => other,
    }
    .to_string()
}

pub fn flip(st: &mut Studio) {
    let pid = PROPS[st.motion.prop].0;
    st.motion.remember(&st.settings);
    let mut keys = read(&st.settings, pid);
    let lo = keys.iter().map(|k| k.v).fold(f64::MAX, f64::min);
    let hi = keys.iter().map(|k| k.v).fold(f64::MIN, f64::max);
    for k in &mut keys {
        k.v = lo + hi - k.v;
    }
    write(&mut st.settings, pid, &keys);
    commit(st);
}

/// Give every step of the selected property the same easing.
pub fn ease_all(st: &mut Studio, name: &str) {
    let pid = PROPS[st.motion.prop].0;
    st.motion.remember(&st.settings);
    let mut keys = read(&st.settings, pid);
    for k in &mut keys {
        k.e = name.to_string();
    }
    write(&mut st.settings, pid, &keys);
    commit(st);
}

/// Back to what a fresh animation has for this property.
pub fn reset(st: &mut Studio) {
    let pid = PROPS[st.motion.prop].0;
    st.motion.remember(&st.settings);
    st.settings["props"][pid] = engine::presets::base_settings()["props"][pid].clone();
    st.motion.sel.clear();
    commit(st);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn studio() -> Studio {
        let mut st = Studio::new(egui::Context::default());
        st.settings = engine::presets::default_settings();
        st.settings["props"]["x"] = json!([[0, 0.0, "linear"], [100, 1.0, "linear"], [200, 0.5, "linear"], ["end", 0.0, "linear"]]);
        st.motion.prop = 0;
        st
    }
    fn xs(st: &Studio) -> Vec<(Option<f64>, f64)> {
        read(&st.settings, "x").iter().map(|k| (k.t, k.v)).collect()
    }

    #[test]
    fn nudge_moves_selected_keys_within_their_neighbours() {
        let mut st = studio();
        let total = Settings::from_json(&st.settings).design_length();
        st.motion.sel = vec![(0, 1), (0, 2)];
        nudge(&mut st, 30.0, 0.25, total);
        let k = xs(&st);
        assert_eq!(k[1], (Some(130.0), 1.25));
        assert_eq!(k[2], (Some(230.0), 0.75));
        // a big nudge stops short of the end key and keeps the order
        nudge(&mut st, 10_000.0, 0.0, total);
        let k = xs(&st);
        assert!(k[2].0.unwrap() < total as f64 && k[1].0 < k[2].0, "{k:?}");
        // the first key at zero and the end key do not move in time
        st.motion.sel = vec![(0, 0), (0, 3)];
        nudge(&mut st, 50.0, 0.0, total);
        let k = xs(&st);
        assert_eq!(k[0].0, Some(0.0));
        assert_eq!(k[3].0, None);
    }

    #[test]
    fn delete_keeps_at_least_one_key_and_undo_brings_them_back() {
        let mut st = studio();
        st.motion.sel = vec![(0, 1), (0, 2)];
        delete_selected(&mut st);
        assert_eq!(xs(&st).len(), 2);
        st.motion.sel = (0..2).map(|i| (0, i)).collect();
        delete_selected(&mut st);
        assert_eq!(xs(&st).len(), 1);
        undo(&mut st);
        assert_eq!(xs(&st).len(), 2);
        undo(&mut st);
        assert_eq!(xs(&st).len(), 4);
        redo(&mut st);
        assert_eq!(xs(&st).len(), 2);
    }

    #[test]
    fn reverse_flip_and_ease_all() {
        let mut st = studio();
        let total = Settings::from_json(&st.settings).design_length() as f64;
        let before = xs(&st);
        reverse(&mut st);
        let after = xs(&st);
        assert_eq!(after.len(), before.len());
        // the first value is now the last and the times are mirrored
        assert_eq!(after.first().map(|k| k.1), before.last().map(|k| k.1));
        assert_eq!(after.last().map(|k| k.1), before.first().map(|k| k.1));
        assert!(after.windows(2).all(|w| w[0].0.unwrap_or(f64::MAX) <= w[1].0.unwrap_or(f64::MAX)));
        assert!(after.iter().all(|k| k.0.is_none_or(|t| t >= 0.0 && t <= total)));
        flip(&mut st);
        let flipped = xs(&st);
        let (lo, hi) = (after.iter().map(|k| k.1).fold(f64::MAX, f64::min), after.iter().map(|k| k.1).fold(f64::MIN, f64::max));
        assert!(flipped.iter().zip(&after).all(|(f, a)| (f.1 - (lo + hi - a.1)).abs() < 1e-6));
        ease_all(&mut st, "smoother");
        assert!(read(&st.settings, "x").iter().all(|k| k.e == "smoother"));
        reset(&mut st);
        assert_eq!(read(&st.settings, "x"), read(&engine::presets::base_settings(), "x"));
    }

    #[test]
    fn a_key_goes_in_at_the_playhead_with_the_curve_value() {
        let mut st = studio();
        let total = Settings::from_json(&st.settings).design_length();
        st.fps = 100.0;
        seek_tau(&mut st, 150.0);
        let at = playhead(&st).unwrap();
        let want = val_at(&st, "x", at.round()) as f64;
        add_key_at_playhead(&mut st, total);
        let k = xs(&st);
        assert!((at - 150.0).abs() < 5.0, "{at}");
        assert!(k.iter().any(|k| k.0 == Some(at.round() as f64) && (k.1 - want).abs() < 2e-3), "{k:?} at {at}");
        assert_eq!(st.motion.sel.len(), 1);
    }

    /// Run one frame of the editor with the given events and the pointer over it.
    fn frame(ctx: &egui::Context, st: &mut Studio, events: Vec<egui::Event>) {
        let mut raw = egui::RawInput::default();
        raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 500.0)));
        let mut all = vec![];
        if let Some(egui::Event::Key { modifiers, .. }) = events.first() {
            all.push(egui::Event::ModifiersChanged(*modifiers));
        }
        all.extend(events);
        raw.events = all;
        let _ = ctx.run_ui(raw, |ui| {
            let r = ui.max_rect();
            editor(ui, st, r);
        });
    }
    fn key(k: Key, mods: egui::Modifiers) -> Vec<egui::Event> {
        vec![egui::Event::Key { key: k, physical_key: Some(k), pressed: true, repeat: false, modifiers: mods }]
    }

    #[test]
    fn keyboard_and_ruler_drive_the_editor() {
        let ctx = egui::Context::default();
        let mut st = studio();
        st.fps = 100.0;
        st.anim_playing = false;
        st.anim_pos = 370.0;
        let hover = vec![egui::Event::PointerMoved(Pos2::new(900.0, 300.0))];
        frame(&ctx, &mut st, hover.clone());
        frame(&ctx, &mut st, hover.clone());
        // K adds a key at the playhead
        frame(&ctx, &mut st, key(Key::K, egui::Modifiers::NONE));
        assert_eq!(xs(&st).len(), 5, "{:?}", xs(&st));
        // Delete removes the selection (the new key)
        frame(&ctx, &mut st, key(Key::Delete, egui::Modifiers::NONE));
        assert_eq!(xs(&st).len(), 4);
        // Ctrl+Z brings it back, Ctrl+Y removes it again
        frame(&ctx, &mut st, key(Key::Z, egui::Modifiers::COMMAND));
        assert_eq!(xs(&st).len(), 5);
        frame(&ctx, &mut st, key(Key::Y, egui::Modifiers::COMMAND));
        assert_eq!(xs(&st).len(), 4);
        // Ctrl+A selects all, arrows move them (the first key at zero and the end key stay)
        frame(&ctx, &mut st, key(Key::A, egui::Modifiers::COMMAND));
        assert_eq!(st.motion.sel.len(), 4);
        // Space toggles the animation card
        let playing = st.anim_playing;
        frame(&ctx, &mut st, key(Key::Space, egui::Modifiers::NONE));
        assert_ne!(st.anim_playing, playing);
        // clicking the ruler scrubs: the ruler is the strip at the top of the plot area
        let at = Pos2::new(700.0, 40.0 + 14.0);
        let before = st.anim_pos;
        frame(&ctx, &mut st, vec![egui::Event::PointerMoved(at)]);
        frame(&ctx, &mut st, vec![egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }]);
        frame(&ctx, &mut st, vec![egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }]);
        assert_ne!(st.anim_pos, before, "the ruler click should move the playhead");
    }

    #[test]
    fn box_select_then_drag_moves_every_selected_key() {
        let ctx = egui::Context::default();
        let mut st = studio();
        st.fps = 100.0;
        st.anim_playing = false;
        let btn = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        let (a, b) = (Pos2::new(258.0, 74.0), Pos2::new(1392.0, 486.0));
        frame(&ctx, &mut st, vec![egui::Event::PointerMoved(a)]);
        frame(&ctx, &mut st, vec![btn(a, true)]);
        frame(&ctx, &mut st, vec![egui::Event::PointerMoved(Pos2::new(800.0, 300.0))]);
        frame(&ctx, &mut st, vec![egui::Event::PointerMoved(b)]);
        frame(&ctx, &mut st, vec![btn(b, false)]);
        frame(&ctx, &mut st, vec![]);
        assert_eq!(st.motion.sel.len(), 4, "box select: {:?}", st.motion.sel);
    }

    #[test]
    fn copy_and_paste_put_keys_at_the_playhead() {
        let ctx = egui::Context::default();
        let mut st = studio();
        st.fps = 100.0;
        st.anim_playing = false;
        let hover = vec![egui::Event::PointerMoved(Pos2::new(900.0, 300.0))];
        frame(&ctx, &mut st, hover.clone());
        st.motion.sel = vec![(0, 1)];
        frame(&ctx, &mut st, key(Key::C, egui::Modifiers::COMMAND));
        seek_tau(&mut st, 300.0);
        frame(&ctx, &mut st, key(Key::V, egui::Modifiers::COMMAND));
        let k = xs(&st);
        let at = playhead(&st).unwrap().round() as f64;
        assert!(k.iter().any(|k| k.0 == Some(at) && (k.1 - 1.0).abs() < 1e-9), "{k:?} at {at}");
        assert_eq!(k.len(), 5);
    }

    /// A mouse drag of the key at (`from`) to (`to`) over several frames.
    fn drag(ctx: &egui::Context, st: &mut Studio, from: Pos2, to: Pos2, mods: egui::Modifiers) {
        let btn = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: mods };
        let mut first = vec![egui::Event::PointerMoved(from)];
        if mods != egui::Modifiers::NONE {
            first.insert(0, egui::Event::ModifiersChanged(mods));
        }
        frame(ctx, st, first);
        frame(ctx, st, vec![btn(from, true)]);
        for i in 1..=6 {
            let q = Pos2::new(from.x + (to.x - from.x) * i as f32 / 6.0, from.y + (to.y - from.y) * i as f32 / 6.0);
            frame(ctx, st, vec![egui::Event::PointerMoved(q)]);
        }
        frame(ctx, st, vec![btn(to, false)]);
        frame(ctx, st, vec![]);
    }

    /// Where the editor puts key `t` ms with value `v` in a 1400x500 frame (see `editor`'s layout).
    fn at(st: &Studio, t: f32, v: f32) -> Pos2 {
        let parsed = Settings::from_json(&st.settings);
        let total = parsed.design_length().max(1.0);
        let (lo, hi) = value_range(st, "x", total);
        let (left, right, top, bottom) = (266.0, 1382.0, 71.0, 488.0);
        Pos2::new(left + t / total * (right - left), bottom - (v - lo) / (hi - lo) * (bottom - top))
    }

    #[test]
    fn dragging_a_selected_key_moves_the_whole_selection() {
        let ctx = egui::Context::default();
        let mut st = studio();
        st.fps = 100.0;
        st.anim_playing = false;
        frame(&ctx, &mut st, vec![]);
        st.motion.sel = vec![(0, 1), (0, 2)];
        st.motion.snap_ms = 0.0;
        let start = at(&st, 100.0, 1.0);
        let (px_per_ms, _) = (at(&st, 1.0, 0.0).x - at(&st, 0.0, 0.0).x, 0);
        drag(&ctx, &mut st, start, Pos2::new(start.x - 40.0 * px_per_ms, start.y + 30.0), egui::Modifiers::NONE);
        let k = xs(&st);
        assert_eq!(k[1].0, Some(60.0), "{k:?}");
        assert_eq!(k[2].0, Some(160.0), "{k:?}");
        // both moved by the same value too
        assert!(k[1].1 < 1.0 && ((1.0 - k[1].1) - (0.5 - k[2].1)).abs() < 1e-6, "{k:?}");
    }

    #[test]
    fn shift_locks_a_drag_to_one_axis_and_snapping_rounds_times() {
        let ctx = egui::Context::default();
        let mut st = studio();
        st.fps = 100.0;
        st.anim_playing = false;
        frame(&ctx, &mut st, vec![]);
        st.motion.snap_ms = 0.0;
        // mostly sideways with shift: the value must not change
        let start = at(&st, 100.0, 1.0);
        let px = at(&st, 1.0, 0.0).x - at(&st, 0.0, 0.0).x;
        drag(&ctx, &mut st, start, Pos2::new(start.x + 20.0 * px, start.y + 8.0), egui::Modifiers::SHIFT);
        let k = xs(&st);
        assert_eq!(k[1].1, 1.0, "{k:?}");
        assert_eq!(k[1].0, Some(120.0), "{k:?}");
        // with snapping on, a drag to 133 ms lands on a round time
        st.motion.snap_ms = 10.0;
        st.motion.sel.clear();
        let start = at(&st, 120.0, 1.0);
        drag(&ctx, &mut st, start, Pos2::new(start.x + 13.0 * px, start.y), egui::Modifiers::NONE);
        let t = xs(&st)[1].0.unwrap();
        assert!(t != 133.0 && (t % 5.0 == 0.0 || t % 10.0 == 0.0 || t % 2.0 == 0.0), "snapped to {t}");
    }

    #[test]
    fn the_ruler_snaps_the_playhead_to_the_chosen_resolution() {
        let ctx = egui::Context::default();
        let mut st = studio();
        st.fps = 240.0;
        st.anim_playing = false;
        frame(&ctx, &mut st, vec![]);
        let total = Settings::from_json(&st.settings).design_length();
        // x of 137 ms on the ruler (the plot runs from 266 to 1382 over the whole animation)
        let x = 266.0 + 137.0 / total * (1382.0 - 266.0);
        let at = Pos2::new(x, 40.0 + 14.0);
        for (res, want) in [(25.0, 125.0), (50.0, 150.0), (100.0, 100.0), (5.0, 135.0)] {
            st.motion.snap_ms = res;
            let btn = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
            frame(&ctx, &mut st, vec![egui::Event::PointerMoved(at)]);
            frame(&ctx, &mut st, vec![btn(true)]);
            frame(&ctx, &mut st, vec![btn(false)]);
            frame(&ctx, &mut st, vec![]);
            let h = playhead(&st).unwrap();
            assert_eq!(h, want, "snap {res}: playhead at {h}, wanted {want}");
        }
        // snapping off: the click lands where it was made
        st.motion.snap_ms = 0.0;
        let btn = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(&ctx, &mut st, vec![egui::Event::PointerMoved(at)]);
        frame(&ctx, &mut st, vec![btn(true)]);
        frame(&ctx, &mut st, vec![btn(false)]);
        assert!((playhead(&st).unwrap() - 137.0).abs() < 4.0);
    }

    #[test]
    fn the_dock_draws_open_and_folded() {
        let ctx = egui::Context::default();
        let mut st = studio();
        st.frames = vec![None; 10];
        let run = |ctx: &egui::Context, st: &mut Studio| {
            let mut raw = egui::RawInput::default();
            raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 800.0)));
            let _ = ctx.run_ui(raw, |ui| dock(ui, st));
        };
        assert!(st.motion.open, "the dock starts open");
        run(&ctx, &mut st);
        st.motion.open = false;
        run(&ctx, &mut st);
        st.motion.open = true;
        run(&ctx, &mut st);
    }

    #[test]
    fn dragging_the_top_edge_resizes_the_dock_and_it_stays() {
        let ctx = egui::Context::default();
        let mut st = studio();
        let run = |ctx: &egui::Context, st: &mut Studio, events: Vec<egui::Event>| {
            let mut raw = egui::RawInput::default();
            raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 900.0)));
            raw.events = events;
            let _ = ctx.run_ui(raw, |ui| dock(ui, st));
        };
        assert_eq!(st.motion.height, 360.0);
        let btn = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        let top = 900.0 - 360.0;
        let a = Pos2::new(700.0, top + 3.0);
        run(&ctx, &mut st, vec![egui::Event::PointerMoved(a)]);
        run(&ctx, &mut st, vec![btn(a, true)]);
        for i in 1..=5 {
            run(&ctx, &mut st, vec![egui::Event::PointerMoved(Pos2::new(700.0, a.y - 10.0 * i as f32))]);
        }
        run(&ctx, &mut st, vec![btn(Pos2::new(700.0, a.y - 50.0), false)]);
        run(&ctx, &mut st, vec![]);
        assert!((st.motion.height - 410.0).abs() < 1.0, "height {}", st.motion.height);
        // and frames later it is still that tall
        for _ in 0..3 {
            run(&ctx, &mut st, vec![]);
        }
        assert!((st.motion.height - 410.0).abs() < 1.0);
        // never smaller than usable, never taller than the window leaves room for
        st.motion.height = 5000.0;
        run(&ctx, &mut st, vec![]);
        let top2 = 900.0 - 660.0;
        let b = Pos2::new(700.0, top2 + 3.0);
        run(&ctx, &mut st, vec![egui::Event::PointerMoved(b)]);
        run(&ctx, &mut st, vec![btn(b, true)]);
        run(&ctx, &mut st, vec![egui::Event::PointerMoved(Pos2::new(700.0, b.y + 20.0))]);
        run(&ctx, &mut st, vec![egui::Event::PointerMoved(Pos2::new(700.0, b.y + 400.0))]);
        run(&ctx, &mut st, vec![btn(Pos2::new(700.0, b.y + 400.0), false)]);
        assert!(st.motion.height >= 300.0 && st.motion.height <= 660.0, "{}", st.motion.height);
    }

    #[test]
    fn the_key_strip_edits_the_selected_key() {
        let ctx = egui::Context::default();
        let mut st = studio();
        st.motion.sel = vec![(0, 1)];
        frame(&ctx, &mut st, vec![]);
        // the strip filled its boxes from the key (100 ms, 1)
        assert_eq!(st.bufs.get("mkey:0:1:0").map(String::as_str), Some("100"));
        assert_eq!(st.bufs.get("mkey:0:1:1").map(String::as_str), Some("1"));
        // typing into the boxes goes through the same edit the old rows used
        crate::state::edit_key(&mut st.settings, "x", 1, 0, "120");
        crate::state::edit_key(&mut st.settings, "x", 1, 1, "0.8");
        crate::state::edit_key(&mut st.settings, "x", 1, 2, "bounce");
        let k = read(&st.settings, "x");
        assert_eq!((k[1].t, k[1].v, k[1].e.as_str()), (Some(120.0), 0.8, "bounce"));
    }

    #[test]
    fn any_window_size_draws_without_running_away() {
        let ctx = egui::Context::default();
        let mut st = studio();
        st.frames = vec![None; 50];
        st.fps = 100.0;
        for (w, h) in [(120.0, 90.0), (420.0, 300.0), (900.0, 500.0), (1000.0, 600.0), (1280.0, 720.0), (1600.0, 940.0), (2560.0, 1440.0), (3840.0, 2160.0)] {
            for open in [true, false] {
                st.motion.open = open;
                for card in [1.0, 3.0, 12.0] {
                    st.anim_zoom = card;
                    let mut raw = egui::RawInput::default();
                    raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h)));
                    let out = ctx.run_ui(raw, |ui| dock(ui, &mut st));
                    assert!(out.shapes.len() < 20_000, "{w}x{h} open {open} zoom {card}: {} shapes", out.shapes.len());
                }
            }
        }
        st.motion.open = true;
    }

    #[test]
    fn the_time_field_moves_the_playhead_to_what_is_typed() {
        let ctx = egui::Context::default();
        let mut st = studio();
        st.fps = 240.0;
        st.anim_playing = true;
        let at = Pos2::new(1290.0, 20.0);
        let btn = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(&ctx, &mut st, vec![egui::Event::PointerMoved(at)]);
        frame(&ctx, &mut st, vec![btn(true)]);
        frame(&ctx, &mut st, vec![btn(false)]);
        // clear what is there, then type 250
        frame(&ctx, &mut st, vec![egui::Event::Key { key: Key::A, physical_key: Some(Key::A), pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND }]);
        frame(&ctx, &mut st, vec![egui::Event::Text("250".into())]);
        frame(&ctx, &mut st, vec![]);
        let h = playhead(&st).unwrap();
        assert_eq!(h, 250.0, "playhead {h}, field {:?}", st.bufs.get("motion-time"));
        assert!(!st.anim_playing, "typing a time pauses the preview");
    }

    #[test]
    fn a_big_preview_never_squeezes_the_graph() {
        let ctx = egui::Context::default();
        let mut st = studio();
        st.frames = vec![None; 50];
        st.fps = 100.0;
        for (w, h) in [(1280.0, 720.0), (1600.0, 940.0), (2560.0, 1440.0)] {
            st.anim_zoom = 12.0;
            let mut raw = egui::RawInput::default();
            raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h)));
            let _ = ctx.run_ui(raw, |ui| dock(ui, &mut st));
            // the list (214) and the graph (at least 300) keep their room; the preview gets at most 45% and fits in the dock's height
            assert!(st.anim_limit.0 <= w * 0.45 + 0.5, "{w}: {:?}", st.anim_limit);
            assert!(st.anim_limit.0 <= w - 214.0 - 300.0 - 24.0 + 0.5 || st.anim_limit.0 < crate::ui::card_min_width(&st), "{w}: {:?}", st.anim_limit);
            assert!(st.anim_limit.1 <= st.motion.height, "{h}: {:?}", st.anim_limit);
            let size = crate::ui::anim_card_size(&ctx, &st);
            assert!((size.x <= st.anim_limit.0 + 16.0 + 0.5 || size.x <= crate::ui::card_min_width(&st) + 0.5) && size.y <= st.anim_limit.1 + 100.0, "card {size:?} limit {:?}", st.anim_limit);
        }
    }

    #[test]
    fn baking_the_shift_keeps_the_animation_and_zeroes_the_shift() {
        for shift in [-100.0f64, 80.0, -250.0, 37.0] {
            let mut st = Studio::new(egui::Context::default());
            st.settings = engine::presets::default_settings();
            st.settings["color_on"] = json!(true);
            st.settings["color_keys"] = json!([[0, "#ffffff", "linear"], [200, "#ff8487", "smoother"], ["end", "#3060ff", "linear"]]);
            st.settings["shift"] = json!(shift);
            let before = Settings::from_json(&st.settings);
            let total = before.design_length();
            assert!(bake_shift(&mut st));
            let after = Settings::from_json(&st.settings);
            assert_eq!(after.shift, 0.0);
            assert!((after.design_length() - total).abs() < 0.01, "length {} -> {}", total, after.design_length());
            // the picture at every moment of the loop is the same: old curve at clamp(t + shift), new curve at t
            for step in 0..=220 {
                let t = step as f32 / 220.0 * total;
                let tau = (t + shift as f32).clamp(0.0, total);
                for pid in ["x", "y", "sx", "sy", "rot", "alpha", "blur"] {
                    let (a, b) = (keys::eval(before.prop(pid), tau, total), keys::eval(after.prop(pid), t, total));
                    assert!((a - b).abs() < 0.01 * (1.0 + a.abs()), "shift {shift} {pid} at {t}: {a} vs {b}");
                }
                let (ca, cb) = (keys::eval_color(&before.color_keys, tau, total), keys::eval_color(&after.color_keys, t, total));
                assert!((0..3).all(|k| (ca[k] - cb[k]).abs() < 1.5), "shift {shift} colour at {t}: {ca:?} vs {cb:?}");
            }
            // the frames themselves agree, not only the curves
            let fa = engine::gen::build_frames(&before, 1, &Default::default());
            let fb = engine::gen::build_frames(&after, 1, &Default::default());
            assert_eq!(fa.len(), fb.len());
            let (mut worst, mut sum, mut cnt) = (0.0f32, 0.0f64, 0usize);
            for (x, y) in fa.iter().zip(&fb) {
                match (x, y) {
                    (Some(x), Some(y)) => {
                        for (p, q) in x.px.iter().zip(&y.px) {
                            let d = (0..4).map(|c| (p[c] - q[c]).abs()).fold(0.0, f32::max);
                            worst = worst.max(d);
                            sum += d as f64;
                            cnt += 1;
                        }
                    }
                    (None, None) => {}
                    (Some(x), None) | (None, Some(x)) => worst = worst.max(x.px.iter().map(|p| p[3]).fold(0.0, f32::max)),
                }
            }
            // a picture is a few rounded sizes and soft edges: a rounded size can flip by a pixel in one frame, so the worst pixel may move by a few percent, the average by nothing
            let mean = sum / cnt.max(1) as f64;
            assert!(worst < 0.15 && mean < 0.001, "shift {shift}: frames differ by {worst} (mean {mean})");
            // undo gives the shift and the old keys back
            undo(&mut st);
            assert_eq!(Settings::from_json(&st.settings).shift as f64, shift);
            assert_eq!(Settings::from_json(&st.settings).prop("blur").len(), before.prop("blur").len());
        }
    }

    #[test]
    fn baking_leaves_few_points_to_work_with() {
        let mut st = Studio::new(egui::Context::default());
        st.settings = engine::presets::default_settings();
        st.settings["shift"] = json!(-100.0);
        assert!(bake_shift(&mut st));
        let count = |pid: &str| read(&st.settings, pid).len();
        eprintln!("points after baking: {:?}", ["x", "y", "sx", "sy", "rot", "alpha", "blur"].map(|p| (p, count(p))));
        // a straight step cut by the window stays a straight step: the position has its ends and the hold, nothing more
        assert_eq!(count("x"), 3, "{:?}", read(&st.settings, "x"));
        // the eased ones keep a handful of points, not a point every 5 ms
        for pid in ["y", "rot", "sx", "sy", "alpha", "blur"] {
            assert!(count(pid) <= 14, "{pid} has {} points", count(pid));
        }
    }

    #[test]
    fn baking_does_nothing_without_a_shift() {
        let mut st = Studio::new(egui::Context::default());
        st.settings = engine::presets::default_settings();
        st.settings["shift"] = json!(0.0);
        let keep = st.settings["props"].clone();
        assert!(!bake_shift(&mut st));
        assert_eq!(st.settings["props"], keep);
    }

    #[test]
    fn steps_are_round() {
        assert_eq!(nice_step(7.0), 10.0);
        assert_eq!(nice_step(0.3), 0.5);
        assert_eq!(nice_step(21.0), 25.0);
    }
}

// ------------------------------------------------------------------------------------------------- baking the time shift
/// One point of a rebuilt curve: where it goes (`t`), where on the old curve its value is read (`tau`), and the original key whose
/// easing it keeps (`None` = a sampled point, joined to the next one in a straight line).
struct Pt {
    t: f32,
    tau: f32,
    ease_of: Option<usize>,
    /// A point that has to stay: the ends, the original keys and where the window cuts. The others only follow a curve and are thinned.
    keep: bool,
}

/// The points that make the animation shown with a time `shift` (ms) the animation of a curve with no shift.
///
/// The frames show the old curve at `clamp(t + shift, 0, total)` for t from 0 to `total`. A step of the old curve that lies fully inside
/// that window is kept as it is (moved by `-shift`, with its easing). A step the window cuts through is read off every 5 ms and joined
/// in straight lines, which follows any easing to well under a thousandth. Before the window (shift < 0) and after it (shift > 0) the
/// curve holds its first or last value.
fn rebase(key_times: &[f32], shift: f32, total: f32) -> Vec<Pt> {
    let a = shift.clamp(0.0, total);
    let b = (total + shift).clamp(0.0, total);
    let find = |tau: f32| key_times.iter().position(|t| (t - tau).abs() < 1e-3);
    let mut bps: Vec<(f32, Option<usize>)> = vec![(a, find(a))];
    for (i, t) in key_times.iter().enumerate() {
        if *t > a + 1e-3 && *t < b - 1e-3 {
            bps.push((*t, Some(i)));
        }
    }
    bps.push((b, find(b)));
    let mut out: Vec<Pt> = vec![];
    if shift < 0.0 {
        out.push(Pt { t: 0.0, tau: 0.0, ease_of: None, keep: true });
    }
    for w in bps.windows(2) {
        let (p, q) = (w[0], w[1]);
        let exact = matches!((p.1, q.1), (Some(i), Some(j)) if j == i + 1);
        out.push(Pt { t: p.0 - shift, tau: p.0, ease_of: if exact { p.1 } else { None }, keep: true });
        if !exact {
            let mut tau = p.0 + 5.0;
            while tau < q.0 - 2.5 {
                out.push(Pt { t: tau - shift, tau, ease_of: None, keep: false });
                tau += 5.0;
            }
        }
    }
    let last = bps[bps.len() - 1];
    out.push(Pt { t: last.0 - shift, tau: last.0, ease_of: None, keep: true });
    if shift > 0.0 {
        out.push(Pt { t: total, tau: total, ease_of: None, keep: true });
    }
    out.dedup_by(|y, x| (y.t - x.t).abs() < 1e-3);
    out
}

/// Drop the points that only follow a curve and are not needed to follow it: between two points that stay, the point that is furthest from
/// the straight line joining them stays if it is further than `tolerance` (of the range of the values), and so on in each half. A cut
/// through a linear step leaves nothing between its ends; a cut through an eased step keeps a handful of points.
fn thin(pts: Vec<Pt>, value: &dyn Fn(f32) -> Vec<f32>, tolerance: f32) -> Vec<Pt> {
    let vals: Vec<Vec<f32>> = pts.iter().map(|p| value(p.tau)).collect();
    let comps = vals.first().map(|v| v.len()).unwrap_or(0);
    let scale: Vec<f32> = (0..comps)
        .map(|c| {
            let (lo, hi) = vals.iter().fold((f32::MAX, f32::MIN), |a, v| (a.0.min(v[c]), a.1.max(v[c])));
            ((hi - lo).abs()).max(1e-3)
        })
        .collect();
    let mut keep: Vec<bool> = pts.iter().map(|p| p.keep).collect();
    let n = pts.len();
    // the runs of points that may go, between two that stay
    let mut stack: Vec<(usize, usize)> = vec![];
    let mut last = 0usize;
    for i in 1..n {
        if keep[i] {
            if i > last + 1 {
                stack.push((last, i));
            }
            last = i;
        }
    }
    while let Some((a, b)) = stack.pop() {
        let (ta, tb) = (pts[a].t, pts[b].t);
        let (mut worst, mut at) = (0.0f32, None);
        for i in a + 1..b {
            let u = if tb > ta { (pts[i].t - ta) / (tb - ta) } else { 0.0 };
            let dev = (0..comps).map(|c| ((vals[a][c] + (vals[b][c] - vals[a][c]) * u) - vals[i][c]).abs() / scale[c]).fold(0.0, f32::max);
            if dev > worst {
                worst = dev;
                at = Some(i);
            }
        }
        if let Some(i) = at.filter(|_| worst > tolerance) {
            keep[i] = true;
            stack.push((a, i));
            stack.push((i, b));
        }
    }
    pts.into_iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| p).collect()
}

/// Make the time shift the base: rebuild every curve so that, with a shift of 0, the animation is the one that is shown now.
/// The first point is at 0 and the last at the animation's length, so the animation keeps its length.
pub fn bake_shift(st: &mut Studio) -> bool {
    let s = Settings::from_json(&st.settings);
    let shift = s.shift;
    if s.is_seamless() || shift.abs() < 0.5 {
        return false;
    }
    let total = s.design_length();
    st.motion.remember(&st.settings);
    let time_of = |t: Option<f64>| t.map(|t| t as f32).unwrap_or(total);
    for (pid, _) in PROPS.iter() {
        let old = read(&st.settings, pid);
        let times: Vec<f32> = old.iter().map(|k| time_of(k.t)).collect();
        let nk = s.prop(pid);
        let pts = thin(rebase(&times, shift, total), &|tau| vec![keys::eval(nk, tau, total)], 0.006);
        let n = pts.len();
        let new: Vec<EKey> = pts
            .iter()
            .enumerate()
            .map(|(i, p)| EKey {
                // every curve ends at the animation's length as a number: that is what keeps the length itself
                t: Some(if i + 1 == n { total as f64 } else { (p.t as f64 * 1000.0).round() / 1000.0 }),
                v: keys::eval(nk, p.tau, total) as f64,
                e: p.ease_of.map(|j| old[j].e.clone()).unwrap_or_else(|| "linear".into()),
            })
            .collect();
        write(&mut st.settings, pid, &new);
    }
    // the colour keys run on the same clock
    if st.settings["color_keys"].as_array().is_some_and(|a| !a.is_empty()) {
        let old = st.settings["color_keys"].as_array().cloned().unwrap_or_default();
        let times: Vec<f32> = old.iter().map(|k| k[0].as_f64().map(|t| t as f32).unwrap_or(total)).collect();
        let pts = thin(rebase(&times, shift, total), &|tau| keys::eval_color(&s.color_keys, tau, total).to_vec(), 0.004);
        let n = pts.len();
        let new: Vec<Value> = pts
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let c = keys::eval_color(&s.color_keys, p.tau, total);
                let hex = format!("#{:02x}{:02x}{:02x}", c[0].round().clamp(0.0, 255.0) as u8, c[1].round().clamp(0.0, 255.0) as u8, c[2].round().clamp(0.0, 255.0) as u8);
                let t = if i + 1 == n { json!(total as f64) } else { json!((p.t as f64 * 1000.0).round() / 1000.0) };
                json!([t, hex, p.ease_of.map(|j| old[j][2].as_str().unwrap_or("linear").to_string()).unwrap_or_else(|| "linear".into())])
            })
            .collect();
        st.settings["color_keys"] = Value::Array(new);
    }
    st.settings["shift"] = json!(0.0);
    st.motion.sel.clear();
    st.status = format!("The {} ms shift is part of the keys now; the shift is 0", nice(shift));
    commit(st);
    true
}
