//! The skin editor: open a skin (a folder or an .osk) or a single picture, pick an asset, edit it with a list of edits (resize without
//! ringing, flip, rotate, crop, colour, blur, sharpen, deband...), see it before and after, and save it back.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use egui::{Align2, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Sense, Stroke, TextureHandle, Ui, Vec2};
use engine::edit::{self, Fill, Grain, Op, Resample};
use engine::restore::{AntiAlias, Dehalo, Denoise, DenoiseKind, Edge, LumaScale, Mask, Target};
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
    Grey,
    Dark,
    Light,
}

/// The pages of the panel on the left (as the Followpoints tab has its own), so that nothing needs a long scroll.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SkinPage {
    Pictures,
    Edits,
    Output,
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

/// A picture ready for the screen: premultiplied RGBA bytes, shrunk to what a graphics card takes. It is made on the worker.
pub struct Display {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
}

pub fn make_display(img: &Img, output: edit::Output) -> Display {
    let shown;
    let src = if img.w.max(img.h) > 4096 {
        let k = 4096.0 / img.w.max(img.h) as f32;
        shown = edit::resize(img, ((img.w as f32 * k).round() as usize).max(1), ((img.h as f32 * k).round() as usize).max(1), Resample::Smooth);
        &shown
    } else {
        img
    };
    // rounded as the saved file is (with the same dither), so that what is seen is what is saved; premultiplied for the screen
    let straight = edit::to_rgba8_with(src, output.dither && output.depth == edit::Depth::Bits8);
    let mut rgba = Vec::with_capacity(straight.len());
    for c in straight.chunks_exact(4) {
        let a = c[3] as u32;
        rgba.extend_from_slice(&[((c[0] as u32 * a + 127) / 255) as u8, ((c[1] as u32 * a + 127) / 255) as u8, ((c[2] as u32 * a + 127) / 255) as u8, c[3]]);
    }
    Display { size: [src.w, src.h], rgba }
}

/// What the worker is asked to make: the newest request replaces one that has not been started yet.
struct Job {
    gen: u64,
    src: Arc<Img>,
    ops: Vec<Op>,
    output: edit::Output,
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
    result_display: Option<Arc<Display>>,
    pub page: SkinPage,
    /// The list of ways to add an edit is open.
    pub add_open: bool,
    /// Edits that are folded to their title line.
    collapsed: std::collections::HashSet<usize>,
    /// How the picture is stored when it is saved (everything before that is in 32-bit floating point).
    pub output: edit::Output,
    slot: Arc<std::sync::Mutex<Option<Job>>>,
    running: Arc<std::sync::atomic::AtomicBool>,
    latest: Arc<std::sync::atomic::AtomicU64>,
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
        SkinEditor { dir: None, packed_from: None, files: vec![], filter: String::new(), current: None, source: None, ops: vec![], result: None, result_display: None, output: edit::Output::default(), page: SkinPage::Pictures, add_open: false, collapsed: Default::default(), slot: Arc::new(std::sync::Mutex::new(None)), running: Arc::new(std::sync::atomic::AtomicBool::new(false)), latest: Arc::new(std::sync::atomic::AtomicU64::new(0)), dims: vec![], gen: 0, busy: false, dirty: false, tex: [None, None], zoom: 0.0, pan: Vec2::ZERO, bg: Bg::Dark, show: Show::After, split: 0.5, keep_aspect: true, bufs: HashMap::new(), undo: vec![], redo: vec![], last_edit: None, thumbs: HashMap::new(), thumb_gen: 0 }
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
#[cfg(test)]
pub fn compute(source: &Img, ops: &[Op]) -> (Img, Vec<(usize, usize)>) {
    compute_until(source, ops, &|| false).unwrap_or_else(|| (source.clone(), vec![]))
}

/// The same, but given up (`None`) between two edits as soon as `stale` says that a newer list of edits is waiting.
fn compute_until(source: &Img, ops: &[Op], stale: &dyn Fn() -> bool) -> Option<(Img, Vec<(usize, usize)>)> {
    let mut cur = source.clone();
    let mut dims = Vec::with_capacity(ops.len());
    for op in ops {
        if stale() {
            return None;
        }
        cur = edit::apply(&cur, op);
        dims.push((cur.w, cur.h));
    }
    Some((cur, dims))
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
        self.page = SkinPage::Pictures;
        self.clear_picture();
        Ok(())
    }

