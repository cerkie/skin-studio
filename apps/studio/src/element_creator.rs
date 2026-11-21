//! Procedural osu! skin element creator UI & state (`Skin editor -> Create` tab).
//! Builds resolution-independent `@2x` elements with analytical SDF shapes, gradients, strokes, and glows,
//! exporting both `@2x` and zero-ringing area-averaged `1x` PNGs while keeping editable JSON in `.skin-studio-elements/`.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use engine::create::{BlendMode, ElementDoc, ElementKind, FillStyle, GlowStyle, Layer, Shape, StrokeAlign};
use engine::edit::{self, Resample};
use engine::img::Img;

use crate::theme::pal::*;
use crate::theme::{self as th, Kind};

pub struct CreatorState {
    pub doc: ElementDoc,
    pub dirty: bool,
    pub rendered: Option<Arc<Img>>,
    /// Monotonic counter incremented on every doc/template change so GPU texture cache never aliases freed heap addresses.
    pub render_gen: u64,
    pub add_open: bool,
    pub collapsed: HashSet<usize>,
    pub guides: bool,
    pub combo_tint: bool,
    pub combo_idx: usize,
    bufs: HashMap<String, String>,
    undo: Vec<ElementDoc>,
    redo: Vec<ElementDoc>,
    last_edit: Option<Instant>,
}

impl Default for CreatorState {
    fn default() -> Self {
        let doc = ElementDoc::template(ElementKind::HitCircle);
        CreatorState {
            doc,
            dirty: false,
            rendered: None,
            render_gen: 1,
            add_open: false,
            collapsed: HashSet::new(),
            guides: true,
            combo_tint: true,
            combo_idx: 0,
            bufs: HashMap::new(),
            undo: vec![],
            redo: vec![],
            last_edit: None,
        }
    }
}

impl CreatorState {
    pub fn ensure_rendered(&mut self) -> Arc<Img> {
        if let Some(img) = &self.rendered {
            return img.clone();
        }
        let img = Arc::new(self.doc.render());
        self.rendered = Some(img.clone());
        img
    }

    pub fn invalidate(&mut self) {
        self.rendered = None;
        self.render_gen = self.render_gen.wrapping_add(1);
    }

    pub fn select_kind(&mut self, kind: ElementKind, skin_dir: Option<&Path>) {
        let stem = kind.file_stem();
        let loaded = skin_dir
            .map(|d| d.join(".skin-studio-elements").join(format!("{stem}.json")))
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<ElementDoc>(&s).ok());
        self.doc = loaded.unwrap_or_else(|| ElementDoc::template(kind));
        self.dirty = false;
        self.collapsed.clear();
        self.bufs.clear();
        self.undo.clear();
        self.redo.clear();
        self.last_edit = None;
        self.invalidate();
    }

    pub fn push_undo(&mut self, before: ElementDoc, coalesce: bool) {
        let quick = coalesce && self.last_edit.is_some_and(|t| t.elapsed().as_millis() < 700);
        if !quick || self.undo.is_empty() {
            self.undo.push(before);
            if self.undo.len() > 200 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last_edit = if coalesce { Some(Instant::now()) } else { None };
    }

    pub fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            let cur = std::mem::replace(&mut self.doc, prev);
            self.redo.push(cur);
            self.bufs.clear();
            self.dirty = true;
            self.last_edit = None;
            self.invalidate();
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            let cur = std::mem::replace(&mut self.doc, next);
            self.undo.push(cur);
            self.bufs.clear();
            self.dirty = true;
            self.last_edit = None;
            self.invalidate();
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Render at even `@2x` dimensions and write both `<name>@2x.png` and zero-ringing area-averaged `<name>.png`,
    /// backing up any existing files into `.skin-studio-backup/` and storing editable JSON in `.skin-studio-elements/`.
    pub fn save_to_skin(&mut self, dir: &Path, output: edit::Output) -> Result<(PathBuf, PathBuf), String> {
        let raw_stem = self.doc.name.trim().trim_end_matches(".png").trim_end_matches("@2x").trim();
        let stem = if raw_stem.is_empty() { self.doc.kind.file_stem() } else { raw_stem };
        // Keep @2x dimensions even so 1x is an exact 2:1 area-average.
        self.doc.width = (self.doc.width.clamp(2, 2048) + 1) & !1;
        self.doc.height = (self.doc.height.clamp(2, 2048) + 1) & !1;
        let img_2x = self.doc.render();
        let img_1x = edit::resize(&img_2x, (img_2x.w / 2).max(1), (img_2x.h / 2).max(1), Resample::Smooth);

        let p2 = dir.join(format!("{stem}@2x.png"));
        let p1 = dir.join(format!("{stem}.png"));
        let backup = dir.join(".skin-studio-backup");
        for p in [&p2, &p1] {
            if p.is_file() {
                if let Some(fname) = p.file_name() {
                    let kept = backup.join(fname);
                    if !kept.exists() {
                        std::fs::create_dir_all(&backup).map_err(|e| e.to_string())?;
                        std::fs::copy(p, &kept).map_err(|e| e.to_string())?;
                    }
                }
            }
        }
        edit::save_png_with(&p2, &img_2x, output)?;
        edit::save_png_with(&p1, &img_1x, output)?;

        let src_dir = dir.join(".skin-studio-elements");
        let _ = std::fs::create_dir_all(&src_dir);
        if let Ok(json) = serde_json::to_string_pretty(&self.doc) {
            let _ = std::fs::write(src_dir.join(format!("{stem}.json")), json);
        }
        self.rendered = Some(Arc::new(img_2x));
        self.dirty = false;
        Ok((p2, p1))
    }
}

