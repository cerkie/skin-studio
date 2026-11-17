//! lazer's overlay design language, taken from its source (OverlayColourProvider with the Purple scheme, FormControl*,
//! SwitchButton, FormSliderBar, RoundedButton, SettingsSidebar, SettingsSection): flat rows with a 5 px corner, a 2.5 px
//! border that appears on hover (gray-violet) and focus (the highlight color), 250 ms color transitions, an outlined
//! switch that fills when on, slider bars with a 10 px nub, buttons filled with Color3 and lightened on hover.
use egui::{emath::easing, Align2, Color32, CornerRadius, FontFamily, FontId, Id, Pos2, Rect, Response, RichText, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};

/// OverlayColourProvider(hue 255). Values computed from its `getColor(saturation, lightness)` table.
pub mod pal {
    use egui::Color32;
    pub const COLOR0: Color32 = Color32::from_rgb(0xb3, 0x99, 0xff);
    pub const COLOR1: Color32 = Color32::from_rgb(0x8c, 0x66, 0xff);
    pub const COLOR2: Color32 = Color32::from_rgb(0x70, 0x47, 0xeb);
    pub const COLOR3: Color32 = Color32::from_rgb(0x59, 0x33, 0xcc);
    pub const HIGHLIGHT1: Color32 = Color32::from_rgb(0x8c, 0x66, 0xff);
    pub const CONTENT1: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
    pub const CONTENT2: Color32 = Color32::from_rgb(0xe0, 0xdb, 0xf0);
    pub const LIGHT1: Color32 = Color32::from_rgb(0xc2, 0xb8, 0xe0);
    pub const LIGHT3: Color32 = Color32::from_rgb(0xa3, 0x94, 0xd1);
    pub const LIGHT4: Color32 = Color32::from_rgb(0x66, 0x4d, 0xb2);
    pub const DARK1: Color32 = Color32::from_rgb(0x50, 0x47, 0x6b);
    pub const DARK2: Color32 = Color32::from_rgb(0x45, 0x3d, 0x5c);
    pub const DARK3: Color32 = Color32::from_rgb(0x39, 0x33, 0x4c);
    pub const DARK4: Color32 = Color32::from_rgb(0x2e, 0x29, 0x3d);
    pub const BACKGROUND1: Color32 = Color32::from_rgb(0x61, 0x5c, 0x70);
    pub const BACKGROUND4: Color32 = Color32::from_rgb(0x30, 0x2e, 0x38);
    pub const BACKGROUND5: Color32 = Color32::from_rgb(0x24, 0x22, 0x2a);
    pub const BACKGROUND6: Color32 = Color32::from_rgb(0x18, 0x17, 0x1c);
    /// `Background4.Darken(0.1)`: the resting color of a form control.
    pub const CONTROL: Color32 = Color32::from_rgb(0x2c, 0x2a, 0x33);
    /// OsuColor.Pink3, the pink of a settings button; PinkDark for hover.
    pub const PINK3: Color32 = Color32::from_rgb(0xcc, 0x33, 0x78);
    pub const PINK1: Color32 = Color32::from_rgb(0xff, 0x66, 0xab);
    pub const TOOLBAR: Color32 = Color32::from_rgb(0x18, 0x18, 0x18);
    pub const RED: Color32 = Color32::from_rgb(0xed, 0x11, 0x21);
}

use pal::*;