    /// Open a single picture (its folder becomes the list when no skin is open).
    pub fn open_picture(&mut self, path: &Path, ctx_tx: Option<(&Sender<Msg>, &egui::Context)>) -> Result<(), String> {
        let img = edit::load_png(path)?;
        self.page = SkinPage::Edits;
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

    /// The edits changed: make the picture again on a worker. There is one worker at a time and it always goes for the newest list of
    /// edits: dragging a slider asks for dozens, and only the last of those that has not been started is made.
    pub fn recompute(&mut self, tx: &Sender<Msg>, ctx: &egui::Context) {
        use std::sync::atomic::Ordering;
        let Some(src) = self.source.clone() else { return };
        self.gen += 1;
        self.busy = true;
        self.latest.store(self.gen, Ordering::SeqCst);
        *self.slot.lock().unwrap() = Some(Job { gen: self.gen, src, ops: self.ops.clone(), output: self.output });
        if self.running.swap(true, Ordering::SeqCst) {
            return; // the worker that is running picks the new request up when it is done
        }
        let (slot, running, latest, tx, ctx) = (self.slot.clone(), self.running.clone(), self.latest.clone(), tx.clone(), ctx.clone());
        std::thread::spawn(move || loop {
            let job = slot.lock().unwrap().take();
            match job {
                Some(j) => {
                    let stale = || latest.load(Ordering::SeqCst) != j.gen;
                    if let Some((img, dims)) = compute_until(&j.src, &j.ops, &stale) {
                        if !stale() {
                            let display = make_display(&img, j.output);
                            let _ = tx.send(Msg::SkinResult(j.gen, Arc::new(img), dims, Arc::new(display)));
                            ctx.request_repaint();
                        }
                    }
                }
                None => {
                    running.store(false, Ordering::SeqCst);
                    // a request that came in just now: take it up again unless another worker already has
                    if slot.lock().unwrap().is_some() && !running.swap(true, Ordering::SeqCst) {
                        continue;
                    }
                    break;
                }
            }
        });
    }

    pub fn got_result(&mut self, gen: u64, img: Arc<Img>, dims: Vec<(usize, usize)>, display: Option<Arc<Display>>) {
        if gen == self.gen {
            self.result = Some(img);
            self.result_display = display;
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

    pub fn out_dims(&self) -> (usize, usize) {
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
        edit::save_png_with(path, &img, self.output)?;
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
        let ready = if which == 1 && self.result.as_ref().is_some_and(|r| Arc::as_ptr(r) as usize == key) { self.result_display.clone() } else { None };
        let ci = match ready {
            Some(d) => egui::ColorImage::from_rgba_premultiplied(d.size, &d.rgba),
            None => {
                let d = make_display(img, self.output);
                egui::ColorImage::from_rgba_premultiplied(d.size, &d.rgba)
            }
        };
        let t = ctx.load_texture(if which == 0 { "skin-before" } else { "skin-after" }, ci, egui::TextureOptions::NEAREST);
        self.tex[which] = Some((key, t.clone()));
        t
    }
}

// ==================================================================================================================== toolbar
/// What the toolbar shows while the skin editor is open.
pub fn toolbar(ui: &mut Ui, st: &mut Studio, right: f32) {
    ui.spacing_mut().item_spacing.x = 8.0; // the pills of neighbouring buttons need air between them
    let can_save = st.skin_ed.current.is_some();
    let has_dir = st.skin_ed.dir.is_some();
    // (label, kind, enabled, how long it stays when the bar is narrow: the larger, the sooner it goes into the "More" menu)
    let items: [(&str, Kind, bool, u8); 6] = [("Open skin", Kind::Ghost, true, 0), ("Open folder", Kind::Ghost, true, 3), ("Open picture", Kind::Ghost, true, 2), ("Save", Kind::Ghost, can_save, 0), ("Save as", Kind::Ghost, can_save, 4), ("Export .osk", Kind::Accent, has_dir, 1)];
    let font = FontId::new(15.0, th::semibold());
    let width = |ui: &Ui, label: &str| ui.painter().layout_no_wrap(label.to_string(), font.clone(), CONTENT1).size().x + 28.0 + 8.0;
    let widths: Vec<f32> = items.iter().map(|i| width(ui, i.0)).collect();
    let more_w = width(ui, "More");
    let budget = right - ui.cursor().left() - 40.0; // (the gaps between the groups, and the dot of unsaved changes)
    let mut shown = [true; 6];
    let total = |shown: &[bool; 6]| -> f32 { widths.iter().zip(shown).filter(|(_, s)| **s).map(|(w, _)| w).sum::<f32>() + if shown.iter().any(|s| !*s) { more_w } else { 0.0 } };
    while total(&shown) > budget {
        // the one that goes first is the one that matters least that is still there
        let Some(k) = (0..6).filter(|k| shown[*k]).max_by_key(|k| items[*k].3) else { break };
        if items[k].3 == 0 && (0..6).filter(|k| shown[*k]).all(|k| items[k].3 == 0) {
            break;
        }
        shown[k] = false;
    }
    let mut clicked: Option<usize> = None;
    for (k, (label, kind, enabled, _)) in items.iter().enumerate() {
        if k == 3 || k == 5 {
            ui.add_space(10.0); // opening and saving are two groups, and Export is apart
        }
        if shown[k] && th::button(ui, label, *kind, *enabled).clicked() {
            clicked = Some(k);
        }
        if k == 4 && st.skin_ed.dirty && shown[3] {
            let (d, _) = ui.allocate_exact_size(Vec2::new(14.0, 14.0), Sense::hover());
            ui.painter().circle_filled(d.center(), 3.5, Color32::from_rgb(0xff, 0xad, 0x4d));
        }
    }
    if shown.iter().any(|s| !*s) {
        let resp = th::button(ui, "More", Kind::Ghost, true);
        egui::Popup::menu(&resp).gap(4.0).show(|ui| {
            ui.set_min_width(150.0);
            for (k, (label, _, enabled, _)) in items.iter().enumerate() {
                if shown[k] {
                    continue;
                }
                let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width().max(140.0), 30.0), Sense::click());
                if resp.hovered() && *enabled {
                    ui.painter().rect_filled(r, CornerRadius::same(4), DARK3);
                }
                ui.painter().text(r.left_center() + Vec2::new(10.0, 0.0), Align2::LEFT_CENTER, label, FontId::new(14.5, FontFamily::Proportional), if *enabled { CONTENT1 } else { LIGHT3 });
                if resp.clicked() && *enabled {
                    clicked = Some(k);
                    ui.close();
                }
            }
        });
    }
    match clicked {
        Some(0) => st.pick(PickKind::SkinEditSkin),
        Some(1) => st.pick(PickKind::SkinEditSkinFolder),
        Some(2) => st.pick(PickKind::SkinEditPicture),
        Some(3) => do_save(st),
        Some(4) => st.pick(PickKind::SkinEditSaveAs),
        Some(5) => st.pick(PickKind::SkinEditExportOsk),
        _ => {}
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
    full
}

// ==================================================================================================================== page
const PANEL_W: f32 = 470.0;
const SIDEBAR_W: f32 = 152.0;

/// The whole skin editor under the toolbar: a panel on the left that is laid out like the one of the Followpoints tab (a rail of pages and the
/// page next to it), the picture on the right.
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
        let side = Rect::from_min_max(full.min, Pos2::new(full.left() + SIDEBAR_W, full.bottom()));
        let body = Rect::from_min_max(Pos2::new(full.left() + SIDEBAR_W, full.top()), full.max);
        th::vgradient(ui.painter(), body, th::lighten(BACKGROUND4, 0.05), th::darken(BACKGROUND4, 0.08));
        th::vgradient(ui.painter(), side, th::lighten(BACKGROUND5, 0.03), th::darken(BACKGROUND5, 0.12));
        for i in 0..14 {
            let x = full.right() + i as f32 * 1.6;
            ui.painter().rect_filled(Rect::from_min_max(Pos2::new(x, full.top()), Pos2::new(x + 1.7, full.bottom())), 0.0, Color32::from_black_alpha((70.0 * (1.0 - i as f32 / 14.0).powi(2)) as u8));
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(side), |ui| {
            ui.add_space(10.0);
            for (page, name, icon) in [(SkinPage::Pictures, "Pictures", 1), (SkinPage::Edits, "Edits", 5), (SkinPage::Output, "Output", 8)] {
                if th::sidebar_item(ui, name, icon, st.skin_ed.page == page).clicked() {
                    st.skin_ed.page = page;
                }
            }
        });
        match st.skin_ed.page {
            SkinPage::Pictures => pictures_page(ui, st, body),
            SkinPage::Edits => scrolled(ui, body, |ui| edits_page(ui, st)),
            SkinPage::Output => scrolled(ui, body, |ui| output_page(ui, st)),
        }
    });
    egui::CentralPanel::default().frame(egui::Frame::new().fill(BACKGROUND6)).show(root, |ui| {
        canvas(ui, st);
    });
}

/// A page that scrolls on its own inside `body`.
fn scrolled(ui: &mut Ui, body: Rect, f: impl FnOnce(&mut Ui)) {
    ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_width(body.width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            f(ui);
            ui.add_space(40.0);
        });
    });
}

/// One line of small text, cut at the start when it is too long (the end of a path is the part that matters).
fn small_line(ui: &mut Ui, s: &str) {
    let max = ((ui.available_width() - 40.0) / 6.6).max(10.0) as usize;
    let n = s.chars().count();
    let shown = if n > max { format!("…{}", s.chars().skip(n - (max - 1)).collect::<String>()) } else { s.to_string() };
    ui.horizontal(|ui| {
        ui.add_space(17.0);
        ui.label(th::text(shown, 13.0, LIGHT3).family(th::light()));
    });
    ui.add_space(4.0);
}

