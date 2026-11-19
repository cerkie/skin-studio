//! The skin editor: open a skin (a folder or an .osk) or a single picture, pick an asset, edit it with a list of edits (resize without
//! ringing, flip, rotate, crop, colour, blur, sharpen, deband...), see it before and after, and save it back.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use egui::{Align2, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Sense, Stroke, TextureHandle, Ui, Vec2};
use engine::edit::{self, Op, Resample};
use engine::img::Img;

use crate::studio::{Msg, PickKind, Studio};
use crate::theme::pal::*;
use crate::theme::{self as th, Kind};

/// One picture of a skin.
pub struct AssetFile {
    pub name: String,
    pub path: PathBuf,
    pub size: Option<(u32, u32)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Bg {
    Checker,
    Dark,
    Light,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Show {
    After,
    Before,
    Split,
}

/// A thumbnail: the picture shrunk to fit a square of `side` pixels (area averaging), as premultiplied RGBA bytes.
pub fn thumbnail(img: &Img, side: usize) -> ([usize; 2], Vec<u8>) {
    let k = (side as f32 / img.w.max(img.h) as f32).min(1.0);
    let (w, h) = (((img.w as f32 * k).round() as usize).max(1), ((img.h as f32 * k).round() as usize).max(1));
    let small = edit::resize(img, w, h, Resample::Smooth);
    ([w, h], small.to_premult_u8())
}

pub struct SkinEditor {
    /// The skin folder being edited (an .osk is unpacked into a folder of its own first), and its pictures.
    pub dir: Option<PathBuf>,
    pub packed_from: Option<PathBuf>,
    pub files: Vec<AssetFile>,
    pub filter: String,
    /// The picture being edited, as loaded, the edits, and what they make of it.
    pub current: Option<PathBuf>,
    pub source: Option<Arc<Img>>,
    pub ops: Vec<Op>,
    pub result: Option<Arc<Img>>,
    /// The size after each edit (the edit's own input is the one before).
    pub dims: Vec<(usize, usize)>,
    pub gen: u64,
    pub busy: bool,
    pub dirty: bool,
    tex: [Option<(usize, TextureHandle)>; 2],
    /// How the picture is shown: screen points per pixel of the picture (0 = fit the window), where it is, what is behind it.
    pub zoom: f32,
    pub pan: Vec2,
    pub bg: Bg,
    pub show: Show,
    pub split: f32,
    pub keep_aspect: bool,
    bufs: HashMap<String, String>,
    /// The edit list as it was before each change, and what undo took off: Ctrl+Z and Ctrl+Y.
    undo: Vec<Vec<Op>>,
    redo: Vec<Vec<Op>>,
    last_edit: Option<std::time::Instant>,
    /// Small pictures of the files in the list, and which list they are for (a thumbnail of an older list is thrown away).
    thumbs: HashMap<PathBuf, (TextureHandle, [usize; 2])>,
    thumb_gen: u64,
}

impl Default for SkinEditor {
    fn default() -> Self {
        SkinEditor { dir: None, packed_from: None, files: vec![], filter: String::new(), current: None, source: None, ops: vec![], result: None, dims: vec![], gen: 0, busy: false, dirty: false, tex: [None, None], zoom: 0.0, pan: Vec2::ZERO, bg: Bg::Checker, show: Show::After, split: 0.5, keep_aspect: true, bufs: HashMap::new(), undo: vec![], redo: vec![], last_edit: None, thumbs: HashMap::new(), thumb_gen: 0 }
    }
}

/// The size of a PNG from its header.
fn png_size(path: &Path) -> Option<(u32, u32)> {
    let f = std::fs::File::open(path).ok()?;
    let r = png::Decoder::new(std::io::BufReader::new(f)).read_info().ok()?;
    let i = r.info();
    Some((i.width, i.height))
}

/// Every picture of a skin folder (the folder itself and one level of folders below it), by name.
fn list_pngs(dir: &Path) -> Vec<AssetFile> {
    let mut out = vec![];
    let mut visit = |d: &Path, prefix: &str| {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if p.is_file() && name.to_lowercase().ends_with(".png") {
                    out.push(AssetFile { name: format!("{prefix}{name}"), size: png_size(&p), path: p });
                }
            }
        }
    };
    visit(dir, "");
    if let Ok(rd) = std::fs::read_dir(dir) {
        let subs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))).collect();
        for s in subs {
            let prefix = format!("{}/", s.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
            visit(&s, &prefix);
        }
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

/// Unpack an .osk (or .zip) next to the other temporary files, into a folder of its own that is kept between runs.
fn unpack(osk: &Path) -> Result<PathBuf, String> {
    let stem = osk.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "skin".into());
    let dir = std::env::temp_dir().join("skinstudio").join("edit").join(stem);
    let _ = std::fs::remove_dir_all(&dir);
    let f = std::fs::File::open(osk).map_err(|e| format!("{}: {e}", osk.display()))?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| format!("{}: {e}", osk.display()))?;
    zip.extract(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// What a list of edits makes of a picture, and the size after each edit.
pub fn compute(source: &Img, ops: &[Op]) -> (Img, Vec<(usize, usize)>) {
    let mut cur = source.clone();
    let mut dims = Vec::with_capacity(ops.len());
    for op in ops {
        cur = edit::apply(&cur, op);
        dims.push((cur.w, cur.h));
    }
    (cur, dims)
}

impl SkinEditor {
    // ------------------------------------------------------------------------------------------------------------ opening
    /// Open a skin: a folder, or an .osk that is unpacked first.
    pub fn open_skin(&mut self, path: &Path) -> Result<(), String> {
        let (dir, packed) = if path.is_dir() { (path.to_path_buf(), None) } else { (unpack(path)?, Some(path.to_path_buf())) };
        self.files = list_pngs(&dir);
        self.dir = Some(dir);
        self.packed_from = packed;
        self.filter.clear();
        self.clear_picture();
        Ok(())
    }

    /// Open a single picture (its folder becomes the list when no skin is open).
    pub fn open_picture(&mut self, path: &Path, ctx_tx: Option<(&Sender<Msg>, &egui::Context)>) -> Result<(), String> {
        let img = edit::load_png(path)?;
        if self.dir.is_none() {
            if let Some(d) = path.parent() {
                self.files = list_pngs(d);
                self.dir = Some(d.to_path_buf());
            }
        }
        self.current = Some(path.to_path_buf());
        self.source = Some(Arc::new(img));
        self.undo.clear();
        self.redo.clear();
        self.last_edit = None;
        self.ops.clear();
        self.dims.clear();
        self.dirty = false;
        self.zoom = 0.0;
        self.pan = Vec2::ZERO;
        self.bufs.clear();
        match ctx_tx {
            Some((tx, ctx)) => self.recompute(tx, ctx),
            None => {
                self.result = self.source.clone();
                self.busy = false;
            }
        }
        Ok(())
    }

    fn clear_picture(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.current = None;
        self.source = None;
        self.result = None;
        self.ops.clear();
        self.dims.clear();
        self.dirty = false;
        self.tex = [None, None];
    }

    // ------------------------------------------------------------------------------------------------------------ editing
    // ------------------------------------------------------------------------------------------------------------ thumbnails
    /// Make small pictures of every file of the list, in the background, one after the other as the list is ordered.
    pub fn load_thumbs(&mut self, tx: &Sender<Msg>, ctx: &egui::Context) {
        self.thumb_gen += 1;
        self.thumbs.clear();
        let gen = self.thumb_gen;
        let paths: Vec<PathBuf> = self.files.iter().map(|f| f.path.clone()).collect();
        let (tx, ctx) = (tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            for p in paths {
                if let Ok(img) = edit::load_png(&p) {
                    let (size, rgba) = thumbnail(&img, 48);
                    if tx.send(Msg::SkinThumb(gen, p, size, rgba)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            }
        });
    }

    pub fn got_thumb(&mut self, ctx: &egui::Context, gen: u64, path: PathBuf, size: [usize; 2], rgba: Vec<u8>) {
        if gen == self.thumb_gen && rgba.len() == size[0] * size[1] * 4 {
            let t = ctx.load_texture("skin-thumb", egui::ColorImage::from_rgba_premultiplied(size, &rgba), egui::TextureOptions::LINEAR);
            self.thumbs.insert(path, (t, size));
        }
    }

    // ------------------------------------------------------------------------------------------------------------ undo
    /// The edit list is about to change from `before`: keep it. Changes that follow each other quickly (dragging a slider) are one step.
    pub fn push_undo(&mut self, before: Vec<Op>) {
        let quick = self.last_edit.is_some_and(|t| t.elapsed().as_millis() < 700);
        if !quick || self.undo.is_empty() {
            self.undo.push(before);
            if self.undo.len() > 200 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last_edit = Some(std::time::Instant::now());
    }

    pub fn undo(&mut self, tx: &Sender<Msg>, ctx: &egui::Context) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.ops, prev));
            self.after_history(tx, ctx);
        }
    }

    pub fn redo(&mut self, tx: &Sender<Msg>, ctx: &egui::Context) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.ops, next));
            self.after_history(tx, ctx);
        }
    }

    fn after_history(&mut self, tx: &Sender<Msg>, ctx: &egui::Context) {
        self.bufs.clear();
        self.dirty = true;
        self.last_edit = None;
        self.recompute(tx, ctx);
    }

    /// The edits changed: make the picture again on a worker (a large picture with a deband takes a moment).
    pub fn recompute(&mut self, tx: &Sender<Msg>, ctx: &egui::Context) {
        let Some(src) = self.source.clone() else { return };
        self.gen += 1;
        self.busy = true;
        let (gen, ops, tx, ctx) = (self.gen, self.ops.clone(), tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let (img, dims) = compute(&src, &ops);
            let _ = tx.send(Msg::SkinResult(gen, Arc::new(img), dims));
            ctx.request_repaint();
        });
    }

    pub fn got_result(&mut self, gen: u64, img: Arc<Img>, dims: Vec<(usize, usize)>) {
        if gen == self.gen {
            self.result = Some(img);
            self.dims = dims;
            self.busy = false;
        }
    }

    /// The size of the picture that goes into edit `i`.
    fn input_dims(&self, i: usize) -> (usize, usize) {
        if i == 0 {
            self.source.as_ref().map(|s| (s.w, s.h)).unwrap_or((1, 1))
        } else {
            self.dims.get(i - 1).copied().or_else(|| self.result.as_ref().map(|r| (r.w, r.h))).unwrap_or((1, 1))
        }
    }

    fn out_dims(&self) -> (usize, usize) {
        self.result.as_ref().map(|r| (r.w, r.h)).or_else(|| self.source.as_ref().map(|s| (s.w, s.h))).unwrap_or((1, 1))
    }

    // ------------------------------------------------------------------------------------------------------------ saving
    /// Write the picture as it is now over the file it came from, keeping the first original in `.skin-studio-backup` next to it.
    pub fn save(&mut self) -> Result<PathBuf, String> {
        let path = self.current.clone().ok_or("There is no picture open")?;
        if let (Some(dir), Some(name)) = (path.parent(), path.file_name()) {
            let backup = dir.join(".skin-studio-backup");
            let kept = backup.join(name);
            if path.is_file() && !kept.exists() {
                std::fs::create_dir_all(&backup).map_err(|e| e.to_string())?;
                std::fs::copy(&path, &kept).map_err(|e| e.to_string())?;
            }
        }
        self.save_as(&path)?;
        Ok(path)
    }

    pub fn save_as(&mut self, path: &Path) -> Result<(), String> {
        let img = self.result.clone().or_else(|| self.source.clone()).ok_or("There is no picture open")?;
        edit::save_png(path, &img)?;
        self.dirty = false;
        if self.dir.as_deref().is_some_and(|d| path.parent() == Some(d)) {
            self.files = list_pngs(self.dir.as_deref().unwrap_or(path));
        }
        Ok(())
    }

    /// Pack the skin folder into an .osk.
    pub fn export_osk(&self, dest: &Path) -> Result<usize, String> {
        use std::io::Write;
        let dir = self.dir.as_ref().ok_or("There is no skin open")?;
        let f = std::fs::File::create(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        let mut zip = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut count = 0;
        let mut stack = vec![dir.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).map_err(|e| e.to_string())?.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue; // the backup folder and other hidden things stay out
                }
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let rel = p.strip_prefix(dir).map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/");
                    zip.start_file(rel, opts).map_err(|e| e.to_string())?;
                    zip.write_all(&std::fs::read(&p).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
                    count += 1;
                }
            }
        }
        zip.finish().map_err(|e| e.to_string())?;
        Ok(count)
    }

    /// A texture for the picture: shrunk first when it is larger than a graphics card takes.
    fn texture(&mut self, ctx: &egui::Context, which: usize, img: &Arc<Img>) -> TextureHandle {
        let key = Arc::as_ptr(img) as usize;
        if let Some((k, t)) = &self.tex[which] {
            if *k == key {
                return t.clone();
            }
        }
        let shown = if img.w.max(img.h) > 4096 {
            let k = 4096.0 / img.w.max(img.h) as f32;
            edit::resize(img, ((img.w as f32 * k) as usize).max(1), ((img.h as f32 * k) as usize).max(1), Resample::Smooth)
        } else {
            (**img).clone()
        };
        let ci = egui::ColorImage::from_rgba_premultiplied([shown.w, shown.h], &shown.to_premult_u8());
        let t = ctx.load_texture(if which == 0 { "skin-before" } else { "skin-after" }, ci, egui::TextureOptions::NEAREST);
        self.tex[which] = Some((key, t.clone()));
        t
    }
}

