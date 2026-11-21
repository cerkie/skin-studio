//! The interface: a big live gameplay view with lazer-style settings overlaid on the left. Every control edits the
//! settings (or the playback options) and the view updates at once.
use egui::{Align2, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use engine::presets;
use serde_json::Value;

use crate::state;
use crate::studio::{Mode, PickKind, Studio, Tab, TABS};
use crate::theme::{self as th, pal::*, Kind};

pub const PANEL_W: f32 = 470.0;
const SIDEBAR_W: f32 = 152.0;

/// What the session looks like right now.
#[derive(Default, Clone)]
pub struct SessionView {
    pub loaded: bool,
    pub clock: f32,
    pub start: f32,
    pub end: f32,
    pub playing: bool,
    pub info: String,
}

/// What the interface asks the app to do to the session.
#[derive(Default)]
pub struct UiOut {
    /// Where the gameplay view goes, in points.
    pub viewport: Option<Rect>,
    pub toggle_play: bool,
    pub seek: Option<f32>,
    pub seek_by: Option<f32>,
    pub volume: Option<f32>,
    pub offset: Option<f32>,
    pub cursor: Option<f32>,
    pub hit_lighting: Option<bool>,
    pub snaking: Option<(bool, bool)>,
    pub hidden: Option<bool>,
    pub difficulty: Option<(Option<f32>, Option<f32>, bool)>,
    pub speed: Option<f32>,
}

/// A rounded bubble with a value, above `anchor` (bottom centre). Painted in the Tooltip layer so it never clips to a panel.
fn bubble(ui: &Ui, anchor: Pos2, text: &str) {
    let p = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, ui.scope_id().with("bubble")));
    let galley = p.layout_no_wrap(text.to_string(), FontId::new(14.0, FontFamily::Proportional), CONTENT1);
    let size = galley.size() + Vec2::new(16.0, 8.0);
    let screen = ui.ctx().content_rect();
    let x = (anchor.x - size.x * 0.5).clamp(screen.left() + 6.0, (screen.right() - size.x - 6.0).max(screen.left() + 6.0));
    let y = (anchor.y - size.y).max(screen.top() + 6.0);
    let rect = Rect::from_min_size(Pos2::new(x, y), size);
    p.rect_filled(rect.translate(Vec2::new(0.0, 2.0)).expand(1.0), CornerRadius::same(6), Color32::from_black_alpha(90));
    p.rect_filled(rect, CornerRadius::same(6), BACKGROUND6);
    p.rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, LIGHT4), egui::StrokeKind::Inside);
    p.galley(rect.min + Vec2::new(8.0, 4.0), galley, CONTENT1);
}

fn fmt_speed(v: f32) -> String {
    let s = format!("{:.2}", v);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A compact slider with a value bubble while it is dragged. Returns true when `f` (0..1) changed.
fn mini_slider(ui: &mut Ui, id: &str, area: Rect, f: &mut f32, tip: impl Fn(f32) -> String) -> bool {
    let resp = ui.interact(area, ui.scope_id().with(id), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let mut changed = false;
    if resp.dragged() || resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            *f = ((p.x - area.left() - 6.0) / (area.width() - 12.0).max(1.0)).clamp(0.0, 1.0);
            changed = true;
        }
    }
    let x = th::mini_track(ui, id, area, *f, resp.hovered() || resp.dragged());
    if resp.dragged() {
        bubble(ui, Pos2::new(x, area.top() - 2.0), &tip(*f));
    }
    changed
}

/// Messages from the app (export done, errors) in a card over the view: wrapped, never cut short, gone after a few seconds.
/// Returns `true` if the user clicked the toast to dismiss it.
fn toast(ui: &mut Ui, st: &mut Studio, view: Rect) -> bool {
    if st.status != st.status_shown {
        st.status_shown = st.status.clone();
        st.status_at = std::time::Instant::now();
    }
    let dur = (4.5 + st.status.len() as f32 * 0.045).clamp(5.0, 10.0);
    let age = st.status_at.elapsed().as_secs_f32();
    if st.status.is_empty() || age > dur {
        return false;
    }
    let a = ((age * 6.0).min(1.0) * ((dur - age) * 2.0).min(1.0)).clamp(0.0, 1.0);
    let is_err = st.status.starts_with("Could not") || st.status.starts_with("Failed") || st.status.starts_with("Error");
    let accent = if is_err { PINK3 } else { COLOR3 };
    let w = (view.width() - 48.0).clamp(220.0, 620.0);
    let galley = ui.painter().layout(st.status.clone(), FontId::new(14.0, FontFamily::Proportional), CONTENT1.gamma_multiply(a), w - 38.0);
    let size = galley.size() + Vec2::new(38.0, 24.0);
    let rect = Rect::from_min_size(Pos2::new(view.center().x - size.x * 0.5, view.bottom() - 20.0 - size.y), size);
    th::shadow(ui.painter(), rect, 10, (110.0 * a) as u8);
    ui.painter().rect_filled(rect, CornerRadius::same(10), Color32::from_rgba_unmultiplied(0x1c, 0x1a, 0x22, (240.0 * a) as u8));
    let bar = Rect::from_min_size(rect.min + Vec2::new(6.0, 7.0), Vec2::new(3.0, (rect.height() - 14.0).max(8.0)));
    ui.painter().rect_filled(bar, CornerRadius::same(2), accent.gamma_multiply(a));
    ui.painter().rect_stroke(rect, CornerRadius::same(10), Stroke::new(1.0, Color32::from_white_alpha((30.0 * a) as u8)), egui::StrokeKind::Inside);
    ui.painter().galley(rect.min + Vec2::new(18.0, 12.0), galley, CONTENT1);
    if ui.interact(rect, ui.scope_id().with("toast"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        st.status_at = std::time::Instant::now() - std::time::Duration::from_secs(15);
        return true;
    }
    false
}

fn fmt_time(ms: f32) -> String {
    let s = (ms.max(0.0) / 1000.0) as u32;
    format!("{}:{:02}", s / 60, s % 60)
}

pub fn draw(root: &mut Ui, st: &mut Studio, sv: &SessionView, out: &mut UiOut) {
    st.warm_logo_sound();
    // Capture before popups close on an outside click this frame so clicking the viewport to dismiss a popup doesn't also toggle play.
    let popup_was_open = egui::Popup::is_any_open(root.ctx());
    // ---- toolbar
    egui::Panel::top("toolbar").exact_size(56.0).frame(egui::Frame::new().fill(BACKGROUND6).inner_margin(egui::Margin::ZERO)).show(root, |ui| {
        let bar = ui.max_rect();
        ui.painter().line_segment([bar.left_bottom(), bar.right_bottom()], Stroke::new(1.0, Color32::from_white_alpha(10)));
        ui.horizontal_centered(|ui| {
            logo(ui, st);
            ui.add_space(16.0);
            ui.label(th::text("skin studio", 17.0, CONTENT1).family(th::bold()));
            ui.add_space(18.0);
            mode_switch(ui, st);
            ui.add_space(14.0);
            if st.mode == Mode::Skin {
                crate::skin_editor::toolbar(ui, st, bar.right() - 16.0);
            } else {
                if th::button(ui, "New", Kind::Ghost, true).on_hover_text("Start fresh with default settings").clicked() {
                    st.new_settings();
                }
                if th::button(ui, "Open", Kind::Ghost, true).on_hover_text("Open a saved settings file").clicked() {
                    st.pick(PickKind::OpenSettings);
                }
                if th::button(ui, "Save", Kind::Ghost, true).on_hover_text("Save settings (Ctrl+S)").clicked() {
                    st.save();
                }
                if st.dirty {
                    // unsaved changes
                    let (d, resp) = ui.allocate_exact_size(Vec2::new(14.0, 14.0), Sense::hover());
                    resp.on_hover_text("Unsaved changes");
                    ui.painter().circle_filled(d.center(), 3.5, Color32::from_rgb(0xff, 0xad, 0x4d));
                }
                ui.add_space(10.0);
                if th::button(ui, "Export images", Kind::Accent, true).on_hover_text("Export followpoint-N.png files to a folder").clicked() {
                    st.export();
                }
            }
            // the path (or what is being edited) in what is left of the bar, cut at its start so that its end is the part that shows, and
            // left out when there is no room for it
            let msg = if st.mode == Mode::Skin {
                crate::skin_editor::title(st)
            } else if let Some(p) = &st.path {
                p.to_string_lossy().to_string()
            } else {
                "Unsaved settings".to_string()
            };
            let left = ui.cursor().left() + 16.0;
            let room = bar.right() - 16.0 - left;
            if room > 80.0 {
                let font = FontId::new(13.0, th::light());
                let shown = th::fit_text_start(ui.painter(), &msg, &font, room);
                ui.painter().text(Pos2::new(bar.right() - 16.0, bar.center().y), Align2::RIGHT_CENTER, shown, font, LIGHT3);
            }
        });
    });

    // the skin editor has the rest of the window to itself: no gameplay, no motion editor
    if st.mode == Mode::Skin {
        crate::skin_editor::draw(root, st);
        let mut r = root.max_rect();
        r.max.y = (r.max.y - 30.0).max(r.min.y);
        toast(root, st, r);
        return;
    }

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
                let icon = match tab { Tab::Animation => 0, Tab::Timing => 2, Tab::Look => 4, Tab::Preview => 6, Tab::Export => 9 };
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
            egui::ScrollArea::vertical().id_salt(("fp-tab", st.tab as u8, !st.search.trim().is_empty())).auto_shrink([false, false]).show(ui, |ui| {
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
                    Tab::Look => tab_look(ui, st),
                    Tab::Preview => tab_preview(ui, st, out),
                    Tab::Export => tab_export(ui, st),
                }
                ui.add_space(40.0);
            });
        });
    });

    // the transport lives in a bar under the view
    egui::Panel::bottom("transport_bar").exact_size(66.0).frame(egui::Frame::new().fill(BACKGROUND6).inner_margin(egui::Margin::ZERO)).show(root, |ui| {
        let bar = ui.max_rect();
        ui.painter().line_segment([bar.left_top(), bar.right_top()], Stroke::new(1.0, Color32::from_white_alpha(10)));
        let tr = Rect::from_min_size(Pos2::new(bar.left() + 16.0, bar.top() + 6.0), Vec2::new(bar.width() - 32.0, 54.0));
        transport(ui, tr, st, sv, out);
    });

    // the motion editor, with the animation preview, is docked above the transport bar (it can be folded away)
    crate::motion::dock(root, st);

    // ---- the gameplay view fills the rest
    egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::TRANSPARENT)).show(root, |ui| {
        let rect = ui.max_rect();
        out.viewport = Some(rect);
        let clicked = ui.interact(rect, ui.scope_id().with("view"), Sense::click()).clicked();
        let (mods_clicked, mods_w) = gameplay_mods_chip(ui, rect, st, out);
        let toast_clicked = toast(ui, st, rect);
        if clicked && !mods_clicked && !toast_clicked && sv.loaded && !popup_was_open && !egui::Popup::is_any_open(ui.ctx()) {
            out.toggle_play = true;
        }
        if st.loading {
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, "Loading the map...", FontId::new(22.0, th::light()), CONTENT2);
        } else if !st.session_error.is_empty() {
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, &st.session_error, FontId::new(18.0, th::light()), CONTENT2);
        }
        if sv.loaded && !sv.info.is_empty() {
            let max_w = (rect.width() - mods_w - 44.0).max(80.0);
            let mut lines = sv.info.lines();
            if let Some(title) = lines.next() {
                let tf = FontId::new(13.5, th::semibold());
                let shown = th::fit_text(ui.painter(), title, &tf, max_w);
                ui.painter().text(Pos2::new(rect.left() + 16.0, rect.top() + 11.0), Align2::LEFT_TOP, shown, tf, Color32::from_white_alpha(190));
            }
            if let Some(meta) = lines.next() {
                let mf = FontId::new(12.0, th::light());
                let shown = th::fit_text(ui.painter(), meta, &mf, max_w);
                ui.painter().text(Pos2::new(rect.left() + 16.0, rect.top() + 29.0), Align2::LEFT_TOP, shown, mf, Color32::from_white_alpha(130));
            }
        }
    });

}