fn pictures_page(ui: &mut Ui, st: &mut Studio, body: Rect) {
    if st.skin_ed.files.is_empty() {
        scrolled(ui, body, |ui| {
            th::section_header(ui, "Pictures");
            if st.skin_ed.dir.is_none() {
                th::note(ui, "Open a skin (a folder or an .osk) to see its pictures, or open a single picture. Edits are made on a copy in memory: nothing changes on disk until you save, and the first original of every picture you save over is kept in a .skin-studio-backup folder next to it.");
            } else {
                th::note(ui, "There are no pictures (.png) in this skin.");
            }
            open_buttons(ui, st);
        });
        return;
    }
    // search, as at the top of the other pages' panel
    let search_rect = Rect::from_min_size(body.min, Vec2::new(body.width(), 56.0));
    th::search_box(ui, search_rect, &mut st.skin_ed.filter);
    let rest = Rect::from_min_max(Pos2::new(body.left(), body.top() + 56.0), body.max);
    let q = st.skin_ed.filter.trim().to_lowercase();
    let rows: Vec<usize> = (0..st.skin_ed.files.len()).filter(|i| q.is_empty() || st.skin_ed.files[*i].name.to_lowercase().contains(&q)).collect();
    let mut open: Option<PathBuf> = None;
    ui.scope_builder(egui::UiBuilder::new().max_rect(rest), |ui| {
        ui.set_width(rest.width());
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let total = st.skin_ed.files.len();
        th::section_header(ui, &if q.is_empty() { format!("Pictures ({total})") } else { format!("Pictures ({} of {total})", rows.len()) });
        if let Some(d) = &st.skin_ed.dir {
            small_line(ui, &format!("{}{}", d.to_string_lossy(), if st.skin_ed.packed_from.is_some() { "  (unpacked from the .osk)" } else { "" }));
        }
        if let Some(dir) = st.current_skin_folder() {
            if st.skin_ed.dir.as_deref() != Some(dir.as_path()) && th::button_wide(ui, "Edit the skin picked for the preview", Kind::Dark, true).clicked() {
                open_preview_skin(st, &dir);
            }
        }
        if rows.is_empty() {
            th::note(ui, "No picture matches the search.");
        }
        let row_h = 40.0;
        egui::ScrollArea::vertical().id_salt("skin-pictures").auto_shrink([false, false]).show_rows(ui, row_h, rows.len(), |ui, range| {
            for i in range {
                let f = &st.skin_ed.files[rows[i]];
                let on = st.skin_ed.current.as_deref() == Some(f.path.as_path());
                let width = ui.available_width() - 22.0;
                let (r, resp) = ui.allocate_exact_size(Vec2::new(width + 22.0, row_h), Sense::click());
                let r = Rect::from_min_size(r.min + Vec2::new(12.0, 1.0), Vec2::new(width, row_h - 2.0));
                if on {
                    ui.painter().rect_filled(r, CornerRadius::same(5), Color32::from_rgba_unmultiplied(0x8c, 0x66, 0xff, 54));
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, CornerRadius::same(5), DARK2);
                }
                let box_ = Rect::from_center_size(Pos2::new(r.left() + 22.0, r.center().y), Vec2::splat(30.0));
                ui.painter().rect_filled(box_, CornerRadius::same(3), Color32::from_gray(0x2a));
                if let Some((t, sz)) = st.skin_ed.thumbs.get(&f.path) {
                    let fit = (30.0 / sz[0].max(1) as f32).min(30.0 / sz[1].max(1) as f32);
                    let size = Vec2::new(sz[0] as f32 * fit, sz[1] as f32 * fit).min(Vec2::splat(30.0));
                    ui.painter().image(t.id(), Rect::from_center_size(box_.center(), size), Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                }
                let name_x = r.left() + 46.0;
                let size_w = if f.size.is_some() { 74.0 } else { 0.0 };
                let max_chars = (((r.right() - name_x - size_w - 8.0) / 7.4).max(6.0)) as usize;
                let name: String = if f.name.chars().count() > max_chars { format!("{}…", f.name.chars().take(max_chars - 1).collect::<String>()) } else { f.name.clone() };
                ui.painter().text(Pos2::new(name_x, r.center().y), Align2::LEFT_CENTER, name, FontId::new(14.0, FontFamily::Proportional), if on { CONTENT1 } else { CONTENT2 });
                if let Some((w, h)) = f.size {
                    ui.painter().text(r.right_center() - Vec2::new(10.0, 0.0), Align2::RIGHT_CENTER, format!("{w} × {h}"), FontId::new(12.5, FontFamily::Proportional), LIGHT3);
                }
                if resp.clicked() {
                    open = Some(f.path.clone());
                }
            }
        });
    });
    if let Some(p) = open {
        let (tx, ctx) = (st.tx.clone(), st.ctx.clone());
        match st.skin_ed.open_picture(&p, Some((&tx, &ctx))) {
            Ok(()) => st.skin_ed.page = SkinPage::Edits,
            Err(e) => st.status = e,
        }
    }
}

fn open_preview_skin(st: &mut Studio, dir: &Path) {
    match st.skin_ed.open_skin(dir) {
        Ok(()) => {
            let (tx, ctx) = (st.tx.clone(), st.ctx.clone());
            st.skin_ed.load_thumbs(&tx, &ctx);
            st.status = format!("{} pictures", st.skin_ed.files.len());
        }
        Err(e) => st.status = e,
    }
}

/// The buttons that open a skin or a picture (the empty state of the pictures page).
fn open_buttons(ui: &mut Ui, st: &mut Studio) {
    if th::button_wide(ui, "Open a skin (.osk)...", Kind::Purple, true).clicked() {
        st.pick(PickKind::SkinEditSkin);
    }
    if th::button_wide(ui, "Open a skin folder...", Kind::Dark, true).clicked() {
        st.pick(PickKind::SkinEditSkinFolder);
    }
    if let Some(dir) = st.current_skin_folder() {
        if th::button_wide(ui, "Edit the skin picked for the preview", Kind::Dark, st.skin_ed.dir.as_deref() != Some(dir.as_path())).clicked() {
            open_preview_skin(st, &dir);
        }
    }
    if th::button_wide(ui, "Open a picture...", Kind::Dark, true).clicked() {
        st.pick(PickKind::SkinEditPicture);
    }
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

/// The step that a slider over `range` moves by: 1, 2 or 5 times a power of ten, giving 40 to 100 positions, so that round values
/// (8, 0.5, 80) can be hit exactly.
fn nice_step(range: f32) -> f32 {
    // a slider over twenty or more units moves in whole ones, so that no value is skipped
    if range >= 20.0 {
        return 1.0;
    }
    let raw = (range / 80.0).max(1e-4);
    let p = 10f32.powf(raw.log10().floor());
    let m = raw / p;
    p * if m >= 5.0 { 5.0 } else if m >= 2.0 { 2.0 } else { 1.0 }
}

/// The first number in a text typed by the user (a unit after it, like "ms" or "%", is ignored).
fn typed_number(s: &str) -> Option<f32> {
    let t: String = s.trim().chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | ',')).collect();
    t.replace(',', ".").parse::<f32>().ok()
}

/// A row with a slider over `min..max`, showing the value; true when it moved. It moves in round steps (Shift while dragging: ten times
/// slower; the arrow keys over it: one step; a click on the number: type one).
fn slider(ui: &mut Ui, id: &str, label: &str, v: &mut f32, min: f32, max: f32, show: impl Fn(f32) -> String) -> bool {
    let range = (max - min).max(1e-6);
    let step = nice_step(range);
    let snap = |x: f32| (((x / step).round() * step * 1e5).round() / 1e5).clamp(min, max);
    let mut f = ((*v - min) / range).clamp(0.0, 1.0);
    let (moved, typed) = th::slider_row_ex(ui, id, label, &show(*v), &mut f, step / range);
    let mut changed = false;
    if moved {
        let x = snap(min + f * range);
        if (x - *v).abs() > 1e-6 {
            *v = x;
            changed = true;
        }
    }
    if let Some(x) = typed.as_deref().and_then(typed_number) {
        // typed values are taken as they are (not snapped), only kept inside the range
        let x = x.clamp(min, max);
        if (x - *v).abs() > 1e-6 {
            *v = x;
            changed = true;
        }
    }
    changed
}