pub fn lerp_c(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

pub fn lighten(c: Color32, amount: f32) -> Color32 {
    // osu-framework's Lighten: multiply towards white
    let f = |v: u8| ((v as f32 * (1.0 + 0.5 * amount) + 255.0 * amount).min(255.0)) as u8;
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

pub fn darken(c: Color32, amount: f32) -> Color32 {
    let f = |v: u8| (v as f32 / (1.0 + amount)) as u8;
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

fn ease(ui: &Ui, id: Id, target: bool) -> f32 {
    ui.ctx().animate_bool_with_time_and_easing(id, target, 0.25, easing::cubic_out)
}

pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}
pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}
pub fn light() -> FontFamily {
    FontFamily::Name("regular".into())
}

/// Inter, embedded: regular for light text, medium for body text, then two heavier weights.
pub fn set_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let b: [&'static [u8]; 4] = [include_bytes!("../ui/fonts/Inter-Regular.otf"), include_bytes!("../ui/fonts/Inter-Medium.otf"), include_bytes!("../ui/fonts/Inter-SemiBold.otf"), include_bytes!("../ui/fonts/Inter-Bold.otf")];
    for (i, label) in ["light", "body", "semibold", "bold"].iter().enumerate() {
        fonts.font_data.insert(label.to_string(), std::sync::Arc::new(egui::FontData::from_static(b[i])));
    }
    fonts.families.insert(FontFamily::Proportional, vec!["body".into()]);
    fonts.families.insert(light(), vec!["light".into()]);
    fonts.families.insert(semibold(), vec!["semibold".into()]);
    fonts.families.insert(bold(), vec!["bold".into()]);
    ctx.set_fonts(fonts);
}

pub fn install(ctx: &egui::Context) {
    set_fonts(ctx);

    let mut style = (*ctx.global_style()).clone();
    style.spacing.item_spacing = Vec2::new(0.0, 4.0);
    style.spacing.button_padding = Vec2::new(10.0, 6.0);
    style.spacing.window_margin = egui::Margin::same(0);
    style.spacing.scroll.bar_width = 6.0;
    style.spacing.scroll.floating = true;
    style.spacing.scroll.floating_width = 4.0;
    style.spacing.scroll.foreground_color = false;
    style.visuals.dark_mode = true;
    style.visuals.panel_fill = BACKGROUND4;
    style.visuals.window_fill = BACKGROUND5;
    style.visuals.extreme_bg_color = BACKGROUND5;
    style.visuals.faint_bg_color = BACKGROUND5;
    style.visuals.window_corner_radius = CornerRadius::same(5);
    style.visuals.window_stroke = Stroke::new(1.0, DARK1);
    style.visuals.selection.bg_fill = Color32::from_rgba_unmultiplied(0x8c, 0x66, 0xff, 90);
    style.visuals.selection.stroke = Stroke::new(1.0, HIGHLIGHT1);
    style.visuals.text_cursor.stroke = Stroke::new(2.0, HIGHLIGHT1);
    for w in [&mut style.visuals.widgets.noninteractive, &mut style.visuals.widgets.inactive, &mut style.visuals.widgets.hovered, &mut style.visuals.widgets.active, &mut style.visuals.widgets.open] {
        w.fg_stroke = Stroke::new(1.0, CONTENT1);
        w.corner_radius = CornerRadius::same(5);
    }
    style.visuals.widgets.noninteractive.bg_fill = Color32::TRANSPARENT;
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::NONE;
    style.visuals.widgets.inactive.bg_fill = Color32::TRANSPARENT;
    style.visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    style.visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    style.visuals.widgets.hovered.bg_fill = Color32::TRANSPARENT;
    style.visuals.widgets.hovered.weak_bg_fill = Color32::TRANSPARENT;
    style.visuals.widgets.hovered.bg_stroke = Stroke::NONE;
    style.visuals.widgets.active.bg_fill = Color32::TRANSPARENT;
    style.visuals.widgets.active.weak_bg_fill = Color32::TRANSPARENT;
    style.visuals.widgets.active.bg_stroke = Stroke::NONE;
    style.visuals.popup_shadow = egui::Shadow::NONE;
    style.visuals.window_shadow = egui::Shadow::NONE;
    ctx.all_styles_mut(|s| *s = style.clone());
}

// ------------------------------------------------------------------------------------------------ text
/// Small outlined glyphs for the sidebar, in the style of lazer's line icons (about 20 px).
pub fn icon_glyph(p: &egui::Painter, c: Pos2, which: usize, col: Color32) {
    let s = Stroke::new(1.6, col);
    let v = |x: f32, y: f32| c + Vec2::new(x, y);
    match which {
        // effect: a sparkle
        0 => {
            for (a, b) in [((0.0, -9.0), (0.0, 9.0)), ((-9.0, 0.0), (9.0, 0.0))] {
                p.line_segment([v(a.0, a.1), v(b.0, b.1)], s);
            }
            p.line_segment([v(-4.5, -4.5), v(4.5, 4.5)], s);
            p.line_segment([v(-4.5, 4.5), v(4.5, -4.5)], s);
        }
        // source: a picture
        1 => {
            p.rect_stroke(Rect::from_center_size(c, Vec2::new(18.0, 14.0)), CornerRadius::same(3), s, StrokeKind::Middle);
            p.add(Shape::line(vec![v(-7.0, 4.0), v(-2.0, -1.0), v(1.0, 2.0), v(4.0, -2.0), v(7.0, 3.0)], s));
            p.circle_stroke(v(-3.5, -3.0), 1.6, s);
        }
        // timing: a clock
        2 => {
            p.circle_stroke(c, 9.0, s);
            p.add(Shape::line(vec![v(0.0, -5.0), v(0.0, 0.0), v(4.0, 2.5)], s));
        }
        // motion: a curve with an arrowhead
        3 => {
            p.add(Shape::line(vec![v(-9.0, 6.0), v(-4.0, -2.0), v(1.0, 3.0), v(7.0, -5.0)], s));
            p.add(Shape::line(vec![v(2.0, -6.5), v(7.0, -5.0), v(5.5, 0.0)], s));
        }
        // color: a palette
        4 => {
            p.circle_stroke(c, 9.0, s);
            for (x, y) in [(-3.5, -3.0), (2.5, -4.0), (4.5, 1.5), (-4.5, 3.0)] {
                p.circle_filled(v(x, y), 1.5, col);
            }
        }
        // finish: sliders
        5 => {
            for (y, x) in [(-5.5, -3.0), (0.0, 3.5), (5.5, -1.0)] {
                p.line_segment([v(-9.0, y), v(9.0, y)], s);
                p.circle_filled(v(x, y), 2.6, col);
            }
        }
        // preview: a play button
        6 => {
            p.circle_stroke(c, 9.0, s);
            p.add(Shape::convex_polygon(vec![v(-2.5, -4.5), v(-2.5, 4.5), v(4.5, 0.0)], col, Stroke::NONE));
        }
        // history: a clock with a back arrow
        7 => {
            p.circle_stroke(c, 8.0, s);
            p.add(Shape::line(vec![v(0.0, -4.0), v(0.0, 0.0), v(3.0, 2.0)], s));
            p.add(Shape::line(vec![v(-9.5, -3.0), v(-8.0, -0.5), v(-5.5, -2.5)], s));
        }
        // import: a tray with an arrow down
        _ => {
            p.add(Shape::line(vec![v(-8.0, 2.0), v(-8.0, 8.0), v(8.0, 8.0), v(8.0, 2.0)], s));
            p.line_segment([v(0.0, -8.0), v(0.0, 3.0)], s);
            p.add(Shape::line(vec![v(-4.0, -1.0), v(0.0, 3.5), v(4.0, -1.0)], s));
        }
    }
}

/// lazer's settings search box: a magnifier at the right, a hint while empty.
pub fn search_box(ui: &mut Ui, rect: Rect, text: &mut String) -> Response {
    ui.painter().rect_filled(rect, 0.0, Color32::from_black_alpha(46));
    ui.painter().line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, Color32::from_white_alpha(10)));
    let edit_rect = Rect::from_min_max(rect.left_top() + Vec2::new(20.0, 14.0), rect.right_bottom() - Vec2::new(56.0, 14.0));
    let resp = ui.put(edit_rect, egui::TextEdit::singleline(text).hint_text(RichText::new("type to search").color(LIGHT3).size(20.0)).frame(egui::Frame::NONE).desired_width(edit_rect.width()).font(FontId::new(20.0, FontFamily::Proportional)).text_color(CONTENT1).margin(egui::Margin::ZERO));
    let c = Pos2::new(rect.right() - 30.0, rect.center().y - 1.0);
    ui.painter().circle_stroke(c, 7.5, Stroke::new(2.4, CONTENT1));
    ui.painter().line_segment([c + Vec2::new(5.5, 5.5), c + Vec2::new(11.0, 11.0)], Stroke::new(2.6, CONTENT1));
    resp
}