/// Top-right dropdown on the gameplay preview for HR/HD mod presets and custom CS/AR overrides.
/// Returns `(interacted, chip_width)`.
fn gameplay_mods_chip(ui: &mut Ui, view: Rect, st: &mut Studio, out: &mut UiOut) -> (bool, f32) {
    if view.width() < 220.0 || view.height() < 60.0 {
        return (false, 0.0);
    }
    let mut parts: Vec<String> = Vec::new();
    if st.cfg.mod_hr { parts.push("HR".into()); }
    if st.cfg.mod_hd { parts.push("HD".into()); }
    if st.cfg.cs_override_on { parts.push(format!("CS {:.1}", st.cfg.cs_override)); }
    if st.cfg.ar_override_on { parts.push(format!("AR {:.1}", st.cfg.ar_override)); }
    let active = !parts.is_empty();
    let label = if active { parts.join(" · ") } else { "Mods / Diff".to_string() };
    let font = FontId::new(13.0, th::semibold());
    let tw = ui.painter().layout_no_wrap(label.clone(), font.clone(), CONTENT1).size().x;
    let w = (tw + 32.0).clamp(96.0, (view.width() - 32.0).max(96.0));
    let chip = Rect::from_min_size(Pos2::new(view.right() - 14.0 - w, view.top() + 10.0), Vec2::new(w, 28.0));
    let resp = ui.interact(chip, ui.scope_id().with("gp-mods"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Preview mods (HR / HD) and custom CS / AR");
    let hov = th::ease(ui, resp.id.with("h"), resp.hovered());
    let bg = if active {
        th::lerp_c(COLOR3, th::lighten(COLOR3, 0.16), hov)
    } else {
        th::lerp_c(Color32::from_rgba_unmultiplied(0x1c, 0x1a, 0x24, 210), DARK3, hov)
    };
    ui.painter().rect_filled(chip, CornerRadius::same(6), bg);
    ui.painter().rect_stroke(chip, CornerRadius::same(6), Stroke::new(1.0, Color32::from_white_alpha(if active { 45 } else { 24 })), egui::StrokeKind::Inside);
    let shown = th::fit_text(ui.painter(), &label, &font, (w - 24.0).max(20.0));
    ui.painter().text(Pos2::new(chip.left() + 10.0, chip.center().y), Align2::LEFT_CENTER, shown, font, CONTENT1);
    let c = Pos2::new(chip.right() - 12.0, chip.center().y + 0.5);
    ui.painter().add(egui::Shape::line(vec![c + Vec2::new(-3.5, -2.0), c + Vec2::new(0.0, 2.0), c + Vec2::new(3.5, -2.0)], Stroke::new(1.6, CONTENT1)));
    egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).gap(6.0).show(|ui| {
        ui.set_width(300.0);
        ui.add_space(4.0);
        let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::hover());
        let gap = 6.0;
        let pw = (row.width() - 24.0 - gap * 2.0) / 3.0;
        let r_hr = Rect::from_min_size(Pos2::new(row.left() + 12.0, row.top()), Vec2::new(pw, 28.0));
        let r_hd = Rect::from_min_size(Pos2::new(r_hr.right() + gap, row.top()), Vec2::new(pw, 28.0));
        let r_rs = Rect::from_min_size(Pos2::new(r_hd.right() + gap, row.top()), Vec2::new(pw, 28.0));
        let mut diff_changed = false;
        if th::pill(ui, r_hr, "mod-hr", "HR", st.cfg.mod_hr, true, "Hard Rock: 1.3x CS, 1.4x AR (max 10), vertical flip") {
            st.cfg.mod_hr = !st.cfg.mod_hr;
            st.cfg.save();
            diff_changed = true;
        }
        if th::pill(ui, r_hd, "mod-hd", "HD", st.cfg.mod_hd, true, "Hidden: hides approach circles and fades hit circles out early") {
            st.cfg.mod_hd = !st.cfg.mod_hd;
            st.cfg.save();
            out.hidden = Some(st.cfg.mod_hd);
        }
        if th::pill(ui, r_rs, "mod-reset", "Reset", false, active, "Clear HR/HD and custom CS/AR overrides") {
            let need_diff = st.cfg.mod_hr || st.cfg.cs_override_on || st.cfg.ar_override_on;
            st.cfg.mod_hr = false;
            st.cfg.mod_hd = false;
            st.cfg.cs_override_on = false;
            st.cfg.ar_override_on = false;
            st.cfg.save();
            out.hidden = Some(false);
            if need_diff {
                diff_changed = true;
            }
        }
        ui.add_space(4.0);
        let mut cs_on = st.cfg.cs_override_on;
        if th::switch_row(ui, "cs_ov_on", "Custom circle size (CS)", &mut cs_on) {
            st.cfg.cs_override_on = cs_on;
            st.cfg.save();
            diff_changed = true;
        }
        let diff_dirty_id = ui.scope_id().with("diff-cfg-dirty");
        if st.cfg.cs_override_on {
            let mut v = st.cfg.cs_override;
            let txt = format!("CS {v:.1}");
            if th::value_slider_def(ui, "cs_ov", "Circle size", &txt, &mut v, (0.0, 10.0), (0.0, 10.0), 0.1, 1.0, Some(4.0)) {
                st.cfg.cs_override = (v * 10.0).round() / 10.0;
                ui.memory_mut(|m| m.data.insert_temp(diff_dirty_id, true));
                diff_changed = true;
            }
        }
        let mut ar_on = st.cfg.ar_override_on;
        if th::switch_row(ui, "ar_ov_on", "Custom approach rate (AR)", &mut ar_on) {
            st.cfg.ar_override_on = ar_on;
            st.cfg.save();
            diff_changed = true;
        }
        if st.cfg.ar_override_on {
            let mut v = st.cfg.ar_override;
            let txt = format!("AR {v:.1}");
            if th::value_slider_def(ui, "ar_ov", "Approach rate", &txt, &mut v, (0.0, 11.0), (0.0, 11.0), 0.1, 1.0, Some(9.0)) {
                st.cfg.ar_override = (v * 10.0).round() / 10.0;
                ui.memory_mut(|m| m.data.insert_temp(diff_dirty_id, true));
                diff_changed = true;
            }
        }
        if !ui.input(|i| i.pointer.any_down()) && ui.memory(|m| m.data.get_temp::<bool>(diff_dirty_id)).unwrap_or(false) {
            ui.memory_mut(|m| m.data.remove::<bool>(diff_dirty_id));
            st.cfg.save();
        }
        if diff_changed {
            out.difficulty = Some((st.cfg.cs_override_on.then_some(st.cfg.cs_override), st.cfg.ar_override_on.then_some(st.cfg.ar_override), st.cfg.mod_hr));
        }
        ui.add_space(4.0);
    });
    (resp.clicked() || resp.hovered(), w)
}

/// Followpoints or Skin editor: the two halves of the application, as two tabs in the toolbar.
fn mode_switch(ui: &mut Ui, st: &mut Studio) {
    for (n, (m, label, tip)) in [(Mode::Followpoints, "Followpoints", "Make followpoint animations and see them on a map"), (Mode::Skin, "Skin editor", "Edit the pictures of a skin")].into_iter().enumerate() {
        if n > 0 {
            ui.add_space(12.0);
        }
        let galley = ui.painter().layout_no_wrap(label.to_string(), FontId::new(15.0, th::semibold()), CONTENT1);
        let (r, resp) = ui.allocate_exact_size(Vec2::new(galley.size().x + 36.0, 34.0), Sense::click());
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip);
        let on = st.mode == m;
        // the same animation as the entries of the sidebar: the pill fades in with the pointer and more with the selection, the text changes
        // colour with it, and the bar under the selected tab grows out from the middle
        let hov = th::ease(ui, resp.id.with("hov"), resp.hovered());
        let sel = th::ease(ui, resp.id.with("sel"), on);
        let col = if on { CONTENT1 } else { th::lerp_c(LIGHT3, LIGHT1, hov) };
        let fill_a = (hov * 22.0).max(sel * 44.0) as u8;
        if fill_a > 0 {
            ui.painter().rect_filled(r, CornerRadius::same(8), Color32::from_rgba_unmultiplied(HIGHLIGHT1.r(), HIGHLIGHT1.g(), HIGHLIGHT1.b(), fill_a));
        }
        let bar_w = (r.width() - 24.0) * sel;
        if bar_w > 0.5 {
            ui.painter().rect_filled(Rect::from_center_size(Pos2::new(r.center().x, r.bottom() - 2.0), Vec2::new(bar_w, 3.0)), CornerRadius::same(2), Color32::from_rgba_unmultiplied(HIGHLIGHT1.r(), HIGHLIGHT1.g(), HIGHLIGHT1.b(), (sel * 255.0) as u8));
        }
        ui.painter().galley(r.center() - galley.size() * 0.5, galley, col);
        if resp.clicked() {
            st.mode = m;
        }
    }
    ui.add_space(8.0);
    ui.painter().line_segment([Pos2::new(ui.cursor().left(), ui.max_rect().center().y - 12.0), Pos2::new(ui.cursor().left(), ui.max_rect().center().y + 12.0)], Stroke::new(1.0, Color32::from_white_alpha(20)));
}