/// The edits that can be added, with what a new one starts as (for the menu, and for the layout audit).
pub fn add_menu_items(cw: usize, ch: usize) -> Vec<(&'static str, Op)> {
    vec![
            ("Resize", Op::Resize { w: cw, h: ch, mode: Resample::Smooth }),
            ("Crop", Op::Crop { x: 0, y: 0, w: cw, h: ch }),
            ("Flip left-right", Op::Flip { horizontal: true }),
            ("Flip up-down", Op::Flip { horizontal: false }),
            ("Rotate right", Op::Rotate { quarter_turns: 1 }),
            ("Rotate left", Op::Rotate { quarter_turns: 3 }),
            ("Trim border", Op::Trim { threshold: 0.0 }),
            ("Expand border", Op::Expand { amount: 16, fill: Fill::Clear }),
            ("Colour", Op::Adjust { brightness: 0.0, contrast: 0.0, saturation: 0.0, hue: 0.0 }),
            ("Tint", Op::Tint { rgb: [1.0, 0.4, 0.6], strength: 0.5 }),
            ("Opacity", Op::Opacity { factor: 1.0 }),
            ("Invert", Op::Invert),
            ("Blur", Op::Blur { sigma: 1.5 }),
            ("Sharpen", Op::Sharpen { amount: 0.8, sigma: 1.2 }),
            ("Deband", Op::Deband { radius: 64.0, threshold: 6.0, iterations: 4, grain: 0.5, kind: Grain::Gaussian, mono: true, room: true, luma: LumaScale { scaling: 4.0, by_average: false }, limit: 0.0, elast: 3.0, narrowing: false, prefilter: 0.0, protect: Mask { invert: true, ..Mask::off() }, adv: edit::Advanced::default() }),
            ("Dehalo", Op::Dehalo(Dehalo::default())),
            ("Denoise", Op::Denoise(Denoise::default())),
            ("Anti-alias", Op::AntiAlias(AntiAlias::default())),
            ]
}

