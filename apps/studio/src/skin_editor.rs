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
    Check,
}

/// The pages of the panel on the left (as the Followpoints tab has its own), so that nothing needs a long scroll.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SkinPage {
    Pictures,
    Create,
    Edits,
    Scan,
    Ini,
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

fn boost_rgba(raw_rgba: &[u8]) -> Vec<u8> {
    let lut: [u8; 256] = std::array::from_fn(|i| ((i as f32 / 255.0).powf(0.32) * 255.0).round() as u8);
    let mut out = vec![0u8; raw_rgba.len()];
    for (dst, c) in out.chunks_exact_mut(4).zip(raw_rgba.chunks_exact(4)) {
        let a = lut[c[3] as usize];
        dst[0] = lut[c[0] as usize].min(a);
        dst[1] = lut[c[1] as usize].min(a);
        dst[2] = lut[c[2] as usize].min(a);
        dst[3] = a;
    }
    out
}
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
    let rgba = edit::to_display_rgba8_with(src, output.dither && output.depth == edit::Depth::Bits8);
    Display { size: [src.w, src.h], rgba }
}

/// What the worker is asked to make: the newest request replaces one that has not been started yet.
struct Job {
    gen: u64,
    src: Arc<Img>,
    ops: Vec<Op>,
    disabled: std::collections::HashSet<usize>,
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
    /// The edit list (and hidden/disabled set) as it was before each change: Ctrl+Z and Ctrl+Y.
    undo: Vec<(Vec<Op>, std::collections::HashSet<usize>)>,
    redo: Vec<(Vec<Op>, std::collections::HashSet<usize>)>,
    last_edit: Option<std::time::Instant>,
    /// Small pictures of the files in the list, and which list they are for (a thumbnail of an older list is thrown away).
    thumbs: HashMap<PathBuf, (TextureHandle, [usize; 2])>,
    thumb_gen: u64,
    /// Cached 2×2 repeating checkerboard texture for `Bg::Check` (drawn as 1 quad instead of thousands of rects).
    check_tex: Option<TextureHandle>,
    /// Cached list of backed-up originals `(display_name, backup_path, dest_path)` refreshed on disk events (never per-frame).
    pub backups: Vec<(String, PathBuf, PathBuf)>,
    /// Indices of edits temporarily hidden (bypassed) via the eye toggle on each edit card.
    pub disabled: std::collections::HashSet<usize>,
    /// When true, opening a clean picture carries over the current edit stack instead of clearing it.
    pub keep_edits: bool,
    /// Display-only dark-tone / banding amplifier on the canvas (never affects saved files).
    pub boost: bool,
    /// Resample mode used when generating missing `@2x` files from `1x` across a skin.
    pub scale_up_mode: Resample,
    /// Skin-wide artifact scanner state.
    pub scan_running: bool,
    pub scan_report: Option<engine::scan::ScanReport>,
    /// Loaded `skin.ini` editor document for the open skin folder.
    pub ini: crate::skin_ini::SkinIniDoc,
    /// Procedural SDF element creator (`Create` page) and its cached canvas texture.
    pub creator: crate::element_creator::CreatorState,
    creator_tex: Option<(u64, bool, bool, [u8; 3], TextureHandle)>,
    /// Saved reusable filter stacks persisted in `filter-presets.json`.
    pub preset_name: String,
    pub presets: Vec<(String, Vec<Op>)>,
    presets_loaded: bool,
    /// Unsaved edit stacks stashed per picture path so switching pictures in a skin never drops work.
    stash: HashMap<PathBuf, (Vec<Op>, std::collections::HashSet<usize>, Vec<(Vec<Op>, std::collections::HashSet<usize>)>, Vec<(Vec<Op>, std::collections::HashSet<usize>)>, bool)>,
}