/// The logo at the left of the toolbar. Pressing it plays the hit sound of the skin that is picked and the logo bounces: it jumps,
/// lands squashed, and settles after a few smaller hops, with a ring of light spreading from it.
fn logo(ui: &mut Ui, st: &mut Studio) {
    // The logo is made for the screen it is on: the 512 pixel picture is averaged down to just above the size it is shown at, so that
    // the graphics card only has a little to shrink (a big picture squeezed by a small factor is what looks jagged)
    let want = ((50.0 * ui.ctx().pixels_per_point()).round() as usize).clamp(24, 512);
    if st.logo_tex.is_none() || st.logo_px != want {
        let mut dec = png::Decoder::new(std::io::Cursor::new(&include_bytes!("../assets/icon-512.png")[..]));
        dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        if let Ok(mut r) = dec.read_info() {
            if let Some(len) = r.output_buffer_size() {
                let mut buf = vec![0; len];
                if let Ok(info) = r.next_frame(&mut buf) {
                    if info.color_type == png::ColorType::Rgba {
                        let big = engine::img::Img::from_straight_u8(info.width as usize, info.height as usize, &buf[..(info.width * info.height * 4) as usize]);
                        let small = engine::edit::resize(&big, want, want, engine::edit::Resample::Smooth);
                        let img = egui::ColorImage::from_rgba_premultiplied([want, want], &small.to_premult_u8());
                        st.logo_tex = Some(ui.ctx().load_texture("logo", img, egui::TextureOptions::LINEAR));
                        st.logo_px = want;
                    }
                }
            }
        }
    }
    let (slot, resp) = ui.allocate_exact_size(Vec2::new(56.0, 56.0), Sense::click());
    // nothing says that it can be pressed: the cursor stays the ordinary one and the logo does not react to the pointer
    if resp.clicked() {
        st.logo_pressed = Some(std::time::Instant::now());
        let vol = st.effective_volume();
        if vol <= 0.0 {
            st.status = "The sound is off: turn it on to hear the hit sound".into();
        } else {
            match st.skin_hitsound() {
                // a hit sound is short and dry: a little above the music's own setting, never loud
                Some(path) => {
                    st.sfx.play_file(&path, (vol * 2.5).min(0.6));
                }
                None => st.status = "This skin has no hit sound of its own".into(),
            }
        }
    }
    let since = st.logo_pressed.map(|t| t.elapsed().as_secs_f32());
    let (mut lift, mut sx, mut sy) = (0.0f32, 1.0f32, 1.0f32);
    let hover = 1.0;
    if let Some(t) = since.filter(|t| *t < 1.4) {
        // hops that get lower and quicker to die out; at every landing the logo is squashed flat for a moment, in the air it is stretched
        let hop = (t * 9.0).sin().abs();
        let decay = (-t * 3.4).exp();
        lift = 15.0 * hop * decay;
        let air = (lift / 6.0).min(1.0);
        let land = (1.0 - hop * 6.0).max(0.0) * decay.sqrt(); // 1 right at a landing, 0 a moment later
        sx = 1.0 + 0.22 * land - 0.06 * air * decay;
        sy = 1.0 - 0.22 * land + 0.09 * air * decay;
        ui.ctx().request_repaint();
    }
    let c = slot.center() + Vec2::new(0.0, 3.0); // the logo's feet are 3 points below the middle of the slot: it hops from there
    // a ring that spreads from the logo and fades
    if let Some(t) = since.filter(|t| *t < 0.55) {
        let u = t / 0.55;
        ui.painter().circle_stroke(slot.center(), 14.0 + 20.0 * (1.0 - (1.0 - u) * (1.0 - u)), Stroke::new(2.5 * (1.0 - u) + 0.5, Color32::from_rgba_unmultiplied(255, 95, 158, ((1.0 - u) * 170.0) as u8)));
    }
    if let Some(tex) = &st.logo_tex {
        let (hw, hh) = (20.0 * hover * sx, 20.0 * hover * sy);
        // squashing keeps the feet on the ground: the middle moves down by what the height gave up
        let mid = c + Vec2::new(0.0, -lift + (20.0 - hh) - 3.0);
        let mut mesh = egui::Mesh::with_texture(tex.id());
        for (p, uv) in [(Pos2::new(mid.x - hw, mid.y - hh), Pos2::new(0.0, 0.0)), (Pos2::new(mid.x + hw, mid.y - hh), Pos2::new(1.0, 0.0)), (Pos2::new(mid.x + hw, mid.y + hh), Pos2::new(1.0, 1.0)), (Pos2::new(mid.x - hw, mid.y + hh), Pos2::new(0.0, 1.0))] {
            mesh.vertices.push(egui::epaint::Vertex { pos: p, uv, color: Color32::WHITE });
        }
        mesh.indices = vec![0, 1, 2, 0, 2, 3];
        ui.painter().add(egui::Shape::mesh(mesh));
    }
}

fn transport(ui: &mut Ui, r: Rect, st: &mut Studio, sv: &SessionView, out: &mut UiOut) {
    let loaded = sv.loaded;
    // play, a new map and the speed: the same size, the same gap
    let (bw, bh, bgap) = (44.0, 36.0, 8.0);
    let btn = Rect::from_min_size(Pos2::new(r.left() + 14.0, r.center().y - bh / 2.0), Vec2::new(bw, bh));
    let mut bresp = ui.interact(btn, ui.scope_id().with("play"), Sense::click());
    if loaded {
        bresp = bresp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(if sv.playing { "Pause (Space)" } else { "Play (Space)" });
    }
    let bhov = th::ease(ui, bresp.id.with("h"), bresp.hovered() && loaded);
    ui.painter().rect_filled(btn, CornerRadius::same(5), if !loaded { BACKGROUND4 } else { th::lerp_c(COLOR3, th::lighten(COLOR3, 0.2), bhov) });
    let play_fg = if loaded { Color32::WHITE } else { BACKGROUND1 };
    if loaded && sv.playing {
        for dx in [-5.0, 5.0] {
            ui.painter().rect_filled(Rect::from_center_size(btn.center() + Vec2::new(dx, 0.0), Vec2::new(4.0, 14.0)), 1.0, play_fg);
        }
    } else {
        let c = btn.center();
        ui.painter().add(egui::Shape::convex_polygon(vec![c + Vec2::new(-5.0, -8.0), c + Vec2::new(-5.0, 8.0), c + Vec2::new(8.0, 0.0)], play_fg, Stroke::NONE));
    }
    if loaded && bresp.clicked() {
        out.toggle_play = true;
    }
    // a new random map, next to play (the die)
    let nm = Rect::from_min_size(Pos2::new(btn.right() + bgap, btn.top()), Vec2::new(bw, bh));
    let mut nresp = ui.interact(nm, ui.scope_id().with("newmap"), Sense::click());
    if loaded {
        nresp = nresp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Play a different random map");
    }
    let nhov = th::ease(ui, nresp.id.with("h"), nresp.hovered() && loaded);
    ui.painter().rect_filled(nm, CornerRadius::same(5), if !loaded { BACKGROUND4 } else { th::lerp_c(DARK2, DARK3, nhov) });
    {
        let c = nm.center();
        let die_fg = if loaded { CONTENT1 } else { BACKGROUND1 };
        let sq = Rect::from_center_size(c, Vec2::new(16.0, 16.0));
        ui.painter().rect_stroke(sq, CornerRadius::same(3), Stroke::new(1.8, die_fg), egui::StrokeKind::Middle);
        for (dx, dy) in [(-4.0, -4.0), (4.0, 4.0), (0.0, 0.0), (4.0, -4.0), (-4.0, 4.0)] {
            ui.painter().circle_filled(c + Vec2::new(dx, dy), 1.6, die_fg);
        }
    }
    if loaded && nresp.clicked() {
        st.new_map();
    }
    // playback speed: a chip that opens a slider
    let sp = Rect::from_min_size(Pos2::new(nm.right() + bgap, btn.top()), Vec2::new(bw, bh));
    let mut spresp = ui.interact(sp, ui.scope_id().with("speed"), Sense::click());
    if loaded {
        spresp = spresp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Playback speed");
    }
    let sphov = th::ease(ui, spresp.id.with("h"), spresp.hovered() && loaded);
    ui.painter().rect_filled(sp, CornerRadius::same(5), if !loaded { BACKGROUND4 } else { th::lerp_c(DARK2, DARK3, sphov) });
    let sp_fg = if !loaded { BACKGROUND1 } else if (st.cfg.speed - 1.0).abs() < 0.001 { CONTENT2 } else { HIGHLIGHT1 };
    ui.painter().text(sp.center(), Align2::CENTER_CENTER, format!("{}x", fmt_speed(st.cfg.speed)), FontId::new(14.0, FontFamily::Proportional), sp_fg);
    if !loaded {
        return;
    }
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
        let presets = [0.25f32, 0.5, 0.75, 1.0, 1.5];
        let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::hover());
        let gap = 5.0;
        let pw = (row.width() - gap * (presets.len() as f32 - 1.0)) / presets.len() as f32;
        for (i, v) in presets.into_iter().enumerate() {
            let pr = Rect::from_min_size(Pos2::new(row.left() + i as f32 * (pw + gap), row.top()), Vec2::new(pw, 28.0));
            let lbl = format!("{}x", fmt_speed(v));
            let on = (st.cfg.speed - v).abs() < 0.005;
            if th::pill(ui, pr, &format!("sp-{i}"), &lbl, on, true, "") {
                st.cfg.speed = v;
                st.cfg.save();
                out.speed = Some(v);
            }
        }
        ui.add_space(4.0);
    });
    // right side, from the edge inwards: time, volume slider, mute button
    // (what does not fit in a narrow window is left out: first the volume and the position bar, then the time)
    let right_room = r.right() - sp.right() - 30.0;
    if right_room < 100.0 {
        return;
    }
    let time = format!("{} / {}", fmt_time(sv.clock - sv.start), fmt_time(sv.end - sv.start));
    ui.painter().text(Pos2::new(r.right() - 14.0, r.center().y), Align2::RIGHT_CENTER, time, FontId::new(14.0, FontFamily::Proportional), CONTENT2);
    if right_room < 320.0 {
        return;
    }
    let vol_w = 108.0;
    let vol = Rect::from_min_max(Pos2::new(r.right() - 14.0 - 96.0 - vol_w, r.center().y - 12.0), Pos2::new(r.right() - 14.0 - 96.0 - 8.0, r.center().y + 12.0));
    let mute = Rect::from_center_size(Pos2::new(vol.left() - 22.0, r.center().y), Vec2::new(32.0, 32.0));
    let mresp = ui.interact(mute, ui.scope_id().with("mute"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(if st.cfg.muted { "Unmute" } else { "Mute" });
    let mhov = th::ease(ui, mresp.id.with("h"), mresp.hovered());
    let col = th::lerp_c(LIGHT1, CONTENT1, mhov);
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
    let vresp = ui.interact(vol, ui.scope_id().with("vol"), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::PointingHand);
    if vresp.dragged() || vresp.clicked() {
        if let Some(p) = vresp.interact_pointer_pos() {
            let v = ((p.x - vol.left() - 6.0) / (vol.width() - 12.0).max(1.0)).clamp(0.0, 1.0);
            st.cfg.volume = v;
            st.cfg.muted = v <= 0.0;
            st.cfg.save();
            out.volume = Some(st.effective_volume());
        }
    }
    let shown = if st.cfg.muted { 0.0 } else { st.cfg.volume };
    let vx = th::mini_track(ui, "vol", vol, shown, vresp.hovered() || vresp.dragged());
    if vresp.dragged() {
        bubble(ui, Pos2::new(vx, vol.top() - 6.0), &format!("{:.0}%", shown * 100.0));
    }
    let bar = Rect::from_min_max(Pos2::new(sp.right() + 22.0, r.center().y - 14.0), Pos2::new(mute.left() - 22.0, r.center().y + 14.0));
    // (in a very narrow window there is no room for a position bar between the buttons and the volume)
    if bar.width() < 80.0 {
        return;
    }
    let resp = ui.interact(bar, ui.scope_id().with("seek"), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let f = if sv.end > sv.start { ((sv.clock - sv.start) / (sv.end - sv.start)).clamp(0.0, 1.0) } else { 0.0 };
    th::mini_track(ui, "seek", bar, f, resp.hovered() || resp.dragged());
    if resp.hovered() || resp.dragged() {
        if let Some(p) = resp.interact_pointer_pos().or_else(|| ui.input(|i| i.pointer.hover_pos())) {
            let hf = ((p.x - bar.left() - 6.0) / (bar.width() - 12.0).max(1.0)).clamp(0.0, 1.0);
            let hx = bar.left() + 6.0 + (bar.width() - 12.0).max(1.0) * hf;
            bubble(ui, Pos2::new(hx, bar.top() - 4.0), &fmt_time((sv.end - sv.start) * hf));
        }
    }
    if resp.dragged() || resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            out.seek = Some(((p.x - bar.left() - 6.0) / (bar.width() - 12.0).max(1.0)).clamp(0.0, 1.0));
        }
    }
}