// ==================================================================================================================== toolbar
/// What the toolbar shows while the skin editor is open.
pub fn toolbar(ui: &mut Ui, st: &mut Studio) {
    if th::button(ui, "Open skin", Kind::Ghost, true).clicked() {
        st.pick(PickKind::SkinEditSkin);
    }
    if th::button(ui, "Open picture", Kind::Ghost, true).clicked() {
        st.pick(PickKind::SkinEditPicture);
    }
    let can_save = st.skin_ed.current.is_some();
    if th::button(ui, "Save", Kind::Ghost, can_save).clicked() {
        do_save(st);
    }
    if th::button(ui, "Save as", Kind::Ghost, can_save).clicked() {
        st.pick(PickKind::SkinEditSaveAs);
    }
    if st.skin_ed.dirty {
        let (d, _) = ui.allocate_exact_size(Vec2::new(14.0, 14.0), Sense::hover());
        ui.painter().circle_filled(d.center(), 3.5, Color32::from_rgb(0xff, 0xad, 0x4d));
    }
    ui.add_space(10.0);
    if th::button(ui, "Export .osk", Kind::Accent, st.skin_ed.dir.is_some()).clicked() {
        st.pick(PickKind::SkinEditExportOsk);
    }
}

pub fn do_save(st: &mut Studio) {
    match st.skin_ed.save() {
        Ok(p) => st.status = format!("Saved {}", p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
        Err(e) => st.status = format!("Could not save: {e}"),
    }
}

/// The line at the right of the toolbar.
pub fn title(st: &Studio) -> String {
    let full = match (&st.skin_ed.current, &st.skin_ed.dir) {
        (Some(p), _) => p.to_string_lossy().to_string(),
        (None, Some(d)) => d.to_string_lossy().to_string(),
        _ => "No skin open".to_string(),
    };
    // the end of the path: it has to leave room for the buttons
    if full.chars().count() > 44 { format!("…{}", full.chars().skip(full.chars().count() - 43).collect::<String>()) } else { full }
}

// ==================================================================================================================== page
const PANEL_W: f32 = 470.0;

/// The whole skin editor under the toolbar: a panel with the pictures and the edits on the left, the picture on the right.
pub fn draw(root: &mut Ui, st: &mut Studio) {
    // Ctrl+Z and Ctrl+Y (or Ctrl+Shift+Z) for the edit list, unless a box is being typed in
    if !root.ctx().memory(|m| m.focused().is_some()) {
        let (z, y, zs) = root.input_mut(|i| (i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z), i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y), i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z)));
        let (tx, ctx) = (st.tx.clone(), st.ctx.clone());
        if z {
            st.skin_ed.undo(&tx, &ctx);
        } else if y || zs {
            st.skin_ed.redo(&tx, &ctx);
        }
    }
    egui::Panel::left("skin_panel").exact_size(PANEL_W).resizable(false).frame(egui::Frame::new().fill(BACKGROUND4).inner_margin(egui::Margin::ZERO)).show(root, |ui| {
        let full = ui.max_rect();
        th::vgradient(ui.painter(), full, th::lighten(BACKGROUND4, 0.05), th::darken(BACKGROUND4, 0.08));
        for i in 0..14 {
            let x = full.right() + i as f32 * 1.6;
            ui.painter().rect_filled(Rect::from_min_max(Pos2::new(x, full.top()), Pos2::new(x + 1.7, full.bottom())), 0.0, Color32::from_black_alpha((70.0 * (1.0 - i as f32 / 14.0).powi(2)) as u8));
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_width(full.width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            page(ui, st);
            ui.add_space(40.0);
        });
    });
    egui::CentralPanel::default().frame(egui::Frame::new().fill(BACKGROUND6)).show(root, |ui| {
        canvas(ui, st);
    });
}