pub enum CreatorAction {
    None,
    SaveToSkin,
    SendToEdits,
}

fn c_slider(ui: &mut Ui, id: &str, label: &str, v: &mut f32, min: f32, max: f32, def: f32, show: impl Fn(f32) -> String) -> bool {
    let range = (max - min).max(1e-6);
    let step = if range >= 20.0 { 1.0 } else if range >= 5.0 { 0.5 } else if range >= 2.0 { 0.1 } else { 0.01 };
    let mut f = ((*v - min) / range).clamp(0.0, 1.0);
    let df = ((def - min) / range).clamp(0.0, 1.0);
    let (moved, typed) = th::slider_row_def(ui, id, label, &show(*v), &mut f, step / range, Some(df));
    let mut ch = false;
    if moved {
        let x = (((min + f * range) / step).round() * step * 1e5).round() / 1e5;
        let x = x.clamp(min, max);
        if (x - *v).abs() > 1e-6 {
            *v = x;
            ch = true;
        }
    }
    if let Some(txt) = typed {
        let cleaned: String = txt.trim().chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | ',')).collect();
        if let Ok(x) = cleaned.replace(',', ".").parse::<f32>() {
            let x = x.clamp(min, max);
            if (x - *v).abs() > 1e-6 {
                *v = x;
                ch = true;
            }
        }
    }
    ch
}

fn color_field(ui: &mut Ui, bufs: &mut HashMap<String, String>, id: &str, label: &str, rgb: &mut [f32; 3]) -> bool {
    let u = [(rgb[0] * 255.0).round().clamp(0.0, 255.0) as u8, (rgb[1] * 255.0).round().clamp(0.0, 255.0) as u8, (rgb[2] * 255.0).round().clamp(0.0, 255.0) as u8];
    let cur_hex = format!("#{:02x}{:02x}{:02x}", u[0], u[1], u[2]);
    let buf = bufs.entry(id.to_string()).or_insert_with(|| cur_hex.clone());
    let (resp, row_rect) = th::text_row_pad(ui, id, label, buf, 34.0);
    let text = buf.clone();
    let mut ch = false;
    if resp.changed() {
        if let Some(c) = crate::state::hex_to_rgb(&text) {
            *rgb = [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0];
            ch = true;
        }
    } else if !resp.has_focus() && text != cur_hex {
        bufs.insert(id.to_string(), cur_hex);
    }
    let sw = Rect::from_center_size(Pos2::new(row_rect.right() - 22.0, row_rect.center().y), Vec2::new(24.0, 24.0));
    if let Some(c) = crate::ui::swatch_picker(ui, id, sw, u) {
        *rgb = [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0];
        bufs.insert(id.to_string(), format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]));
        ch = true;
    }
    ch
}

fn size_field(ui: &mut Ui, bufs: &mut HashMap<String, String>, id: &str, label: &str, val: &mut usize) -> bool {
    let cur = val.to_string();
    let buf = bufs.entry(id.to_string()).or_insert_with(|| cur.clone());
    let resp = th::text_row(ui, id, label, buf);
    let text = buf.clone();
    if resp.changed() {
        if let Ok(v) = text.trim().parse::<usize>() {
            let clamped = v.clamp(2, 2048);
            if clamped != *val {
                *val = clamped;
                return true;
            }
        }
    } else if !resp.has_focus() && text != cur {
        bufs.insert(id.to_string(), cur);
    }
    false
}