const CARD_HEADER: f32 = 40.0;
const ANIM_ZOOMS: [f32; 10] = [0.5, 0.75, 1.0, 1.5, 2.0, 2.5, 3.0, 4.0, 6.0, 8.0];

fn next_anim_zoom(k: f32, up: bool, fit: f32) -> f32 {
    if up {
        ANIM_ZOOMS.iter().copied().find(|z| *z > k + 0.01 && *z <= fit + 0.01).unwrap_or(k)
    } else {
        ANIM_ZOOMS.iter().rev().copied().find(|z| *z < k - 0.01).unwrap_or(ANIM_ZOOMS[0])
    }
}

/// Stable canvas dimensions for the animation preview card: uses the configured canvas width/height (so toggling outline or
/// drop shadow does not abruptly resize the dock column), or the frame bounds for Seamless Line.
fn card_base_dims(st: &Studio) -> (usize, usize) {
    let (mw, mh) = st.frames.iter().flatten().fold((1usize, 1usize), |a, f| (a.0.max(f.w), a.1.max(f.h)));
    if st.is_seamless() {
        return (mw, mh);
    }
    let cw = st.settings["width"].as_u64().unwrap_or(0) as usize;
    let ch = st.settings["height"].as_u64().unwrap_or(0) as usize;
    let scale = if st.settings["hires"].as_bool().unwrap_or(false) { 2 } else { 1 };
    if cw > 0 && ch > 0 {
        (cw * scale, ch * scale)
    } else {
        (mw, mh)
    }
}

/// How many screen pixels each pixel of the animation gets, as `(now, max_zoom)` from `ANIM_ZOOMS`.
/// All zoom steps up to 800% are available; the card's outer box clamps cleanly to `anim_limit`.
fn card_scale(st: &Studio, _ppp: f32, _mw: usize, _mh: usize) -> (f32, f32) {
    let max_z = ANIM_ZOOMS[ANIM_ZOOMS.len() - 1];
    let k = st.anim_zoom.clamp(ANIM_ZOOMS[0], max_z);
    (k, max_z)
}

/// The least width the preview column needs for its controls (the play button, the frame counter, the speed button), in points.
pub(crate) fn card_min_width(st: &Studio) -> f32 {
    let digits = |v: usize| v.to_string().len() as f32;
    let chars = 2.0 * digits(st.frames.len()) + digits(st.fps.round() as usize) + 10.0;
    (12.0 + 38.0 + 12.0 + chars * 6.9 + 10.0 + 52.0 + 12.0).max(260.0)
}

/// The size of the animation card: the configured canvas at its zoom, clamped to `anim_limit` and at least `card_min_width`.
pub(crate) fn anim_card_size(ctx: &egui::Context, st: &Studio) -> Vec2 {
    let (bw, bh) = card_base_dims(st);
    let ppp = ctx.pixels_per_point();
    let k = card_scale(st, ppp, bw, bh).0;
    let (iw, ih) = (bw as f32 * k / ppp, bh as f32 * k / ppp);
    let max_w = st.anim_limit.0.max(card_min_width(st));
    let max_h = st.anim_limit.1.max(CARD_HEADER + 40.0 + 90.0);
    Vec2::new((iw + 16.0).clamp(card_min_width(st), max_w), (CARD_HEADER + ih + 90.0).min(max_h))
}