/// Soft triangles drifting upwards behind a coloured button, as lazer draws behind its own.
fn button_triangles(ui: &Ui, rect: Rect, col: Color32, label: &str) {
    let t = ui.ctx().input(|i| i.time) as f32;
    let p = ui.painter().with_clip_rect(rect);
    // seeded by the label, not the position, so scrolling the panel does not shift the pattern
    let seed = label.bytes().fold(17i32, |a, b| a.wrapping_mul(31).wrapping_add(b as i32));
    for k in 0..6 {
        let h = ((seed + k * 7919).wrapping_mul(2654435761u32 as i32) as u32 >> 8) as f32 / 16777216.0;
        let size = 22.0 + 30.0 * ((h * 13.0).fract());
        let speed = 6.0 + 10.0 * ((h * 7.0).fract());
        let x = rect.left() + rect.width() * ((h * 5.0 + k as f32 * 0.17).fract());
        let travel = rect.height() + size * 2.0;
        let y = rect.bottom() + size - ((t * speed + h * 100.0) % travel);
        let tri = vec![Pos2::new(x, y - size * 0.6), Pos2::new(x - size * 0.5, y + size * 0.4), Pos2::new(x + size * 0.5, y + size * 0.4)];
        p.add(Shape::convex_polygon(tri, Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 20), Stroke::NONE));
    }
}