/// Draw the `Create` left-panel page.
pub fn draw_panel(ui: &mut Ui, cr: &mut CreatorState, skin_dir: Option<&Path>) -> CreatorAction {
    th::section_header(ui, "Create element");
    let before = cr.doc.clone();
    let mut changed = false;
    let mut discrete = false;
    let mut action = CreatorAction::None;

    let kinds: Vec<String> = ElementKind::ALL.iter().map(|k| k.label().to_string()).collect();
    if let Some(idx) = th::dropdown_row(ui, "cr-kind", "Element template", cr.doc.kind.label(), &kinds) {
        cr.select_kind(ElementKind::ALL[idx], skin_dir);
    }
    if cr.doc.kind.is_combo_tinted() {
        th::note(ui, "osu! multiplies this element by the active combo color in-game. Keep the base white or grayscale, and use the Combo tint pill on the canvas to preview it.");
    }
    if th::text_row(ui, "cr-name", "Filename (without @2x.png)", &mut cr.doc.name).changed() {
        cr.dirty = true;
    }
    changed |= size_field(ui, &mut cr.bufs, "cr-w", "Canvas width @2x (px)", &mut cr.doc.width);
    changed |= size_field(ui, &mut cr.bufs, "cr-h", "Canvas height @2x (px)", &mut cr.doc.height);

    let save_lbl = if cr.dirty { "Save to skin (@2x + 1x) *" } else { "Save to skin (@2x + 1x)" };
    if th::button_wide(ui, save_lbl, Kind::Accent, skin_dir.is_some()).on_hover_text("Write <name>@2x.png and zero-ringing <name>.png into the open skin (Ctrl+S)").clicked() {
        action = CreatorAction::SaveToSkin;
    }
    if skin_dir.is_none() {
        th::note(ui, "Open a skin folder or .osk on the Pictures page to save directly into it, or send the element to Edits to save anywhere.");
    }
    if th::button_wide(ui, "Send to Edits tab", Kind::Dark, true).on_hover_text("Open the rendered @2x image in Edits to stack Deband, Blur, Sharpen, or Save as").clicked() {
        action = CreatorAction::SendToEdits;
    }

    th::subsection_header(ui, "Layers");
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        if th::button(ui, "Undo", Kind::Dark, cr.can_undo()).on_hover_text("Undo (Ctrl+Z)").clicked() {
            cr.undo();
        }
        if th::button(ui, "Redo", Kind::Dark, cr.can_redo()).on_hover_text("Redo (Ctrl+Y)").clicked() {
            cr.redo();
        }
        if th::button_fixed(ui, 118.0, if cr.add_open { "Cancel" } else { "+ Add layer" }, Kind::Accent, true).clicked() {
            cr.add_open = !cr.add_open;
        }
    });
    ui.add_space(8.0);

    if cr.add_open {
        let r_def = (cr.doc.width.min(cr.doc.height) as f32 * 0.35).clamp(12.0, 120.0);
        let choices: [(&str, Shape); 6] = [
            ("Disc", Shape::Disc { radius: r_def }),
            ("Ring", Shape::Ring { radius: r_def, thickness: 10.0 }),
            ("Rounded rect", Shape::RoundedRect { width: r_def * 1.6, height: r_def * 1.6, corner: 16.0 }),
            ("Polygon", Shape::Polygon { sides: 6, radius: r_def, corner: 6.0 }),
            ("Chevron", Shape::Chevron { size: r_def * 0.6, thickness: 12.0, spread: r_def * 0.5 }),
            ("Crosshair", Shape::Crosshair { length: r_def * 0.35, thickness: 5.0, gap: 6.0 }),
        ];
        let labels: Vec<&str> = choices.iter().map(|c| c.0).collect();
        if let Some(k) = th::button_grid(ui, &labels, 2, Kind::Dark) {
            let n = cr.doc.layers.len();
            cr.collapsed = (0..n).collect();
            cr.doc.layers.push(Layer::new(choices[k].0, choices[k].1));
            cr.add_open = false;
            cr.bufs.clear();
            changed = true;
            discrete = true;
        }
        ui.add_space(6.0);
    }

    let n = cr.doc.layers.len();
    cr.collapsed.retain(|i| *i < n);
    let (mut remove, mut up, mut down, mut dup) = (None, None, None, None);
    for i in 0..n {
        let folded = cr.collapsed.contains(&i);
        let is_off = !cr.doc.layers[i].visible;
        let width = ui.available_width() - 22.0;
        let (r, _) = ui.allocate_exact_size(Vec2::new(width + 22.0, 36.0), Sense::hover());
        let r = Rect::from_min_size(r.min + Vec2::new(12.0, 3.0), Vec2::new(width, 30.0));
        let head = ui.interact(r, ui.scope_id().with(("cr-head", i)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        let hh = th::ease(ui, head.id.with("h"), head.hovered());
        ui.painter().rect_filled(r, CornerRadius::same(6), th::lerp_c(BACKGROUND5, th::lighten(BACKGROUND5, 0.04), hh));
        let c = r.left_center() + Vec2::new(16.0, 0.0);
        let arrow = if folded { vec![c + Vec2::new(-2.5, -4.5), c + Vec2::new(3.0, 0.0), c + Vec2::new(-2.5, 4.5)] } else { vec![c + Vec2::new(-4.5, -2.5), c + Vec2::new(0.0, 3.0), c + Vec2::new(4.5, -2.5)] };
        ui.painter().add(egui::Shape::convex_polygon(arrow, if is_off { LIGHT3 } else { LIGHT1 }, Stroke::NONE));
        let tf = FontId::new(14.5, th::semibold());
        let title = th::fit_text(ui.painter(), &format!("{}. {}", i + 1, cr.doc.layers[i].name), &tf, (r.width() - 32.0 - 148.0).max(36.0));
        ui.painter().text(r.left_center() + Vec2::new(32.0, 0.0), Align2::LEFT_CENTER, title, tf, if is_off { LIGHT3 } else { CONTENT1 });

        let icon_btn = |ui: &mut Ui, id: &str, right: f32, tip: &str, draw: &dyn Fn(&egui::Painter, Pos2, Color32)| -> bool {
            let b = Rect::from_center_size(Pos2::new(r.right() - right, r.center().y), Vec2::new(24.0, 24.0));
            let resp = ui.interact(b, ui.scope_id().with(("cr-btn", id, i)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip);
            let h = th::ease(ui, resp.id.with("h"), resp.hovered());
            if h > 0.01 {
                ui.painter().rect_filled(b, CornerRadius::same(5), Color32::from_rgba_unmultiplied(DARK3.r(), DARK3.g(), DARK3.b(), (h * 255.0) as u8));
            }
            draw(ui.painter(), b.center(), th::lerp_c(LIGHT1, CONTENT1, h));
            resp.clicked()
        };
        let mut clicked_btn = false;
        if icon_btn(ui, "rm", 15.0, "Remove layer", &|p, c, col| {
            p.line_segment([c + Vec2::new(-4.0, -4.0), c + Vec2::new(4.0, 4.0)], Stroke::new(1.8, col));
            p.line_segment([c + Vec2::new(-4.0, 4.0), c + Vec2::new(4.0, -4.0)], Stroke::new(1.8, col));
        }) {
            remove = Some(i);
            clicked_btn = true;
        }
        if i + 1 < n && icon_btn(ui, "dn", 41.0, "Move layer up (drawn on top)", &|p, c, col| {
            p.add(egui::Shape::line(vec![c + Vec2::new(-4.0, -2.0), c + Vec2::new(0.0, 2.5), c + Vec2::new(4.0, -2.0)], Stroke::new(1.8, col)));
        }) {
            down = Some(i);
            clicked_btn = true;
        }
        if i > 0 && icon_btn(ui, "up", 67.0, "Move layer down (drawn behind)", &|p, c, col| {
            p.add(egui::Shape::line(vec![c + Vec2::new(-4.0, 2.0), c + Vec2::new(0.0, -2.5), c + Vec2::new(4.0, 2.0)], Stroke::new(1.8, col)));
        }) {
            up = Some(i);
            clicked_btn = true;
        }
        if icon_btn(ui, "dup", 93.0, "Duplicate layer", &|p, c, col| {
            p.line_segment([c + Vec2::new(-4.0, 0.0), c + Vec2::new(4.0, 0.0)], Stroke::new(1.8, col));
            p.line_segment([c + Vec2::new(0.0, -4.0), c + Vec2::new(0.0, 4.0)], Stroke::new(1.8, col));
        }) {
            dup = Some(i);
            clicked_btn = true;
        }
        if icon_btn(ui, "vis", 119.0, if is_off { "Show layer" } else { "Hide layer" }, &|p, c, col| {
            let ec = if is_off { LIGHT3 } else { col };
            let pts_top: Vec<Pos2> = (-4..=4).map(|k| { let t = k as f32 / 4.0; c + Vec2::new(t * 6.5, -(1.0 - t * t) * 4.0) }).collect();
            let pts_bot: Vec<Pos2> = (-4..=4).map(|k| { let t = k as f32 / 4.0; c + Vec2::new(t * 6.5, (1.0 - t * t) * 4.0) }).collect();
            p.add(egui::Shape::line(pts_top, Stroke::new(1.5, ec)));
            p.add(egui::Shape::line(pts_bot, Stroke::new(1.5, ec)));
            p.circle_filled(c, 1.8, ec);
            if is_off {
                p.line_segment([c + Vec2::new(-5.5, 5.0), c + Vec2::new(5.5, -5.0)], Stroke::new(1.8, PINK3));
            }
        }) {
            cr.doc.layers[i].visible = !cr.doc.layers[i].visible;
            changed = true;
            discrete = true;
            clicked_btn = true;
        }
        if head.clicked() && !clicked_btn {
            if folded {
                cr.collapsed.remove(&i);
            } else {
                cr.collapsed.insert(i);
            }
        }
        if !folded {
            let max_r = (cr.doc.width.max(cr.doc.height) as f32 * 0.6).max(64.0);
            let l = &mut cr.doc.layers[i];
            changed |= layer_controls(ui, &mut cr.bufs, l, i, max_r);
        }
        ui.add_space(6.0);
    }

    if let Some(i) = remove {
        cr.doc.layers.remove(i);
        cr.collapsed = cr.collapsed.iter().filter(|c| **c != i).map(|c| if *c > i { c - 1 } else { *c }).collect();
        cr.bufs.clear();
        changed = true;
        discrete = true;
    } else if let Some(i) = dup {
        let mut copy = cr.doc.layers[i].clone();
        copy.name = format!("{} copy", copy.name);
        cr.doc.layers.insert(i + 1, copy);
        cr.collapsed.insert(i);
        cr.bufs.clear();
        changed = true;
        discrete = true;
    } else if let Some(i) = up {
        cr.doc.layers.swap(i - 1, i);
        let (a, b) = (cr.collapsed.remove(&(i - 1)), cr.collapsed.remove(&i));
        if a { cr.collapsed.insert(i); }
        if b { cr.collapsed.insert(i - 1); }
        cr.bufs.clear();
        changed = true;
        discrete = true;
    } else if let Some(i) = down {
        cr.doc.layers.swap(i, i + 1);
        let (a, b) = (cr.collapsed.remove(&i), cr.collapsed.remove(&(i + 1)));
        if a { cr.collapsed.insert(i + 1); }
        if b { cr.collapsed.insert(i); }
        cr.bufs.clear();
        changed = true;
        discrete = true;
    }

    if cr.doc.layers.is_empty() && !cr.add_open {
        th::note(ui, "No layers yet. Use + Add layer to build shapes, rings, glows, and cutouts.");
    }

    if changed && cr.doc != before {
        cr.push_undo(before, !discrete);
        cr.dirty = true;
        cr.invalidate();
    }
    action
}

fn layer_controls(ui: &mut Ui, bufs: &mut HashMap<String, String>, l: &mut Layer, i: usize, max_r: f32) -> bool {
    let mut ch = false;
    ch |= th::text_row(ui, &format!("cln{i}"), "Layer name", &mut l.name).changed();

    // Shape controls
    match &mut l.shape {
        Shape::Disc { radius } => {
            ch |= c_slider(ui, &format!("csd{i}"), "Radius", radius, 1.0, max_r, 96.0, |v| format!("{v:.1} px"));
        }
        Shape::Ring { radius, thickness } => {
            ch |= c_slider(ui, &format!("csr{i}"), "Radius", radius, 1.0, max_r, 112.0, |v| format!("{v:.1} px"));
            ch |= c_slider(ui, &format!("cst{i}"), "Thickness", thickness, 1.0, 64.0, 10.0, |v| format!("{v:.1} px"));
        }
        Shape::RoundedRect { width, height, corner } => {
            ch |= c_slider(ui, &format!("csw{i}"), "Width", width, 2.0, max_r * 2.0, 140.0, |v| format!("{v:.0} px"));
            ch |= c_slider(ui, &format!("csh{i}"), "Height", height, 2.0, max_r * 2.0, 140.0, |v| format!("{v:.0} px"));
            ch |= c_slider(ui, &format!("csc{i}"), "Corner radius", corner, 0.0, 100.0, 16.0, |v| format!("{v:.0} px"));
        }
        Shape::Polygon { sides, radius, corner } => {
            let mut s = *sides as f32;
            if c_slider(ui, &format!("csps{i}"), "Sides", &mut s, 3.0, 12.0, 6.0, |v| format!("{:.0}", v)) {
                *sides = s.round().clamp(3.0, 12.0) as u8;
                ch = true;
            }
            ch |= c_slider(ui, &format!("cspr{i}"), "Radius", radius, 2.0, max_r, 80.0, |v| format!("{v:.1} px"));
            ch |= c_slider(ui, &format!("cspc{i}"), "Corner radius", corner, 0.0, 48.0, 6.0, |v| format!("{v:.1} px"));
        }
        Shape::Chevron { size, thickness, spread } => {
            ch |= c_slider(ui, &format!("cscs{i}"), "Length", size, 4.0, max_r, 52.0, |v| format!("{v:.1} px"));
            ch |= c_slider(ui, &format!("csct{i}"), "Thickness", thickness, 1.0, 48.0, 14.0, |v| format!("{v:.1} px"));
            ch |= c_slider(ui, &format!("cscw{i}"), "Spread", spread, 4.0, max_r, 44.0, |v| format!("{v:.1} px"));
        }
        Shape::Crosshair { length, thickness, gap } => {
            ch |= c_slider(ui, &format!("csxl{i}"), "Arm length", length, 1.0, max_r, 18.0, |v| format!("{v:.1} px"));
            ch |= c_slider(ui, &format!("csxt{i}"), "Thickness", thickness, 1.0, 24.0, 5.0, |v| format!("{v:.1} px"));
            ch |= c_slider(ui, &format!("csxg{i}"), "Center gap", gap, 0.0, 64.0, 6.0, |v| format!("{v:.1} px"));
        }
    }

    ch |= c_slider(ui, &format!("cop{i}"), "Opacity", &mut l.opacity, 0.0, 1.0, 1.0, |v| format!("{:.0}%", v * 100.0));
    let blends: Vec<String> = BlendMode::ALL.iter().map(|b| b.name().to_string()).collect();
    if let Some(k) = th::dropdown_row(ui, &format!("cbl{i}"), "Blend mode", l.blend.name(), &blends) {
        l.blend = BlendMode::ALL[k];
        ch = true;
    }
    ch |= c_slider(ui, &format!("cfth{i}"), "Edge softness", &mut l.feather, 0.0, 24.0, 0.0, |v| format!("{v:.1} px"));
    ch |= c_slider(ui, &format!("crot{i}"), "Rotation", &mut l.rotation, -180.0, 180.0, 0.0, |v| format!("{v:+.0}°"));
    ch |= c_slider(ui, &format!("cox{i}"), "Offset X", &mut l.offset[0], -128.0, 128.0, 0.0, |v| format!("{v:+.0} px"));
    ch |= c_slider(ui, &format!("coy{i}"), "Offset Y", &mut l.offset[1], -128.0, 128.0, 0.0, |v| format!("{v:+.0} px"));

    // Fill style
    let fill_kinds = ["Solid".to_string(), "Radial gradient".to_string(), "Linear gradient".to_string()];
    if let Some(k) = th::dropdown_row(ui, &format!("cfk{i}"), "Fill style", l.fill.kind_name(), &fill_kinds) {
        if k != l.fill.kind_index() {
            l.fill = match k {
                0 => FillStyle::Solid { color: [1.0, 1.0, 1.0], alpha: 1.0 },
                1 => FillStyle::Radial { inner: [1.0, 1.0, 1.0], inner_a: 1.0, outer: [0.6, 0.6, 0.6], outer_a: 0.9, power: 1.4 },
                _ => FillStyle::Linear { top: [1.0, 1.0, 1.0], top_a: 1.0, bottom: [0.75, 0.78, 0.88], bottom_a: 1.0, angle: 90.0 },
            };
            ch = true;
        }
    }
    match &mut l.fill {
        FillStyle::Solid { color, alpha } => {
            ch |= color_field(ui, bufs, &format!("cfs{i}"), "Fill color", color);
            ch |= c_slider(ui, &format!("cfsa{i}"), "Fill alpha", alpha, 0.0, 1.0, 1.0, |v| format!("{:.0}%", v * 100.0));
        }
        FillStyle::Radial { inner, inner_a, outer, outer_a, power } => {
            ch |= color_field(ui, bufs, &format!("cfri{i}"), "Inner color", inner);
            ch |= c_slider(ui, &format!("cfria{i}"), "Inner alpha", inner_a, 0.0, 1.0, 1.0, |v| format!("{:.0}%", v * 100.0));
            ch |= color_field(ui, bufs, &format!("cfro{i}"), "Outer color", outer);
            ch |= c_slider(ui, &format!("cfroa{i}"), "Outer alpha", outer_a, 0.0, 1.0, 0.9, |v| format!("{:.0}%", v * 100.0));
            ch |= c_slider(ui, &format!("cfrp{i}"), "Gradient curve", power, 0.2, 4.0, 1.4, |v| format!("{v:.2}"));
        }
        FillStyle::Linear { top, top_a, bottom, bottom_a, angle } => {
            ch |= color_field(ui, bufs, &format!("cflt{i}"), "Start color", top);
            ch |= c_slider(ui, &format!("cflta{i}"), "Start alpha", top_a, 0.0, 1.0, 1.0, |v| format!("{:.0}%", v * 100.0));
            ch |= color_field(ui, bufs, &format!("cflb{i}"), "End color", bottom);
            ch |= c_slider(ui, &format!("cflba{i}"), "End alpha", bottom_a, 0.0, 1.0, 1.0, |v| format!("{:.0}%", v * 100.0));
            ch |= c_slider(ui, &format!("cfla{i}"), "Gradient angle", angle, 0.0, 360.0, 90.0, |v| format!("{v:.0}°"));
        }
    }

    // Stroke
    ch |= th::switch_row(ui, &format!("cske{i}"), "Stroke", &mut l.stroke.enabled);
    if l.stroke.enabled {
        ch |= c_slider(ui, &format!("cskw{i}"), "Stroke width", &mut l.stroke.width, 0.5, 32.0, 4.0, |v| format!("{v:.1} px"));
        ch |= color_field(ui, bufs, &format!("cskc{i}"), "Stroke color", &mut l.stroke.color);
        ch |= c_slider(ui, &format!("csko{i}"), "Stroke opacity", &mut l.stroke.opacity, 0.0, 1.0, 1.0, |v| format!("{:.0}%", v * 100.0));
        let aligns: Vec<String> = StrokeAlign::ALL.iter().map(|a| a.name().to_string()).collect();
        if let Some(k) = th::dropdown_row(ui, &format!("cska{i}"), "Stroke alignment", l.stroke.align.name(), &aligns) {
            l.stroke.align = StrokeAlign::ALL[k];
            ch = true;
        }
    }

    // Analytical SDF Glow
    ch |= th::switch_row(ui, &format!("cgle{i}"), "Glow", &mut l.glow.enabled);
    if l.glow.enabled {
        let GlowStyle { radius, color, intensity, inner, .. } = &mut l.glow;
        ch |= th::switch_row(ui, &format!("cgli{i}"), "Inner glow (off: outer)", inner);
        ch |= c_slider(ui, &format!("cglr{i}"), "Glow radius", radius, 1.0, 64.0, 16.0, |v| format!("{v:.0} px"));
        ch |= color_field(ui, bufs, &format!("cglc{i}"), "Glow color", color);
        ch |= c_slider(ui, &format!("cgls{i}"), "Glow intensity", intensity, 0.0, 1.5, 0.6, |v| format!("{:.0}%", v * 100.0));
    }
    ch
}

/// Apply a display-only osu! combo tint multiply to an `Img` for canvas preview (never saved to disk).
pub fn apply_preview_tint(src: &Img, rgb: [u8; 3]) -> Img {
    let t = [rgb[0] as f32 / 255.0, rgb[1] as f32 / 255.0, rgb[2] as f32 / 255.0];
    let px = src.px.iter().map(|p| [p[0] * t[0], p[1] * t[1], p[2] * t[2], p[3]]).collect();
    Img { w: src.w, h: src.h, px }
}
