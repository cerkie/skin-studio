//! A layout audit that runs the whole interface without a window, at several window sizes and interface scales, and looks at everything it
//! painted: text that runs into other text, text that is covered by something drawn after it, text that is cut by the edge of the
//! area it is in or by the edge of the window. Anything it finds fails the test with a list of where it is, so that a layout that does not
//! fit cannot be added without the test saying so.
use egui::epaint::{ClippedShape, Shape};
use egui::{Pos2, Rect, Vec2};

use crate::studio::Studio;
use crate::ui::{self, SessionView, UiOut};

/// What was painted as text, and the opaque rectangles that came after it.
struct Item {
    rect: Rect,
    clip: Rect,
    text: String,
    order: usize,
}

fn flatten(shape: &Shape, clip: Rect, order: &mut usize, texts: &mut Vec<Item>, rects: &mut Vec<(Rect, usize)>, boxes: &mut Vec<Rect>) {
    match shape {
        Shape::Vec(v) => {
            for s in v {
                flatten(s, clip, order, texts, rects, boxes);
            }
        }
        Shape::Text(t) => {
            *order += 1;
            let r = t.galley.rect.translate(t.pos.to_vec2());
            if r.width() > 0.5 && r.height() > 0.5 && !t.galley.text().trim().is_empty() {
                texts.push(Item { rect: r, clip, text: t.galley.text().to_string(), order: *order });
            }
        }
        Shape::Rect(r) => {
            *order += 1;
            // a rectangle that hides what is under it (not a faint tint, not a border)
            if r.fill.a() >= 235 && r.rect.width() > 2.0 && r.rect.height() > 2.0 {
                rects.push((r.rect.intersect(clip), *order));
            }
            // a button, a pill, a field: a filled box that is not a panel or a background
            if r.fill.a() >= 120 && r.rect.width() >= 18.0 && r.rect.width() <= 240.0 && r.rect.height() >= 20.0 && r.rect.height() <= 46.0 {
                boxes.push(r.rect.intersect(clip));
            }
        }
        _ => {
            *order += 1;
        }
    }
}

fn area(r: Rect) -> f32 {
    r.width().max(0.0) * r.height().max(0.0)
}