fn page(ui: &mut Ui, st: &mut Studio) {
    th::section_header(ui, "Skin");
    // where the pictures come from
    let have_dir = st.skin_ed.dir.is_some();
    if !have_dir {
        th::note(ui, "Open a skin (a folder or an .osk) to see its pictures, or open a single picture. Edits are made on a copy in memory: nothing changes on disk until you save, and the first original of every picture you save over is kept in a .skin-studio-backup folder next to it.");
    }
    if th::button_wide(ui, "Open a skin...", Kind::Purple, true).clicked() {
        st.pick(PickKind::SkinEditSkin);
    }
    let current_skin = st.current_skin_folder();
    if let Some(dir) = current_skin {
        if th::button_wide(ui, "Edit the skin picked for the preview", Kind::Dark, st.skin_ed.dir.as_deref() != Some(dir.as_path())).clicked() {
            match st.skin_ed.open_skin(&dir) {
                Ok(()) => {
                    let (tx, ctx) = (st.tx.clone(), st.ctx.clone());
                    st.skin_ed.load_thumbs(&tx, &ctx);
                    st.status = format!("{} pictures", st.skin_ed.files.len());
                }
                Err(e) => st.status = e,
            }
        }
    }
    if th::button_wide(ui, "Open a picture...", Kind::Dark, true).clicked() {
        st.pick(PickKind::SkinEditPicture);
    }
    if let Some(d) = &st.skin_ed.dir {
        th::note(ui, &format!("{}{}", d.to_string_lossy(), if st.skin_ed.packed_from.is_some() { "  (unpacked from the .osk)" } else { "" }));
    }

    // the pictures
    if !st.skin_ed.files.is_empty() {
        th::subsection_header(ui, &format!("Pictures ({})", st.skin_ed.files.len()));
        let mut filter = std::mem::take(&mut st.skin_ed.filter);
        let r = th::text_row(ui, "skin_filter", "Search by name", &mut filter);
        let _ = r;
        st.skin_ed.filter = filter;
        let q = st.skin_ed.filter.trim().to_lowercase();
        let mut open: Option<PathBuf> = None;
        let rows: Vec<usize> = (0..st.skin_ed.files.len()).filter(|i| q.is_empty() || st.skin_ed.files[*i].name.to_lowercase().contains(&q)).collect();
        egui::ScrollArea::vertical().id_salt("skin-pictures").max_height(300.0).auto_shrink([false, true]).show(ui, |ui| {
            for i in rows.iter().take(400) {
                let f = &st.skin_ed.files[*i];
                let on = st.skin_ed.current.as_deref() == Some(f.path.as_path());
                let width = ui.available_width() - 22.0;
                let (r, resp) = ui.allocate_exact_size(Vec2::new(width, 38.0), Sense::click());
                let r = Rect::from_min_size(r.min + Vec2::new(12.0, 0.0), r.size());
                if on {
                    ui.painter().rect_filled(r, CornerRadius::same(5), Color32::from_rgba_unmultiplied(0x8c, 0x66, 0xff, 54));
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, CornerRadius::same(5), DARK2);
                }
                // the thumbnail, on a little checker so that transparent pictures can be told from empty ones
            let box_ = Rect::from_center_size(Pos2::new(r.left() + 26.0, r.center().y), Vec2::splat(30.0));
            ui.painter().rect_filled(box_, CornerRadius::same(3), Color32::from_gray(0x2a));
            for cy in 0..3 {
                for cx in 0..3 {
                    if (cx + cy) % 2 == 0 {
                        ui.painter().rect_filled(Rect::from_min_size(box_.min + Vec2::new(cx as f32 * 10.0, cy as f32 * 10.0), Vec2::splat(10.0)), 0.0, Color32::from_gray(0x36));
                    }
                }
            }
            if let Some((t, sz)) = st.skin_ed.thumbs.get(&f.path) {
                let k = (30.0 / sz[0].max(sz[1]) as f32).min(30.0 / sz[0].min(sz[1]).max(1) as f32).min(1.0e9);
                let fit = (30.0 / sz[0] as f32).min(30.0 / sz[1] as f32);
                let _ = k;
                let sz_pts = Vec2::new(sz[0] as f32 * fit.min(1.0).max(fit), sz[1] as f32 * fit.min(1.0).max(fit));
                ui.painter().image(t.id(), Rect::from_center_size(box_.center(), sz_pts.min(Vec2::splat(30.0))), Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
            ui.painter().text(r.left_center() + Vec2::new(50.0, 0.0), Align2::LEFT_CENTER, &f.name, FontId::new(14.0, FontFamily::Proportional), if on { CONTENT1 } else { CONTENT2 });
                if let Some((w, h)) = f.size {
                    ui.painter().text(r.right_center() - Vec2::new(10.0, 0.0), Align2::RIGHT_CENTER, format!("{w} × {h}"), FontId::new(12.5, FontFamily::Proportional), LIGHT3);
                }
                if resp.clicked() {
                    open = Some(f.path.clone());
                }
            }
        });
        if rows.len() > 400 {
            th::note(ui, "Only the first 400 matches are listed: type in the search box to narrow it down.");
        }
        if let Some(p) = open {
            let (tx, ctx) = (st.tx.clone(), st.ctx.clone());
            if let Err(e) = st.skin_ed.open_picture(&p, Some((&tx, &ctx))) {
                st.status = e;
            }
        }
    }

    edits(ui, st);
}

// ==================================================================================================================== the edits
fn num_row(ui: &mut Ui, bufs: &mut HashMap<String, String>, id: &str, label: &str, value: usize) -> Option<usize> {
    let cur = value.to_string();
    let buf = bufs.entry(id.to_string()).or_insert_with(|| cur.clone());
    let resp = th::text_row(ui, id, label, buf);
    let text = buf.clone();
    let mut out = None;
    if resp.changed() {
        if let Ok(v) = text.trim().parse::<usize>() {
            out = Some(v);
        }
    } else if !resp.has_focus() && text != cur {
        bufs.insert(id.to_string(), cur);
    }
    out
}

/// A row with a slider over `min..max`, showing the value; true when it moved.
fn slider(ui: &mut Ui, id: &str, label: &str, v: &mut f32, min: f32, max: f32, show: impl Fn(f32) -> String) -> bool {
    let mut f = ((*v - min) / (max - min)).clamp(0.0, 1.0);
    if th::slider_row(ui, id, label, &show(*v), &mut f) {
        *v = min + f * (max - min);
        true
    } else {
        false
    }
}

fn edits(ui: &mut Ui, st: &mut Studio) {
    if st.skin_ed.source.is_none() {
        return;
    }
    th::subsection_header(ui, "Edits");
    let before_ops = st.skin_ed.ops.clone();
    let mut changed = false;
    {
        // undo and redo
        let (tx, ctx) = (st.tx.clone(), st.ctx.clone());
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
            if th::button(ui, "Undo", Kind::Dark, !st.skin_ed.undo.is_empty()).on_hover_text("Undo the last change to the edits (Ctrl+Z)").clicked() {
                st.skin_ed.undo(&tx, &ctx);
            }
            if th::button(ui, "Redo", Kind::Dark, !st.skin_ed.redo.is_empty()).on_hover_text("Redo it (Ctrl+Y)").clicked() {
                st.skin_ed.redo(&tx, &ctx);
            }
        });
        ui.add_space(6.0);
    }
    let (mut remove, mut up) = (None, None);
    let n = st.skin_ed.ops.len();
    for i in 0..n {
        let (iw, ih) = st.skin_ed.input_dims(i);
        let keep = st.skin_ed.keep_aspect;
        // a title line with the buttons that move and remove the edit
        let width = ui.available_width() - 22.0;
        let (r, _) = ui.allocate_exact_size(Vec2::new(width, 34.0), Sense::hover());
        let r = Rect::from_min_size(r.min + Vec2::new(12.0, 4.0), Vec2::new(r.width(), 30.0));
        ui.painter().rect_filled(r, CornerRadius::same(6), BACKGROUND5);
        ui.painter().text(r.left_center() + Vec2::new(12.0, 0.0), Align2::LEFT_CENTER, format!("{}. {}", i + 1, st.skin_ed.ops[i].name()), FontId::new(15.0, th::semibold()), CONTENT1);
        let btn = |ui: &mut Ui, id: &str, right: f32, label: &str, tip: &str| -> bool {
            let b = Rect::from_center_size(Pos2::new(r.right() - right, r.center().y), Vec2::new(28.0, 24.0));
            let resp = ui.interact(b, ui.scope_id().with((id, i)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip);
            if resp.hovered() {
                ui.painter().rect_filled(b, CornerRadius::same(5), DARK3);
            }
            ui.painter().text(b.center(), Align2::CENTER_CENTER, label, FontId::new(14.0, th::semibold()), if resp.hovered() { CONTENT1 } else { LIGHT1 });
            resp.clicked()
        };
        if btn(ui, "rm", 18.0, "x", "Remove this edit") {
            remove = Some(i);
        }
        if i > 0 && btn(ui, "up", 50.0, "^", "Move it up (earlier)") {
            up = Some(i);
        }
        let SkinEditor { ops, bufs, .. } = &mut st.skin_ed;
        changed |= op_controls(ui, bufs, &mut ops[i], i, (iw, ih), keep);
        ui.add_space(6.0);
    }
    if let Some(i) = remove {
        st.skin_ed.ops.remove(i);
        st.skin_ed.bufs.clear();
        changed = true;
    }
    if let Some(i) = up {
        st.skin_ed.ops.swap(i - 1, i);
        st.skin_ed.bufs.clear();
        changed = true;
    }
    if n == 0 {
        th::note(ui, "Nothing yet. Add an edit below; they are applied from the first to the last.");
    }

    // add an edit
    th::subsection_header(ui, "Add an edit");
    let (cw, ch) = st.skin_ed.out_dims();
    let mut add: Option<Op> = None;
    ui.horizontal_wrapped(|ui| {
        ui.add_space(12.0);
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        let items: Vec<(&str, Op)> = vec![
            ("Resize", Op::Resize { w: cw, h: ch, mode: Resample::Smooth }),
            ("Flip left-right", Op::Flip { horizontal: true }),
            ("Flip up-down", Op::Flip { horizontal: false }),
            ("Rotate right", Op::Rotate { quarter_turns: 1 }),
            ("Rotate left", Op::Rotate { quarter_turns: 3 }),
            ("Crop", Op::Crop { x: 0, y: 0, w: cw, h: ch }),
            ("Trim border", Op::Trim { threshold: 0.0 }),
            ("Colour", Op::Adjust { brightness: 0.0, contrast: 0.0, saturation: 0.0, hue: 0.0 }),
            ("Tint", Op::Tint { rgb: [1.0, 0.4, 0.6], strength: 0.5 }),
            ("Opacity", Op::Opacity { factor: 1.0 }),
            ("Invert", Op::Invert),
            ("Blur", Op::Blur { sigma: 1.5 }),
            ("Sharpen", Op::Sharpen { amount: 0.8, sigma: 1.2 }),
            ("Deband", Op::Deband { strength: 0.6, grain: 0.0 }),
        ];
        for (label, op) in items {
            if th::button(ui, label, Kind::Dark, true).clicked() {
                add = Some(op);
            }
        }
    });
    if let Some(op) = add {
        st.skin_ed.ops.push(op);
        st.skin_ed.bufs.clear();
        changed = true;
    }
    if !st.skin_ed.ops.is_empty() && th::button_wide(ui, "Remove every edit", Kind::Dark, true).clicked() {
        st.skin_ed.ops.clear();
        st.skin_ed.bufs.clear();
        changed = true;
    }
    if changed && st.skin_ed.ops != before_ops {
        st.skin_ed.push_undo(before_ops);
        st.skin_ed.dirty = true;
        let (tx, ctx) = (st.tx.clone(), st.ctx.clone());
        st.skin_ed.recompute(&tx, &ctx);
    }
}