impl Default for SkinEditor {
    fn default() -> Self {
        SkinEditor { dir: None, packed_from: None, files: vec![], filter: String::new(), current: None, source: None, ops: vec![], result: None, result_display: None, output: edit::Output::default(), page: SkinPage::Pictures, add_open: false, collapsed: Default::default(), slot: Arc::new(std::sync::Mutex::new(None)), running: Arc::new(std::sync::atomic::AtomicBool::new(false)), latest: Arc::new(std::sync::atomic::AtomicU64::new(0)), dims: vec![], gen: 0, busy: false, dirty: false, tex: [None, None], zoom: 0.0, pan: Vec2::ZERO, bg: Bg::Dark, show: Show::After, split: 0.5, keep_aspect: true, bufs: HashMap::new(), undo: vec![], redo: vec![], last_edit: None, thumbs: HashMap::new(), thumb_gen: 0, check_tex: None, backups: vec![], disabled: Default::default(), keep_edits: false, boost: false, scale_up_mode: Resample::Crisp, scan_running: false, scan_report: None, ini: Default::default(), creator: Default::default(), creator_tex: None, preset_name: String::new(), presets: vec![], presets_loaded: false, stash: HashMap::new() }
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
    compute_until(source, ops, &std::collections::HashSet::new(), &|| false).unwrap_or_else(|| (source.clone(), vec![]))
}

/// The same, but given up (`None`) between two edits as soon as `stale` says that a newer list of edits is waiting.
fn compute_until(source: &Img, ops: &[Op], disabled: &std::collections::HashSet<usize>, stale: &dyn Fn() -> bool) -> Option<(Img, Vec<(usize, usize)>)> {
    let mut cur = source.clone();
    let mut dims = Vec::with_capacity(ops.len());
    for (i, op) in ops.iter().enumerate() {
        if stale() {
            return None;
        }
        if !disabled.contains(&i) {
            cur = edit::apply(&cur, op);
        }
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
        self.ini = crate::skin_ini::SkinIniDoc::load_from_dir(&dir);
        self.creator.select_kind(self.creator.doc.kind, Some(&dir));
        self.scan_report = None;
        self.dir = Some(dir);
        self.packed_from = packed;
        self.filter.clear();
        self.page = SkinPage::Pictures;
        self.stash.clear();
        self.clear_picture();
        self.refresh_backups();
        Ok(())
    }

    pub fn refresh_backups(&mut self) {
        self.backups.clear();
        let Some(dir) = self.dir.clone() else { return };
        let mut scan_dir = |parent: &Path, prefix: &str| {
            let bdir = parent.join(".skin-studio-backup");
            if let Ok(rd) = std::fs::read_dir(&bdir) {
                for e in rd.flatten() {
                    let bp = e.path();
                    if bp.is_file() {
                        let fname = e.file_name().to_string_lossy().to_string();
                        let shown = if prefix.is_empty() { fname.clone() } else { format!("{prefix}/{fname}") };
                        self.backups.push((shown, bp, parent.join(&fname)));
                    }
                }
            }
        };
        scan_dir(&dir, "");
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if p.is_dir() && !name.starts_with('.') {
                    scan_dir(&p, &name);
                }
            }
        }
        self.backups.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    }

    pub fn refresh_files(&mut self, tx: &Sender<Msg>, ctx: &egui::Context) {
        if let Some(dir) = self.dir.clone() {
            self.files = list_pngs(&dir);
            self.load_thumbs(tx, ctx);
        }
        self.refresh_backups();
    }

    /// Reconcile in-memory state after a background disk mutation (batch apply, Redo 1x, or full-skin scale).
    pub fn after_disk_mutation(&mut self, modified: &[PathBuf], resolved_downscale: Option<&Path>, tx: &Sender<Msg>, ctx: &egui::Context) {
        for p in modified {
            self.stash.remove(p);
        }
        if let Some(res_p) = resolved_downscale {
            if let Some(rep) = &mut self.scan_report {
                for item in &mut rep.flagged {
                    if item.path == res_p {
                        item.issues.retain(|i| !matches!(i.kind, engine::scan::IssueKind::PoorDownscale));
                    }
                }
                rep.flagged.retain(|item| !item.issues.is_empty());
            }
        }
        self.refresh_files(tx, ctx);
        if let Some(cur) = self.current.clone() {
            if modified.iter().any(|p| p == &cur) {
                self.current = None;
                self.ops.clear();
                self.disabled.clear();
                self.dirty = false;
                let _ = self.open_picture(&cur, Some((tx, ctx)));
            }
        }
    }

    fn ensure_presets(&mut self) {
        if self.presets_loaded {
            return;
        }
        self.presets_loaded = true;
        if cfg!(test) {
            return;
        }
        if let Some(p) = crate::paths::config_dir().map(|d| d.join("filter-presets.json")) {
            if let Ok(text) = std::fs::read_to_string(p) {
                if let Ok(list) = serde_json::from_str::<Vec<(String, Vec<Op>)>>(&text) {
                    self.presets = list;
                }
            }
        }
    }

    fn save_presets(&self) {
        if cfg!(test) {
            return;
        }
        if let Some(d) = crate::paths::config_dir() {
            let _ = std::fs::create_dir_all(&d);
            if let Ok(json) = serde_json::to_string_pretty(&self.presets) {
                let _ = std::fs::write(d.join("filter-presets.json"), json);
            }
        }
    }

    /// Open a single picture (its folder becomes the list when no skin is open).
    pub fn open_picture(&mut self, path: &Path, ctx_tx: Option<(&Sender<Msg>, &egui::Context)>) -> Result<(), String> {
        if self.current.as_deref() == Some(path) && self.source.is_some() {
            self.page = SkinPage::Edits;
            return Ok(());
        }
        let img = edit::load_png(path)?;
        let carry_ops = if self.keep_edits { Some((self.ops.clone(), self.disabled.clone())) } else { None };
        // Stash outgoing picture's unsaved edits only when dirty (saved edits are already baked into disk).
        if let Some(prev) = self.current.clone() {
            if self.dirty {
                self.stash.insert(prev, (self.ops.clone(), self.disabled.clone(), self.undo.clone(), self.redo.clone(), true));
            } else {
                self.stash.remove(&prev);
            }
        }
        self.page = SkinPage::Edits;
        if self.dir.is_none() {
            if let Some(d) = path.parent() {
                self.files = list_pngs(d);
                self.ini = crate::skin_ini::SkinIniDoc::load_from_dir(d);
                self.dir = Some(d.to_path_buf());
                self.refresh_backups();
            }
        }
        self.current = Some(path.to_path_buf());
        self.source = Some(Arc::new(img));
        self.last_edit = None;
        self.dims.clear();
        self.collapsed.clear();
        if let Some((ops, disabled, undo, redo, dirty)) = self.stash.remove(path) {
            self.ops = ops;
            self.disabled = disabled;
            self.undo = undo;
            self.redo = redo;
            self.dirty = dirty;
        } else if let Some((ops, disabled)) = carry_ops.filter(|(o, _)| !o.is_empty()) {
            self.undo.clear();
            self.redo.clear();
            self.ops = ops;
            self.disabled = disabled;
            self.dirty = true;
        } else {
            self.undo.clear();
            self.redo.clear();
            self.ops.clear();
            self.disabled.clear();
            self.dirty = false;
        }
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
        self.disabled.clear();
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
    /// The edit list (and visibility set) is about to change from `before`: keep it. Rapid slider drags coalesce into one step.
    #[allow(dead_code)]
    pub fn push_undo(&mut self, before: Vec<Op>) {
        self.push_undo_state(before, self.disabled.clone(), true);
    }

    pub fn push_undo_state(&mut self, before_ops: Vec<Op>, before_disabled: std::collections::HashSet<usize>, coalesce: bool) {
        let quick = coalesce && self.last_edit.is_some_and(|t| t.elapsed().as_millis() < 700);
        if !quick || self.undo.is_empty() {
            self.undo.push((before_ops, before_disabled));
            if self.undo.len() > 200 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last_edit = if coalesce { Some(std::time::Instant::now()) } else { None };
    }

    pub fn undo(&mut self, tx: &Sender<Msg>, ctx: &egui::Context) {
        if let Some((prev_ops, prev_dis)) = self.undo.pop() {
            let cur_ops = std::mem::replace(&mut self.ops, prev_ops);
            let cur_dis = std::mem::replace(&mut self.disabled, prev_dis);
            self.redo.push((cur_ops, cur_dis));
            self.after_history(tx, ctx);
        }
    }

    pub fn redo(&mut self, tx: &Sender<Msg>, ctx: &egui::Context) {
        if let Some((next_ops, next_dis)) = self.redo.pop() {
            let cur_ops = std::mem::replace(&mut self.ops, next_ops);
            let cur_dis = std::mem::replace(&mut self.disabled, next_dis);
            self.undo.push((cur_ops, cur_dis));
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
        *self.slot.lock().unwrap() = Some(Job { gen: self.gen, src, ops: self.ops.clone(), disabled: self.disabled.clone(), output: self.output });
        if self.running.swap(true, Ordering::SeqCst) {
            return; // the worker that is running picks the new request up when it is done
        }
        let (slot, running, latest, tx, ctx) = (self.slot.clone(), self.running.clone(), self.latest.clone(), tx.clone(), ctx.clone());
        std::thread::spawn(move || loop {
            let job = slot.lock().unwrap().take();
            match job {
                Some(j) => {
                    let stale = || latest.load(Ordering::SeqCst) != j.gen;
                    if let Some((img, dims)) = compute_until(&j.src, &j.ops, &j.disabled, &stale) {
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
        self.refresh_backups();
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
        let (size, raw_rgba) = match ready {
            Some(d) => (d.size, d.rgba.clone()),
            None => {
                let d = make_display(img, self.output);
                (d.size, d.rgba)
            }
        };
        let rgba = if self.boost { boost_rgba(&raw_rgba) } else { raw_rgba };
        let ci = egui::ColorImage::from_rgba_premultiplied(size, &rgba);
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
    let tips: [&str; 6] = [
        "Open a skin archive (.osk or .zip)",
        "Open a skin folder",
        "Open a single PNG picture",
        "Save over the current file (Ctrl+S)",
        "Save as a new file (Ctrl+Shift+S)",
        "Pack this skin folder into an .osk",
    ];
    let mut clicked: Option<usize> = None;
    for (k, (label, kind, enabled, _)) in items.iter().enumerate() {
        if !shown[k] {
            continue;
        }
        if k == 3 || k == 5 {
            ui.add_space(10.0); // opening and saving are two groups, and Export is apart
        }
        if th::button(ui, label, *kind, *enabled).on_hover_text(tips[k]).clicked() {
            clicked = Some(k);
        }
        if k == 3 && (st.skin_ed.dirty || st.skin_ed.ini.dirty || st.skin_ed.creator.dirty || !st.skin_ed.stash.is_empty()) {
            let (d, resp) = ui.allocate_exact_size(Vec2::new(14.0, 14.0), Sense::hover());
            resp.on_hover_text("Unsaved changes");
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
                let (r, mut resp) = ui.allocate_exact_size(Vec2::new(ui.available_width().max(140.0), 30.0), Sense::click());
                if *enabled {
                    resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                }
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
        Ok(p) => {
            if st.skin_ed.dir.is_some() && st.skin_ed.dir == st.current_skin_folder() {
                st.load_session = true;
                st.reroll_map = false;
            }
            st.status = format!("Saved {}", p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
        }
        Err(e) => st.status = format!("Could not save: {e}"),
    }
}

pub fn do_save_ini(st: &mut Studio) {
    let Some(dir) = st.skin_ed.dir.clone() else { return };
    match st.skin_ed.ini.save_to_dir(&dir) {
        Ok(()) => {
            st.skin_ed.refresh_backups();
            if st.skin_ed.dir == st.current_skin_folder() {
                st.load_session = true;
                st.reroll_map = false;
            }
            st.status = "Saved skin.ini".into();
        }
        Err(e) => st.status = format!("Could not save skin.ini: {e}"),
    }
}

pub fn do_save_creator(st: &mut Studio) {
    let Some(dir) = st.skin_ed.dir.clone() else { return };
    let out = st.skin_ed.output;
    match st.skin_ed.creator.save_to_skin(&dir, out) {
        Ok((p2, p1)) => {
            let (tx, ctx) = (st.tx.clone(), st.ctx.clone());
            let name = p2.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            st.skin_ed.after_disk_mutation(&[p2, p1], None, &tx, &ctx);
            if st.skin_ed.dir == st.current_skin_folder() {
                st.load_session = true;
                st.reroll_map = false;
            }
            st.status = format!("Saved {name} + 1x pair");
        }
        Err(e) => st.status = format!("Could not save element: {e}"),
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
    // Ctrl+Z/Y for the edit list, plus canvas shortcuts (F/0 = Fit, 1 = 100%, B/\ = Before/After, S = Split) when not typing or in a popup
    if !root.ctx().memory(|m| m.focused().is_some()) {
        let popup_open = egui::Popup::is_any_open(root.ctx());
        let (redo_k, undo_k, fit_k, one_k, cmp_k, split_k) = root.input_mut(|i| {
            let none = egui::Modifiers::NONE;
            (
                i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z) || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z),
                !popup_open && (i.consume_key(none, egui::Key::F) || i.consume_key(none, egui::Key::Num0)),
                !popup_open && i.consume_key(none, egui::Key::Num1),
                !popup_open && (i.consume_key(none, egui::Key::B) || i.consume_key(none, egui::Key::Backslash)),
                !popup_open && i.consume_key(none, egui::Key::S),
            )
        });
        let (tx, ctx) = (st.tx.clone(), st.ctx.clone());
        if redo_k {
            if st.skin_ed.page == SkinPage::Create {
                st.skin_ed.creator.redo();
            } else {
                st.skin_ed.redo(&tx, &ctx);
            }
        } else if undo_k {
            if st.skin_ed.page == SkinPage::Create {
                st.skin_ed.creator.undo();
            } else {
                st.skin_ed.undo(&tx, &ctx);
            }
        }
        if st.skin_ed.source.is_some() || st.skin_ed.page == SkinPage::Create {
            if fit_k {
                st.skin_ed.zoom = 0.0;
                st.skin_ed.pan = Vec2::ZERO;
            } else if one_k {
                st.skin_ed.zoom = 1.0 / root.ctx().pixels_per_point();
                st.skin_ed.pan = Vec2::ZERO;
            } else if cmp_k {
                st.skin_ed.show = if st.skin_ed.show == Show::Before { Show::After } else { Show::Before };
            } else if split_k {
                st.skin_ed.show = Show::Split;
            }
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
            for (page, name, icon) in [
                (SkinPage::Pictures, "Pictures", 1),
                (SkinPage::Create, "Create", 0),
                (SkinPage::Edits, "Edits", 5),
                (SkinPage::Scan, "Scan", 7),
                (SkinPage::Ini, "skin.ini", 3),
                (SkinPage::Output, "Output", 9),
            ] {
                let resp = th::sidebar_item(ui, name, icon, st.skin_ed.page == page);
                let show_dot = (page == SkinPage::Ini && st.skin_ed.ini.dirty) || (page == SkinPage::Create && st.skin_ed.creator.dirty);
                if show_dot {
                    ui.painter().circle_filled(Pos2::new(resp.rect.right() - 18.0, resp.rect.center().y), 3.0, Color32::from_rgb(0xff, 0xad, 0x4d));
                }
                if resp.clicked() {
                    st.skin_ed.page = page;
                }
            }
        });
        match st.skin_ed.page {
            SkinPage::Pictures => pictures_page(ui, st, body),
            SkinPage::Create => scrolled(ui, "skin-create", body, |ui| create_page(ui, st)),
            SkinPage::Edits => scrolled(ui, "skin-edits", body, |ui| edits_page(ui, st)),
            SkinPage::Scan => scrolled(ui, "skin-scan", body, |ui| scan_page(ui, st)),
            SkinPage::Ini => scrolled(ui, "skin-ini", body, |ui| ini_page(ui, st)),
            SkinPage::Output => scrolled(ui, "skin-output", body, |ui| output_page(ui, st)),
        }
    });
    egui::CentralPanel::default().frame(egui::Frame::new().fill(BACKGROUND6)).show(root, |ui| {
        canvas(ui, st);
    });
}

/// A page that scrolls on its own inside `body`, remembering its own scroll position across tab switches.
fn scrolled(ui: &mut Ui, id: &str, body: Rect, f: impl FnOnce(&mut Ui)) {
    ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
        egui::ScrollArea::vertical().id_salt(id).auto_shrink([false, false]).show(ui, |ui| {
            ui.set_width(body.width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            f(ui);
            ui.add_space(40.0);
        });
    });
}

/// One line of small text, cut at the start when it is too long (the end of a path is the part that matters).
fn small_line(ui: &mut Ui, s: &str) {
    let font = FontId::new(13.0, th::light());
    let max_w = (ui.available_width() - 34.0).max(40.0);
    let shown = th::fit_text_start(ui.painter(), s, &font, max_w);
    ui.horizontal(|ui| {
        ui.add_space(17.0);
        ui.label(th::text(shown, 13.0, LIGHT3).family(th::light()));
    });
    ui.add_space(4.0);
}

fn pictures_page(ui: &mut Ui, st: &mut Studio, body: Rect) {
    if st.skin_ed.files.is_empty() {
        scrolled(ui, "skin-pictures-empty", body, |ui| {
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
                let is_dirty = (on && st.skin_ed.dirty) || st.skin_ed.stash.get(&f.path).is_some_and(|s| s.4);
                let width = ui.available_width() - 22.0;
                let (r, resp) = ui.allocate_exact_size(Vec2::new(width + 22.0, row_h), Sense::click());
                let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                let r = Rect::from_min_size(r.min + Vec2::new(12.0, 1.0), Vec2::new(width, row_h - 2.0));
                let hov = th::ease(ui, resp.id.with("h"), resp.hovered() && !on);
                if on {
                    ui.painter().rect_filled(r, CornerRadius::same(5), Color32::from_rgba_unmultiplied(0x8c, 0x66, 0xff, 54));
                } else if hov > 0.01 {
                    ui.painter().rect_filled(r, CornerRadius::same(5), Color32::from_rgba_unmultiplied(DARK2.r(), DARK2.g(), DARK2.b(), (hov * 255.0) as u8));
                }
                let box_ = Rect::from_center_size(Pos2::new(r.left() + 22.0, r.center().y), Vec2::splat(30.0));
                ui.painter().rect_filled(box_, CornerRadius::same(3), Color32::from_gray(0x2a));
                if let Some((t, sz)) = st.skin_ed.thumbs.get(&f.path) {
                    let fit = (30.0 / sz[0].max(1) as f32).min(30.0 / sz[1].max(1) as f32);
                    let size = Vec2::new(sz[0] as f32 * fit, sz[1] as f32 * fit).min(Vec2::splat(30.0));
                    ui.painter().image(t.id(), Rect::from_center_size(box_.center(), size), Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                }
                let name_x = r.left() + 46.0;
                let size_text = f.size.map(|(w, h)| format!("{w} × {h}"));
                let size_font = FontId::new(12.5, FontFamily::Proportional);
                let size_w = size_text.as_ref().map(|s| ui.painter().layout_no_wrap(s.clone(), size_font.clone(), LIGHT3).size().x + 14.0).unwrap_or(0.0);
                let dot_w = if is_dirty { 14.0 } else { 0.0 };
                let name_font = FontId::new(14.0, FontFamily::Proportional);
                let name_budget = (r.right() - name_x - size_w - dot_w - 8.0).max(24.0);
                let name = th::fit_text(ui.painter(), &f.name, &name_font, name_budget);
                let cut = name != f.name;
                ui.painter().text(Pos2::new(name_x, r.center().y), Align2::LEFT_CENTER, name, name_font, if on { CONTENT1 } else { CONTENT2 });
                if is_dirty {
                    ui.painter().circle_filled(Pos2::new(r.right() - size_w - 6.0, r.center().y), 3.0, Color32::from_rgb(0xff, 0xad, 0x4d));
                }
                if let Some(stxt) = size_text {
                    ui.painter().text(r.right_center() - Vec2::new(10.0, 0.0), Align2::RIGHT_CENTER, stxt, size_font, LIGHT3);
                }
                if cut {
                    resp.clone().on_hover_text(&f.name);
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

/// A row with a slider over `min..max`, showing the value; true when it moved. Double-clicking resets to `default_v` (when given).
fn slider_def(ui: &mut Ui, id: &str, label: &str, v: &mut f32, min: f32, max: f32, default_v: Option<f32>, show: impl Fn(f32) -> String) -> bool {
    let range = (max - min).max(1e-6);
    let step = nice_step(range);
    let snap = |x: f32| (((x / step).round() * step * 1e5).round() / 1e5).clamp(min, max);
    let mut f = ((*v - min) / range).clamp(0.0, 1.0);
    let def_f = default_v.map(|d| ((d - min) / range).clamp(0.0, 1.0));
    let (moved, typed) = th::slider_row_def(ui, id, label, &show(*v), &mut f, step / range, def_f);
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

fn slider(ui: &mut Ui, id: &str, label: &str, v: &mut f32, min: f32, max: f32, show: impl Fn(f32) -> String) -> bool {
    slider_def(ui, id, label, v, min, max, None, show)
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
            ("Color", Op::Adjust { brightness: 0.0, contrast: 0.0, saturation: 0.0, hue: 0.0 }),
            ("Tint", Op::Tint { rgb: [1.0, 0.4, 0.6], strength: 0.5 }),
            ("Opacity", Op::Opacity { factor: 1.0 }),
            ("Invert", Op::Invert),
            ("Blur", Op::Blur { sigma: 1.5 }),
            ("Sharpen", Op::Sharpen { amount: 0.8, sigma: 1.2 }),
            ("Deband", Op::Deband { radius: 64.0, threshold: 6.0, iterations: 4, grain: 0.5, kind: Grain::Gaussian, mono: true, room: true, luma: LumaScale { scaling: 4.0, by_average: false }, limit: 0.0, elast: 3.0, narrowing: false, prefilter: 0.0, protect: Mask { invert: true, ..Mask::off() }, adv: edit::Advanced::default(), target: Target::Both }),
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
    let before_disabled = st.skin_ed.disabled.clone();
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
            if th::button_fixed(ui, 112.0, if st.skin_ed.add_open { "Cancel" } else { "+ Add edit" }, Kind::Accent, true).clicked() {
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
    let (mut remove, mut up, mut down, mut toggle_vis) = (None, None, None, None);
    let n = st.skin_ed.ops.len();
    st.skin_ed.disabled.retain(|i| *i < n);
    for i in 0..n {
        let (iw, ih) = st.skin_ed.input_dims(i);
        let folded = st.skin_ed.collapsed.contains(&i);
        let is_off = st.skin_ed.disabled.contains(&i);
        // a title line: fold, name, and the buttons that toggle, move and remove the edit
        let width = ui.available_width() - 22.0;
        let (r, _) = ui.allocate_exact_size(Vec2::new(width + 22.0, 36.0), Sense::hover());
        let r = Rect::from_min_size(r.min + Vec2::new(12.0, 3.0), Vec2::new(width, 30.0));
        let head = ui.interact(r, ui.scope_id().with(("head", i)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        let hh = th::ease(ui, head.id.with("h"), head.hovered());
        ui.painter().rect_filled(r, CornerRadius::same(6), th::lerp_c(BACKGROUND5, th::lighten(BACKGROUND5, 0.04), hh));
        // the fold arrow
        let c = r.left_center() + Vec2::new(16.0, 0.0);
        let arrow = if folded { vec![c + Vec2::new(-2.5, -4.5), c + Vec2::new(3.0, 0.0), c + Vec2::new(-2.5, 4.5)] } else { vec![c + Vec2::new(-4.5, -2.5), c + Vec2::new(0.0, 3.0), c + Vec2::new(4.5, -2.5)] };
        ui.painter().add(egui::Shape::convex_polygon(arrow, if is_off { LIGHT3 } else { LIGHT1 }, Stroke::NONE));
        let title_font = FontId::new(15.0, th::semibold());
        let title_budget = (r.width() - 32.0 - 122.0).max(40.0);
        let title = th::fit_text(ui.painter(), &format!("{}. {}", i + 1, st.skin_ed.ops[i].name()), &title_font, title_budget);
        ui.painter().text(r.left_center() + Vec2::new(32.0, 0.0), Align2::LEFT_CENTER, title, title_font, if is_off { LIGHT3 } else { CONTENT1 });
        let icon_btn = |ui: &mut Ui, id: &str, right: f32, tip: &str, draw: &dyn Fn(&egui::Painter, Pos2, Color32)| -> bool {
            let b = Rect::from_center_size(Pos2::new(r.right() - right, r.center().y), Vec2::new(26.0, 24.0));
            let resp = ui.interact(b, ui.scope_id().with((id, i)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip);
            let h = th::ease(ui, resp.id.with("h"), resp.hovered());
            if h > 0.01 {
                ui.painter().rect_filled(b, CornerRadius::same(5), Color32::from_rgba_unmultiplied(DARK3.r(), DARK3.g(), DARK3.b(), (h * 255.0) as u8));
            }
            draw(ui.painter(), b.center(), th::lerp_c(LIGHT1, CONTENT1, h));
            resp.clicked()
        };
        let mut clicked_btn = false;
        if icon_btn(ui, "rm", 16.0, "Remove this edit", &|p, c, col| {
            p.line_segment([c + Vec2::new(-4.5, -4.5), c + Vec2::new(4.5, 4.5)], Stroke::new(1.8, col));
            p.line_segment([c + Vec2::new(-4.5, 4.5), c + Vec2::new(4.5, -4.5)], Stroke::new(1.8, col));
        }) {
            remove = Some(i);
            clicked_btn = true;
        }
        if i + 1 < n && icon_btn(ui, "dn", 44.0, "Move down (later)", &|p, c, col| {
            p.add(egui::Shape::line(vec![c + Vec2::new(-4.5, -2.0), c + Vec2::new(0.0, 2.5), c + Vec2::new(4.5, -2.0)], Stroke::new(1.8, col)));
        }) {
            down = Some(i);
            clicked_btn = true;
        }
        if i > 0 && icon_btn(ui, "up", 72.0, "Move up (earlier)", &|p, c, col| {
            p.add(egui::Shape::line(vec![c + Vec2::new(-4.5, 2.0), c + Vec2::new(0.0, -2.5), c + Vec2::new(4.5, 2.0)], Stroke::new(1.8, col)));
        }) {
            up = Some(i);
            clicked_btn = true;
        }
        if icon_btn(ui, "vis", 100.0, if is_off { "Show this edit (currently hidden)" } else { "Hide this edit (bypass)" }, &|p, c, col| {
            let ec = if is_off { LIGHT3 } else { col };
            let pts_top: Vec<Pos2> = (-4..=4).map(|k| { let t = k as f32 / 4.0; c + Vec2::new(t * 7.0, -(1.0 - t * t) * 4.2) }).collect();
            let pts_bot: Vec<Pos2> = (-4..=4).map(|k| { let t = k as f32 / 4.0; c + Vec2::new(t * 7.0, (1.0 - t * t) * 4.2) }).collect();
            p.add(egui::Shape::line(pts_top, Stroke::new(1.5, ec)));
            p.add(egui::Shape::line(pts_bot, Stroke::new(1.5, ec)));
            p.circle_filled(c, 2.0, ec);
            if is_off {
                p.line_segment([c + Vec2::new(-6.0, 5.5), c + Vec2::new(6.0, -5.5)], Stroke::new(1.8, PINK3));
            }
        }) {
            toggle_vis = Some(i);
            clicked_btn = true;
        }
        if head.clicked() && !clicked_btn {
            if folded {
                st.skin_ed.collapsed.remove(&i);
            } else {
                st.skin_ed.collapsed.insert(i);
            }
        }
        if !folded {
            let SkinEditor { ops, bufs, keep_aspect, .. } = &mut st.skin_ed;
            changed |= op_controls(ui, bufs, &mut ops[i], i, (iw, ih), keep_aspect);
        }
        ui.add_space(6.0);
    }
    let mut vis_changed = false;
    if let Some(i) = toggle_vis {
        if !st.skin_ed.disabled.remove(&i) {
            st.skin_ed.disabled.insert(i);
        }
        vis_changed = true;
    }
    if let Some(i) = remove {
        st.skin_ed.ops.remove(i);
        st.skin_ed.collapsed = st.skin_ed.collapsed.iter().filter(|c| **c != i).map(|c| if *c > i { c - 1 } else { *c }).collect();
        st.skin_ed.disabled = st.skin_ed.disabled.iter().filter(|c| **c != i).map(|c| if *c > i { c - 1 } else { *c }).collect();
        st.skin_ed.bufs.clear();
        changed = true;
    }
    if let Some(i) = up {
        st.skin_ed.ops.swap(i - 1, i);
        for set in [&mut st.skin_ed.collapsed, &mut st.skin_ed.disabled] {
            let (a, b) = (set.remove(&(i - 1)), set.remove(&i));
            if a { set.insert(i); }
            if b { set.insert(i - 1); }
        }
        st.skin_ed.bufs.clear();
        changed = true;
    }
    if let Some(i) = down {
        st.skin_ed.ops.swap(i, i + 1);
        for set in [&mut st.skin_ed.collapsed, &mut st.skin_ed.disabled] {
            let (a, b) = (set.remove(&i), set.remove(&(i + 1)));
            if a { set.insert(i + 1); }
            if b { set.insert(i); }
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
        st.skin_ed.disabled.clear();
        st.skin_ed.bufs.clear();
        changed = true;
    }
    th::switch_row(ui, "sk-keep", "Carry edits to next picture", &mut st.skin_ed.keep_edits);
    // Saved reusable filter presets
    st.skin_ed.ensure_presets();
    th::subsection_header(ui, "Filter presets");
    if !st.skin_ed.presets.is_empty() {
        let names: Vec<String> = st.skin_ed.presets.iter().map(|(n, _)| n.clone()).collect();
        if let Some(k) = th::dropdown_row(ui, "sk-preset-load", "Apply saved preset", "Choose a preset...", &names) {
            let ops = st.skin_ed.presets[k].1.clone();
            st.skin_ed.ops = ops;
            st.skin_ed.disabled.clear();
            st.skin_ed.collapsed.clear();
            st.skin_ed.bufs.clear();
            changed = true;
        }
        if let Some(k) = th::dropdown_row(ui, "sk-preset-del", "Delete a preset", "Pick preset to remove...", &names) {
            let removed = st.skin_ed.presets.remove(k).0;
            st.skin_ed.save_presets();
            st.status = format!("Deleted preset \"{removed}\"");
        }
    }
    th::text_row(ui, "sk-preset-name", "Preset name", &mut st.skin_ed.preset_name);
    if th::button_wide(ui, "Save current edits as preset", Kind::Dark, !st.skin_ed.ops.is_empty() && !st.skin_ed.preset_name.trim().is_empty()).clicked() {
        let name = st.skin_ed.preset_name.trim().to_string();
        let active_ops: Vec<Op> = st.skin_ed.ops.iter().enumerate().filter(|(i, _)| !st.skin_ed.disabled.contains(i)).map(|(_, o)| o.clone()).collect();
        if !active_ops.is_empty() {
            if let Some(slot) = st.skin_ed.presets.iter_mut().find(|(n, _)| n == &name) {
                slot.1 = active_ops;
            } else {
                st.skin_ed.presets.push((name.clone(), active_ops));
            }
            st.skin_ed.save_presets();
            st.skin_ed.preset_name.clear();
            st.status = format!("Saved preset \"{name}\"");
        }
    }
    // Batch-apply active edits to currently filtered pictures in the skin
    let active_ops: Vec<Op> = st.skin_ed.ops.iter().enumerate().filter(|(i, _)| !st.skin_ed.disabled.contains(i)).map(|(_, o)| o.clone()).collect();
    if !active_ops.is_empty() && st.skin_ed.dir.is_some() {
        let q = st.skin_ed.filter.trim().to_lowercase();
        let targets: Vec<PathBuf> = st.skin_ed.files.iter().filter(|f| q.is_empty() || f.name.to_lowercase().contains(&q)).map(|f| f.path.clone()).collect();
        if targets.len() > 1 {
            th::subsection_header(ui, "Batch apply");
            let lbl = if q.is_empty() {
                format!("Apply active edits to all {} pictures", targets.len())
            } else {
                format!("Apply active edits to {} filtered pictures", targets.len())
            };
            if th::button_wide(ui, &lbl, Kind::Purple, true).clicked() {
                let output = st.skin_ed.output;
                let (tx, ctx) = (st.tx.clone(), st.ctx.clone());
                st.status = format!("Applying edits to {} pictures...", targets.len());
                std::thread::spawn(move || {
                    let mut done = 0usize;
                    let mut skipped = 0usize;
                    let mut failed = 0usize;
                    let mut modified = Vec::new();
                    for p in targets {
                        let Ok(img) = edit::load_png(&p) else { failed += 1; continue };
                        if img.w <= 1 && img.h <= 1 {
                            skipped += 1;
                            continue;
                        }
                        if let (Some(dir), Some(name)) = (p.parent(), p.file_name()) {
                            let backup = dir.join(".skin-studio-backup");
                            let kept = backup.join(name);
                            if !kept.exists() {
                                let _ = std::fs::create_dir_all(&backup);
                                let _ = std::fs::copy(&p, &kept);
                            }
                        }
                        let mut cur = img;
                        for op in &active_ops {
                            cur = edit::apply(&cur, op);
                        }
                        if edit::save_png_with(&p, &cur, output).is_ok() {
                            done += 1;
                            modified.push(p);
                        } else {
                            failed += 1;
                        }
                    }
                    let msg = if failed > 0 {
                        format!("Batch applied to {done} pictures ({skipped} skipped, {failed} failed)")
                    } else {
                        format!("Batch applied to {done} pictures ({skipped} skipped)")
                    };
                    let _ = tx.send(Msg::SkinReloadFiles { msg, modified, resolved_downscale: None });
                    ctx.request_repaint();
                });
            }
        }
    }
    if vis_changed {
        st.skin_ed.push_undo_state(before_ops, before_disabled, false);
        st.skin_ed.dirty = true;
        let (tx, ctx) = (st.tx.clone(), st.ctx.clone());
        st.skin_ed.recompute(&tx, &ctx);
    } else if changed && st.skin_ed.ops != before_ops {
        let discrete = remove.is_some() || up.is_some() || down.is_some() || st.skin_ed.ops.len() != before_ops.len();
        st.skin_ed.push_undo_state(before_ops, before_disabled, !discrete);
        st.skin_ed.dirty = true;
        let (tx, ctx) = (st.tx.clone(), st.ctx.clone());
        st.skin_ed.recompute(&tx, &ctx);
    }
}

/// Skin-wide artifact scanner: finds poor 1x downscales (compared to @2x), banding, bright/dark haloing, aliasing, and noise.
fn scan_page(ui: &mut Ui, st: &mut Studio) {
    th::section_header(ui, "Scan");
    let Some(dir) = st.skin_ed.dir.clone() else {
        th::note(ui, "Open a skin folder or .osk on the Pictures page to scan its assets for banding, haloing, aliasing, noise, and poor 1x downscales.");
        return;
    };
    let btn_label = if st.skin_ed.scan_running { "Scanning..." } else { "Scan skin for issues" };
    if th::button_wide(ui, btn_label, Kind::Accent, !st.skin_ed.scan_running).clicked() {
        st.skin_ed.scan_running = true;
        st.status = "Scanning skin assets...".into();
        let (tx, ctx) = (st.tx.clone(), st.ctx.clone());
        std::thread::spawn(move || {
            let report = engine::scan::scan_skin(&dir);
            let _ = tx.send(Msg::SkinScanDone(report));
            ctx.request_repaint();
        });
    }
    th::note(ui, "Checks every PNG for 1x vs @2x downscale ringing, gradient banding, bright/dark edge haloing, diagonal aliasing, and flat-region noise. 1×1 blank files are skipped.");
    let Some(report) = st.skin_ed.scan_report.clone() else { return };
    let clean = report.total_files.saturating_sub(report.flagged.len() + report.skipped_blank);
    small_line(ui, &format!("Scanned {} files · {} flagged · {} clean ({} blank skipped)", report.total_files, report.flagged.len(), clean, report.skipped_blank));
    if report.flagged.is_empty() {
        th::note(ui, "No issues spotted across this skin.");
        return;
    }
    let mut inspect: Option<(PathBuf, Vec<Op>)> = None;
    let mut regen: Option<(PathBuf, PathBuf)> = None;
    for (idx, item) in report.flagged.iter().enumerate() {
        let width = ui.available_width() - 22.0;
        let tags: Vec<&str> = item.issues.iter().map(|i| i.label.as_str()).collect();
        let summary = tags.join(" · ");
        let has_regen = item.pair_2x.is_some() && item.issues.iter().any(|i| matches!(i.kind, engine::scan::IssueKind::PoorDownscale));
        let card_h = 92.0;
        let (r, _) = ui.allocate_exact_size(Vec2::new(width + 22.0, card_h + 6.0), Sense::hover());
        let r = Rect::from_min_size(r.min + Vec2::new(12.0, 2.0), Vec2::new(width, card_h));
        ui.painter().rect_filled(r, CornerRadius::same(6), CONTROL);
        // Thumbnail box (40x40, bottom at r.top() + 50)
        let tbox = Rect::from_min_size(r.min + Vec2::new(10.0, 10.0), Vec2::splat(40.0));
        ui.painter().rect_filled(tbox, CornerRadius::same(4), Color32::from_gray(0x22));
        if let Some((t, sz)) = st.skin_ed.thumbs.get(&item.path) {
            let fit = (38.0 / sz[0].max(1) as f32).min(38.0 / sz[1].max(1) as f32);
            let size = Vec2::new(sz[0] as f32 * fit, sz[1] as f32 * fit).min(Vec2::splat(38.0));
            ui.painter().image(t.id(), Rect::from_center_size(tbox.center(), size), Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
        let tx = tbox.right() + 10.0;
        // Row 1: full-width filename (never collides with dimensions)
        let nf = FontId::new(14.5, th::semibold());
        let name = th::fit_text(ui.painter(), &item.name, &nf, (r.right() - tx - 10.0).max(40.0));
        ui.painter().text(Pos2::new(tx, r.top() + 10.0), Align2::LEFT_TOP, name, nf, CONTENT1);
        // Row 2: issue tags on the left, measured dimensions on the right
        let dims_str = format!("{} × {}", item.dims.0, item.dims.1);
        let df = FontId::new(12.0, th::light());
        let dims_w = ui.painter().layout_no_wrap(dims_str.clone(), df.clone(), LIGHT3).size().x;
        let sf = FontId::new(12.5, FontFamily::Proportional);
        let shown_sum = th::fit_text(ui.painter(), &summary, &sf, (r.right() - tx - dims_w - 18.0).max(30.0));
        ui.painter().text(Pos2::new(tx, r.top() + 31.0), Align2::LEFT_TOP, shown_sum, sf, Color32::from_rgb(0xff, 0xad, 0x4d));
        ui.painter().text(Pos2::new(r.right() - 10.0, r.top() + 32.0), Align2::RIGHT_TOP, dims_str, df, LIGHT3);
        // Hovering the header always shows full filename, dimensions, and detailed per-issue findings
        let head_tip_rect = Rect::from_min_max(Pos2::new(tx, r.top() + 6.0), Pos2::new(r.right() - 6.0, r.top() + 52.0));
        let details = item.issues.iter().map(|i| format!("• {}: {}", i.label, i.detail)).collect::<Vec<_>>().join("\n");
        ui.interact(head_tip_rect, ui.scope_id().with(("sc-tip", idx)), Sense::hover())
            .on_hover_text(format!("{} ({} × {})\n{}", item.name, item.dims.0, item.dims.1, details));
        // Row 3: action buttons cleanly below thumbnail and text (y = r.top() + 58..82)
        let mut bx = r.left() + 10.0;
        let by = r.top() + 58.0;
        if has_regen {
            if let Some(p2) = &item.pair_2x {
                let br = Rect::from_min_size(Pos2::new(bx, by), Vec2::new(146.0, 24.0));
                if th::pill(ui, br, &format!("sc-rg-{idx}"), "Redo 1x from @2x", true, true, "Replace 1x with a clean zero-ringing downscale of @2x (backs up original)") {
                    regen = Some((item.path.clone(), p2.clone()));
                }
                bx += 152.0;
            }
        }
        let has_ops = !item.suggested_ops.is_empty();
        let ir = Rect::from_min_size(Pos2::new(bx, by), Vec2::new(if has_ops { 116.0 } else { 84.0 }, 24.0));
        let ilbl = if has_ops { "Inspect & fix" } else { "Inspect" };
        if th::pill(ui, ir, &format!("sc-in-{idx}"), ilbl, false, true, "Open in Split view with suggested edits loaded") {
            inspect = Some((item.path.clone(), item.suggested_ops.clone()));
        }
    }
    if let Some((dst_1x, src_2x)) = regen {
        let output = st.skin_ed.output;
        let (tx, ctx) = (st.tx.clone(), st.ctx.clone());
        let name = dst_1x.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        std::thread::spawn(move || {
            let (msg, ok) = match edit::load_png(&src_2x) {
                Ok(hi) => {
                    if let (Some(dir), Some(fname)) = (dst_1x.parent(), dst_1x.file_name()) {
                        let backup = dir.join(".skin-studio-backup");
                        let kept = backup.join(fname);
                        if dst_1x.is_file() && !kept.exists() {
                            let _ = std::fs::create_dir_all(&backup);
                            let _ = std::fs::copy(&dst_1x, &kept);
                        }
                    }
                    let out = edit::resize(&hi, (hi.w / 2).max(1), (hi.h / 2).max(1), Resample::Smooth);
                    match edit::save_png_with(&dst_1x, &out, output) {
                        Ok(()) => (format!("Regenerated {name} from @2x"), true),
                        Err(e) => (format!("Could not save {name}: {e}"), false),
                    }
                }
                Err(e) => (format!("Could not read @2x: {e}"), false),
            };
            let modified = if ok { vec![dst_1x.clone()] } else { vec![] };
            let resolved_downscale = ok.then_some(dst_1x);
            let _ = tx.send(Msg::SkinReloadFiles { msg, modified, resolved_downscale });
            ctx.request_repaint();
        });
    }
    if let Some((path, ops)) = inspect {
        let (tx, ctx) = (st.tx.clone(), st.ctx.clone());
        if let Err(e) = st.skin_ed.open_picture(&path, Some((&tx, &ctx))) {
            st.status = e;
        } else {
            if !ops.is_empty() {
                st.skin_ed.ops = ops;
                st.skin_ed.disabled.clear();
                st.skin_ed.collapsed.clear();
                st.skin_ed.dirty = true;
                st.skin_ed.recompute(&tx, &ctx);
            }
            st.skin_ed.show = Show::Split;
            st.skin_ed.page = SkinPage::Edits;
        }
    }
}

fn create_page(ui: &mut Ui, st: &mut Studio) {
    let dir = st.skin_ed.dir.clone();
    match crate::element_creator::draw_panel(ui, &mut st.skin_ed.creator, dir.as_deref()) {
        crate::element_creator::CreatorAction::None => {}
        crate::element_creator::CreatorAction::SaveToSkin => do_save_creator(st),
        crate::element_creator::CreatorAction::SendToEdits => {
            let img = st.skin_ed.creator.ensure_rendered();
            let stem = st.skin_ed.creator.doc.name.trim();
            let stem = if stem.is_empty() { st.skin_ed.creator.doc.kind.file_stem() } else { stem };
            let target = st.skin_ed.dir.as_ref().map(|d| d.join(format!("{stem}@2x.png"))).unwrap_or_else(|| PathBuf::from(format!("{stem}@2x.png")));
            st.skin_ed.current = Some(target);
            st.skin_ed.source = Some(img);
            st.skin_ed.ops.clear();
            st.skin_ed.disabled.clear();
            st.skin_ed.undo.clear();
            st.skin_ed.redo.clear();
            st.skin_ed.dirty = true;
            st.skin_ed.zoom = 0.0;
            st.skin_ed.pan = Vec2::ZERO;
            st.skin_ed.page = SkinPage::Edits;
            let (tx, ctx) = (st.tx.clone(), st.ctx.clone());
            st.skin_ed.recompute(&tx, &ctx);
            st.status = "Sent rendered @2x element to Edits".into();
        }
    }
}

fn ini_page(ui: &mut Ui, st: &mut Studio) {
    let has_dir = st.skin_ed.dir.is_some();
    if crate::skin_ini::draw_page(ui, &mut st.skin_ed.ini, has_dir) {
        do_save_ini(st);
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
    th::subsection_header(ui, "Scale full skin (@2x / 1x)");
    let has_dir = st.skin_ed.dir.is_some();
    if th::button_wide(ui, "Generate missing 1x from @2x", Kind::Purple, has_dir).clicked() {
        run_skin_scale(st, ScaleJob::DownMissing);
    }
    if th::button_wide(ui, "Regenerate all 1x from @2x", Kind::Dark, has_dir).clicked() {
        run_skin_scale(st, ScaleJob::DownAll);
    }
    let modes: Vec<String> = Resample::ALL.iter().map(|m| m.name().to_string()).collect();
    if let Some(k) = th::dropdown_row(ui, "scaleupmode", "Upscale mode (1x → @2x)", st.skin_ed.scale_up_mode.name(), &modes) {
        st.skin_ed.scale_up_mode = Resample::ALL[k];
    }
    if th::button_wide(ui, "Generate missing @2x from 1x", Kind::Dark, has_dir).clicked() {
        run_skin_scale(st, ScaleJob::UpMissing(st.skin_ed.scale_up_mode));
    }
    th::note(ui, "Downscaling always area-averages with zero ringing. 1×1 blank PNGs are skipped, and overwritten files are backed up into .skin-studio-backup.");
    th::subsection_header(ui, "Export");
    if th::button_wide(ui, "Export the skin as .osk...", Kind::Purple, has_dir).clicked() {
        st.pick(PickKind::SkinEditExportOsk);
    }
    th::note(ui, "Save writes the picture over the file it came from (the first original is kept in a .skin-studio-backup folder next to it); Save as writes a new file.");
    // Backup restore viewer (uses cached `backups` list — zero per-frame filesystem I/O)
    if let Some(dir) = st.skin_ed.dir.clone() {
        let backups = st.skin_ed.backups.clone();
        if !backups.is_empty() {
            th::subsection_header(ui, &format!("Backups ({})", backups.len()));
            if th::button_wide(ui, "Restore all original backups", Kind::Dark, true).clicked() {
                let mut n = 0usize;
                let mut restored = Vec::new();
                for (_, bp, dest) in &backups {
                    if std::fs::copy(bp, dest).is_ok() {
                        n += 1;
                        st.skin_ed.stash.remove(dest);
                        restored.push(dest.clone());
                    }
                }
                let (tx, ctx) = (st.tx.clone(), st.ctx.clone());
                st.skin_ed.ini = crate::skin_ini::SkinIniDoc::load_from_dir(&dir);
                st.skin_ed.after_disk_mutation(&restored, None, &tx, &ctx);
                if st.skin_ed.dir == st.current_skin_folder() {
                    st.load_session = true;
                    st.reroll_map = false;
                }
                st.status = format!("Restored {n} original files from backup");
            }
            for (idx, (shown_name, bp, dest)) in backups.iter().enumerate() {
                let width = ui.available_width() - 22.0;
                let (r, _) = ui.allocate_exact_size(Vec2::new(width + 22.0, 36.0), Sense::hover());
                let r = Rect::from_min_size(r.min + Vec2::new(12.0, 2.0), Vec2::new(width, 32.0));
                ui.painter().rect_filled(r, CornerRadius::same(5), CONTROL);
                let nf = FontId::new(13.5, FontFamily::Proportional);
                let shown = th::fit_text(ui.painter(), shown_name, &nf, (r.width() - 86.0).max(30.0));
                let cut = shown != *shown_name;
                ui.painter().text(r.left_center() + Vec2::new(10.0, 0.0), Align2::LEFT_CENTER, shown, nf, CONTENT2);
                if cut {
                    ui.interact(Rect::from_min_max(r.min, Pos2::new(r.right() - 78.0, r.bottom())), ui.scope_id().with(("bk-tip", idx)), Sense::hover()).on_hover_text(shown_name);
                }
                let br = Rect::from_center_size(Pos2::new(r.right() - 40.0, r.center().y), Vec2::new(66.0, 24.0));
                if th::pill(ui, br, &format!("bk-r-{idx}"), "Restore", false, true, "Copy original backup back over the edited file") {
                    match std::fs::copy(bp, dest) {
                        Ok(_) => {
                            let (tx, ctx) = (st.tx.clone(), st.ctx.clone());
                            st.skin_ed.stash.remove(dest);
                            if shown_name.eq_ignore_ascii_case("skin.ini") {
                                st.skin_ed.ini = crate::skin_ini::SkinIniDoc::load_from_dir(&dir);
                            } else {
                                st.skin_ed.after_disk_mutation(std::slice::from_ref(dest), None, &tx, &ctx);
                            }
                            if st.skin_ed.dir == st.current_skin_folder() {
                                st.load_session = true;
                                st.reroll_map = false;
                            }
                            st.status = format!("Restored {shown_name}");
                        }
                        Err(e) => st.status = format!("Could not restore {shown_name}: {e}"),
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum ScaleJob {
    DownMissing,
    DownAll,
    UpMissing(Resample),
}

fn run_skin_scale(st: &mut Studio, job: ScaleJob) {
    let Some(dir) = st.skin_ed.dir.clone() else { return };
    let output = st.skin_ed.output;
    let (tx, ctx) = (st.tx.clone(), st.ctx.clone());
    st.status = "Scaling skin pictures...".into();
    std::thread::spawn(move || {
        let files = list_pngs(&dir);
        let mut made = 0usize;
        let mut skipped = 0usize;
        let mut failed = 0usize;
        let mut modified = Vec::new();
        for f in files {
            let p = f.path;
            let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else { continue };
            let Some(parent) = p.parent() else { continue };
            let is_2x = stem.ends_with("@2x");
            match job {
                ScaleJob::DownMissing | ScaleJob::DownAll => {
                    if !is_2x {
                        continue;
                    }
                    let base = &stem[..stem.len() - 3];
                    let dest = parent.join(format!("{base}.png"));
                    if matches!(job, ScaleJob::DownMissing) && dest.exists() {
                        skipped += 1;
                        continue;
                    }
                    let Ok(img) = edit::load_png(&p) else { failed += 1; continue };
                    if img.w <= 1 && img.h <= 1 {
                        skipped += 1;
                        continue;
                    }
                    if dest.is_file() {
                        let backup = parent.join(".skin-studio-backup");
                        let kept = backup.join(format!("{base}.png"));
                        if !kept.exists() {
                            let _ = std::fs::create_dir_all(&backup);
                            let _ = std::fs::copy(&dest, &kept);
                        }
                    }
                    let out = edit::resize(&img, (img.w / 2).max(1), (img.h / 2).max(1), Resample::Smooth);
                    if edit::save_png_with(&dest, &out, output).is_ok() { made += 1; modified.push(dest); } else { failed += 1; }
                }
                ScaleJob::UpMissing(mode) => {
                    if is_2x {
                        continue;
                    }
                    let dest = parent.join(format!("{stem}@2x.png"));
                    if dest.exists() {
                        skipped += 1;
                        continue;
                    }
                    let Ok(img) = edit::load_png(&p) else { failed += 1; continue };
                    if img.w <= 1 && img.h <= 1 {
                        skipped += 1;
                        continue;
                    }
                    let out = edit::resize(&img, img.w * 2, img.h * 2, mode);
                    if edit::save_png_with(&dest, &out, output).is_ok() { made += 1; modified.push(dest); } else { failed += 1; }
                }
            }
        }
        let msg = if failed > 0 {
            format!("Scaled {made} pictures ({skipped} skipped, {failed} failed)")
        } else {
            format!("Scaled {made} pictures ({skipped} skipped)")
        };
        let _ = tx.send(Msg::SkinReloadFiles { msg, modified, resolved_downscale: None });
        ctx.request_repaint();
    });
}

/// The settings of one edit; true when something changed.
fn op_controls(ui: &mut Ui, bufs: &mut HashMap<String, String>, op: &mut Op, i: usize, (iw, ih): (usize, usize), keep_aspect: &mut bool) -> bool {
    let mut ch = false;
    match op {
        Op::Resize { w, h, mode } => {
            th::switch_row(ui, &format!("ka{i}"), "Keep aspect ratio", keep_aspect);
            if let Some(v) = num_row(ui, bufs, &format!("rw{i}"), "Width (px)", *w) {
                if v > 0 {
                    *w = v.min(16384);
                    if *keep_aspect {
                        *h = ((*w as f64 * ih as f64 / iw.max(1) as f64).round() as usize).clamp(1, 16384);
                        bufs.remove(&format!("rh{i}"));
                    }
                    ch = true;
                }
            }
            if let Some(v) = num_row(ui, bufs, &format!("rh{i}"), "Height (px)", *h) {
                if v > 0 {
                    *h = v.min(16384);
                    if *keep_aspect {
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
            ch |= slider_def(ui, &format!("tr{i}"), "Empty up to an alpha of", threshold, 0.0, 0.5, Some(0.0), |v| format!("{:.0}%", v * 100.0));
        }
        Op::Adjust { brightness, contrast, saturation, hue } => {
            ch |= slider_def(ui, &format!("ab{i}"), "Brightness", brightness, -1.0, 1.0, Some(0.0), |v| format!("{:+.0}", v * 100.0));
            ch |= slider_def(ui, &format!("ac{i}"), "Contrast", contrast, -1.0, 1.0, Some(0.0), |v| format!("{:+.0}", v * 100.0));
            ch |= slider_def(ui, &format!("as{i}"), "Saturation", saturation, -1.0, 1.0, Some(0.0), |v| format!("{:+.0}", v * 100.0));
            ch |= slider_def(ui, &format!("ah{i}"), "Hue", hue, -180.0, 180.0, Some(0.0), |v| format!("{v:+.0}°"));
        }
        Op::Tint { rgb, strength } => {
            let u = [(rgb[0] * 255.0).round().clamp(0.0, 255.0) as u8, (rgb[1] * 255.0).round().clamp(0.0, 255.0) as u8, (rgb[2] * 255.0).round().clamp(0.0, 255.0) as u8];
            let cur_hex = format!("#{:02x}{:02x}{:02x}", u[0], u[1], u[2]);
            let key = format!("tc{i}");
            let buf = bufs.entry(key.clone()).or_insert_with(|| cur_hex.clone());
            let (resp, row_rect) = th::text_row_pad(ui, &key, "Color (#rrggbb)", buf, 34.0);
            let text = buf.clone();
            if resp.changed() {
                if let Some(c) = crate::state::hex_to_rgb(&text) {
                    *rgb = [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0];
                    ch = true;
                }
            } else if !resp.has_focus() && text != cur_hex {
                bufs.insert(key.clone(), cur_hex);
            }
            let sw = Rect::from_center_size(Pos2::new(row_rect.right() - 22.0, row_rect.center().y), Vec2::new(24.0, 24.0));
            if let Some(c) = crate::ui::swatch_picker(ui, &format!("skt{i}"), sw, u) {
                *rgb = [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0];
                bufs.insert(key, format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]));
                ch = true;
            }
            ch |= slider_def(ui, &format!("ts{i}"), "Strength", strength, 0.0, 1.0, Some(0.5), |v| format!("{:.0}%", v * 100.0));
        }
        Op::Opacity { factor } => {
            ch |= slider_def(ui, &format!("op{i}"), "Opacity", factor, 0.0, 1.0, Some(1.0), |v| format!("{:.0}%", v * 100.0));
        }
        Op::Invert => {}
        Op::Blur { sigma } => {
            ch |= slider_def(ui, &format!("bl{i}"), "Blur radius", sigma, 0.1, 20.0, Some(1.5), |v| format!("{v:.1} px"));
        }
        Op::Sharpen { amount, sigma } => {
            ch |= slider_def(ui, &format!("sa{i}"), "Amount", amount, 0.0, 3.0, Some(0.8), |v| format!("{:.0}%", v * 100.0));
            ch |= slider_def(ui, &format!("ss{i}"), "Radius", sigma, 0.3, 6.0, Some(1.2), |v| format!("{v:.1} px"));
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
        Op::Deband { radius, threshold, iterations, grain, kind, mono, room, luma, limit, elast, narrowing, prefilter, protect, adv, target } => {
            ch |= target_row(ui, &format!("dtg{i}"), target);
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
                ch |= th::switch_row(ui, &format!("dm{i}"), "Gray grain", mono);
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
    if m.enabled || (with_luma && m.luma.scaling != 0.0) {
        ch |= th::switch_row(ui, &format!("{id}v"), "Show the mask", &mut m.show);
    } else if m.show {
        m.show = false;
        ch = true;
    }
    ch
}

/// Luma scaling (adaptivegrain, as used by vodesfunc): more of the filter in the dark, hardly any in the light.
fn luma_controls(ui: &mut Ui, id: &str, l: &mut LumaScale) -> bool {
    let mut ch = slider_def(ui, &format!("{id}s"), "Favor dark areas (luma scaling)", &mut l.scaling, -20.0, 20.0, Some(0.0), |v| if v == 0.0 { "off".to_string() } else if v > 0.0 { format!("dark {v:.1}") } else { format!("bright {:.1}", -v) });
    if l.scaling != 0.0 {
        ch |= th::switch_row(ui, &format!("{id}a"), "Scale by the average brightness", &mut l.by_average);
    }
    ch
}

// ==================================================================================================================== the picture
fn canvas(ui: &mut Ui, st: &mut Studio) {
    if st.skin_ed.page == SkinPage::Create {
        creator_canvas(ui, st);
        return;
    }
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
        th::pill(ui, r, &format!("sk-{label}"), label, on, true, tip)
    };
    // everything that is in the bar, with how long it stays when the bar is narrow (the larger, the sooner it goes) and the gap before it
    let zooms: [(&str, f32); 4] = [("100%", 1.0 / ppp), ("200%", 2.0 / ppp), ("400%", 4.0 / ppp), ("800%", 8.0 / ppp)];
    #[derive(Clone, Copy)]
    enum Pill {
        Show(Show, &'static str, &'static str),
        Fit,
        Zoom(usize),
        Back(Bg, &'static str),
        Boost,
    }
    let mut pills: Vec<(Pill, f32, u8, f32)> = vec![
        (Pill::Show(Show::After, "After", "The picture with the edits (B or \\ to compare)"), 62.0, 0, 0.0),
        (Pill::Show(Show::Before, "Before", "The picture as it was loaded (B or \\ to compare)"), 62.0, 0, 0.0),
        (Pill::Show(Show::Split, "Split", "Before on the left, after on the right (S)"), 58.0, 0, 0.0),
        (Pill::Fit, 44.0, 0, 12.0),
        (Pill::Zoom(0), 52.0, 2, 0.0),
        (Pill::Zoom(1), 52.0, 3, 0.0),
        (Pill::Zoom(2), 52.0, 4, 0.0),
        (Pill::Zoom(3), 52.0, 5, 0.0),
        (Pill::Back(Bg::Dark, "Black"), 52.0, 1, 12.0),
        (Pill::Back(Bg::Grey, "Gray"), 52.0, 1, 0.0),
        (Pill::Back(Bg::Light, "Light"), 52.0, 1, 0.0),
        (Pill::Back(Bg::Check, "Check"), 56.0, 1, 0.0),
        (Pill::Boost, 56.0, 2, 12.0),
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
                if pill(ui, r, "Fit", ed.zoom == 0.0, "Fit the picture in the window (F or 0)") {
                    ed.zoom = 0.0;
                    ed.pan = Vec2::ZERO;
                }
            }
            Pill::Zoom(i) => {
                let (label, z) = zooms[i];
                let tip = if i == 0 { "Show at 100%: one picture pixel per screen pixel (1)" } else { "Show the picture at this size: one pixel of it is this many pixels of the screen" };
                if pill(ui, r, label, (ed.zoom - z).abs() < 1e-3, tip) {
                    ed.zoom = z;
                    ed.pan = Vec2::ZERO;
                }
            }
            Pill::Back(b, label) => {
                if pill(ui, r, label, ed.bg == b, "What is behind the picture") {
                    ed.bg = b;
                }
            }
            Pill::Boost => {
                if pill(ui, r, "Boost", ed.boost, "Amplify dark tones and banding on screen (does not affect saved file)") {
                    ed.boost = !ed.boost;
                    ed.tex = [None, None];
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
    // Dedicated 30 px bottom status strip so the caption always has clean contrast on every background.
    let stage = Rect::from_min_max(area.min, Pos2::new(area.right(), (area.bottom() - 30.0).max(area.top() + 1.0)));
    let status_bar = Rect::from_min_max(Pos2::new(area.left(), stage.bottom()), area.max);
    let fit = ((stage.width() - 40.0) / iw).min((stage.height() - 40.0) / ih).max(0.01);
    let z = if ed.zoom == 0.0 { fit } else { ed.zoom };
    // Register canvas first, then split divider on top so the 18 px divider wins hit-testing while panning works everywhere else.
    let resp = ui.interact(stage, ui.scope_id().with("sk-canvas"), Sense::click_and_drag());
    let mut split_dragged = false;
    let sx = stage.left() + stage.width() * ed.split;
    if ed.show == Show::Split {
        let h = Rect::from_center_size(Pos2::new(sx, stage.center().y), Vec2::new(18.0, stage.height()));
        let hr = ui.interact(h, ui.scope_id().with("sk-split"), Sense::drag()).on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        if hr.dragged() {
            ed.split = ((sx + hr.drag_delta().x - stage.left()) / stage.width().max(1.0)).clamp(0.02, 0.98);
            split_dragged = true;
        }
    }
    if resp.dragged() && !split_dragged {
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
    let painter = ui.painter().with_clip_rect(stage);
    match ed.bg {
        Bg::Grey => {
            painter.rect_filled(stage, 0.0, Color32::from_gray(0x80));
        }
        Bg::Dark => {
            painter.rect_filled(stage, 0.0, Color32::BLACK);
        }
        Bg::Light => {
            painter.rect_filled(stage, 0.0, Color32::from_gray(0xe6));
        }
        Bg::Check => {
            // Single repeating 2x2 NEAREST quad (2 triangles total instead of thousands of per-tile rects at 480 Hz).
            let c0 = Color32::from_rgb(0x22, 0x20, 0x29);
            let c1 = Color32::from_rgb(0x30, 0x2d, 0x3a);
            let tex = ed.check_tex.get_or_insert_with(|| {
                let ci = egui::ColorImage { size: [2, 2], source_size: Vec2::new(2.0, 2.0), pixels: vec![c0, c1, c1, c0] };
                ctx.load_texture("skin-check", ci, egui::TextureOptions::NEAREST_REPEAT)
            });
            let tile = 24.0; // 2x2 pixels = 12 pt per checker square
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(stage.width() / tile, stage.height() / tile));
            painter.image(tex.id(), stage, uv, Color32::WHITE);
        }
    }
    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    let tex_after = ed.texture(&ctx, 1, &after);
    let sx = stage.left() + stage.width() * ed.split;
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
            let r = Rect::from_center_size(rect.center(), Vec2::new(before.w as f32 * z, before.h as f32 * z));
            ui.painter().with_clip_rect(Rect::from_min_max(stage.min, Pos2::new(sx, stage.bottom()))).image(t.id(), r, uv, Color32::WHITE);
            ui.painter().with_clip_rect(Rect::from_min_max(Pos2::new(sx, stage.top()), stage.max)).image(tex_after.id(), rect, uv, Color32::WHITE);
            painter.line_segment([Pos2::new(sx, stage.top()), Pos2::new(sx, stage.bottom())], Stroke::new(2.0, Color32::from_rgb(0xff, 0x66, 0xab)));
            painter.circle_filled(Pos2::new(sx, stage.center().y), 7.0, Color32::from_rgb(0xff, 0x66, 0xab));
            if stage.width() > 240.0 {
                let badge_font = FontId::new(12.0, th::semibold());
                for (lbl, pos, align) in [("Before", Pos2::new(stage.left() + 12.0, stage.top() + 12.0), Align2::LEFT_TOP), ("After", Pos2::new(stage.right() - 12.0, stage.top() + 12.0), Align2::RIGHT_TOP)] {
                    let g = painter.layout_no_wrap(lbl.to_string(), badge_font.clone(), CONTENT1);
                    let bsize = g.size() + Vec2::new(14.0, 6.0);
                    let brect = if align == Align2::LEFT_TOP { Rect::from_min_size(pos, bsize) } else { Rect::from_min_size(Pos2::new(pos.x - bsize.x, pos.y), bsize) };
                    painter.rect_filled(brect, CornerRadius::same(4), Color32::from_rgba_unmultiplied(0x17, 0x15, 0x1e, 185));
                    painter.galley(brect.min + Vec2::new(7.0, 3.0), g, CONTENT1);
                }
            }
        }
    }
    let border_col = if ed.bg == Bg::Light { Color32::from_black_alpha(36) } else { Color32::from_white_alpha(30) };
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0, border_col), egui::StrokeKind::Outside);
    // --- bottom status strip
    ui.painter().rect_filled(status_bar, 0.0, BACKGROUND5);
    ui.painter().line_segment([status_bar.left_top(), status_bar.right_top()], Stroke::new(1.0, Color32::from_white_alpha(12)));
    let name = ed.current.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let rest = if (before.w, before.h) == (after.w, after.h) { format!("   {} × {} px   {:.0}%", after.w, after.h, z * ppp * 100.0) } else { format!("   {} × {} px  →  {} × {} px   {:.0}%", before.w, before.h, after.w, after.h, z * ppp * 100.0) } + if ed.busy { "   working..." } else { "" };
    let info_font = FontId::new(13.0, FontFamily::Proportional);
    let rest_w = ui.painter().layout_no_wrap(rest.clone(), info_font.clone(), CONTENT1).size().x;
    let name = th::fit_text(ui.painter(), &name, &info_font, (status_bar.width() - 32.0 - rest_w).max(40.0));
    ui.painter().text(Pos2::new(status_bar.left() + 16.0, status_bar.center().y), Align2::LEFT_CENTER, name + &rest, info_font, CONTENT2);
}

/// Live canvas when on the `Create` tab: renders the procedural SDF element with optional osu! alignment guides and combo-color tint preview.
fn creator_canvas(ui: &mut Ui, st: &mut Studio) {
    let full = ui.max_rect();
    let bar = Rect::from_min_size(full.min, Vec2::new(full.width(), 44.0));
    let area = Rect::from_min_max(Pos2::new(full.left(), bar.bottom()), full.max);
    let ctx = ui.ctx().clone();
    let ppp = ctx.pixels_per_point();
    let ed = &mut st.skin_ed;

    // --- top bar
    ui.painter().rect_filled(bar, 0.0, BACKGROUND5);
    let x = std::cell::Cell::new(bar.left() + 12.0);
    let place = |w: f32| {
        let r = Rect::from_min_size(Pos2::new(x.get(), bar.top() + 7.0), Vec2::new(w, 30.0));
        x.set(x.get() + w + 6.0);
        r
    };
    let pill = |ui: &mut Ui, r: Rect, label: &str, on: bool, tip: &str| -> bool {
        th::pill(ui, r, &format!("cr-p-{label}"), label, on, true, tip)
    };
    let zooms: [(&str, f32); 4] = [("100%", 1.0 / ppp), ("200%", 2.0 / ppp), ("400%", 4.0 / ppp), ("800%", 8.0 / ppp)];
    let is_tinted_kind = ed.creator.doc.kind.is_combo_tinted();
    #[derive(Clone, Copy)]
    enum CPill { Fit, Zoom(usize), Back(Bg, &'static str), Boost, Guides, ComboTint }
    let mut pills: Vec<(CPill, f32, u8, f32)> = vec![
        (CPill::Fit, 44.0, 0, 0.0),
        (CPill::Zoom(0), 52.0, 2, 0.0),
        (CPill::Zoom(1), 52.0, 3, 0.0),
        (CPill::Zoom(2), 52.0, 4, 0.0),
        (CPill::Zoom(3), 52.0, 5, 0.0),
        (CPill::Back(Bg::Dark, "Black"), 52.0, 1, 12.0),
        (CPill::Back(Bg::Grey, "Gray"), 52.0, 1, 0.0),
        (CPill::Back(Bg::Light, "Light"), 52.0, 1, 0.0),
        (CPill::Back(Bg::Check, "Check"), 56.0, 1, 0.0),
        (CPill::Boost, 56.0, 2, 12.0),
        (CPill::Guides, 62.0, 0, 6.0),
    ];
    if is_tinted_kind {
        pills.push((CPill::ComboTint, 88.0, 0, 6.0));
    }
    let fits = |p: &Vec<(CPill, f32, u8, f32)>| p.iter().map(|q| q.1 + 6.0 + q.3).sum::<f32>() + 48.0 <= bar.width();
    while !fits(&pills) {
        let Some(k) = pills.iter().enumerate().filter(|(_, q)| q.2 > 0).max_by_key(|(_, q)| q.2).map(|(k, _)| k) else { break };
        pills.remove(k);
    }
    let combos: Vec<[u8; 3]> = if ed.ini.combos.is_empty() {
        vec![[255, 192, 0], [0, 202, 0], [18, 124, 255], [242, 24, 57]]
    } else {
        ed.ini.combos.clone()
    };
    let tint_rgb = combos[ed.creator.combo_idx % combos.len()];
    for (pk, w, _, gap) in pills {
        x.set(x.get() + gap);
        let r = place(w);
        match pk {
            CPill::Fit => if pill(ui, r, "Fit", ed.zoom == 0.0, "Fit in view (F or 0)") { ed.zoom = 0.0; ed.pan = Vec2::ZERO; },
            CPill::Zoom(i) => {
                let (label, z) = zooms[i];
                if pill(ui, r, label, (ed.zoom - z).abs() < 1e-3, "Show at this pixel zoom") { ed.zoom = z; ed.pan = Vec2::ZERO; }
            }
            CPill::Back(b, label) => if pill(ui, r, label, ed.bg == b, "Canvas background") { ed.bg = b; },
            CPill::Boost => if pill(ui, r, "Boost", ed.boost, "Amplify dark tones and banding on screen") { ed.boost = !ed.boost; ed.creator_tex = None; },
            CPill::Guides => if pill(ui, r, "Guides", ed.creator.guides, "Toggle osu! center crosshair and size guide") { ed.creator.guides = !ed.creator.guides; },
            CPill::ComboTint => {
                if pill(ui, r, "Combo tint", ed.creator.combo_tint, "Preview osu! combo color multiplication without baking it into the PNG") {
                    ed.creator.combo_tint = !ed.creator.combo_tint;
                    ed.creator_tex = None;
                }
            }
        }
    }
    if is_tinted_kind && ed.creator.combo_tint && x.get() + 28.0 <= bar.right() - 8.0 {
        let sw = Rect::from_min_size(Pos2::new(x.get() + 2.0, bar.top() + 10.0), Vec2::splat(24.0));
        let resp = ui.interact(sw, ui.scope_id().with("cr-combo-cyc"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Click to cycle combo color");
        ui.painter().rect_filled(sw, CornerRadius::same(4), Color32::from_rgb(tint_rgb[0], tint_rgb[1], tint_rgb[2]));
        ui.painter().rect_stroke(sw, CornerRadius::same(4), Stroke::new(1.5, CONTENT1), egui::StrokeKind::Inside);
        if resp.clicked() {
            ed.creator.combo_idx = (ed.creator.combo_idx + 1) % combos.len();
            ed.creator_tex = None;
        }
    }

    // --- stage & rendering
    let img = ed.creator.ensure_rendered();
    let (iw, ih) = (img.w as f32, img.h as f32);
    let stage = Rect::from_min_max(area.min, Pos2::new(area.right(), (area.bottom() - 30.0).max(area.top() + 1.0)));
    let status_bar = Rect::from_min_max(Pos2::new(area.left(), stage.bottom()), area.max);
    let fit = ((stage.width() - 40.0) / iw).min((stage.height() - 40.0) / ih).max(0.01);
    let z0 = if ed.zoom == 0.0 { fit } else { ed.zoom };
    let resp = ui.interact(stage, ui.scope_id().with("cr-canvas"), Sense::click_and_drag());
    if resp.dragged() {
        ed.pan += resp.drag_delta();
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.1 {
            let new = (z0 * (scroll * 0.003).exp()).clamp(0.02, 64.0);
            if let Some(m) = ui.input(|i| i.pointer.hover_pos()) {
                let c = stage.center() + ed.pan;
                ed.pan = (m - stage.center()) - (m - c) * (new / z0);
            }
            ed.zoom = new;
        }
    }
    let z = if ed.zoom == 0.0 { fit } else { ed.zoom };
    let rect = Rect::from_center_size(stage.center() + ed.pan, Vec2::new(iw * z, ih * z));

    let painter = ui.painter().with_clip_rect(stage);
    match ed.bg {
        Bg::Grey => { painter.rect_filled(stage, 0.0, Color32::from_gray(0x80)); }
        Bg::Dark => { painter.rect_filled(stage, 0.0, Color32::BLACK); }
        Bg::Light => { painter.rect_filled(stage, 0.0, Color32::from_gray(0xe6)); }
        Bg::Check => {
            let c0 = Color32::from_rgb(0x22, 0x20, 0x29);
            let c1 = Color32::from_rgb(0x30, 0x2d, 0x3a);
            let tex = ed.check_tex.get_or_insert_with(|| {
                let ci = egui::ColorImage { size: [2, 2], source_size: Vec2::new(2.0, 2.0), pixels: vec![c0, c1, c1, c0] };
                ctx.load_texture("skin-check", ci, egui::TextureOptions::NEAREST_REPEAT)
            });
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(stage.width() / 24.0, stage.height() / 24.0));
            painter.image(tex.id(), stage, uv, Color32::WHITE);
        }
    }

    let apply_tint = is_tinted_kind && ed.creator.combo_tint;
    let gen_key = ed.creator.render_gen;
    let tex = if let Some((k, b, at, rgb, t)) = &ed.creator_tex {
        if *k == gen_key && *b == ed.boost && *at == apply_tint && (!apply_tint || *rgb == tint_rgb) {
            t.clone()
        } else {
            ed.creator_tex = None;
            let shown_img = if apply_tint { crate::element_creator::apply_preview_tint(&img, tint_rgb) } else { (*img).clone() };
            let d = make_display(&shown_img, ed.output);
            let rgba = if ed.boost { boost_rgba(&d.rgba) } else { d.rgba };
            let t = ctx.load_texture("cr-preview", egui::ColorImage::from_rgba_premultiplied(d.size, &rgba), egui::TextureOptions::NEAREST);
            ed.creator_tex = Some((gen_key, ed.boost, apply_tint, tint_rgb, t.clone()));
            t
        }
    } else {
        let shown_img = if apply_tint { crate::element_creator::apply_preview_tint(&img, tint_rgb) } else { (*img).clone() };
        let d = make_display(&shown_img, ed.output);
        let rgba = if ed.boost { boost_rgba(&d.rgba) } else { d.rgba };
        let t = ctx.load_texture("cr-preview", egui::ColorImage::from_rgba_premultiplied(d.size, &rgba), egui::TextureOptions::NEAREST);
        ed.creator_tex = Some((gen_key, ed.boost, apply_tint, tint_rgb, t.clone()));
        t
    };

    painter.image(tex.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
    let border_col = if ed.bg == Bg::Light { Color32::from_black_alpha(36) } else { Color32::from_white_alpha(30) };
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0, border_col), egui::StrokeKind::Outside);

    // Optional osu! guides: center crosshair + exact 236 px (@2x = 118 px radius) official osu! hitcircle boundary ring
    if ed.creator.guides {
        let gc = if ed.bg == Bg::Light { Color32::from_rgba_unmultiplied(0x59, 0x33, 0xcc, 90) } else { Color32::from_rgba_unmultiplied(0x8c, 0x66, 0xff, 90) };
        let c = rect.center();
        painter.line_segment([Pos2::new(rect.left(), c.y), Pos2::new(rect.right(), c.y)], Stroke::new(1.0, gc));
        painter.line_segment([Pos2::new(c.x, rect.top()), Pos2::new(c.x, rect.bottom())], Stroke::new(1.0, gc));
        if matches!(ed.creator.doc.kind, engine::create::ElementKind::HitCircle | engine::create::ElementKind::HitCircleOverlay | engine::create::ElementKind::ApproachCircle | engine::create::ElementKind::SliderBall) {
            painter.circle_stroke(c, 118.0 * z, Stroke::new(1.0, gc));
        }
    }

    // --- bottom status strip
    ui.painter().rect_filled(status_bar, 0.0, BACKGROUND5);
    ui.painter().line_segment([status_bar.left_top(), status_bar.right_top()], Stroke::new(1.0, Color32::from_white_alpha(12)));
    let stem = ed.creator.doc.name.trim();
    let stem = if stem.is_empty() { ed.creator.doc.kind.file_stem() } else { stem };
    let boost_tag = if ed.boost { "   Boost on" } else { "" };
    let info = format!("{stem}@2x.png   {} × {} px  (1x: {} × {})   {:.0}%{boost_tag}", img.w, img.h, (img.w / 2).max(1), (img.h / 2).max(1), z * ppp * 100.0);
    let info_font = FontId::new(13.0, FontFamily::Proportional);
    let shown = th::fit_text(ui.painter(), &info, &info_font, (status_bar.width() - 32.0).max(40.0));
    ui.painter().text(Pos2::new(status_bar.left() + 16.0, status_bar.center().y), Align2::LEFT_CENTER, shown, info_font, CONTENT2);
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
                for bg in [Bg::Grey, Bg::Dark, Bg::Light, Bg::Check] {
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