/// Everything wrong with the layout in one frame of the interface, as lines of text.
pub fn problems(shapes: &[ClippedShape], screen: Rect, tolerate: &[&str]) -> Vec<String> {
    let (mut texts, mut rects, mut boxes) = (vec![], vec![], vec![]);
    let mut order = 0;
    for cs in shapes {
        flatten(&cs.shape, cs.clip_rect, &mut order, &mut texts, &mut rects, &mut boxes);
    }
    let mut out = vec![];
    // two boxes that overlap without one being inside the other: buttons or fields that run into each other
    for (i, a) in boxes.iter().enumerate() {
        for b in &boxes[i + 1..] {
            let inter = a.intersect(*b);
            let inside = a.contains_rect(*b) || b.contains_rect(*a);
            if !inside && inter.width() > 3.0 && inter.height() > 6.0 {
                out.push(format!("boxes overlap: {:.0}x{:.0} at {:.0},{:.0} and {:.0}x{:.0} at {:.0},{:.0}", a.width(), a.height(), a.left(), a.top(), b.width(), b.height(), b.left(), b.top()));
            }
        }
    }
    let skip = |s: &str| tolerate.iter().any(|t| s.contains(t));
    for (i, a) in texts.iter().enumerate() {
        let short = |s: &str| -> String { s.chars().take(34).collect::<String>().replace('\n', " ") };
        if skip(&a.text) {
            continue;
        }
        // cut by the edge of the area it is drawn in (or the window)
        let visible = a.clip.intersect(screen);
        // (only sideways: what is scrolled out of a list at the top or the bottom is meant to be cut; a text that is wholly out of sight is not drawn)
        let seen = a.rect.intersect(visible);
        let cut_x = (visible.left() - a.rect.left()).max(a.rect.right() - visible.right()).max(0.0);
        if seen.height() > 4.0 && cut_x > 1.5 {
            out.push(format!("cut by its area ({cut_x:.0} px): \"{}\" at {:.0},{:.0}", short(&a.text), a.rect.left(), a.rect.top()));
        }
        // run into another text
        for b in &texts[i + 1..] {
            if skip(&b.text) {
                continue;
            }
            let inter = a.rect.intersect(b.rect);
            if inter.width() > 2.0 && inter.height() > 4.0 && a.text != b.text {
                out.push(format!("overlap: \"{}\" and \"{}\" at {:.0},{:.0}", short(&a.text), short(&b.text), inter.left(), inter.top()));
            }
        }
        // covered by something drawn after it
        for (r, ord) in &rects {
            if *ord > a.order {
                let inter = a.rect.intersect(*r);
                if inter.width() > 2.0 && inter.height() > 4.0 && area(inter) > 8.0 {
                    out.push(format!("covered: \"{}\" at {:.0},{:.0} by a {:.0}x{:.0} box", short(&a.text), a.rect.left(), a.rect.top(), r.width(), r.height()));
                    break;
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// One frame of the interface in a window of `size` points (a few frames, so that animations and sizes have settled).
pub fn frame(ctx: &egui::Context, st: &mut Studio, size: Vec2, sv: &SessionView) -> (Vec<ClippedShape>, Rect) {
    let screen = Rect::from_min_size(Pos2::ZERO, size);
    let mut last = vec![];
    for _ in 0..3 {
        let mut raw = egui::RawInput::default();
        raw.screen_rect = Some(screen);
        let mut out = UiOut::default();
        let full = ctx.run_ui(raw, |ui| ui::draw(ui, st, sv, &mut out));
        last = full.shapes;
    }
    (last, screen)
}

pub fn session() -> SessionView {
    SessionView { loaded: true, clock: 8000.0, start: 0.0, end: 184000.0, playing: true, info: "Some Artist - Some Title [Hard] | skin Argon Pro | 434 objects".into() }
}

pub fn sizes() -> Vec<Vec2> {
    vec![Vec2::new(853.0, 480.0), Vec2::new(1067.0, 600.0), Vec2::new(1280.0, 720.0), Vec2::new(1600.0, 940.0), Vec2::new(2048.0, 1152.0)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::studio::{Mode, Tab};

    fn studio() -> (egui::Context, Studio) {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut st = Studio::new(ctx.clone());
        st.status.clear();
        st.settings = engine::presets::default_settings();
        (ctx, st)
    }

    /// The text of the gameplay view's own overlay and the things the tests fake are not part of the audit.
    const TOLERATED: &[&str] = &["Some Artist"];

    /// The same problem seen in many windows is listed once, with the first window it showed in.
    fn unique(all: Vec<String>) -> Vec<String> {
        let mut seen = std::collections::BTreeMap::new();
        for line in &all {
            let (win, rest) = line.split_once("] ").unwrap_or(("", line));
            seen.entry(rest.to_string()).or_insert_with(|| win.trim_start_matches('[').to_string());
        }
        seen.iter().map(|(p, w)| format!("{p}   [{w}]")).collect()
    }

    fn report(name: &str, found: Vec<String>, all: &mut Vec<String>) {
        for f in found {
            all.push(format!("[{name}] {f}"));
        }
    }

    #[test]
    fn nothing_overlaps_or_is_cut_in_the_followpoints_tab() {
        let (ctx, mut st) = studio();
        let sv = session();
        let mut all = vec![];
        for size in sizes() {
            for tab in [Tab::Animation, Tab::Timing, Tab::Look, Tab::Preview, Tab::Export] {
                st.tab = tab;
                st.mode = Mode::Followpoints;
                for (curves, lane) in [(true, 0usize), (false, 0), (true, 7), (true, 5)] {
                    st.motion.curves = curves;
                    st.motion.prop = lane;
                    st.motion.sel = vec![(lane, 1)];
                    let (shapes, screen) = frame(&ctx, &mut st, size, &sv);
                    report(&format!("{}x{} {:?} curves={curves} lane={lane}", size.x, size.y, tab as u8), problems(&shapes, screen, TOLERATED), &mut all);
                }
            }
        }
        let lines = unique(all);
        assert!(lines.is_empty(), "{} problems:
{}", lines.len(), lines.join("
"));
    }

    /// Data that is longer than it should be: long paths, names and labels.
    fn long_data(st: &mut Studio) {
        st.path = Some(std::path::PathBuf::from("C:/Users/Someone With A Long Name/Documents/skin-studio/saved-followpoints/a very long folder name for a set of settings/followpoint-settings.json"));
        st.history = (0..4)
            .map(|i| crate::state::HistoryEntry { id: format!("h{i}"), name: format!("Chevron (bold) | 250-ease-in-out fast start slow end | speed 0.917 | shift -100 ms {i}"), when: "2026-11-19 13:10".into(), settings: serde_json::json!({}) })
            .collect();
        st.skins = (0..6).map(|i| (format!("-     # owoTuna {{1.4}} CK # -     - [- # owoTuna {{1.4}} CK # -] (osu!lazer) {i}"), format!("C:/skins/{i}"))).collect();
        st.cfg.skin = st.skins[0].1.clone();
        st.status = "A status line that is much longer than the room that is left for it in the corner of the window, to see what happens".into();
    }

    #[test]
    fn nothing_overlaps_or_is_cut_with_long_names_and_paths() {
        let (ctx, mut st) = studio();
        long_data(&mut st);
        let sv = session();
        let mut all = vec![];
        for size in sizes() {
            for tab in [Tab::Animation, Tab::Timing, Tab::Look, Tab::Preview, Tab::Export] {
                st.tab = tab;
                st.mode = Mode::Followpoints;
                let (shapes, screen) = frame(&ctx, &mut st, size, &sv);
                report(&format!("{}x{} {:?}", size.x, size.y, tab as u8), problems(&shapes, screen, TOLERATED), &mut all);
            }
        }
        let lines = unique(all);
        assert!(lines.is_empty(), "{} problems:\n{}", lines.len(), lines.join("\n"));
    }

    fn skin_folder() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("ss-audit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for (i, name) in ["hitcircle", "cursor@2x", "a-really-long-picture-name-that-is-longer-than-the-list-is-wide@2x", "sliderb", "default-0"].iter().enumerate() {
            let mut im = engine::img::Img::new(24 + i * 8, 24);
            for p in im.px.iter_mut() {
                *p = [0.2, 0.4, 0.6, 1.0];
            }
            engine::edit::save_png(&d.join(format!("{name}.png")), &im).unwrap();
        }
        d
    }

    #[test]
    fn nothing_overlaps_or_is_cut_in_the_skin_editor() {
        use crate::skin_editor::SkinPage;
        let (ctx, mut st) = studio();
        let d = skin_folder();
        st.mode = Mode::Skin;
        let sv = session();
        let mut all = vec![];
        for size in sizes() {
            st.skin_ed = crate::skin_editor::SkinEditor::default();
            for page in [SkinPage::Pictures, SkinPage::Edits, SkinPage::Output] {
                st.skin_ed.page = page;
                let (shapes, screen) = frame(&ctx, &mut st, size, &sv);
                report(&format!("{}x{} empty {}", size.x, size.y, page as u8), problems(&shapes, screen, TOLERATED), &mut all);
            }
            st.skin_ed.open_skin(&d).unwrap();
            let pic = d.join("a-really-long-picture-name-that-is-longer-than-the-list-is-wide@2x.png");
            st.skin_ed.open_picture(&pic, None).unwrap();
            let (cw, ch) = st.skin_ed.out_dims();
            for (name, op) in crate::skin_editor::add_menu_items(cw, ch) {
                st.skin_ed.ops = vec![op];
                st.skin_ed.page = SkinPage::Edits;
                let (shapes, screen) = frame(&ctx, &mut st, size, &sv);
                report(&format!("{}x{} edit {name}", size.x, size.y), problems(&shapes, screen, TOLERATED), &mut all);
            }
            for page in [SkinPage::Pictures, SkinPage::Output] {
                st.skin_ed.page = page;
                let (shapes, screen) = frame(&ctx, &mut st, size, &sv);
                report(&format!("{}x{} skin {}", size.x, size.y, page as u8), problems(&shapes, screen, TOLERATED), &mut all);
            }
            st.skin_ed.page = SkinPage::Edits;
            st.skin_ed.ops.clear();
            st.skin_ed.add_open = true;
            let (shapes, screen) = frame(&ctx, &mut st, size, &sv);
            report(&format!("{}x{} add menu", size.x, size.y), problems(&shapes, screen, TOLERATED), &mut all);
        }
        let _ = std::fs::remove_dir_all(&d);
        let lines = unique(all);
        assert!(lines.is_empty(), "{} problems:\n{}", lines.len(), lines.join("\n"));
    }
}