/// A rectangle filled with a vertical gradient (no rounded corners).
pub fn vgradient(p: &egui::Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut m = egui::Mesh::default();
    m.colored_vertex(rect.left_top(), top);
    m.colored_vertex(rect.right_top(), top);
    m.colored_vertex(rect.right_bottom(), bottom);
    m.colored_vertex(rect.left_bottom(), bottom);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    p.add(Shape::mesh(m));
}

/// A rectangle filled with a horizontal gradient (no rounded corners).
pub fn hgradient(p: &egui::Painter, rect: Rect, left: Color32, right: Color32) {
    let mut m = egui::Mesh::default();
    m.colored_vertex(rect.left_top(), left);
    m.colored_vertex(rect.right_top(), right);
    m.colored_vertex(rect.right_bottom(), right);
    m.colored_vertex(rect.left_bottom(), left);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    p.add(Shape::mesh(m));
}

/// A soft drop shadow under a rounded rectangle.
pub fn shadow(p: &egui::Painter, rect: Rect, radius: u8, strength: u8) {
    for i in 0..8 {
        let g = 1.0 + i as f32 * 2.2;
        let a = (strength as f32 * (1.0 - i as f32 / 8.0).powi(2) / 5.0) as u8;
        p.rect_filled(rect.translate(Vec2::new(0.0, 4.0)).expand(g), CornerRadius::same(radius.saturating_add(i as u8 * 2)), Color32::from_black_alpha(a));
    }
}

pub fn text(s: impl Into<String>, size: f32, color: Color32) -> RichText {
    RichText::new(s).size(size).color(color)
}

pub fn caption(ui: &mut Ui, s: &str) {
    ui.label(text(s, 14.0, CONTENT2).family(light()));
}

/// Section title: lazer sets these in Torus Alternate at 30, above a 2 px Background6 rule.
pub fn section_header(ui: &mut Ui, s: &str) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.label(text(s, 30.0, CONTENT1).family(light()));
    });
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 3.0), Sense::hover());
    let acc = Rect::from_min_size(Pos2::new(rect.left() + 16.0, rect.top()), Vec2::new(rect.width() - 32.0, 2.0));
    hgradient(ui.painter(), acc, HIGHLIGHT1, Color32::from_rgba_unmultiplied(HIGHLIGHT1.r(), HIGHLIGHT1.g(), HIGHLIGHT1.b(), 0));
    ui.add_space(6.0);
}

/// Subsection header (OsuFont size 20 in SettingsSubsection).
pub fn subsection_header(ui: &mut Ui, s: &str) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(17.0);
        ui.label(text(s, 20.0, CONTENT1).family(light()));
    });
    ui.add_space(4.0);
}

// ------------------------------------------------------------------------------------------------ form controls
/// The background of a form control: resting fill, a hover border and a focus border, eased over 250 ms.
struct Bg {
    slot: egui::layers::ShapeIdx,
}

fn bg_begin(ui: &mut Ui) -> Bg {
    Bg { slot: ui.painter().add(Shape::Noop) }
}

fn bg_end(ui: &mut Ui, bg: Bg, rect: Rect, id: Id, hovered: bool, focused: bool, disabled: bool) {
    let h = ease(ui, id.with("hover"), hovered || focused);
    let f = ease(ui, id.with("focus"), focused);
    let fill = if disabled { BACKGROUND4 } else { lerp_c(CONTROL, lerp_c(BACKGROUND5, DARK4, 0.5), h) };
    let border = lerp_c(lerp_c(fill, LIGHT4, h), HIGHLIGHT1, f);
    let mut shapes = vec![Shape::rect_filled(rect, CornerRadius::same(5), fill)];
    if h > 0.01 {
        shapes.push(Shape::rect_stroke(rect.shrink(1.25), CornerRadius::same(4), Stroke::new(2.5, border), StrokeKind::Middle));
    }
    ui.painter().set(bg.slot, Shape::Vec(shapes));
}