fn edits_page(ui: &mut Ui, st: &mut Studio) {
    th::section_header(ui, "Edits");
    if st.skin_ed.source.is_none() {
        th::note(ui, "Pick a picture on the Pictures page to edit it.");
        return;
    }
    if let Some(p) = &st.skin_ed.current {
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let (w, h) = st.skin_ed.out_dims();
        small_line(ui, &format!("{name}  ·  {} × {} → {w} × {h}", st.skin_ed.source.as_ref().map_or(0, |s| s.w), st.skin_ed.source.as_ref().map_or(0, |s| s.h)));
    }
    let before_ops = st.skin_ed.ops.clone();
    let mut changed = false;
    let n = st.skin_ed.ops.len();
    st.skin_ed.collapsed.retain(|i| *i < n);
    {
        // undo and redo, and the way to add an edit
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
            if th::button(ui, if st.skin_ed.add_open { "Cancel" } else { "+ Add edit" }, Kind::Accent, true).clicked() {
                st.skin_ed.add_open = !st.skin_ed.add_open;
            }
        });
        ui.add_space(8.0);
    }
    if st.skin_ed.add_open {
        let (cw, ch) = st.skin_ed.out_dims();
        let items = add_menu_items(cw, ch);
        let labels: Vec<&str> = items.iter().map(|(l, _)| *l).collect();
        if let Some(i) = th::button_grid(ui, &labels, 2, Kind::Dark) {
            // the new edit is open and the others fold away, so that its settings are in view
            st.skin_ed.collapsed = (0..n).collect();
            st.skin_ed.ops.push(items[i].1.clone());
            st.skin_ed.bufs.clear();
            st.skin_ed.add_open = false;
            changed = true;
        }
        ui.add_space(6.0);
    }
    let (mut remove, mut up) = (None, None);
    let n = st.skin_ed.ops.len();
    for i in 0..n {
        let (iw, ih) = st.skin_ed.input_dims(i);
        let keep = st.skin_ed.keep_aspect;
        let folded = st.skin_ed.collapsed.contains(&i);
        // a title line: fold, name, and the buttons that move and remove the edit
        let width = ui.available_width() - 22.0;
        let (r, _) = ui.allocate_exact_size(Vec2::new(width + 22.0, 36.0), Sense::hover());
        let r = Rect::from_min_size(r.min + Vec2::new(12.0, 3.0), Vec2::new(width, 30.0));
        let head = ui.interact(r, ui.scope_id().with(("head", i)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        ui.painter().rect_filled(r, CornerRadius::same(6), if head.hovered() { th::lighten(BACKGROUND5, 0.04) } else { BACKGROUND5 });
        // the fold arrow
        let c = r.left_center() + Vec2::new(16.0, 0.0);
        let arrow = if folded { vec![c + Vec2::new(-2.5, -4.5), c + Vec2::new(3.0, 0.0), c + Vec2::new(-2.5, 4.5)] } else { vec![c + Vec2::new(-4.5, -2.5), c + Vec2::new(0.0, 3.0), c + Vec2::new(4.5, -2.5)] };
        ui.painter().add(egui::Shape::convex_polygon(arrow, LIGHT1, Stroke::NONE));
        ui.painter().text(r.left_center() + Vec2::new(32.0, 0.0), Align2::LEFT_CENTER, format!("{}. {}", i + 1, st.skin_ed.ops[i].name()), FontId::new(15.0, th::semibold()), CONTENT1);
        let btn = |ui: &mut Ui, id: &str, right: f32, label: &str, tip: &str| -> bool {
            let b = Rect::from_center_size(Pos2::new(r.right() - right, r.center().y), Vec2::new(28.0, 24.0));
            let resp = ui.interact(b, ui.scope_id().with((id, i)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip);
            if resp.hovered() {
                ui.painter().rect_filled(b, CornerRadius::same(5), DARK3);
            }
            ui.painter().text(b.center(), Align2::CENTER_CENTER, label, FontId::new(14.0, th::semibold()), if resp.hovered() { CONTENT1 } else { LIGHT1 });
            resp.clicked()
        };
        let (mut clicked_btn, mut is_rm, mut is_up) = (false, false, false);
        if btn(ui, "rm", 18.0, "x", "Remove this edit") {
            is_rm = true;
            clicked_btn = true;
        }
        if i > 0 && btn(ui, "up", 50.0, "^", "Move it up (earlier)") {
            is_up = true;
            clicked_btn = true;
        }
        if is_rm {
            remove = Some(i);
        }
        if is_up {
            up = Some(i);
        }
        if head.clicked() && !clicked_btn {
            if folded {
                st.skin_ed.collapsed.remove(&i);
            } else {
                st.skin_ed.collapsed.insert(i);
            }
        }
        if !folded {
            let SkinEditor { ops, bufs, .. } = &mut st.skin_ed;
            changed |= op_controls(ui, bufs, &mut ops[i], i, (iw, ih), keep);
        }
        ui.add_space(6.0);
    }
    if let Some(i) = remove {
        st.skin_ed.ops.remove(i);
        st.skin_ed.collapsed = st.skin_ed.collapsed.iter().filter(|c| **c != i).map(|c| if *c > i { c - 1 } else { *c }).collect();
        st.skin_ed.bufs.clear();
        changed = true;
    }
    if let Some(i) = up {
        st.skin_ed.ops.swap(i - 1, i);
        let (a, b) = (st.skin_ed.collapsed.remove(&(i - 1)), st.skin_ed.collapsed.remove(&i));
        if a {
            st.skin_ed.collapsed.insert(i);
        }
        if b {
            st.skin_ed.collapsed.insert(i - 1);
        }
        st.skin_ed.bufs.clear();
        changed = true;
    }
    if n == 0 && !st.skin_ed.add_open {
        th::note(ui, "No edits yet. Use + Add edit; the edits are applied from the first to the last.");
    }
    if n > 1 && th::button_wide(ui, "Remove every edit", Kind::Dark, true).clicked() {
        st.skin_ed.ops.clear();
        st.skin_ed.collapsed.clear();
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

/// How the picture is stored when it is saved.
fn output_page(ui: &mut Ui, st: &mut Studio) {
    th::section_header(ui, "Output");
    if st.skin_ed.source.is_some() {
        let (w, h) = st.skin_ed.out_dims();
        small_line(ui, &format!("The result is {w} × {h} pixels"));
    }
    let mut out = st.skin_ed.output;
    let depths: Vec<String> = edit::Depth::ALL.iter().map(|d| d.name().to_string()).collect();
    if let Some(k) = th::dropdown_row(ui, "outdepth", "Bit depth of the file", out.depth.name(), &depths) {
        out.depth = edit::Depth::ALL[k];
    }
    if out.depth == edit::Depth::Bits8 {
        th::switch_row(ui, "outdither", "Dither when rounding to 8 bit", &mut out.dither);
    }
    th::note(ui, "Every edit is done in 32-bit floating point. Only the saved file is rounded: to 8 bit with dither (the usual choice), or kept at 16 bit.");
    if out != st.skin_ed.output {
        st.skin_ed.output = out;
        let (tx, ctx) = (st.tx.clone(), st.ctx.clone());
        st.skin_ed.recompute(&tx, &ctx);
    }
    th::subsection_header(ui, "Export");
    if th::button_wide(ui, "Export the skin as .osk...", Kind::Purple, st.skin_ed.dir.is_some()).clicked() {
        st.pick(PickKind::SkinEditExportOsk);
    }
    th::note(ui, "Save writes the picture over the file it came from (the first original is kept in a .skin-studio-backup folder next to it); Save as writes a new file.");
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
            if th::switch_row(ui, &format!("fl{i}"), "Left to right (off: down)", &mut h) {
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
            ch |= slider(ui, &format!("tr{i}"), "Empty up to an alpha of", threshold, 0.0, 0.5, |v| format!("{:.0}%", v * 100.0));
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
        Op::Dehalo(o) => {
            ch |= th::switch_row(ui, &format!("hf{i}"), "Fine dehalo (spares lines)", &mut o.fine);
            ch |= slider(ui, &format!("hb{i}"), "Blur", &mut o.blur, 0.5, 4.0, |v| format!("{v:.2}"));
            ch |= slider(ui, &format!("hl{i}"), "Low sensitivity", &mut o.lowsens, -1.0, 100.0, |v| if v < 0.0 { "no mask".into() } else { format!("{v:.0}") });
            ch |= slider(ui, &format!("hh{i}"), "High sensitivity", &mut o.highsens, -1.0, 100.0, |v| if v < 0.0 { "no mask".into() } else { format!("{v:.0}") });
            ch |= slider(ui, &format!("hs{i}"), "Supersampling", &mut o.ss, 1.0, 3.0, |v| format!("{v:.2}x"));
            ch |= slider(ui, &format!("hbr{i}"), "Bright halo strength", &mut o.brightstr, 0.0, 1.0, |v| format!("{:.0}%", v * 100.0));
            ch |= slider(ui, &format!("hd{i}"), "Dark halo strength", &mut o.darkstr, 0.0, 1.0, |v| format!("{:.0}%", v * 100.0));
            ch |= slider_u8(ui, &format!("hi{i}"), "Iterations", &mut o.iterations, 1, 5);
            if o.fine {
                ch |= slider_u8(ui, &format!("hx{i}"), "Halo width (px)", &mut o.rx, 1, 8);
                o.ry = o.rx;
                ch |= edge_row(ui, &format!("he{i}"), "Edge detector", &mut o.edge);
                ch |= slider(ui, &format!("hm{i}"), "Line edges from", &mut o.thmi, 0.0, 255.0, |v| format!("{v:.0}"));
                ch |= slider(ui, &format!("hM{i}"), "Line edges up to", &mut o.thma, 1.0, 255.0, |v| format!("{v:.0}"));
                ch |= slider(ui, &format!("hn{i}"), "Details from", &mut o.thlimi, 0.0, 255.0, |v| format!("{v:.0}"));
                ch |= slider(ui, &format!("hN{i}"), "Details up to", &mut o.thlima, 1.0, 255.0, |v| format!("{v:.0}"));
                ch |= th::switch_row(ui, &format!("hx2{i}"), "Spare close edges", &mut o.exclude);
                ch |= slider(ui, &format!("hp{i}"), "Edge processing", &mut o.edgeproc, 0.0, 3.0, |v| format!("{v:.2}"));
            }
            ch |= target_row(ui, &format!("ht{i}"), &mut o.target);
            ch |= mask_controls(ui, &format!("hk{i}"), &mut o.mask, true);
            th::note(ui, "After vs-jetpack's dehalo_alpha and fine_dehalo. Bright halos are taken away by the bright strength, dark ones by the dark strength.");
        }
        Op::Denoise(o) => {
            let names: Vec<String> = DenoiseKind::ALL.iter().map(|k| k.name().to_string()).collect();
            if let Some(k) = th::dropdown_row(ui, &format!("nk{i}"), "Method", o.kind.name(), &names) {
                o.kind = DenoiseKind::ALL[k];
                ch = true;
            }
            if o.kind == DenoiseKind::NlMeans {
                ch |= slider(ui, &format!("nh{i}"), "Strength (noise to remove)", &mut o.h, 0.5, 30.0, |v| format!("{v:.1} levels"));
                ch |= slider_u8(ui, &format!("na{i}"), "Search radius", &mut o.search, 1, 6);
                ch |= slider_u8(ui, &format!("ns{i}"), "Similarity radius", &mut o.patch, 0, 5);
            } else {
                ch |= slider(ui, &format!("nss{i}"), "Distance", &mut o.sigma_s, 0.5, 6.0, |v| format!("{v:.1} px"));
                ch |= slider(ui, &format!("nsr{i}"), "Difference", &mut o.sigma_r, 1.0, 60.0, |v| format!("{v:.0} levels"));
            }
            ch |= target_row(ui, &format!("nt{i}"), &mut o.target);
            ch |= mask_controls(ui, &format!("nm{i}"), &mut o.mask, true);
            th::note(ui, "The mask is turned around by default: noise is taken out away from the edges, so details stay sharp.");
        }
        Op::AntiAlias(o) => {
            ch |= slider(ui, &format!("ar{i}"), "Working size", &mut o.rfactor, 1.0, 4.0, |v| format!("{v:.1}x"));
            ch |= slider(ui, &format!("ae{i}"), "Reach along an edge", &mut o.reach, 1.0, 12.0, |v| format!("{v:.1} px"));
            ch |= slider_u8(ui, &format!("ap{i}"), "Passes", &mut o.passes, 1, 6);
            ch |= th::switch_row(ui, &format!("af{i}"), "Clean up afterwards", &mut o.postfilter);
            ch |= target_row(ui, &format!("at{i}"), &mut o.target);
            ch |= mask_controls(ui, &format!("am{i}"), &mut o.mask, true);
            th::note(ui, "Edges are smoothed along their own direction, within the mask, at a larger size. (Not the neural scalers of vs-jetpack.)");
        }
        Op::Expand { amount, fill } => {
            if let Some(v) = num_row(ui, bufs, &format!("ex{i}"), "On every side (px)", *amount) {
                *amount = v.min(4096);
                ch = true;
            }
            let names: Vec<String> = Fill::ALL.iter().map(|f| f.name().to_string()).collect();
            if let Some(k) = th::dropdown_row(ui, &format!("ef{i}"), "Fill with", fill.name(), &names) {
                *fill = Fill::ALL[k];
                ch = true;
            }
        }
        Op::Deband { radius, threshold, iterations, grain, kind, mono, room, luma, limit, elast, narrowing, prefilter, protect, adv } => {
            ch |= slider(ui, &format!("dr{i}"), "Radius", radius, 2.0, 128.0, |v| format!("{v:.0} px"));
            ch |= slider(ui, &format!("dt{i}"), "Threshold", threshold, 1.0, 40.0, |v| format!("{v:.1} levels"));
            let mut it = *iterations as f32;
            if slider(ui, &format!("di{i}"), "Iterations", &mut it, 1.0, 12.0, |v| format!("{v:.0}")) {
                *iterations = it.round().max(1.0) as usize;
                ch = true;
            }
            let modes: Vec<String> = edit::DebandMode::ALL.iter().map(|m| m.name().to_string()).collect();
            if let Some(k) = th::dropdown_row(ui, &format!("dmode{i}"), "Method", adv.mode.name(), &modes) {
                adv.mode = edit::DebandMode::ALL[k];
                if adv.mode != edit::DebandMode::Pairs && adv.thr1 == 0.0 {
                    // vszip's two extra limits: wide for the references (slopes are no reason to stop), the line limit as the threshold
                    adv.thr1 = 40.0;
                    adv.thr2 = *threshold;
                }
                ch = true;
            }
            if adv.mode != edit::DebandMode::Pairs {
                ch |= slider(ui, &format!("dt1{i}"), "Reference limit (thr1)", &mut adv.thr1, 0.0, 40.0, |v| if v < 0.05 { "as threshold".to_string() } else { format!("{v:.1} levels") });
                ch |= slider(ui, &format!("dt2{i}"), "Line limit (thr2)", &mut adv.thr2, 0.0, 40.0, |v| if v < 0.05 { "as threshold".to_string() } else { format!("{v:.1} levels") });
            }
            if adv.mode == edit::DebandMode::Angle {
                ch |= slider(ui, &format!("dab{i}"), "Gradient boost", &mut adv.angle_boost, 1.0, 4.0, |v| format!("{v:.2}x"));
                ch |= slider(ui, &format!("dma{i}"), "Direction tolerance", &mut adv.max_angle, 0.01, 0.5, |v| format!("{v:.2}"));
            }
            ch |= slider(ui, &format!("dds{i}"), "Reference spread (0: even)", &mut adv.dist_sigma, 0.0, 2.0, |v| if v < 0.005 { "even".to_string() } else { format!("{v:.2}") });
            ch |= th::switch_row(ui, &format!("dn{i}"), "Narrow the radius pass by pass", narrowing);
            ch |= slider(ui, &format!("dp{i}"), "Prefilter blur", prefilter, 0.0, 4.0, |v| if v < 0.05 { "off".to_string() } else { format!("{v:.1} px") });
            ch |= slider(ui, &format!("dq{i}"), "Limit the change to", limit, 0.0, 16.0, |v| if v < 0.05 { "no limit".to_string() } else { format!("{v:.1} levels") });
            if *limit > 0.0 {
                ch |= slider(ui, &format!("de{i}"), "Limit elasticity", elast, 1.1, 8.0, |v| format!("{v:.1}"));
            }
            let names: Vec<String> = Grain::ALL.iter().map(|g| g.name().to_string()).collect();
            if let Some(k) = th::dropdown_row(ui, &format!("dk{i}"), "Grain", kind.name(), &names) {
                *kind = Grain::ALL[k];
                ch = true;
            }
            if *kind != Grain::None {
                ch |= slider(ui, &format!("dg{i}"), "Grain strength", grain, 0.0, 3.0, |v| format!("{v:.2} levels"));
                ch |= th::switch_row(ui, &format!("dm{i}"), "Grey grain", mono);
            }
            ch |= luma_controls(ui, &format!("dl{i}"), luma);
            ch |= mask_controls(ui, &format!("dmk{i}"), protect, false);
            ch |= th::switch_row(ui, &format!("dx{i}"), "Make room for cut-off glows", room);
            th::note(ui, "Radius: how far it looks for pixels to average with. Threshold: how different they may be and still count as one band. Smooth and gradient-aware methods are vszip's (f3kdb) modes 6 and 7: the line limit tells a step on a slope (banding) from a detail, so the reference limit can stay wide on steep gradients. Prefilter: the banding is looked for in a blurred copy and the fine detail put back (some of the band steps come back with it, so use it only on noisy pictures). Limit: a change bigger than this is no banding and is taken back. The mask (set to away from edges) keeps it off edges and details. Poisson grain follows the brightness; blue grain has less texture than Gaussian.");
        }
    }
    ch
}

fn slider_u8(ui: &mut Ui, id: &str, label: &str, v: &mut u8, lo: u8, hi: u8) -> bool {
    let mut f = *v as f32;
    if slider(ui, id, label, &mut f, lo as f32, hi as f32, |x| format!("{x:.0}")) {
        *v = f.round() as u8;
        return true;
    }
    false
}

fn edge_row(ui: &mut Ui, id: &str, label: &str, e: &mut Edge) -> bool {
    let names: Vec<String> = Edge::ALL.iter().map(|k| k.name().to_string()).collect();
    if let Some(k) = th::dropdown_row(ui, id, label, e.name(), &names) {
        *e = Edge::ALL[k];
        return true;
    }
    false
}

fn target_row(ui: &mut Ui, id: &str, t: &mut Target) -> bool {
    let names: Vec<String> = Target::ALL.iter().map(|k| k.name().to_string()).collect();
    if let Some(k) = th::dropdown_row(ui, id, "Works on", t.name(), &names) {
        *t = Target::ALL[k];
        return true;
    }
    false
}

/// The controls of an edge mask that limits a filter.
fn mask_controls(ui: &mut Ui, id: &str, m: &mut Mask, with_luma: bool) -> bool {
    let mut ch = th::switch_row(ui, &format!("{id}e"), "Limit with an edge mask", &mut m.enabled);
    if m.enabled {
        ch |= edge_row(ui, &format!("{id}d"), "Mask edge detector", &mut m.edge);
        ch |= slider(ui, &format!("{id}t"), "Mask threshold", &mut m.threshold, 1.0, 255.0, |v| format!("{v:.0}"));
        ch |= slider_u8(ui, &format!("{id}g"), "Mask grow (px)", &mut m.grow, 0, 12);
        ch |= slider_u8(ui, &format!("{id}s"), "Mask softening", &mut m.soften, 0, 6);
        ch |= th::switch_row(ui, &format!("{id}i"), "Away from the edges instead", &mut m.invert);
    }
    if with_luma {
        ch |= luma_controls(ui, &format!("{id}l"), &mut m.luma);
    }
    ch |= th::switch_row(ui, &format!("{id}v"), "Show the mask", &mut m.show);
    ch
}

/// Luma scaling (adaptivegrain, as used by vodesfunc): more of the filter in the dark, hardly any in the light.
fn luma_controls(ui: &mut Ui, id: &str, l: &mut LumaScale) -> bool {
    let mut ch = slider(ui, &format!("{id}s"), "Favour dark areas (luma scaling)", &mut l.scaling, -20.0, 20.0, |v| if v == 0.0 { "off".to_string() } else if v > 0.0 { format!("dark {v:.1}") } else { format!("bright {:.1}", -v) });
    if l.scaling != 0.0 {
        ch |= th::switch_row(ui, &format!("{id}a"), "Scale by the average brightness", &mut l.by_average);
    }
    ch
}

// ==================================================================================================================== the picture
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
    // everything that is in the bar, with how long it stays when the bar is narrow (the larger, the sooner it goes) and the gap before it
    let zooms: [(&str, f32); 4] = [("100%", 1.0 / ppp), ("200%", 2.0 / ppp), ("400%", 4.0 / ppp), ("800%", 8.0 / ppp)];
    #[derive(Clone, Copy)]
    enum Pill {
        Show(Show, &'static str, &'static str),
        Fit,
        Zoom(usize),
        Back(Bg, &'static str),
    }
    let mut pills: Vec<(Pill, f32, u8, f32)> = vec![
        (Pill::Show(Show::After, "After", "The picture with the edits"), 62.0, 0, 0.0),
        (Pill::Show(Show::Before, "Before", "The picture as it was loaded"), 62.0, 0, 0.0),
        (Pill::Show(Show::Split, "Split", "Before on the left, after on the right"), 58.0, 0, 0.0),
        (Pill::Fit, 44.0, 0, 12.0),
        (Pill::Zoom(0), 52.0, 2, 0.0),
        (Pill::Zoom(1), 52.0, 3, 0.0),
        (Pill::Zoom(2), 52.0, 4, 0.0),
        (Pill::Zoom(3), 52.0, 5, 0.0),
        (Pill::Back(Bg::Dark, "Black"), 52.0, 1, 12.0),
        (Pill::Back(Bg::Grey, "Grey"), 52.0, 1, 0.0),
        (Pill::Back(Bg::Light, "Light"), 52.0, 1, 0.0),
    ];
    let fits = |p: &Vec<(Pill, f32, u8, f32)>| p.iter().map(|q| q.1 + 6.0 + q.3).sum::<f32>() + 12.0 <= bar.width();
    while !fits(&pills) {
        // the least important goes (the biggest zoom first), but what always stays stays
        let Some(k) = pills.iter().enumerate().filter(|(_, q)| q.2 > 0).max_by_key(|(_, q)| q.2).map(|(k, _)| k) else { break };
        pills.remove(k);
    }
    for (pill_kind, w, _, gap) in pills {
        x.set(x.get() + gap);
        let r = place(w);
        match pill_kind {
            Pill::Show(s, label, tip) => {
                if pill(ui, r, label, ed.show == s, tip) {
                    ed.show = s;
                }
            }
            Pill::Fit => {
                if pill(ui, r, "Fit", ed.zoom == 0.0, "Fit the picture in the window") {
                    ed.zoom = 0.0;
                    ed.pan = Vec2::ZERO;
                }
            }
            Pill::Zoom(i) => {
                let (label, z) = zooms[i];
                if pill(ui, r, label, (ed.zoom - z).abs() < 1e-3, "Show the picture at this size: one pixel of it is this many pixels of the screen") {
                    ed.zoom = z;
                    ed.pan = Vec2::ZERO;
                }
            }
            Pill::Back(b, label) => {
                if pill(ui, r, label, ed.bg == b, "What is behind the picture") {
                    ed.bg = b;
                }
            }
        }
    }

    // --- the picture
    let Some(after) = ed.result.clone().or_else(|| ed.source.clone()) else {
        ui.painter().rect_filled(area, 0.0, BACKGROUND6);
        let f = FontId::new(16.0, th::light());
        let msg = th::fit_text(ui.painter(), "Open a skin or a picture, then pick a picture from the list", &f, area.width() - 32.0);
        ui.painter().text(area.center(), Align2::CENTER_CENTER, msg, f, LIGHT3);
        return;
    };
    let before = ed.source.clone().unwrap_or_else(|| after.clone());
    let (iw, ih) = (after.w as f32, after.h as f32);
    // the line about the picture runs along the bottom: the picture keeps clear of it
    let stage = Rect::from_min_max(area.min, Pos2::new(area.right(), area.bottom() - 30.0));
    let fit = ((stage.width() - 40.0) / iw).min((stage.height() - 40.0) / ih).max(0.01);
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
                let c = stage.center() + ed.pan;
                ed.pan = (m - stage.center()) - (m - c) * (new / old);
            }
            ed.zoom = new;
        }
    }
    let z = if ed.zoom == 0.0 { fit } else { ed.zoom };
    let size = Vec2::new(iw * z, ih * z);
    let rect = Rect::from_center_size(stage.center() + ed.pan, size);
    let clip = area;
    match ed.bg {
        Bg::Grey => {
            ui.painter().with_clip_rect(clip).rect_filled(clip, 0.0, Color32::from_gray(0x80));
        }
        Bg::Dark => {
            // perfectly black, around the picture too: a glow fades into it without anything to compare with
            ui.painter().with_clip_rect(clip).rect_filled(clip, 0.0, Color32::BLACK);
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
    let rest = if (before.w, before.h) == (after.w, after.h) { format!("   {} × {} px   {:.0}%", after.w, after.h, z * ppp * 100.0) } else { format!("   {} × {} px  →  {} × {} px   {:.0}%", before.w, before.h, after.w, after.h, z * ppp * 100.0) } + if ed.busy { "   working..." } else { "" };
    let info_font = FontId::new(13.0, FontFamily::Proportional);
    let rest_w = painter.layout_no_wrap(rest.clone(), info_font.clone(), CONTENT1).size().x;
    let name = th::fit_text(&painter, &name, &info_font, (area.width() - 32.0 - rest_w).max(40.0));
    painter.text(Pos2::new(area.left() + 16.0, area.bottom() - 14.0), Align2::LEFT_CENTER, name + &rest, info_font, Color32::from_white_alpha(170));
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
        ed.got_result(0, Arc::new(img.clone()), dims, None);
        ed.gen = 0;
        ed.got_result(0, Arc::new(img), vec![(20, 10)], None);
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
    fn the_worker_makes_only_the_newest_request_and_never_runs_twice() {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut ed = SkinEditor::default();
        let big = Img::new(900, 900);
        ed.source = Some(Arc::new(big));
        // thirty requests in a row, as when a slider is dragged
        for i in 0..30 {
            ed.ops = vec![Op::Blur { sigma: 1.0 + i as f32 * 0.1 }, Op::deband(0.5, 0.0)];
            ed.recompute(&tx, &ctx);
        }
        let last = ed.gen;
        let mut got = vec![];
        let t = std::time::Instant::now();
        while t.elapsed().as_secs() < 20 {
            if let Ok(Msg::SkinResult(g, _, _, d)) = rx.recv_timeout(std::time::Duration::from_millis(200)) {
                got.push(g);
                assert_eq!(d.size, [900, 900]);
                if g == last {
                    break;
                }
            }
        }
        assert_eq!(got.last(), Some(&last), "the newest request was made: {got:?}");
        assert!(got.len() <= 6, "only a few of the thirty were made: {got:?}");
        assert!(got.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn the_screen_picture_is_made_from_the_result_and_shrunk_when_huge() {
        let mut im = Img::new(3, 2);
        im.px[0] = [0.5, 0.25, 0.0, 0.5];
        let d = make_display(&im, edit::Output { dither: false, ..edit::Output::default() });
        assert_eq!(d.size, [3, 2]);
        assert_eq!(&d.rgba[0..4], &[128, 64, 0, 128]);
        let huge = Img::new(5000, 100);
        let d = make_display(&huge, edit::Output::default());
        assert_eq!(d.size, [4096, 82]);
    }

    #[test]
    fn a_result_from_an_older_list_of_edits_is_ignored() {
        let mut ed = SkinEditor::default();
        ed.gen = 5;
        ed.got_result(4, Arc::new(Img::new(3, 3)), vec![], None);
        assert!(ed.result.is_none());
        ed.got_result(5, Arc::new(Img::new(3, 3)), vec![(3, 3)], None);
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
        st.skin_ed.ops = vec![Op::Adjust { brightness: 0.1, contrast: 0.0, saturation: 0.0, hue: 0.0 }, Op::deband(0.5, 0.0), Op::Trim { threshold: 0.0 }, Op::Tint { rgb: [1.0, 0.0, 0.0], strength: 0.3 }, Op::Rotate { quarter_turns: 1 }, Op::Crop { x: 0, y: 0, w: 5, h: 5 }];
        let (img, dims) = compute(st.skin_ed.source.as_ref().unwrap(), &st.skin_ed.ops);
        st.skin_ed.dims = dims;
        st.skin_ed.result = Some(Arc::new(img));
        for (w, h) in [(400.0, 300.0), (1280.0, 720.0), (2560.0, 1440.0)] {
            for show in [Show::After, Show::Before, Show::Split] {
                for bg in [Bg::Grey, Bg::Dark, Bg::Light] {
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

#[cfg(test)]
mod frame_timing {
    use super::*;

    /// `cargo test --release -p studio frame_timing -- --ignored --nocapture`: the CPU time of one frame of the skin editor.
    #[test]
    #[ignore]
    fn one_frame_of_the_editor() {
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(1.6);
        let mut st = Studio::new(ctx.clone());
        let d = std::env::temp_dir().join("ss-frame-timing");
        std::fs::create_dir_all(&d).unwrap();
        let mut rgba = vec![];
        for y in 0..900usize {
            for x in 0..900usize {
                rgba.extend([(x / 4) as u8, (y / 4) as u8, 128, 255]);
            }
        }
        let img = Img::from_straight_u8(900, 900, &rgba);
        edit::save_png(&d.join("big.png"), &img).unwrap();
        for i in 0..160 {
            edit::save_png(&d.join(format!("a{i:03}.png")), &Img::new(32, 32)).unwrap();
        }
        st.skin_ed.open_skin(&d).unwrap();
        st.skin_ed.open_picture(&d.join("big.png"), None).unwrap();
        st.skin_ed.ops = vec![Op::Resize { w: 1800, h: 1800, mode: Resample::Smooth }, Op::deband(0.5, 0.0)];
        let (r, dims) = compute(st.skin_ed.source.as_ref().unwrap(), &st.skin_ed.ops);
        st.skin_ed.dims = dims;
        st.skin_ed.result = Some(Arc::new(r));
        let run = |st: &mut Studio| {
            let mut raw = egui::RawInput::default();
            raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(2576.0 / 1.6, 1408.0 / 1.6)));
            let t = std::time::Instant::now();
            let out = ctx.run_ui(raw, |ui| draw(ui, st));
            let ran = t.elapsed();
            let t = std::time::Instant::now();
            let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
            let tess = t.elapsed();
            (ran, tess, prims.len())
        };
        let first = run(&mut st);
        eprintln!("first frame: run {:?}, tessellate {:?}, {} primitives", first.0, first.1, first.2);
        let mut total = (std::time::Duration::ZERO, std::time::Duration::ZERO);
        for _ in 0..20 {
            let f = run(&mut st);
            total.0 += f.0;
            total.1 += f.1;
        }
        eprintln!("steady frame: run {:?}, tessellate {:?}", total.0 / 20, total.1 / 20);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn sliders_move_in_round_steps_and_typed_numbers_are_read() {
        assert_eq!(nice_step(39.0), 1.0);
        assert_eq!(nice_step(1.0), 0.01);
        assert_eq!(nice_step(126.0), 1.0);
        assert_eq!(nice_step(255.0), 1.0);
        assert_eq!(nice_step(3.0), 0.02);
        // 8 is a multiple of the step of a 1..40 slider
        let step = nice_step(39.0);
        assert!(((8.0f32 / step).round() * step - 8.0).abs() < 1e-6);
        assert_eq!(typed_number("8"), Some(8.0));
        assert_eq!(typed_number(" 7,5 levels"), Some(7.5));
        assert_eq!(typed_number("-3 px"), Some(-3.0));
        assert_eq!(typed_number("abc"), None);
    }

    /// Drags a slider with the pointer through a few frames of a headless egui.
    #[test]
    fn a_slider_dragged_with_the_pointer_lands_on_round_values() {
        let ctx = egui::Context::default();
        let mut v = 6.0f32;
        let frame = |ctx: &egui::Context, events: Vec<egui::Event>, mods: egui::Modifiers, v: &mut f32| {
            let mut raw = egui::RawInput::default();
            raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0)));
            let _ = mods;
            raw.events = events;
            let _ = ctx.run_ui(raw, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                slider(ui, "t", "Threshold", v, 1.0, 40.0, |x| format!("{x:.1} levels"));
            });
        };
        frame(&ctx, vec![], egui::Modifiers::NONE, &mut v);
        // the bar is the right half of the row: x from 200 to ~390 at this width
        let press = |x: f32, y: f32| vec![egui::Event::PointerMoved(Pos2::new(x, y)), egui::Event::PointerButton { pos: Pos2::new(x, y), button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE }];
        frame(&ctx, press(300.0, 30.0), egui::Modifiers::NONE, &mut v);
        for k in 0..6 {
            frame(&ctx, vec![egui::Event::PointerMoved(Pos2::new(300.0 + k as f32 * 2.0, 30.0))], egui::Modifiers::NONE, &mut v);
        }
        let steps = v / 1.0;
        assert!((steps - steps.round()).abs() < 1e-3, "{v} is a whole number");
        let after_fast = v;
        frame(&ctx, vec![egui::Event::PointerButton { pos: Pos2::new(312.0, 30.0), button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE }], egui::Modifiers::NONE, &mut v);
        assert!(after_fast > 6.0 && after_fast < 39.0, "{after_fast}");
    }
}
