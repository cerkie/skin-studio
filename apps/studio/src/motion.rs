//! The motion editor's graph: one property over time, with its keyframes as dots you can drag.
use egui::{Align2, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use engine::keys;
use engine::settings::Settings;

use crate::state;
use crate::studio::Studio;
use crate::theme::pal::*;

const H: f32 = 150.0;

fn nice(v: f32) -> String {
    let s = format!("{:.3}", v);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Draw the graph of property `pid`. Drag a dot to change its time and value, double-click the empty graph to add a key,
/// right-click a dot to remove it. The first key stays at the start and a key set to "end" stays at the end (only their
/// values move); type exact numbers in the rows below.
pub fn graph(ui: &mut Ui, st: &mut Studio, pid: &str) {
    let parsed = Settings::from_json(&st.settings);
    let total = parsed.design_length();
    let nk = parsed.prop(pid).clone();
    let json_keys = engine::presets::keys_of(&st.settings["props"][pid]);

    let width = ui.available_width() - 22.0;
    let (outer, bg) = ui.allocate_exact_size(Vec2::new(width, H + crate::theme::ROW_GAP), Sense::click());
    let outer = Rect::from_min_size(outer.min + Vec2::new(12.0, 0.0), Vec2::new(outer.width(), H));
    let plot = Rect::from_min_max(outer.min + Vec2::new(40.0, 14.0), outer.max - Vec2::new(16.0, 24.0));
    let p = ui.painter().clone();
    p.rect_filled(outer, CornerRadius::same(8), BACKGROUND5);

    // the value range: wide enough for the keys and the curve between them, held steady while a dot is dragged
    let rid = ui.scope_id().with(("motion-range", pid));
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
    let pad = (hi - lo) * 0.18;
    let (mut lo, mut hi) = (lo - pad, hi + pad);
    let held: Option<(f32, f32)> = ui.memory(|m| m.data.get_temp(rid));
    if let Some(h) = held {
        lo = h.0;
        hi = h.1;
    }
    let to_x = |t: f32| plot.left() + t / total.max(1.0) * plot.width();
    let to_y = |v: f32| plot.bottom() - (v - lo) / (hi - lo) * plot.height();
    let from_x = |x: f32| ((x - plot.left()) / plot.width()).clamp(0.0, 1.0) * total;
    let from_y = |y: f32| lo + (plot.bottom() - y) / plot.height() * (hi - lo);

    // grid and labels
    for i in 0..=3 {
        let v = lo + (hi - lo) * i as f32 / 3.0;
        let y = to_y(v);
        p.line_segment([Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)], Stroke::new(1.0, Color32::from_white_alpha(if i == 0 { 22 } else { 10 })));
        p.text(Pos2::new(plot.left() - 6.0, y), Align2::RIGHT_CENTER, format!("{:.2}", v), FontId::new(11.0, FontFamily::Proportional), LIGHT3);
    }
    for (t, a) in [(0.0, Align2::LEFT_TOP), (total, Align2::RIGHT_TOP)] {
        p.text(Pos2::new(to_x(t), plot.bottom() + 6.0), a, format!("{:.0} ms", t), FontId::new(11.0, FontFamily::Proportional), LIGHT3);
    }

    // the curve
    let pts: Vec<Pos2> = (0..=160).map(|i| {
        let t = i as f32 / 160.0 * total;
        Pos2::new(to_x(t), to_y(keys::eval(&nk, t, total)))
    }).collect();
    p.add(egui::Shape::line(pts, Stroke::new(2.4, HIGHLIGHT1)));

    // where the animation card is playing
    let n = st.frames.len();
    if n > 0 {
        let loop_ms = n as f32 * 1000.0 / st.fps.max(1.0);
        let x = to_x(st.anim_pos / loop_ms * total);
        p.line_segment([Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())], Stroke::new(1.0, Color32::from_white_alpha(46)));
    }

    // the keys
    let mut dragging = false;
    let mut remove = None;
    for (i, k) in nk.iter().enumerate() {
        let jk = json_keys.get(i);
        let is_end = jk.map(|k| k[0].is_string()).unwrap_or(false);
        let t = k.t.at(total);
        let pos = Pos2::new(to_x(t), to_y(k.v));
        let id = ui.scope_id().with(("motion-node", pid, i));
        let r = ui.interact(Rect::from_center_size(pos, Vec2::splat(22.0)), id, Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::Grab);
        let first = i == 0 && t <= 0.0;
        if r.dragged() {
            dragging = true;
            if let Some(m) = r.interact_pointer_pos() {
                let nv = from_y(m.y);
                state::edit_key(&mut st.settings, pid, i, 1, &nice(nv));
                if !is_end && !first {
                    // between the neighbours, in 5 ms steps
                    let before = if i > 0 { nk[i - 1].t.at(total) + 1.0 } else { 0.0 };
                    let after = if i + 1 < nk.len() { nk[i + 1].t.at(total) - 1.0 } else { total };
                    let nt = ((from_x(m.x) / 5.0).round() * 5.0).clamp(before, after.max(before));
                    state::edit_key(&mut st.settings, pid, i, 0, &nice(nt));
                }
                for f in 0..2 {
                    st.bufs.remove(&format!("key:{pid}:{i}:{f}"));
                }
                st.edited();
            }
        }
        if r.secondary_clicked() && nk.len() > 1 {
            remove = Some(i);
        }
        let active = r.hovered() || r.dragged();
        if active {
            p.circle_filled(pos, 12.0, Color32::from_rgba_unmultiplied(0x8c, 0x66, 0xff, 50));
        }
        p.circle_filled(pos, 6.0, if active { Color32::WHITE } else { CONTENT1 });
        p.circle_stroke(pos, 6.0, Stroke::new(2.0, HIGHLIGHT1));
        if active {
            let text = format!("{} ms, {}", t.round(), nice(k.v));
            let g = p.layout_no_wrap(text, FontId::new(12.0, FontFamily::Proportional), CONTENT1);
            let size = g.size() + Vec2::new(14.0, 8.0);
            let mut rect = Rect::from_min_size(Pos2::new(pos.x - size.x * 0.5, pos.y - 18.0 - size.y), size);
            if rect.left() < outer.left() + 4.0 {
                rect = rect.translate(Vec2::new(outer.left() + 4.0 - rect.left(), 0.0));
            }
            if rect.right() > outer.right() - 4.0 {
                rect = rect.translate(Vec2::new(outer.right() - 4.0 - rect.right(), 0.0));
            }
            if rect.top() < outer.top() {
                rect = rect.translate(Vec2::new(0.0, pos.y + 18.0 - rect.top()));
            }
            p.rect_filled(rect, CornerRadius::same(5), BACKGROUND6);
            p.rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, LIGHT4), egui::StrokeKind::Inside);
            p.galley(rect.min + Vec2::new(7.0, 4.0), g, CONTENT1);
        }
    }
    ui.memory_mut(|m| {
        if dragging {
            m.data.insert_temp(rid, (lo, hi));
        } else {
            m.data.remove::<(f32, f32)>(rid);
        }
    });

    if let Some(i) = remove {
        state::remove_key(&mut st.settings, pid, i);
        st.bufs.clear();
        st.edited();
    }
    // double-click on empty graph: a new key on the curve there
    if bg.double_clicked() && !dragging {
        if let Some(m) = bg.interact_pointer_pos() {
            if plot.expand(6.0).contains(m) {
                let t = (from_x(m.x) / 5.0).round() * 5.0;
                state::insert_key(&mut st.settings, pid, t as f64, from_y(m.y) as f64);
                st.bufs.clear();
                st.edited();
            }
        }
    }
    let _ = Align2::CENTER_CENTER;
}