const PAD: f32 = 9.0;

/// A caption above a single-line text box.
pub fn text_row(ui: &mut Ui, id: &str, caption_text: &str, buf: &mut String) -> Response {
    let rid = ui.id().with(id);
    let width = ui.available_width() - 22.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 54.0), Sense::hover());
    let rect = Rect::from_min_size(rect.min + Vec2::new(12.0, 0.0), rect.size());
    let bg = bg_begin(ui);
    ui.painter().text(rect.min + Vec2::new(PAD, PAD - 1.0), Align2::LEFT_TOP, caption_text, FontId::new(14.0, light()), CONTENT2);
    let edit_rect = Rect::from_min_size(rect.min + Vec2::new(PAD, PAD + 17.0), Vec2::new(rect.width() - 2.0 * PAD, 20.0));
    let resp = ui.put(
        edit_rect,
        egui::TextEdit::singleline(buf).id(rid).frame(egui::Frame::NONE).desired_width(edit_rect.width()).font(FontId::new(16.0, FontFamily::Proportional)).text_color(CONTENT1).margin(egui::Margin::ZERO),
    );
    let hovered = ui.rect_contains_pointer(rect);
    bg_end(ui, bg, rect, rid, hovered, resp.has_focus(), false);
    resp
}

/// A caption and the current choice, a chevron, and a list that opens below. Returns the index picked.
pub fn dropdown_row(ui: &mut Ui, id: &str, caption_text: &str, current: &str, items: &[String]) -> Option<usize> {
    let rid = ui.id().with(id);
    let width = ui.available_width() - 22.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 54.0), Sense::click());
    let rect = Rect::from_min_size(rect.min + Vec2::new(12.0, 0.0), rect.size());
    let bg = bg_begin(ui);
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    ui.painter().text(rect.min + Vec2::new(PAD, PAD - 1.0), Align2::LEFT_TOP, caption_text, FontId::new(14.0, light()), CONTENT2);
    let label_rect = ui.painter().text(rect.min + Vec2::new(PAD, PAD + 17.0), Align2::LEFT_TOP, elide(current, 34), FontId::new(16.0, FontFamily::Proportional), CONTENT1);
    let _ = label_rect;
    // the chevron turns over while the list is open
    let t = ease(ui, rid.with("chev"), open);
    let c = Pos2::new(rect.right() - PAD - 8.0, rect.center().y);
    let s = 1.0 - 2.0 * t;
    ui.painter().add(Shape::line(vec![c + Vec2::new(-6.0, -3.0 * s), c + Vec2::new(0.0, 3.0 * s), c + Vec2::new(6.0, -3.0 * s)], Stroke::new(2.0, CONTENT1)));
    bg_end(ui, bg, rect, rid, resp.hovered(), open, false);
    let mut picked = None;
    egui::Popup::menu(&resp).width(rect.width()).gap(2.0).show(|ui| {
        ui.set_min_width(rect.width());
        egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
            for (i, it) in items.iter().enumerate() {
                let (r, ir) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::click());
                let sel = it == current;
                if ir.hovered() {
                    ui.painter().rect_filled(r, CornerRadius::same(4), DARK3);
                }
                ui.painter().text(r.left_center() + Vec2::new(10.0, 0.0), Align2::LEFT_CENTER, it, FontId::new(16.0, FontFamily::Proportional), if sel { HIGHLIGHT1 } else { CONTENT1 });
                if ir.clicked() {
                    picked = Some(i);
                    ui.close();
                }
            }
        });
    });
    picked
}

fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max - 1).collect();
        format!("{t}…")
    }
}