/// The settings of one edit; true when something changed.
fn op_controls(ui: &mut Ui, bufs: &mut HashMap<String, String>, op: &mut Op, i: usize, (iw, ih): (usize, usize), keep_aspect: bool) -> bool {
    let mut ch = false;
    match op {
        Op::Resize { w, h, mode } => {
            if let Some(v) = num_row(ui, bufs, &format!("rw{i}"), "Width (px)", *w) {
                if v > 0 {
                    *w = v.min(16384);
                    if keep_aspect {
                        *h = ((*w as f64 * ih as f64 / iw.max(1) as f64).round() as usize).clamp(1, 16384);
                        bufs.remove(&format!("rh{i}"));
                    }
                    ch = true;
                }
            }
            if let Some(v) = num_row(ui, bufs, &format!("rh{i}"), "Height (px)", *h) {
                if v > 0 {
                    *h = v.min(16384);
                    if keep_aspect {
                        *w = ((*h as f64 * iw as f64 / ih.max(1) as f64).round() as usize).clamp(1, 16384);
                        bufs.remove(&format!("rw{i}"));
                    }
                    ch = true;
                }
            }
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                for (label, f) in [("× 2", 2.0f64), ("÷ 2", 0.5), ("× 0.75", 0.75), ("× 1.5", 1.5)] {
                    if th::button(ui, label, Kind::Dark, true).clicked() {
                        *w = ((iw as f64 * f).round() as usize).clamp(1, 16384);
                        *h = ((ih as f64 * f).round() as usize).clamp(1, 16384);
                        bufs.remove(&format!("rw{i}"));
                        bufs.remove(&format!("rh{i}"));
                        ch = true;
                    }
                }
            });
            ui.add_space(6.0);
            let names: Vec<String> = Resample::ALL.iter().map(|m| m.name().to_string()).collect();
            if let Some(k) = th::dropdown_row(ui, &format!("rm{i}"), "How to resample", mode.name(), &names) {
                *mode = Resample::ALL[k];
                ch = true;
            }
            th::note(ui, "Smaller always averages the pixels each new one covers, so nothing rings. Larger: Smooth blends, Crisp is sharper but never overshoots, Pixels keeps hard pixels.");
        }
        Op::Flip { horizontal } => {
            let mut h = *horizontal;
            if th::switch_row(ui, &format!("fl{i}"), "Left to right (off: top to bottom)", &mut h) {
                *horizontal = h;
                ch = true;
            }
        }
        Op::Rotate { quarter_turns } => {
            let names = ["90° to the right".to_string(), "180°".to_string(), "90° to the left".to_string()];
            let cur = names[(*quarter_turns as usize + 3) % 4 % 3].clone();
            if let Some(k) = th::dropdown_row(ui, &format!("ro{i}"), "Turn", &cur, &names) {
                *quarter_turns = [1, 2, 3][k];
                ch = true;
            }
        }
        Op::Crop { x, y, w, h } => {
            for (id, label, val, hi) in [("cx", "Left (px)", x as &mut usize, iw), ("cy", "Top (px)", y, ih), ("cw", "Width (px)", w, iw), ("chh", "Height (px)", h, ih)] {
                if let Some(v) = num_row(ui, bufs, &format!("{id}{i}"), label, *val) {
                    *val = v.min(hi);
                    ch = true;
                }
            }
        }
        Op::Trim { threshold } => {
            ch |= slider(ui, &format!("tr{i}"), "Counts as empty up to an alpha of", threshold, 0.0, 0.5, |v| format!("{:.0}%", v * 100.0));
        }
        Op::Adjust { brightness, contrast, saturation, hue } => {
            ch |= slider(ui, &format!("ab{i}"), "Brightness", brightness, -1.0, 1.0, |v| format!("{:+.0}", v * 100.0));
            ch |= slider(ui, &format!("ac{i}"), "Contrast", contrast, -1.0, 1.0, |v| format!("{:+.0}", v * 100.0));
            ch |= slider(ui, &format!("as{i}"), "Saturation", saturation, -1.0, 1.0, |v| format!("{:+.0}", v * 100.0));
            ch |= slider(ui, &format!("ah{i}"), "Hue", hue, -180.0, 180.0, |v| format!("{v:+.0}°"));
        }
        Op::Tint { rgb, strength } => {
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                let mut c = Color32::from_rgb((rgb[0] * 255.0) as u8, (rgb[1] * 255.0) as u8, (rgb[2] * 255.0) as u8);
                if egui::color_picker::color_edit_button_srgba(ui, &mut c, egui::color_picker::Alpha::Opaque).changed() {
                    *rgb = [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0];
                    ch = true;
                }
            });
            ch |= slider(ui, &format!("ts{i}"), "Strength", strength, 0.0, 1.0, |v| format!("{:.0}%", v * 100.0));
        }
        Op::Opacity { factor } => {
            ch |= slider(ui, &format!("op{i}"), "Opacity", factor, 0.0, 1.0, |v| format!("{:.0}%", v * 100.0));
        }
        Op::Invert => {}
        Op::Blur { sigma } => {
            ch |= slider(ui, &format!("bl{i}"), "Blur radius", sigma, 0.1, 20.0, |v| format!("{v:.1} px"));
        }
        Op::Sharpen { amount, sigma } => {
            ch |= slider(ui, &format!("sa{i}"), "Amount", amount, 0.0, 3.0, |v| format!("{:.0}%", v * 100.0));
            ch |= slider(ui, &format!("ss{i}"), "Radius", sigma, 0.3, 6.0, |v| format!("{v:.1} px"));
            th::note(ui, "Held between the pixels around each one, so it leaves no halos.");
        }
        Op::Deband { strength, grain } => {
            ch |= slider(ui, &format!("db{i}"), "Strength", strength, 0.0, 1.0, |v| format!("{:.0}%", v * 100.0));
            ch |= slider(ui, &format!("dg{i}"), "Dither grain", grain, 0.0, 4.0, |v| format!("{v:.1} levels"));
        }
    }
    ch
}