/// The animation playing in a card: a title with its size and fold buttons, the frames, a scrub bar, and play, frame count and speed.
pub(crate) fn anim_card(ctx: &egui::Context, ui: &mut Ui, st: &mut Studio, origin: Pos2) {
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
        let img = egui::ColorImage::from_rgba_unmultiplied([mw, mh], &px);
        match &mut st.anim_tex {
            Some(tex) => tex.set(img, egui::TextureOptions::NEAREST),
            None => st.anim_tex = Some(ctx.load_texture("anim", img, egui::TextureOptions::NEAREST)),
        }
        st.anim_idx = idx;
        st.anim_gen = st.atlas_gen;
    }
    let Some(tex) = st.anim_tex.clone() else { return };
    // Size the card from the stable base canvas, while drawing the actual (possibly outline/shadow-padded) frame centered inside it.
    let (bw, bh) = card_base_dims(st);
    let ppp = ctx.pixels_per_point();
    let (k, k_fit) = card_scale(st, ppp, bw, bh);
    let (box_w, box_h) = (bw as f32 * k / ppp, bh as f32 * k / ppp);
    let (iw, ih) = (mw as f32 * k / ppp, mh as f32 * k / ppp);
    // (the dock paints the column behind it: no card of its own)
    let card = Rect::from_min_size(origin, anim_card_size(ctx, st));
    let w = card.width();
    ui.painter().text(card.min + Vec2::new(8.0, CARD_HEADER * 0.5), Align2::LEFT_CENTER, "Preview", FontId::new(15.0, th::semibold()), CONTENT2);
    let btn = |ui: &mut Ui, id: &str, right: f32, enabled: bool, draw: &dyn Fn(&egui::Painter, Pos2, Color32), tip: &str| -> bool {
        let r = Rect::from_center_size(Pos2::new(card.right() - right, card.top() + CARD_HEADER * 0.5), Vec2::new(30.0, 28.0));
        let mut resp = ui.interact(r, ui.scope_id().with(id), Sense::click()).on_hover_text(tip);
        if enabled {
            resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        let hov = th::ease(ui, resp.id.with("h"), resp.hovered() && enabled);
        if hov > 0.01 {
            ui.painter().rect_filled(r, CornerRadius::same(5), Color32::from_rgba_unmultiplied(DARK2.r(), DARK2.g(), DARK2.b(), (hov * 255.0) as u8));
        }
        draw(ui.painter(), r.center(), if !enabled { Color32::from_white_alpha(40) } else { th::lerp_c(LIGHT1, CONTENT1, hov) });
        enabled && resp.clicked()
    };
    let next_up = next_anim_zoom(k, true, k_fit);
    let next_dn = next_anim_zoom(k, false, k_fit);
    if btn(ui, "animbigger", 22.0, next_up > k + 0.01, &|p, c, col| {
        p.line_segment([c + Vec2::new(-5.0, 0.0), c + Vec2::new(5.0, 0.0)], Stroke::new(2.0, col));
        p.line_segment([c + Vec2::new(0.0, -5.0), c + Vec2::new(0.0, 5.0)], Stroke::new(2.0, col));
    }, &if next_up > k + 0.01 { format!("Show the animation larger ({}%)", (next_up * 100.0).round() as i32) } else { "Maximum zoom (800%)".to_string() }) {
        st.anim_zoom = next_up;
    }
    ui.painter().text(Pos2::new(card.right() - 62.0, card.top() + CARD_HEADER * 0.5), Align2::CENTER_CENTER, format!("{}%", (k * 100.0).round() as i32), FontId::new(13.0, th::semibold()), LIGHT1);
    if btn(ui, "animsmaller", 102.0, next_dn < k - 0.01, &|p, c, col| {
        p.line_segment([c + Vec2::new(-5.0, 0.0), c + Vec2::new(5.0, 0.0)], Stroke::new(2.0, col));
    }, &if next_dn < k - 0.01 { format!("Show the animation smaller ({}%)", (next_dn * 100.0).round() as i32) } else { "Smallest zoom".to_string() }) {
        st.anim_zoom = next_dn;
    }
    // Snap preview box edges to exact physical pixels and draw the 1-physical-pixel border in screen space
    // so fractional zooms (150%, 75%) stay crisp and uniform and outline/shadow never clips the border.
    let snap = |v: f32| (v * ppp).round() / ppp;
    let stage_h = (card.height() - CARD_HEADER - 90.0).max(1.0);
    let stage = Rect::from_min_size(Pos2::new(card.left() + 8.0, card.top() + CARD_HEADER), Vec2::new((w - 16.0).max(1.0), stage_h));
    let raw_box = Rect::from_center_size(stage.center(), Vec2::new(box_w.min(stage.width()), box_h.min(stage.height())));
    let box_rect = Rect::from_min_max(Pos2::new(snap(raw_box.min.x), snap(raw_box.min.y)), Pos2::new(snap(raw_box.max.x), snap(raw_box.max.y)));
    ui.painter().rect_filled(box_rect, CornerRadius::ZERO, Color32::from_rgb(0x18, 0x17, 0x1c));
    let fit = Rect::from_center_size(box_rect.center(), Vec2::new(iw, ih));
    let fit = Rect::from_min_size(Pos2::new(snap(fit.min.x), snap(fit.min.y)), fit.size());
    ui.painter().with_clip_rect(box_rect).image(tex.id(), fit, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
    ui.painter().rect_stroke(box_rect, CornerRadius::ZERO, Stroke::new(1.0 / ppp, Color32::from_rgb(0x50, 0x47, 0x6b)), egui::StrokeKind::Inside);
    // a scrub bar over the frames of the animation
    let bar = Rect::from_min_size(Pos2::new(card.left() + 12.0, card.bottom() - 82.0), Vec2::new(w - 24.0, 24.0));
    let sresp = ui.interact(bar, ui.scope_id().with("animseek"), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::PointingHand);
    st.anim_scrub = sresp.dragged();
    if sresp.dragged() || sresp.clicked() {
        if let Some(p) = sresp.interact_pointer_pos() {
            let f = ((p.x - bar.left() - 6.0) / (bar.width() - 12.0).max(1.0)).clamp(0.0, 0.999);
            st.anim_pos = f * loop_ms;
        }
    }
    th::mini_track(ui, "animseek", bar, st.anim_pos / loop_ms, sresp.hovered() || sresp.dragged());
    if sresp.dragged() {
        let fx = bar.left() + 6.0 + (bar.width() - 12.0).max(1.0) * (st.anim_pos / loop_ms).clamp(0.0, 1.0);
        bubble(ui, Pos2::new(fx, bar.top() - 4.0), &format!("{} ms", st.anim_pos.round() as i32));
    }
    // play / pause (a round button), the frame count, and the speed
    let ctrl = Rect::from_min_size(Pos2::new(card.left() + 12.0, card.bottom() - 48.0), Vec2::new(w - 24.0, 38.0));
    let play = Rect::from_min_size(ctrl.min, Vec2::new(38.0, 38.0));
    let resp = ui.interact(play, ui.scope_id().with("animplay"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(if st.anim_playing { "Pause the preview (Space)" } else { "Play the preview (Space)" });
    let phov = th::ease(ui, resp.id.with("h"), resp.hovered());
    let fill = th::lerp_c(COLOR3, th::lighten(COLOR3, 0.18), phov);
    ui.painter().circle_filled(play.center(), 19.0, fill);
    let c = play.center();
    if st.anim_playing {
        for dx in [-4.5, 4.5] {
            ui.painter().rect_filled(Rect::from_center_size(c + Vec2::new(dx, 0.0), Vec2::new(4.0, 14.0)), 1.5, Color32::WHITE);
        }
    } else {
        ui.painter().add(egui::Shape::convex_polygon(vec![c + Vec2::new(-5.0, -8.0), c + Vec2::new(-5.0, 8.0), c + Vec2::new(9.0, 0.0)], Color32::WHITE, Stroke::NONE));
    }
    if resp.clicked() {
        st.anim_playing = !st.anim_playing;
    }
    ui.painter().text(Pos2::new(play.right() + 12.0, ctrl.center().y - 8.0), Align2::LEFT_CENTER, if st.anim_playing { "Playing" } else { "Paused" }, FontId::new(14.0, th::semibold()), CONTENT1);
    // the frame number, the count and the rate each have a place of their own, so that nothing shifts when the digits change
    {
        let font = FontId::new(11.5, FontFamily::Proportional);
        let wide = ui.painter().layout_no_wrap("8".repeat(n.max(1).to_string().len()), font.clone(), LIGHT3).size().x;
        let (x0, y) = (play.right() + 12.0, ctrl.center().y + 9.0);
        ui.painter().text(Pos2::new(x0 + wide, y), Align2::RIGHT_CENTER, format!("{}", idx + 1), font.clone(), LIGHT3);
        ui.painter().text(Pos2::new(x0 + wide, y), Align2::LEFT_CENTER, format!("/{}  ·  {} fps", n.max(1), st.fps.round() as i32), font, LIGHT3);
    }
    let chip = Rect::from_min_size(Pos2::new(ctrl.right() - 52.0, ctrl.center().y - 14.0), Vec2::new(52.0, 28.0));
    let cresp = ui.interact(chip, ui.scope_id().with("animspeed"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Animation playback speed");
    let chov = th::ease(ui, cresp.id.with("h"), cresp.hovered());
    ui.painter().rect_filled(chip, CornerRadius::same(5), th::lerp_c(DARK2, DARK3, chov));
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
        let presets = [0.1f32, 0.25, 0.5, 1.0, 2.0];
        let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::hover());
        let gap = 5.0;
        let pw = (row.width() - gap * (presets.len() as f32 - 1.0)) / presets.len() as f32;
        for (i, v) in presets.into_iter().enumerate() {
            let pr = Rect::from_min_size(Pos2::new(row.left() + i as f32 * (pw + gap), row.top()), Vec2::new(pw, 28.0));
            let lbl = format!("{}x", fmt_speed(v));
            let on = (st.anim_speed - v).abs() < 0.005;
            if th::pill(ui, pr, &format!("asp-{i}"), &lbl, on, true, "") {
                st.anim_speed = v;
            }
        }
        ui.add_space(4.0);
    });
}

// ------------------------------------------------------------------------------------------------ form helpers
fn num(ui: &mut Ui, st: &mut Studio, id: &str, label: &str) {
    let cur = state::fmt_num(&st.settings[id]);
    let buf = st.bufs.entry(id.to_string()).or_insert_with(|| cur.clone());
    let is_color = id.ends_with("_color");
    let (resp, row_rect) = th::text_row_pad(ui, id, label, buf, if is_color { 34.0 } else { 0.0 });
    let text = buf.clone();
    if resp.changed() {
        st.set_field(id, &text);
    } else if !resp.has_focus() && text != cur {
        st.bufs.insert(id.to_string(), cur.clone());
    }
    if is_color {
        let rgb = state::hex_to_rgb(&text).or_else(|| state::hex_to_rgb(&cur)).unwrap_or([255, 255, 255]);
        let sw = Rect::from_center_size(Pos2::new(row_rect.right() - 22.0, row_rect.center().y), Vec2::new(24.0, 24.0));
        if let Some(c) = swatch_picker(ui, id, sw, rgb) {
            let hex = format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
            st.set_field(id, &hex);
            st.bufs.insert(id.to_string(), hex);
        }
    }
}

/// A Followpoints JSON numeric setting backed by the standard `value_slider_def` (drag, arrow keys, double-click to reset, or click-to-type).
#[allow(clippy::too_many_arguments)]
fn slider_field(ui: &mut Ui, st: &mut Studio, id: &str, label: &str, lo: f32, hi: f32, hard: (f32, f32), step: f32, display: f32, fmt: impl Fn(f32) -> String) {
    let cur = st.settings[id].as_f64().unwrap_or(lo as f64) as f32;
    let def_v = presets::default_settings().get(id).and_then(|x| x.as_f64()).map(|x| x as f32);
    let mut v = cur;
    let text = fmt(v);
    if th::value_slider_def(ui, id, label, &text, &mut v, (lo, hi), hard, step, display, def_v) {
        let s = if step >= 1.0 { format!("{}", v.round() as i64) } else { format!("{v:.3}") };
        st.set_field(id, &s);
        st.bufs.remove(id);
    }
}

fn rgb_to_hsv(rgb: [u8; 3]) -> (f32, f32, f32) {
    let (r, g, b) = (rgb[0] as f32 / 255.0, rgb[1] as f32 / 255.0, rgb[2] as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d < 1e-6 {
        0.0
    } else if (max - r).abs() < 1e-6 {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if (max - g).abs() < 1e-6 {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    let s = if max < 1e-6 { 0.0 } else { d / max };
    (h, s, max)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let h = h.rem_euclid(1.0) * 6.0;
    let i = h.floor() as i32;
    let f = h - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    let (r, g, b) = match i.rem_euclid(6) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [(r * 255.0).round().clamp(0.0, 255.0) as u8, (g * 255.0).round().clamp(0.0, 255.0) as u8, (b * 255.0).round().clamp(0.0, 255.0) as u8]
}

/// Custom large osu!lazer-styled HSV color picker popup (296 px wide: big 2D SV field, thick rounded hue bar with pill marker,
/// live preview swatch, editable hex input, and RGB readout).
fn custom_color_picker(ui: &mut Ui, id: egui::Id, rgb: [u8; 3]) -> Option<[u8; 3]> {
    // Keep HSV in temp memory so dragging S=0 or V=0 never loses the current Hue.
    let hsv_id = id.with("hsv");
    let last_id = id.with("last_rgb");
    let last_rgb = ui.memory(|m| m.data.get_temp::<[u8; 3]>(last_id));
    let (mut h, mut s, mut v) = if last_rgb == Some(rgb) {
        ui.memory(|m| m.data.get_temp::<(f32, f32, f32)>(hsv_id)).unwrap_or_else(|| rgb_to_hsv(rgb))
    } else {
        rgb_to_hsv(rgb)
    };
    let mut changed = false;

    ui.set_width(304.0);
    ui.add_space(8.0);

    // 1. Large 2D Saturation × Value gradient field (284 x 210, 10 px side margin so 8.5 px ring never clips)
    let (sv_outer, _) = ui.allocate_exact_size(Vec2::new(304.0, 210.0), Sense::hover());
    let sv_rect = Rect::from_min_size(sv_outer.min + Vec2::new(10.0, 0.0), Vec2::new(284.0, 210.0));
    let sv_resp = ui.interact(sv_rect, id.with("sv"), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::Crosshair);
    if sv_resp.dragged() || sv_resp.clicked() || sv_resp.is_pointer_button_down_on() {
        if let Some(p) = sv_resp.interact_pointer_pos() {
            s = ((p.x - sv_rect.left()) / sv_rect.width().max(1.0)).clamp(0.0, 1.0);
            v = (1.0 - (p.y - sv_rect.top()) / sv_rect.height().max(1.0)).clamp(0.0, 1.0);
            changed = true;
        }
    }
    // Paint smooth SV mesh (16x16 grid for crisp sRGB interpolation)
    {
        let mut mesh = egui::Mesh::default();
        let cols = 16usize;
        let rows = 16usize;
        for ry in 0..=rows {
            let vy = 1.0 - ry as f32 / rows as f32;
            let py = sv_rect.top() + sv_rect.height() * (ry as f32 / rows as f32);
            for rx in 0..=cols {
                let sx = rx as f32 / cols as f32;
                let px = sv_rect.left() + sv_rect.width() * sx;
                let c = hsv_to_rgb(h, sx, vy);
                mesh.colored_vertex(Pos2::new(px, py), Color32::from_rgb(c[0], c[1], c[2]));
            }
        }
        let stride = (cols + 1) as u32;
        for ry in 0..rows as u32 {
            for rx in 0..cols as u32 {
                let i = ry * stride + rx;
                mesh.add_triangle(i, i + 1, i + stride);
                mesh.add_triangle(i + 1, i + stride + 1, i + stride);
            }
        }
        ui.painter().add(egui::Shape::mesh(mesh));
        ui.painter().rect_stroke(sv_rect, CornerRadius::ZERO, Stroke::new(1.0, Color32::from_white_alpha(28)), egui::StrokeKind::Inside);
        let cx = sv_rect.left() + s * sv_rect.width();
        let cy = sv_rect.top() + (1.0 - v) * sv_rect.height();
        ui.painter().circle_stroke(Pos2::new(cx, cy), 8.5, Stroke::new(2.2, Color32::from_black_alpha(200)));
        ui.painter().circle_stroke(Pos2::new(cx, cy), 6.8, Stroke::new(2.0, Color32::WHITE));
    }

    ui.add_space(10.0);

    // 2. Thick Hue bar (284 x 24) with slim off-white vertical pill marker
    let (hue_outer, _) = ui.allocate_exact_size(Vec2::new(304.0, 24.0), Sense::hover());
    let hue_rect = Rect::from_min_size(hue_outer.min + Vec2::new(10.0, 0.0), Vec2::new(284.0, 24.0));
    let hue_resp = ui.interact(hue_rect, id.with("hue"), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let room = 14.0;
    if hue_resp.dragged() || hue_resp.clicked() || hue_resp.is_pointer_button_down_on() {
        if let Some(p) = hue_resp.interact_pointer_pos() {
            h = ((p.x - hue_rect.left() - room * 0.5) / (hue_rect.width() - room).max(1.0)).clamp(0.0, 1.0);
            changed = true;
        }
    }
    {
        let stops = 24usize;
        let mut mesh = egui::Mesh::default();
        for i in 0..=stops {
            let f = i as f32 / stops as f32;
            let px = hue_rect.left() + hue_rect.width() * f;
            let c = hsv_to_rgb(f, 1.0, 1.0);
            let col = Color32::from_rgb(c[0], c[1], c[2]);
            mesh.colored_vertex(Pos2::new(px, hue_rect.top()), col);
            mesh.colored_vertex(Pos2::new(px, hue_rect.bottom()), col);
        }
        for i in 0..stops as u32 {
            let k = i * 2;
            mesh.add_triangle(k, k + 2, k + 1);
            mesh.add_triangle(k + 2, k + 3, k + 1);
        }
        ui.painter().add(egui::Shape::mesh(mesh));
        ui.painter().rect_stroke(hue_rect, CornerRadius::ZERO, Stroke::new(1.0, Color32::from_white_alpha(28)), egui::StrokeKind::Inside);
        let hx = hue_rect.left() + room * 0.5 + (hue_rect.width() - room) * h;
        let marker = Rect::from_center_size(Pos2::new(hx, hue_rect.center().y), Vec2::new(8.0, 18.0));
        ui.painter().rect_filled(marker.expand(1.0), CornerRadius::same(4), Color32::from_black_alpha(150));
        ui.painter().rect_filled(marker, CornerRadius::same(3), Color32::WHITE);
    }

    ui.add_space(10.0);

    // 3. Bottom bar: large preview swatch + editable hex input + RGB readout
    let mut cur_rgb = if changed { hsv_to_rgb(h, s, v) } else { rgb };
    let (bot_outer, _) = ui.allocate_exact_size(Vec2::new(304.0, 34.0), Sense::hover());
    let bot = Rect::from_min_size(bot_outer.min + Vec2::new(10.0, 0.0), Vec2::new(284.0, 32.0));
    let preview_rect = Rect::from_min_size(bot.min, Vec2::new(48.0, 32.0));
    ui.painter().rect_filled(preview_rect, CornerRadius::same(5), Color32::from_rgb(cur_rgb[0], cur_rgb[1], cur_rgb[2]));
    ui.painter().rect_stroke(preview_rect, CornerRadius::same(5), Stroke::new(1.0, Color32::from_white_alpha(38)), egui::StrokeKind::Inside);

    let hex_rect = Rect::from_min_size(Pos2::new(preview_rect.right() + 10.0, bot.top()), Vec2::new(98.0, 32.0));
    ui.painter().rect_filled(hex_rect, CornerRadius::same(5), CONTROL);
    ui.painter().rect_stroke(hex_rect, CornerRadius::same(5), Stroke::new(1.0, DARK3), egui::StrokeKind::Inside);
    let hex_id = id.with("hexbuf");
    let canonical = format!("#{:02x}{:02x}{:02x}", cur_rgb[0], cur_rgb[1], cur_rgb[2]);
    let mut buf = ui.memory(|m| m.data.get_temp::<String>(hex_id)).unwrap_or_else(|| canonical.clone());
    if changed {
        buf = canonical.clone();
    }
    let hresp = th::put_free(
        ui,
        hex_rect.shrink2(Vec2::new(8.0, 6.0)),
        egui::TextEdit::singleline(&mut buf)
            .id(id.with("hexedit"))
            .frame(egui::Frame::NONE)
            .desired_width(hex_rect.width() - 16.0)
            .font(FontId::new(14.5, FontFamily::Proportional))
            .text_color(CONTENT1)
            .margin(egui::Margin::ZERO),
    );
    if hresp.changed() {
        if let Some(c) = state::hex_to_rgb(&buf) {
            cur_rgb = c;
            (h, s, v) = rgb_to_hsv(c);
            changed = true;
        }
    } else if !hresp.has_focus() {
        buf = canonical;
    }
    ui.memory_mut(|m| m.data.insert_temp(hex_id, buf));

    let rgb_text = format!("{}, {}, {}", cur_rgb[0], cur_rgb[1], cur_rgb[2]);
    ui.painter().text(Pos2::new(bot.right() - 2.0, bot.center().y), Align2::RIGHT_CENTER, rgb_text, FontId::new(13.0, th::light()), CONTENT2);
    ui.add_space(6.0);

    ui.memory_mut(|m| {
        m.data.insert_temp(hsv_id, (h, s, v));
        m.data.insert_temp(last_id, cur_rgb);
    });
    if changed { Some(cur_rgb) } else { None }
}

/// A color swatch that opens our custom large HSV color picker; returns the new color when it changes.
pub(crate) fn swatch_picker(ui: &mut Ui, id: &str, rect: Rect, rgb: [u8; 3]) -> Option<[u8; 3]> {
    let pid = ui.scope_id().with(("swatch", id));
    let resp = ui.interact(rect, pid, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Pick a color");
    let h = th::ease(ui, resp.id.with("h"), resp.hovered());
    ui.painter().rect_filled(rect, CornerRadius::same(4), Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
    ui.painter().rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0 + h, th::lerp_c(DARK3, LIGHT4, h)), egui::StrokeKind::Inside);
    let mut out = None;
    egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).gap(6.0).show(|ui| {
        out = custom_color_picker(ui, pid, rgb);
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
    (Tab::Timing, "Frame rate fps"), (Tab::Timing, "Speed"), (Tab::Timing, "Total duration"), (Tab::Timing, "Time shift"), (Tab::Timing, "Make the time shift the base"), (Tab::Timing, "Fit the speed to the window"), (Tab::Timing, "Appear after"), (Tab::Timing, "Disappear at"), (Tab::Timing, "Appear / disappear window"), (Tab::Timing, "Blank at the end"),
    (Tab::Timing, "Fade in"), (Tab::Timing, "Fade out"), (Tab::Timing, "Copies in a row"), (Tab::Timing, "Play forward and back"), (Tab::Timing, "Play once"),
    (Tab::Timing, "Color keys tint"), (Tab::Timing, "Add a color"), (Tab::Look, "Outline"), (Tab::Look, "Outline color"), (Tab::Look, "Drop shadow"), (Tab::Look, "Shadow blur"), (Tab::Look, "Debanding"), (Tab::Look, "Grain dither"),
    (Tab::Preview, "Interface size scale"), (Tab::Preview, "Skin"), (Tab::Preview, "Hit sounds"), (Tab::Preview, "Map"), (Tab::Preview, "Random maps lazer stable library"), (Tab::Preview, "Snaking sliders in out"), (Tab::Preview, "Hit lighting"), (Tab::Preview, "Sound"), (Tab::Preview, "Volume"), (Tab::Preview, "Music offset"), (Tab::Preview, "Playback speed"), (Tab::Preview, "Cursor size"),
    (Tab::Export, "Export sizes 1x 2x"), (Tab::Export, "Export images"), (Tab::Export, "History saved versions"), (Tab::Export, "Import followpoint images"),
];

fn search_results(ui: &mut Ui, st: &mut Studio) {
    let q = st.search.trim().to_lowercase().replace("color", "colour");
    th::section_header(ui, "Search");
    let mut hits = 0;
    for (tab, label) in SEARCHABLE {
        if !label.to_lowercase().replace("color", "colour").contains(&q) {
            continue;
        }
        hits += 1;
        let tab_name = TABS.iter().find(|(t, _)| t == tab).map(|(_, n)| *n).unwrap_or("");
        let w = ui.available_width() - 22.0;
        let (r, resp) = ui.allocate_exact_size(Vec2::new(w, 46.0 + th::ROW_GAP), Sense::click());
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        let r = Rect::from_min_size(r.min + Vec2::new(12.0, 0.0), Vec2::new(r.width(), r.height() - th::ROW_GAP));
        let h = th::ease(ui, resp.id.with("h"), resp.hovered());
        ui.painter().rect_filled(r, CornerRadius::same(5), th::lerp_c(CONTROL, DARK3, h));
        let lf = FontId::new(15.0, FontFamily::Proportional);
        let shown = th::fit_text(ui.painter(), label, &lf, (r.width() - 20.0).max(20.0));
        ui.painter().text(r.left_center() + Vec2::new(10.0, -8.0), Align2::LEFT_CENTER, shown, lf, CONTENT1);
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
        num(ui, st, "sl_cs", "Circle size (CS)");
        num(ui, st, "sl_ar", "Approach rate (AR)");
        slider_field(ui, st, "sl_soft", "Soften", 0.0, 150.0, (0.0, 1000.0), 1.0, 1.0, |v| format!("{:.0} ms", v));
        slider_field(ui, st, "sl_tight", "Tightness", 0.0, 1.0, (0.0, 1.0), 0.01, 100.0, |v| format!("{:.0}%", v * 100.0));
        slider_field(ui, st, "sl_thick", "Thickness", 1.0, 20.0, (0.5, 100.0), 0.5, 1.0, |v| format!("{v:.1} px"));
        slider_field(ui, st, "sl_alpha", "Opacity", 0.0, 1.0, (0.0, 1.0), 0.01, 100.0, |v| format!("{:.0}%", v * 100.0));
        slider_field(ui, st, "sl_overlap", "Tile overlap", 0.0, 1.0, (0.0, 1.0), 0.01, 100.0, |v| format!("{:.0}%", v * 100.0));
    }
}

fn tab_timing(ui: &mut Ui, st: &mut Studio) {
    th::subsection_header(ui, "Playback");
    num(ui, st, "fps", "Frame rate (fps, at most 1000)");
    num(ui, st, "speed", "Speed multiplier (1 = as designed)");
    duration_row(ui, st);
    num(ui, st, "shift", "Time shift (ms; - waits, + skips ahead)");
    {
        let shifted = engine::settings::Settings::from_json(&st.settings).shift.abs() >= 0.5;
        if th::button_wide(ui, "Make the time shift the base", Kind::Dark, shifted).clicked() {
            crate::motion::bake_shift(st);
        }
        th::note(ui, "Moves the shift into the keyframes: the animation looks the same, and the shift is 0 from then on, so the animation you adapted is the new starting point (undo is in the motion editor).");
    }
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
    // the tint changes over time like the rest of the motion (and has a lane in the motion editor)
    th::subsection_header(ui, "Color");
    tab_color(ui, st);
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

fn tab_look(ui: &mut Ui, st: &mut Studio) {
    tab_finish(ui, st);
}

fn tab_color(ui: &mut Ui, st: &mut Studio) {
    let mut on = st.settings["color_on"].as_bool().unwrap_or(false);
    if th::switch_row(ui, "color_on", "Tint the animation", &mut on) {
        st.motion.remember(&st.settings);
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
    slider_field(ui, st, "color_strength", "Strength", 0.0, 1.0, (0.0, 1.0), 0.01, 100.0, |v| format!("{:.0}%", v * 100.0));
    th::subsection_header(ui, "Color keys");
    let keys = presets::color_keys_of(&st.settings);
    for (i, k) in keys.iter().enumerate() {
        color_row(ui, st, i, k);
    }
    if th::button_wide(ui, "Add a color", Kind::Dark, true).clicked() {
        st.motion.remember(&st.settings);
        state::add_color(&mut st.settings);
        st.bufs.clear();
        st.edited();
    }
}

fn color_row(ui: &mut Ui, st: &mut Studio, i: usize, k: &Value) {
    let rid = ui.scope_id().with(("col", i));
    let width = ui.available_width() - 22.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 38.0 + th::ROW_GAP), Sense::hover());
    let rect = Rect::from_min_size(rect.min + Vec2::new(12.0, 0.0), Vec2::new(rect.width(), rect.height() - th::ROW_GAP));
    let row_hov = th::ease(ui, rid.with("rh"), ui.rect_contains_pointer(rect));
    ui.painter().rect_filled(rect, CornerRadius::same(5), th::lerp_c(CONTROL, DARK3, row_hov * 0.35));
    if row_hov > 0.01 {
        ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, Color32::from_rgba_unmultiplied(HIGHLIGHT1.r(), HIGHLIGHT1.g(), HIGHLIGHT1.b(), (row_hov * 110.0) as u8)), egui::StrokeKind::Inside);
    }
    let hex = k[1].as_str().unwrap_or("#ffffff").to_string();
    let rgb = state::hex_to_rgb(&hex).unwrap_or([255, 255, 255]);
    if let Some(c) = swatch_picker(ui, &format!("col{i}"), Rect::from_min_size(Pos2::new(rect.left() + 8.0, rect.top() + 8.0), Vec2::new(22.0, 22.0)), rgb) {
        st.motion.remember_coalesced(&st.settings);
        let hex = format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
        state::edit_color(&mut st.settings, i, 1, &hex);
        st.bufs.insert(format!("col:{i}:1"), hex);
        st.edited();
    }
    let cur = [state::fmt_num(&k[0]), hex.clone()];
    let mut x = rect.left() + 40.0;
    for (f, w) in [(0usize, 52.0f32), (1, 78.0)] {
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
        x += w + 10.0;
    }
    let ease = k[2].as_str().unwrap_or("linear").to_string();
    let er = Rect::from_min_max(Pos2::new(x, rect.top() + 5.0), Pos2::new((rect.right() - 36.0).max(x + 24.0), rect.bottom() - 5.0));
    let resp = ui.interact(er, rid.with("ease"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(format!("Easing: {ease} (click to change)"));
    let eh = th::ease(ui, resp.id.with("h"), resp.hovered());
    if eh > 0.01 {
        ui.painter().rect_filled(er, CornerRadius::same(4), Color32::from_rgba_unmultiplied(DARK3.r(), DARK3.g(), DARK3.b(), (eh * 255.0) as u8));
    }
    let efont = FontId::new(13.5, FontFamily::Proportional);
    let shown_ease = th::fit_text(ui.painter(), &ease, &efont, (er.width() - 8.0).max(12.0));
    ui.painter().text(er.left_center() + Vec2::new(4.0, 0.0), Align2::LEFT_CENTER, shown_ease, efont, th::lerp_c(CONTENT2, CONTENT1, eh));
    let mut picked = None;
    egui::Popup::menu(&resp).gap(2.0).show(|ui| {
        picked = crate::motion::ease_picker(ui, &ease);
    });
    if let Some(name) = picked {
        st.motion.remember(&st.settings);
        state::edit_color(&mut st.settings, i, 2, &name);
        st.edited();
    }
    let xr = Rect::from_center_size(Pos2::new(rect.right() - 18.0, rect.center().y), Vec2::new(26.0, 26.0));
    let xresp = ui.interact(xr, rid.with("rm"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Remove this color key");
    let xh = th::ease(ui, xresp.id.with("h"), xresp.hovered());
    let c = xr.center();
    let col = th::lerp_c(LIGHT3, CONTENT1, xh);
    ui.painter().line_segment([c + Vec2::new(-5.0, -5.0), c + Vec2::new(5.0, 5.0)], Stroke::new(2.0, col));
    ui.painter().line_segment([c + Vec2::new(-5.0, 5.0), c + Vec2::new(5.0, -5.0)], Stroke::new(2.0, col));
    if xresp.clicked() {
        st.motion.remember(&st.settings);
        state::remove_color(&mut st.settings, i);
        st.bufs.clear();
        st.edited();
    }
}

fn tab_finish(ui: &mut Ui, st: &mut Studio) {
    th::subsection_header(ui, "Outline");
    toggle(ui, st, "outline_on", "Outline");
    if st.settings["outline_on"].as_bool().unwrap_or(false) {
        slider_field(ui, st, "outline_w", "Width", 0.0, 12.0, (0.0, 64.0), 0.5, 1.0, |v| format!("{v:.1} px"));
        num(ui, st, "outline_color", "Color (#rrggbb)");
        slider_field(ui, st, "outline_opacity", "Opacity", 0.0, 1.0, (0.0, 1.0), 0.01, 100.0, |v| format!("{:.0}%", v * 100.0));
    }
    th::subsection_header(ui, "Drop shadow");
    toggle(ui, st, "shadow_on", "Drop shadow");
    if st.settings["shadow_on"].as_bool().unwrap_or(false) {
        th::note(ui, "The shadow is cast by the source image alone, so it does not change when the outline does.");
        slider_field(ui, st, "shadow_dx", "Offset X", -20.0, 20.0, (-200.0, 200.0), 1.0, 1.0, |v| format!("{v:+.0} px"));
        slider_field(ui, st, "shadow_dy", "Offset Y", -20.0, 20.0, (-200.0, 200.0), 1.0, 1.0, |v| format!("{v:+.0} px"));
        slider_field(ui, st, "shadow_blur", "Blur", 0.0, 20.0, (0.0, 100.0), 0.5, 1.0, |v| format!("{v:.1} px"));
        num(ui, st, "shadow_color", "Color (#rrggbb)");
        slider_field(ui, st, "shadow_opacity", "Opacity", 0.0, 1.0, (0.0, 1.0), 0.01, 100.0, |v| format!("{:.0}%", v * 100.0));
    }
    th::subsection_header(ui, "Debanding");
    toggle(ui, st, "deband_on", "Remove banding and dither");
    if st.settings["deband_on"].as_bool().unwrap_or(false) {
        slider_field(ui, st, "deband_radius", "Radius", 2.0, 64.0, (1.0, 256.0), 1.0, 1.0, |v| format!("{:.0} px", v));
        slider_field(ui, st, "deband_iter", "Passes", 1.0, 8.0, (1.0, 16.0), 1.0, 1.0, |v| format!("{:.0}", v));
        slider_field(ui, st, "deband_thr_rgb", "Color threshold", 0.0, 48.0, (0.0, 255.0), 1.0, 1.0, |v| format!("{:.0}", v));
        slider_field(ui, st, "deband_thr_a", "Alpha threshold", 0.0, 48.0, (0.0, 255.0), 1.0, 1.0, |v| format!("{:.0}", v));
        slider_field(ui, st, "deband_grain", "Extra grain", 0.0, 24.0, (0.0, 128.0), 0.5, 1.0, |v| format!("{v:.1}"));
    }
}

fn tab_preview(ui: &mut Ui, st: &mut Studio, out: &mut UiOut) {
    th::subsection_header(ui, "Interface");
    {
        // a slider from 50% to 200% in steps of 1%; the size changes when it is let go (the slider is part of the interface, so it
        // would move under the pointer while it is dragged)
        let shown = st.ui_pending.unwrap_or(st.ui_now);
        let mut v = shown.clamp(0.5, 2.0);
        let auto = st.cfg.ui_scale < 0.5 && st.ui_pending.is_none();
        let text = if auto { format!("Auto ({}%)", (shown * 100.0).round() as i32) } else { format!("{}%", (shown * 100.0).round() as i32) };
        if th::value_slider_def(ui, "ui_scale", "Interface size", &text, &mut v, (0.5, 2.0), (0.5, 2.0), 0.01, 100.0, Some(1.0)) {
            st.ui_pending = Some(v);
        }
        if st.ui_pending.is_some() && !ui.input(|i| i.pointer.any_down()) {
            if let Some(x) = st.ui_pending.take() {
                st.cfg.ui_scale = x;
                st.cfg.save();
            }
        }
        if th::button_wide(ui, "Back to automatic", Kind::Dark, st.cfg.ui_scale >= 0.5).clicked() {
            st.cfg.ui_scale = 0.0;
            st.cfg.save();
        }
        th::note(ui, "A smaller interface leaves more room for the gameplay preview, for example in a full screen window. Ctrl + and Ctrl - change it from anywhere, Ctrl 0 goes back to automatic.");
    }
    th::subsection_header(ui, "Skin");
    let names: Vec<String> = std::iter::once("Argon Pro".to_string()).chain(st.skins.iter().map(|(l, _)| l.clone())).collect();
    let cur = st.skin_label();
    if let Some(i) = th::dropdown_row(ui, "skin", "Skin", &cur, &names) {
        st.cfg.skin = if i == 0 { "argon".into() } else { st.skins[i - 1].1.clone() };
        st.cfg.save();
        st.load_session = true;
    }
    if th::button_wide(ui, "Browse for a skin (.osk)...", Kind::Purple, true).clicked() {
        st.pick(PickKind::Skin);
    }
    if th::button_wide(ui, "Browse for a skin folder...", Kind::Dark, true).clicked() {
        st.pick(PickKind::SkinFolder);
    }
    th::note(ui, &format!("Hit sounds come from: {}", if st.cfg.sound_skin.is_empty() { "the skin itself (Argon Pro has none of its own)".to_string() } else { crate::studio::label(&st.cfg.sound_skin) }));
    if th::button_wide(ui, "Choose a skin for hit sounds (.osk)...", Kind::Dark, true).clicked() {
        st.pick(PickKind::SoundSkin);
    }
    if th::button_wide(ui, "Choose a skin folder for hit sounds...", Kind::Dark, true).clicked() {
        st.pick(PickKind::SoundSkinFolder);
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
    if th::value_slider_def(ui, "volume", "Volume", &format!("{:.0}%", st.cfg.volume * 100.0), &mut v, (0.0, 1.0), (0.0, 1.0), 0.01, 100.0, Some(0.10)) {
        st.cfg.volume = v;
        st.cfg.muted = v <= 0.0;
        st.cfg.save();
        out.volume = Some(st.effective_volume());
    }
    // the music offset, in whole milliseconds: Shift while dragging is ten times slower, the arrow keys over it move 1 ms, a click on the number types one
    let mut off = st.cfg.offset_ms.round();
    if th::value_slider_def(ui, "offset_ms", "Music offset (+ plays the song later)", &format!("{:+.0} ms", off), &mut off, (-75.0, 75.0), (-1000.0, 1000.0), 1.0, 1.0, Some(0.0)) {
        st.cfg.offset_ms = off;
        st.cfg.save();
        out.offset = Some(off);
    }
    let mut sp = st.cfg.speed;
    if th::value_slider_def(ui, "speedrow", "Playback speed", &format!("{}x", fmt_speed(st.cfg.speed)), &mut sp, (0.25, 2.0), (0.1, 2.0), 0.01, 1.0, Some(1.0)) {
        st.cfg.speed = sp.clamp(0.1, 2.0);
        st.cfg.save();
        out.speed = Some(st.cfg.speed);
    }
    let mut c = st.cfg.cursor_scale;
    if th::value_slider_def(ui, "cursor", "Cursor size", &format!("{:.0}%", st.cfg.cursor_scale * 100.0), &mut c, (0.5, 2.0), (0.3, 3.0), 0.01, 100.0, Some(1.0)) {
        st.cfg.cursor_scale = c;
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
        // the name stops where the buttons begin (the whole of it is in the tip)
        let name_font = FontId::new(15.0, th::semibold());
        let name = th::fit_text(ui.painter(), &h.name, &name_font, rect.right() - 92.0 - 32.0 - rect.left() - 18.0);
        let cut = name != h.name;
        ui.painter().text(rect.min + Vec2::new(9.0, 9.0), Align2::LEFT_TOP, name, name_font, CONTENT1);
        if cut {
            let tip = Rect::from_min_max(rect.min, Pos2::new(rect.right() - 92.0 - 32.0, rect.min.y + 30.0));
            ui.interact(tip, ui.scope_id().with(("histname", &h.name, &h.when)), Sense::hover()).on_hover_text(&h.name);
        }
        ui.painter().text(rect.min + Vec2::new(9.0, 31.0), Align2::LEFT_TOP, &h.when, FontId::new(13.0, th::light()), CONTENT2);
        let load = Rect::from_center_size(Pos2::new(rect.right() - 92.0, rect.center().y), Vec2::new(64.0, 30.0));
        let del = Rect::from_center_size(Pos2::new(rect.right() - 30.0, rect.center().y), Vec2::new(54.0, 30.0));
        let armed_rem = st.hist_confirm.as_ref().filter(|(id, _)| id == &h.id).map(|(_, at)| 3.0 - at.elapsed().as_secs_f32()).filter(|rem| *rem > 0.0);
        let armed = armed_rem.is_some();
        if let Some(rem) = armed_rem {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f32(rem));
        }
        if st.hist_confirm.as_ref().is_some_and(|(_, at)| at.elapsed().as_secs_f32() >= 3.0) {
            st.hist_confirm = None;
        }
        for (r, label, base, which) in [
            (load, "Load", COLOR3, 0),
            (del, if armed { "Sure?" } else { "Delete" }, if armed { PINK3 } else { DARK2 }, 1),
        ] {
            let resp = ui.interact(r, ui.scope_id().with((&h.id, which)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
            let hov = th::ease(ui, resp.id.with("h"), resp.hovered());
            ui.painter().rect_filled(r, CornerRadius::same(5), th::lerp_c(base, th::lighten(base, 0.2), hov));
            ui.painter().text(r.center(), Align2::CENTER_CENTER, label, FontId::new(13.0, th::semibold()), CONTENT1);
            if resp.clicked() {
                if which == 0 {
                    st.hist_confirm = None;
                    st.load_history(&h.id);
                } else if armed {
                    st.hist_confirm = None;
                    st.delete_history(&h.id);
                } else {
                    st.hist_confirm = Some((h.id.clone(), std::time::Instant::now()));
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

#[cfg(test)]
mod logo_tests {
    use super::*;

    #[test]
    fn pressing_the_logo_bounces_it_and_tells_when_there_is_nothing_to_play() {
        let ctx = egui::Context::default();
        let mut st = Studio::new(ctx.clone());
        st.cfg.muted = true; // the test must not make a sound
        let at = Pos2::new(28.0, 28.0);
        let run = |st: &mut Studio, events: Vec<egui::Event>| {
            let mut raw = egui::RawInput::default();
            raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 100.0)));
            raw.events = events;
            let out = ctx.run_ui(raw, |ui| logo(ui, st));
            out.shapes.len()
        };
        let btn = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        run(&mut st, vec![egui::Event::PointerMoved(at)]);
        assert!(st.logo_pressed.is_none());
        let resting = run(&mut st, vec![]);
        run(&mut st, vec![btn(true)]);
        let shapes = run(&mut st, vec![btn(false)]);
        assert!(st.logo_pressed.is_some(), "the press is noted");
        assert!(st.logo_tex.is_some(), "the logo picture is loaded");
        assert!(st.status.contains("turn it on"), "muted: says where the sound is: {}", st.status);
        assert!(shapes > resting, "the ring is drawn on top of the bounce ({shapes} shapes vs {resting})");
    }

    #[test]
    fn the_hit_sound_of_the_picked_skin_is_found_in_a_skin_folder() {
        let dir = std::env::temp_dir().join(format!("ss-skin-hit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("skin.ini"), "[General]\nName: t\nVersion: 2.5\n").unwrap();
        std::fs::write(dir.join("normal-hitnormal.wav"), b"RIFF").unwrap();
        let mut st = Studio::new(egui::Context::default());
        st.cfg.skin = dir.to_string_lossy().to_string();
        st.cfg.sound_skin = String::new();
        let found = st.skin_hitsound().expect("found");
        assert!(found.ends_with("normal-hitnormal.wav"), "{found:?}");
        // a skin without one (Argon) and no fallback: nothing
        st.cfg.skin = "argon".into();
        assert!(st.skin_hitsound().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod zoom_tests {
    use super::*;

    fn studio_with_frames() -> Studio {
        let mut st = Studio::new(egui::Context::default());
        let img = engine::img::Img::new(100, 50);
        st.frames = vec![Some(img); 257];
        st.fps = 240.0;
        st
    }

    #[test]
    fn the_preview_starts_at_real_size_and_steps_only_as_far_as_it_can_grow() {
        let ctx = egui::Context::default();
        let mut st = studio_with_frames();
        st.anim_limit = (500.0, 340.0);
        let (mw, mh) = (100, 50);
        let ppp = ctx.pixels_per_point();
        assert_eq!(st.anim_zoom, 1.0);
        let (k, fit) = card_scale(&st, ppp, mw, mh);
        assert_eq!(k, 1.0, "real size by default");
        assert!(fit >= 3.0);
        // ten presses on + change nothing beyond the largest size; one press on - goes down one step, not ten
        for _ in 0..10 {
            let (k, fit) = card_scale(&st, ppp, mw, mh);
            st.anim_zoom = next_anim_zoom(k, true, fit);
        }
        let (k, fit) = card_scale(&st, ppp, mw, mh);
        assert_eq!(k, fit);
        let one_down = next_anim_zoom(k, false, fit);
        st.anim_zoom = one_down;
        assert_eq!(card_scale(&st, ppp, mw, mh).0, one_down);
        // the picture is as big as the zoom says: 100 px wide at real size
        st.anim_zoom = 1.0;
        let size = anim_card_size(&ctx, &st);
        assert!(size.x >= card_min_width(&st) && size.x < 300.0, "{size:?}");
    }

    #[test]
    fn the_smallest_card_has_room_for_all_its_controls() {
        let st = studio_with_frames();
        // play button, 12 + "238/257 · 240 fps" + the speed button: the counter must end before the speed button starts
        let need = 12.0 + 38.0 + 12.0 + 19.0 * 6.9 + 10.0 + 52.0 + 12.0;
        assert!(card_min_width(&st) >= need - 0.5, "{} < {need}", card_min_width(&st));
    }
}