/// An outlined pill that fills with the highlight color when on (SwitchButton, 56 x 16).
pub fn switch_row(ui: &mut Ui, id: &str, label: &str, on: &mut bool) -> bool {
    let rid = ui.id().with(id);
    let width = ui.available_width() - 22.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 38.0), Sense::click());
    let rect = Rect::from_min_size(rect.min + Vec2::new(12.0, 0.0), rect.size());
    let bg = bg_begin(ui);
    ui.painter().text(rect.left_center() + Vec2::new(PAD, 0.0), Align2::LEFT_CENTER, elide(label, 44), FontId::new(14.0, FontFamily::Proportional), CONTENT2);
    let mut changed = false;
    if resp.clicked() {
        *on = !*on;
        changed = true;
    }
    let t = ease(ui, rid.with("on"), *on);
    let hov = ease(ui, rid.with("sw"), resp.hovered());
    let w = 56.0 * (0.75 + 0.25 * t);
    let sw = Rect::from_min_size(Pos2::new(rect.right() - PAD - 56.0 + (56.0 - w) / 2.0, rect.center().y - 8.0), Vec2::new(w, 16.0));
    let border = lerp_c(lerp_c(LIGHT4, darken(HIGHLIGHT1, 0.1), hov), HIGHLIGHT1, t);
    ui.painter().rect_filled(sw, CornerRadius::same(8), lerp_c(Color32::TRANSPARENT, border, t));
    ui.painter().rect_stroke(sw.shrink(1.0), CornerRadius::same(8), Stroke::new(2.0, border), StrokeKind::Middle);
    bg_end(ui, bg, rect, rid, resp.hovered(), false, false);
    changed
}

/// Caption and value on the left, a 40 px slider bar on the right (FormSliderBar). `v` is 0..1.
pub fn slider_row(ui: &mut Ui, id: &str, caption_text: &str, value_text: &str, v: &mut f32) -> bool {
    let rid = ui.id().with(id);
    let width = ui.available_width() - 22.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 56.0), Sense::hover());
    let rect = Rect::from_min_size(rect.min + Vec2::new(12.0, 0.0), rect.size());
    let bg = bg_begin(ui);
    ui.painter().text(rect.min + Vec2::new(PAD, PAD - 1.0), Align2::LEFT_TOP, caption_text, FontId::new(14.0, light()), CONTENT2);
    ui.painter().text(rect.min + Vec2::new(PAD, PAD + 17.0), Align2::LEFT_TOP, value_text, FontId::new(16.0, FontFamily::Proportional), CONTENT1);
    let bar = Rect::from_min_size(Pos2::new(rect.center().x, rect.top() + 8.0), Vec2::new(rect.width() / 2.0 - 8.0, 40.0));
    let resp = ui.interact(bar, rid.with("bar"), Sense::click_and_drag());
    let nub_w = 10.0;
    let mut changed = false;
    if resp.dragged() || resp.clicked() || resp.is_pointer_button_down_on() {
        if let Some(p) = resp.interact_pointer_pos() {
            let nv = ((p.x - bar.left() - nub_w / 2.0) / (bar.width() - nub_w)).clamp(0.0, 1.0);
            if (nv - *v).abs() > 1e-4 {
                *v = nv;
                changed = true;
            }
        }
    }
    let hov = ease(ui, rid.with("h"), resp.hovered() || resp.dragged());
    ui.painter().rect_filled(bar, CornerRadius::same(5), BACKGROUND5);
    let nx = bar.left() + nub_w / 2.0 + (bar.width() - nub_w) * *v;
    let fill = Rect::from_min_max(bar.min, Pos2::new(nx, bar.bottom()));
    ui.painter().rect_filled(fill, CornerRadius { nw: 5, sw: 5, ne: 0, se: 0 }, lerp_c(darken(LIGHT4, 0.4), LIGHT4, hov.max(0.6)));
    let nub = Rect::from_center_size(Pos2::new(nx, bar.center().y), Vec2::new(nub_w, 34.0));
    ui.painter().rect_filled(nub, CornerRadius::same(5), lerp_c(darken(HIGHLIGHT1, 0.1), HIGHLIGHT1, hov));
    bg_end(ui, bg, rect, rid, resp.hovered() || resp.dragged(), false, false);
    changed
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    /// Color3, the settings button.
    Purple,
    /// The pink of destructive-looking or primary actions.
    Pink,
    /// A dark button for secondary actions.
    Dark,
    /// No fill until hovered (the toolbar).
    Ghost,
    /// A fully rounded primary action.
    Accent,
}

/// A full-width settings button (RoundedButton: 5 px corner, lightened by 0.2 on hover).
pub fn button_wide(ui: &mut Ui, label: &str, kind: Kind, enabled: bool) -> Response {
    let width = ui.available_width() - 22.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 40.0), Sense::click());
    let rect = Rect::from_min_size(rect.min + Vec2::new(12.0, 0.0), rect.size());
    paint_button(ui, rect, &resp, label, kind, enabled);
    resp
}