// ==================================================================================================================== the picture
fn checker(ui: &Ui, area: Rect, clip: Rect) {
    let p = ui.painter().with_clip_rect(clip);
    let s = 14.0;
    p.rect_filled(area, 0.0, Color32::from_gray(0x2a));
    let (x0, y0) = (((clip.left() - area.left()) / s).floor().max(0.0) as i32, ((clip.top() - area.top()) / s).floor().max(0.0) as i32);
    let (x1, y1) = (((clip.right() - area.left()) / s).ceil() as i32, ((clip.bottom() - area.top()) / s).ceil() as i32);
    for y in y0..y1 {
        for x in x0..x1 {
            if (x + y) % 2 == 0 {
                p.rect_filled(Rect::from_min_size(area.min + Vec2::new(x as f32 * s, y as f32 * s), Vec2::splat(s)), 0.0, Color32::from_gray(0x36));
            }
        }
    }
}

fn canvas(ui: &mut Ui, st: &mut Studio) {
    let full = ui.max_rect();
    let bar = Rect::from_min_size(full.min, Vec2::new(full.width(), 44.0));
    let area = Rect::from_min_max(Pos2::new(full.left(), bar.bottom()), full.max);
    let ctx = ui.ctx().clone();
    let ppp = ctx.pixels_per_point();
    let ed = &mut st.skin_ed;
    // --- the bar over the picture
    ui.painter().rect_filled(bar, 0.0, BACKGROUND5);
    let x = std::cell::Cell::new(bar.left() + 12.0);
    let place = |w: f32| {
        let r = Rect::from_min_size(Pos2::new(x.get(), bar.top() + 7.0), Vec2::new(w, 30.0));
        x.set(x.get() + w + 6.0);
        r
    };
    let pill = |ui: &mut Ui, r: Rect, label: &str, on: bool, tip: &str| -> bool {
        let resp = ui.interact(r, ui.scope_id().with(("sk-pill", label)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip);
        ui.painter().rect_filled(r, CornerRadius::same(6), if on { Color32::from_rgb(0x66, 0x3d, 0xd4) } else if resp.hovered() { DARK3 } else { BACKGROUND6 });
        ui.painter().text(r.center(), Align2::CENTER_CENTER, label, FontId::new(13.5, th::semibold()), if on || resp.hovered() { CONTENT1 } else { LIGHT1 });
        resp.clicked()
    };
    for (s, label, tip) in [(Show::After, "After", "The picture with the edits"), (Show::Before, "Before", "The picture as it was loaded"), (Show::Split, "Split", "Before on the left, after on the right")] {
        let r = place(if s == Show::Split { 58.0 } else { 62.0 });
        if pill(ui, r, label, ed.show == s, tip) {
            ed.show = s;
        }
    }
    x.set(x.get() + 12.0);
    let r = place(44.0);
    if pill(ui, r, "Fit", ed.zoom == 0.0, "Fit the picture in the window") {
        ed.zoom = 0.0;
        ed.pan = Vec2::ZERO;
    }
    for (label, z) in [("100%", 1.0 / ppp), ("200%", 2.0 / ppp), ("400%", 4.0 / ppp), ("800%", 8.0 / ppp)] {
        let r = place(52.0);
        if pill(ui, r, label, (ed.zoom - z).abs() < 1e-3, "Show the picture at this size: one pixel of it is this many pixels of the screen") {
            ed.zoom = z;
            ed.pan = Vec2::ZERO;
        }
    }
    x.set(x.get() + 12.0);
    for (b, label) in [(Bg::Checker, "Checker"), (Bg::Dark, "Dark"), (Bg::Light, "Light")] {
        let r = place(if b == Bg::Checker { 70.0 } else { 52.0 });
        if pill(ui, r, label, ed.bg == b, "What is behind the picture") {
            ed.bg = b;
        }
    }

    // --- the picture
    let Some(after) = ed.result.clone().or_else(|| ed.source.clone()) else {
        ui.painter().rect_filled(area, 0.0, BACKGROUND6);
        ui.painter().text(area.center(), Align2::CENTER_CENTER, "Open a skin or a picture, then pick a picture from the list", FontId::new(16.0, th::light()), LIGHT3);
        return;
    };
    let before = ed.source.clone().unwrap_or_else(|| after.clone());
    let (iw, ih) = (after.w as f32, after.h as f32);
    let fit = ((area.width() - 40.0) / iw).min((area.height() - 40.0) / ih).max(0.01);
    let z = if ed.zoom == 0.0 { fit } else { ed.zoom };
    let resp = ui.interact(area, ui.scope_id().with("sk-canvas"), Sense::click_and_drag());
    if resp.dragged() && ed.show != Show::Split {
        ed.pan += resp.drag_delta();
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.1 {
            let old = z;
            let new = (old * (scroll * 0.003).exp()).clamp(0.02, 64.0);
            if let Some(m) = ui.input(|i| i.pointer.hover_pos()) {
                // keep the point under the pointer where it is
                let c = area.center() + ed.pan;
                ed.pan = (m - area.center()) - (m - c) * (new / old);
            }
            ed.zoom = new;
        }
    }
    let z = if ed.zoom == 0.0 { fit } else { ed.zoom };
    let size = Vec2::new(iw * z, ih * z);
    let rect = Rect::from_center_size(area.center() + ed.pan, size);
    let clip = area;
    match ed.bg {
        Bg::Checker => checker(ui, rect, rect.intersect(clip)),
        Bg::Dark => {
            ui.painter().with_clip_rect(clip).rect_filled(rect, 0.0, Color32::from_gray(0x12));
        }
        Bg::Light => {
            ui.painter().with_clip_rect(clip).rect_filled(rect, 0.0, Color32::from_gray(0xe6));
        }
    }
    let painter = ui.painter().with_clip_rect(clip);
    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    let tex_after = ed.texture(&ctx, 1, &after);
    match ed.show {
        Show::After => {
            painter.image(tex_after.id(), rect, uv, Color32::WHITE);
        }
        Show::Before => {
            let t = ed.texture(&ctx, 0, &before);
            let r = Rect::from_center_size(rect.center(), Vec2::new(before.w as f32 * z, before.h as f32 * z));
            painter.image(t.id(), r, uv, Color32::WHITE);
        }
        Show::Split => {
            let t = ed.texture(&ctx, 0, &before);
            let sx = area.left() + area.width() * ed.split;
            let r = Rect::from_center_size(rect.center(), Vec2::new(before.w as f32 * z, before.h as f32 * z));
            ui.painter().with_clip_rect(Rect::from_min_max(area.min, Pos2::new(sx, area.bottom()))).image(t.id(), r, uv, Color32::WHITE);
            ui.painter().with_clip_rect(Rect::from_min_max(Pos2::new(sx, area.top()), area.max)).image(tex_after.id(), rect, uv, Color32::WHITE);
            let h = Rect::from_center_size(Pos2::new(sx, area.center().y), Vec2::new(18.0, area.height()));
            let hr = ui.interact(h, ui.scope_id().with("sk-split"), Sense::drag()).on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
            if hr.dragged() {
                ed.split = ((sx + hr.drag_delta().x - area.left()) / area.width()).clamp(0.02, 0.98);
            }
            painter.line_segment([Pos2::new(sx, area.top()), Pos2::new(sx, area.bottom())], Stroke::new(2.0, Color32::from_rgb(0xff, 0x66, 0xab)));
            painter.circle_filled(Pos2::new(sx, area.center().y), 7.0, Color32::from_rgb(0xff, 0x66, 0xab));
        }
    }
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::from_white_alpha(30)), egui::StrokeKind::Outside);
    // --- what it is
    let name = ed.current.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let info = if (before.w, before.h) == (after.w, after.h) { format!("{name}   {} × {} px   {:.0}%", after.w, after.h, z * ppp * 100.0) } else { format!("{name}   {} × {} px  →  {} × {} px   {:.0}%", before.w, before.h, after.w, after.h, z * ppp * 100.0) };
    painter.text(Pos2::new(area.left() + 16.0, area.bottom() - 14.0), Align2::LEFT_CENTER, info + if ed.busy { "   working..." } else { "" }, FontId::new(13.0, FontFamily::Proportional), Color32::from_white_alpha(170));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ss-skined-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn picture(w: usize, h: usize) -> Img {
        let mut rgba = vec![];
        for y in 0..h {
            for x in 0..w {
                rgba.extend([(x * 255 / w.max(1)) as u8, (y * 255 / h.max(1)) as u8, 128, if (x + y) % 5 == 0 { 120 } else { 255 }]);
            }
        }
        Img::from_straight_u8(w, h, &rgba)
    }

    #[test]
    fn a_skin_folder_lists_its_pictures_with_their_sizes() {
        let d = tmp("list");
        edit::save_png(&d.join("hitcircle.png"), &picture(20, 10)).unwrap();
        edit::save_png(&d.join("hitcircle@2x.png"), &picture(40, 20)).unwrap();
        std::fs::create_dir_all(d.join("extra")).unwrap();
        edit::save_png(&d.join("extra").join("cursor.png"), &picture(8, 8)).unwrap();
        std::fs::write(d.join("skin.ini"), "[General]").unwrap();
        let mut ed = SkinEditor::default();
        ed.open_skin(&d).unwrap();
        let names: Vec<&str> = ed.files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["extra/cursor.png", "hitcircle.png", "hitcircle@2x.png"]);
        assert_eq!(ed.files[2].size, Some((40, 20)));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn edits_save_over_the_file_keep_a_backup_and_pack_into_an_osk() {
        let d = tmp("save");
        let file = d.join("hitcircle@2x.png");
        let original = picture(40, 20);
        edit::save_png(&file, &original).unwrap();
        let mut ed = SkinEditor::default();
        ed.open_skin(&d).unwrap();
        ed.open_picture(&file, None).unwrap();
        // the @2x picture made into the @1x one
        ed.ops = vec![Op::Resize { w: 20, h: 10, mode: Resample::Smooth }];
        let (img, dims) = compute(ed.source.as_ref().unwrap(), &ed.ops);
        assert_eq!(dims, vec![(20, 10)]);
        ed.got_result(0, Arc::new(img.clone()), dims);
        ed.gen = 0;
        ed.got_result(0, Arc::new(img), vec![(20, 10)]);
        let saved = ed.save().unwrap();
        assert_eq!(saved, file);
        let back = edit::load_png(&file).unwrap();
        assert_eq!((back.w, back.h), (20, 10));
        // the first original is kept, and kept as it was after a second save
        let kept = d.join(".skin-studio-backup").join("hitcircle@2x.png");
        assert_eq!(edit::load_png(&kept).unwrap().w, 40);
        ed.save().unwrap();
        assert_eq!(edit::load_png(&kept).unwrap().w, 40, "the backup is not overwritten");
        // save as another name lists it
        ed.save_as(&d.join("hitcircle.png")).unwrap();
        assert!(ed.files.iter().any(|f| f.name == "hitcircle.png"));
        // an .osk of the folder has the pictures and not the backup
        let osk = d.join("out.osk");
        std::fs::write(d.join("skin.ini"), "[General]\nName: x\n").unwrap();
        let n = ed.export_osk(&osk).unwrap();
        assert!(n >= 3);
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&osk).unwrap()).unwrap();
        let names: Vec<String> = (0..zip.len()).map(|i| zip.by_index(i).unwrap().name().to_string()).collect();
        assert!(names.contains(&"hitcircle.png".to_string()) && names.contains(&"skin.ini".to_string()), "{names:?}");
        assert!(!names.iter().any(|n| n.contains("backup")), "{names:?}");
        // and an .osk opens again as a skin
        let mut ed2 = SkinEditor::default();
        ed2.open_skin(&osk).unwrap();
        assert!(ed2.files.iter().any(|f| f.name == "hitcircle.png"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn undo_and_redo_walk_the_edit_list() {
        let ctx = egui::Context::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ed = SkinEditor::default();
        ed.source = Some(Arc::new(Img::new(8, 8)));
        // three changes spaced out in time are three steps; changes that follow at once are one
        let blur = |s| Op::Blur { sigma: s };
        for (i, ops) in [vec![blur(1.0)], vec![blur(1.0), Op::Invert], vec![blur(1.0), Op::Invert, Op::Flip { horizontal: true }]].into_iter().enumerate() {
            let before = ed.ops.clone();
            ed.ops = ops;
            ed.push_undo(before);
            ed.last_edit = Some(std::time::Instant::now() - std::time::Duration::from_secs(2));
            assert_eq!(ed.undo.len(), i + 1);
        }
        ed.undo(&tx, &ctx);
        assert_eq!(ed.ops.len(), 2);
        ed.undo(&tx, &ctx);
        ed.undo(&tx, &ctx);
        assert!(ed.ops.is_empty());
        ed.undo(&tx, &ctx);
        assert!(ed.ops.is_empty() && ed.undo.is_empty(), "nothing left to undo");
        ed.redo(&tx, &ctx);
        ed.redo(&tx, &ctx);
        assert_eq!(ed.ops.len(), 2);
        // a new change drops what could have been redone
        let before = ed.ops.clone();
        ed.ops.push(blur(2.0));
        ed.last_edit = None;
        ed.push_undo(before);
        assert!(ed.redo.is_empty());
        // a slider dragged over a second is one step
        ed.last_edit = Some(std::time::Instant::now() - std::time::Duration::from_secs(2));
        let n = ed.undo.len();
        for s in 0..20 {
            let before = ed.ops.clone();
            *ed.ops.last_mut().unwrap() = blur(3.0 + s as f32 * 0.1);
            ed.push_undo(before);
        }
        assert_eq!(ed.undo.len(), n + 1, "{} steps", ed.undo.len() - n);
    }

    #[test]
    fn a_thumbnail_fits_its_box_and_keeps_the_proportions() {
        let (size, rgba) = thumbnail(&picture(200, 100), 48);
        assert_eq!(size, [48, 24]);
        assert_eq!(rgba.len(), 48 * 24 * 4);
        let (small, _) = thumbnail(&picture(10, 20), 48);
        assert_eq!(small, [10, 20], "small pictures are not enlarged");
    }

    #[test]
    fn thumbnails_of_an_older_list_are_thrown_away() {
        let ctx = egui::Context::default();
        let mut ed = SkinEditor::default();
        ed.thumb_gen = 3;
        ed.got_thumb(&ctx, 2, PathBuf::from("a.png"), [1, 1], vec![0, 0, 0, 255]);
        assert!(ed.thumbs.is_empty());
        ed.got_thumb(&ctx, 3, PathBuf::from("a.png"), [1, 1], vec![0, 0, 0, 255]);
        assert_eq!(ed.thumbs.len(), 1);
        ed.got_thumb(&ctx, 3, PathBuf::from("b.png"), [2, 2], vec![0; 3]);
        assert_eq!(ed.thumbs.len(), 1, "a broken one is ignored");
    }

    #[test]
    fn a_result_from_an_older_list_of_edits_is_ignored() {
        let mut ed = SkinEditor::default();
        ed.gen = 5;
        ed.got_result(4, Arc::new(Img::new(3, 3)), vec![]);
        assert!(ed.result.is_none());
        ed.got_result(5, Arc::new(Img::new(3, 3)), vec![(3, 3)]);
        assert!(ed.result.is_some() && !ed.busy);
    }

    #[test]
    fn the_editor_draws_empty_and_with_a_picture_at_any_size() {
        let ctx = egui::Context::default();
        let mut st = Studio::new(ctx.clone());
        for (w, h) in [(400.0, 300.0), (1280.0, 720.0), (2560.0, 1440.0)] {
            for show in [Show::After, Show::Before, Show::Split] {
                st.skin_ed.show = show;
                let mut raw = egui::RawInput::default();
                raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h)));
                let _ = ctx.run_ui(raw, |ui| draw(ui, &mut st));
            }
        }
        let d = tmp("draw");
        let file = d.join("a.png");
        edit::save_png(&file, &picture(64, 32)).unwrap();
        st.skin_ed.open_skin(&d).unwrap();
        st.skin_ed.open_picture(&file, None).unwrap();
        st.skin_ed.ops = vec![Op::Adjust { brightness: 0.1, contrast: 0.0, saturation: 0.0, hue: 0.0 }, Op::Deband { strength: 0.5, grain: 0.0 }, Op::Trim { threshold: 0.0 }, Op::Tint { rgb: [1.0, 0.0, 0.0], strength: 0.3 }, Op::Rotate { quarter_turns: 1 }, Op::Crop { x: 0, y: 0, w: 5, h: 5 }];
        let (img, dims) = compute(st.skin_ed.source.as_ref().unwrap(), &st.skin_ed.ops);
        st.skin_ed.dims = dims;
        st.skin_ed.result = Some(Arc::new(img));
        for (w, h) in [(400.0, 300.0), (1280.0, 720.0), (2560.0, 1440.0)] {
            for show in [Show::After, Show::Before, Show::Split] {
                for bg in [Bg::Checker, Bg::Dark, Bg::Light] {
                    st.skin_ed.show = show;
                    st.skin_ed.bg = bg;
                    let mut raw = egui::RawInput::default();
                    raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h)));
                    let _ = ctx.run_ui(raw, |ui| draw(ui, &mut st));
                }
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