/// A button sized to its text.
pub fn button(ui: &mut Ui, label: &str, kind: Kind, enabled: bool) -> Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), FontId::new(15.0, semibold()), CONTENT1);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(galley.size().x + 28.0, 32.0), Sense::click());
    paint_button(ui, rect, &resp, label, kind, enabled);
    resp
}

fn paint_button(ui: &mut Ui, rect: Rect, resp: &Response, label: &str, kind: Kind, enabled: bool) {
    let base = match kind {
        Kind::Purple => COLOR3,
        Kind::Pink => PINK3,
        Kind::Dark => DARK2,
        Kind::Ghost => Color32::TRANSPARENT,
        Kind::Accent => COLOR3,
    };
    let h = ease(ui, resp.id.with("btn"), resp.hovered() && enabled);
    let col = if !enabled {
        BACKGROUND4
    } else if kind == Kind::Ghost {
        Color32::from_white_alpha((h * 22.0) as u8)
    } else if kind == Kind::Accent {
        lerp_c(base, HIGHLIGHT1, h)
    } else {
        lerp_c(base, lighten(base, 0.2), h)
    };
    let radius = if matches!(kind, Kind::Ghost | Kind::Accent) { 255 } else { 8 };
    let press = if resp.is_pointer_button_down_on() && enabled { 0.97 } else { 1.0 };
    let r = Rect::from_center_size(rect.center(), rect.size() * press);
    if kind == Kind::Accent && enabled {
        // a soft glow under the primary action
        for i in 1..5 {
            ui.painter().rect_filled(r.expand(i as f32 * 2.0), CornerRadius::same(255), Color32::from_rgba_unmultiplied(0x8c, 0x66, 0xff, ((0.35 + 0.65 * h) * 22.0 / i as f32) as u8));
        }
    }
    ui.painter().rect_filled(r, CornerRadius::same(radius), col);
    if enabled && matches!(kind, Kind::Purple | Kind::Pink) && r.width() > 120.0 {
        button_triangles(ui, r, lighten(base, 0.5), label);
    }
    ui.painter().text(r.center(), Align2::CENTER_CENTER, label, FontId::new(15.0, semibold()), if enabled { CONTENT1 } else { BACKGROUND1 });
}

/// A note box (SettingsNote): Dark2 background, Content2 text, 8 px padding.
pub fn note(ui: &mut Ui, s: &str) {
    let w = ui.available_width() - 22.0;
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        egui::Frame::new().fill(DARK2).corner_radius(CornerRadius::same(5)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
            ui.set_width(w - 16.0);
            ui.add(egui::Label::new(text(s, 14.0, CONTENT2).family(semibold())).wrap());
        });
    });
}

/// One entry of the sidebar: icon-less text with the 4 px selection bar on the left.
pub fn sidebar_item(ui: &mut Ui, label: &str, icon: usize, selected: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 46.0), Sense::click());
    let id = resp.id;
    let sel = ease(ui, id.with("sel"), selected);
    let hov = ease(ui, id.with("hov"), resp.hovered());
    let col = if selected { CONTENT1 } else { lerp_c(LIGHT3, LIGHT1, hov) };
    let pill = Rect::from_min_max(rect.left_top() + Vec2::new(14.0, 3.0), rect.right_bottom() - Vec2::new(8.0, 3.0));
    let fill_a = (hov * 22.0).max(sel * 44.0) as u8;
    if fill_a > 0 {
        ui.painter().rect_filled(pill, CornerRadius::same(8), Color32::from_rgba_unmultiplied(HIGHLIGHT1.r(), HIGHLIGHT1.g(), HIGHLIGHT1.b(), fill_a));
    }
    icon_glyph(ui.painter(), Pos2::new(rect.left() + 34.0, rect.center().y), icon, col);
    ui.painter().text(Pos2::new(rect.left() + 56.0, rect.center().y), Align2::LEFT_CENTER, label, FontId::new(16.0, if selected { semibold() } else { FontFamily::Proportional }), col);
    let h = 14.0 + 12.0 * sel;
    let bar = Rect::from_center_size(Pos2::new(rect.left() + 8.0, rect.center().y), Vec2::new(4.0, h));
    ui.painter().rect_filled(bar, CornerRadius::same(2), Color32::from_rgba_unmultiplied(HIGHLIGHT1.r(), HIGHLIGHT1.g(), HIGHLIGHT1.b(), (sel * 255.0) as u8));
    resp
}
