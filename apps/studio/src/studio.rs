//! Editor state and everything that happens to it that is not drawing: settings edits, background jobs (frame
//! generation, loading maps, rasterizing skins, import, export) and their results.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

use engine::atlas::Atlas;
use engine::gen::Frame;
use engine::presets;
use engine::settings::{Settings, SEAMLESS};
use preview_window::session::{self, Prepared, SessionConfig};
use serde_json::{json, Value};

use crate::paths::{self, Config};
use crate::state::{self, HistoryEntry};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Animation,
    Timing,
    Look,
    Preview,
    Export,
}

pub const TABS: [(Tab, &str); 5] = [
    (Tab::Animation, "Animation"),
    (Tab::Timing, "Timing"),
    (Tab::Look, "Look"),
    (Tab::Preview, "Preview"),
    (Tab::Export, "Export"),
];

#[derive(Clone, Copy, Debug)]
pub enum PickKind {
    SkinEditSkin,
    SkinEditSkinFolder,
    SkinEditPicture,
    SkinEditSaveAs,
    SkinEditExportOsk,
    OpenSettings,
    SaveSettings,
    CustomImage,
    Skin,
    SkinFolder,
    SoundSkin,
    SoundSkinFolder,
    Map,
    Songs,
    ExportDir,
    ImportDir,
}

pub enum Msg {
    SkinResult(u64, Arc<engine::img::Img>, Vec<(usize, usize)>, Arc<crate::skin_editor::Display>),
    /// A thumbnail of a picture of the skin: the list it belongs to, the file, its size and premultiplied RGBA.
    SkinThumb(u64, PathBuf, [usize; 2], Vec<u8>),
    SkinScanDone(engine::scan::ScanReport),
    SkinReloadFiles { msg: String, modified: Vec<PathBuf>, resolved_downscale: Option<PathBuf> },
    /// The hit sound of the picked skin, found and decoded in the background: (the skin and the sound skin it is for, where it is, the sound).
    LogoSound((String, String), Option<PathBuf>, Option<preview_window::sfx::Clip>),
    Frames { frames: Vec<Frame>, atlas: Arc<Atlas>, fps: f32 },
    Prepared(Result<Prepared, String>),
    Picked(PickKind, PathBuf),
    Status(String),
    ImportDone(Result<engine::analyze::Analysis, String>),
    HistoryChanged,
}

#[derive(Default)]
pub struct ImportState {
    pub folder: String,
    pub fps: String,
    pub result: Option<Value>,
    pub report: String,
    pub running: bool,
}

/// The two halves of the application: making followpoint animations (with the gameplay preview) and editing the pictures of a skin.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Followpoints,
    Skin,
}

pub struct Studio {
    pub mode: Mode,
    pub skin_ed: crate::skin_editor::SkinEditor,
    pub cfg: Config,
    pub settings: Value,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    /// The text in the settings search box.
    pub search: String,
    pub frames: Vec<Frame>,
    pub fps: f32,
    pub atlas: Option<Arc<Atlas>>,
    /// Bumped whenever `atlas` changes, so the running session can pick it up.
    pub atlas_gen: u64,
    pub skins: Vec<(String, String)>,
    pub tab: Tab,
    /// Text being edited, by field id; refreshed from the settings whenever a box is not focused.
    pub bufs: HashMap<String, String>,
    pub status: String,
    /// The status text the toast last showed, and when it appeared.
    pub status_shown: String,
    pub status_at: std::time::Instant,
    pub regen_pending: bool,
    regen_busy: bool,
    last_regen: Instant,
    pub map_cs_ar: Option<(f32, f32)>,
    pub history: Vec<HistoryEntry>,
    pub hist_confirm: Option<(String, Instant)>,
    pub import: ImportState,
    /// Where the animation card is in its loop (ms), how fast it plays, and the last time it moved.
    pub anim_pos: f32,
    /// The logo in the toolbar: its picture, when it was last pressed (it bounces), the sound player, and the hit sound of the picked skin
    /// (found once per skin and kept).
    pub logo_tex: Option<egui::TextureHandle>,
    /// The size in pixels the logo texture was made for.
    pub logo_px: usize,
    pub logo_pressed: Option<Instant>,
    pub sfx: preview_window::sfx::Sfx,
    logo_sound: Option<((String, String), Option<PathBuf>)>,
    /// The skins whose hit sound is being found in the background.
    logo_warm: Option<(String, String)>,
    skin_folder: std::cell::RefCell<Option<(String, Option<PathBuf>)>>,
    /// The motion editor's selection, zoom and undo history.
    pub motion: crate::motion::MotionState,
    pub anim_speed: f32,
    /// The animation preview: how many screen pixels each pixel of the animation gets (1 = real size). It is kept to what fits.
    pub anim_zoom: f32,
    /// The interface size in use now (a factor of the system's scaling, whether chosen or automatic), and the size a slider is being
    /// dragged to (only applied when it is let go: the slider itself grows and shrinks with the interface).
    pub ui_now: f32,
    pub ui_pending: Option<f32>,
    /// The most room the dock leaves the preview (width, height in points); the dock sets it every frame.
    pub anim_limit: (f32, f32),
    pub anim_last: Instant,
    pub anim_scrub: bool,
    pub anim_playing: bool,
    pub anim_tex: Option<egui::TextureHandle>,
    pub anim_idx: usize,
    pub anim_gen: u64,
    pub load_session: bool,
    /// Map time and play state to carry on from when the next session loads (set by the app).
    pub resume: Option<(f32, bool)>,
    /// Pick a new map when the session loads; a skin change keeps the one being played.
    pub reroll_map: bool,
    /// The .osu file the current session is playing.
    pub resolved_map: Option<PathBuf>,
    pub loading: bool,
    pub session_error: String,
    pub tx: Sender<Msg>,
    pub rx: Receiver<Msg>,
    pub ctx: egui::Context,
}

impl Studio {
    pub fn new(ctx: egui::Context) -> Studio {
        let (tx, rx) = channel();
        let cfg = Config::load();
        let skins = paths::detect_skins().into_iter().map(|(l, p)| (l, p.to_string_lossy().to_string())).collect();
        let _ = paths::ensure_skin(std::path::Path::new(&cfg.skin)); // a skin of osu!lazer's database, copied out the first time
        let cfg_dock = (cfg.dock_open, cfg.dock_height);
        let mut s = Studio {
            mode: if std::env::var("SKIN_STUDIO_TAB").is_ok_and(|v| v == "skin") { Mode::Skin } else { Mode::Followpoints },
            skin_ed: Default::default(),
            cfg,
            settings: presets::default_settings(),
            path: None,
            dirty: false,
            search: String::new(),
            frames: vec![],
            fps: 60.0,
            atlas: None,
            atlas_gen: 0,
            skins,
            // SKIN_STUDIO_TAB opens a page directly (for checking the UI without clicking through it)
            tab: match std::env::var("SKIN_STUDIO_TAB").unwrap_or_default().as_str() {
                "timing" => Tab::Timing,
                "look" => Tab::Look,
                "preview" => Tab::Preview,
                "export" => Tab::Export,
                _ => Tab::Animation,
            },
            bufs: HashMap::new(),
            status: String::new(),
            status_shown: String::new(),
            status_at: std::time::Instant::now(),
            regen_pending: true,
            regen_busy: false,
            last_regen: Instant::now() - std::time::Duration::from_secs(1),
            map_cs_ar: None,
            history: vec![],
            hist_confirm: None,
            import: ImportState::default(),
            anim_pos: 0.0,
            logo_tex: None,
            logo_px: 0,
            logo_pressed: None,
            sfx: preview_window::sfx::Sfx::new(),
            logo_sound: None,
            logo_warm: None,
            skin_folder: Default::default(),
            motion: crate::motion::MotionState::from_config(&cfg_dock),
            anim_speed: 1.0,
            anim_zoom: 1.0,
            ui_now: 1.0,
            ui_pending: None,
            anim_limit: (900.0, 900.0),
            anim_last: Instant::now(),
            anim_scrub: false,
            anim_playing: true,
            anim_tex: None,
            anim_idx: usize::MAX,
            anim_gen: 0,
            load_session: true,
            resume: None,
            reroll_map: true,
            resolved_map: None,
            loading: false,
            session_error: String::new(),
            tx,
            rx,
            ctx,
        };
        // a settings file on the command line, or the last one used, opens right away
        let first = std::env::args().nth(1).filter(|a| !a.starts_with("--")).or_else(|| {
            let c = s.cfg.settings.clone();
            (!c.is_empty() && Path::new(&c).is_file()).then_some(c)
        });
        if let Some(p) = first {
            match load_settings_file(Path::new(&p)) {
                Ok(v) => {
                    s.settings = v;
                    s.path = Some(PathBuf::from(&p));
                }
                Err(e) => s.status = e,
            }
        }
        s.reload_history();
        if let Ok(m) = std::env::var("STUDIO_MAP") {
            s.cfg.map = m;
        }
        if let Some(i) = std::env::var("STUDIO_TAB").ok().and_then(|v| v.parse::<usize>().ok()) {
            s.tab = TABS[i.min(TABS.len() - 1)].0;
        }
        // for looking at the skin editor without clicking through it: SKIN_STUDIO_EDIT is a skin folder, SKIN_STUDIO_EDIT_PIC part of a name
        if let Ok(dir) = std::env::var("SKIN_STUDIO_EDIT") {
            if s.skin_ed.open_skin(std::path::Path::new(&dir)).is_ok() {
                let (tx0, c0) = (s.tx.clone(), s.ctx.clone());
                s.skin_ed.load_thumbs(&tx0, &c0);
                let pic = std::env::var("SKIN_STUDIO_EDIT_PIC").unwrap_or_default().to_lowercase();
                if let Some(f) = s.skin_ed.files.iter().find(|f| !pic.is_empty() && f.name.to_lowercase().contains(&pic)).map(|f| f.path.clone()) {
                    let (tx, c) = (s.tx.clone(), s.ctx.clone());
                    let _ = s.skin_ed.open_picture(&f, Some((&tx, &c)));
                    if let Ok(v) = std::env::var("SKIN_STUDIO_EDIT_DEBAND") {
                        // "strength,grain"
                        let mut it = v.split(',').map(|x| x.trim().parse::<f32>().unwrap_or(0.0));
                        s.skin_ed.ops = vec![engine::edit::Op::deband(it.next().unwrap_or(0.8), it.next().unwrap_or(0.0))];
                        s.skin_ed.recompute(&tx, &c);
                    }
                    if std::env::var("SKIN_STUDIO_EDIT_OPS").is_ok() {
                        use engine::edit::{Op, Resample};
                        s.skin_ed.ops = vec![Op::Resize { w: 256, h: 256, mode: Resample::Crisp }, Op::Adjust { brightness: 0.0, contrast: 0.1, saturation: 0.2, hue: 20.0 }, Op::deband(0.6, 0.0)];
                        s.skin_ed.recompute(&tx, &c);
                    }
                }
            }
        }
        // SKIN_STUDIO_LANE=lane,key picks a lane of the motion editor and one of its keys (for checking it without clicking)
        if let Ok(v) = std::env::var("SKIN_STUDIO_LANE") {
            let mut it = v.split(',').map(|x| x.trim().parse::<usize>().unwrap_or(0));
            let (lane, key) = (it.next().unwrap_or(0), it.next());
            s.motion.prop = lane;
            s.motion.curves = std::env::var("SKIN_STUDIO_KEYFRAMES").is_err();
            if let Some(k) = key {
                s.motion.sel = vec![(lane, k)];
            }
        }
        s.warm_logo_sound();
        s
    }

    pub fn reload_history(&mut self) {
        self.history = paths::config_dir().map(|d| state::history_list(&d)).unwrap_or_default();
    }

    pub fn skin_label(&self) -> String {
        if self.cfg.skin == "argon" || self.cfg.skin.is_empty() {
            "Argon Pro".to_string()
        } else {
            self.skins.iter().find(|(_, p)| *p == self.cfg.skin).map(|(l, _)| l.clone()).unwrap_or_else(|| label(&self.cfg.skin))
        }
    }

    /// The hit sound of the skin that is picked: the skin's own `normal-hitnormal` when it has one, else the one of the skin the hit
    /// sounds fall back to (the same order the preview uses). `None` when neither has it.
    pub fn skin_hitsound(&mut self) -> Option<PathBuf> {
        let key = (self.cfg.skin.clone(), self.cfg.sound_skin.clone());
        if let Some((k, found)) = &self.logo_sound {
            if *k == key {
                return found.clone();
            }
        }
        let found = find_hitsound(&key.0, &key.1);
        self.logo_sound = Some((key, found.clone()));
        found
    }

    /// Finds and decodes the hit sound of the picked skin in the background, and opens the audio device, so that the first click on the logo
    /// does not have to (it is cheap to call every frame).
    pub fn warm_logo_sound(&mut self) {
        let same = |k: &(String, String)| k.0 == self.cfg.skin && k.1 == self.cfg.sound_skin;
        if self.logo_sound.as_ref().is_some_and(|(k, _)| same(k)) || self.logo_warm.as_ref().is_some_and(same) {
            return;
        }
        let key = (self.cfg.skin.clone(), self.cfg.sound_skin.clone());
        self.logo_warm = Some(key.clone());
        self.sfx.warm();
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let found = find_hitsound(&key.0, &key.1);
            let clip = found.as_deref().and_then(preview_window::sfx::decode_file);
            let _ = tx.send(Msg::LogoSound(key, found, clip));
            ctx.request_repaint();
        });
    }

    /// The folder of the skin picked for the followpoint preview, if it is one (not Argon).
    pub fn current_skin_folder(&self) -> Option<PathBuf> {
        if self.cfg.skin.is_empty() || self.cfg.skin == "argon" {
            return None;
        }
        // worked out once per picked skin: copying a skin out of lazer's database takes tens of milliseconds, and this is asked every frame
        if let Some((k, v)) = &*self.skin_folder.borrow() {
            if *k == self.cfg.skin {
                return v.clone();
            }
        }
        let p = PathBuf::from(&self.cfg.skin);
        let _ = paths::ensure_skin(&p); // a skin of osu!lazer's database is copied out first
        let found = p.is_dir().then_some(p);
        *self.skin_folder.borrow_mut() = Some((self.cfg.skin.clone(), found.clone()));
        found
    }

    /// The volume the session should have: nothing while muted.
    pub fn effective_volume(&self) -> f32 {
        if self.cfg.muted { 0.0 } else { self.cfg.volume }
    }

    fn session_config(&self, map: PathBuf) -> SessionConfig {
        SessionConfig {
            map,
            skin: PathBuf::from(if self.cfg.skin.is_empty() { "argon" } else { &self.cfg.skin }),
            sound_skin: (!self.cfg.sound_skin.is_empty()).then(|| PathBuf::from(&self.cfg.sound_skin)),
            follow: self.atlas.clone(),
            audio: true,
            volume: self.effective_volume(),
            offset_ms: self.cfg.offset_ms,
            cursor_scale: self.cfg.cursor_scale,
            hit_lighting: self.cfg.hit_lighting,
            snake_in: self.cfg.snake_in,
            snake_out: self.cfg.snake_out,
            speed: self.cfg.speed,
            override_cs: self.cfg.cs_override_on.then_some(self.cfg.cs_override),
            override_ar: self.cfg.ar_override_on.then_some(self.cfg.ar_override),
            hard_rock: self.cfg.mod_hr,
            hidden: self.cfg.mod_hd,
            resume: None,
        }
    }

    /// Start loading the map and skin on a worker thread; `ss` is the pixels per view unit it should be rasterized for.
    pub fn start_session_load(&mut self, ss: f32) {
        self.load_session = false;
        self.session_error.clear();
        self.loading = true;
        // a skin change keeps the map that is playing; only a request for another map picks one
        let keep = (!self.reroll_map).then(|| self.resolved_map.clone()).flatten();
        let keep_same = keep.is_some();
        self.reroll_map = false;
        let spec = keep.map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| self.cfg.map.clone());
        let songs = self.cfg.songs.clone();
        let library = self.cfg.library.clone();
        let mut template = self.session_config(PathBuf::new());
        // only when the same map is kept; a new map starts from its beginning
        template.resume = if keep_same { self.resume } else { None };
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let r = paths::ensure_skin(&template.skin)
                .and_then(|_| paths::resolve_map(&spec, if songs.is_empty() { None } else { Some(&songs) }, &library))
                .and_then(|map| {
                    let mut cfg = template;
                    cfg.map = map;
                    session::prepare(cfg, ss)
                });
            let _ = tx.send(Msg::Prepared(r));
            ctx.request_repaint();
        });
    }

    /// Settings changed: regenerate soon. Generation runs on a worker so typing never stalls.
    pub fn edited(&mut self) {
        self.dirty = true;
        self.regen_pending = true;
    }

    pub fn pump_regen(&mut self) {
        if !self.regen_pending || self.regen_busy {
            return;
        }
        // Throttle full 240 fps frame regeneration during an active motion-key drag so rayon doesn't starve the UI thread.
        if self.motion.is_dragging() && self.last_regen.elapsed() < std::time::Duration::from_millis(65) {
            return;
        }
        self.regen_pending = false;
        self.regen_busy = true;
        self.last_regen = Instant::now();
        let settings = self.settings.clone();
        let cs_ar = self.map_cs_ar;
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let parsed = Settings::from_json(&settings);
            let frames = engine::gen::build_frames(&parsed, 1, &Default::default());
            // the live views show the line as it would look on the map being played: CS and AR decide the spacing
            let atlas = match (parsed.is_seamless(), cs_ar) {
                (true, Some((cs, ar))) => {
                    let mut matched = settings.clone();
                    matched["sl_cs"] = json!(cs);
                    matched["sl_ar"] = json!(ar);
                    let f = engine::gen::build_frames(&Settings::from_json(&matched), 1, &Default::default());
                    Atlas::from_frames(&f, parsed.fps)
                }
                _ => Atlas::from_frames(&frames, parsed.fps),
            };
            let _ = tx.send(Msg::Frames { frames, atlas: Arc::new(atlas), fps: parsed.fps });
            ctx.request_repaint();
        });
    }

    /// Handle every message that arrived from workers and dialogs. Returns a prepared session when one is ready.
    pub fn drain(&mut self) -> Option<Prepared> {
        let mut prepared = None;
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Frames { frames, atlas, fps } => {
                    self.regen_busy = false;
                    self.frames = frames;
                    self.fps = fps;
                    self.atlas = Some(atlas);
                    self.atlas_gen += 1;
                }
                Msg::Prepared(Ok(p)) => {
                    self.resolved_map = Some(p.map_path().to_path_buf());
                    prepared = Some(p);
                }
                Msg::Prepared(Err(e)) => {
                    self.loading = false;
                    self.session_error = e;
                }
                Msg::Status(s) => self.status = s,
                Msg::SkinResult(gen, img, dims, display) => self.skin_ed.got_result(gen, img, dims, Some(display)),
                Msg::LogoSound(key, path, clip) => {
                    if let Some(p) = &path {
                        self.sfx.insert(p, clip);
                    }
                    self.logo_sound = Some((key, path));
                    self.logo_warm = None;
                }
                Msg::SkinThumb(gen, path, size, rgba) => self.skin_ed.got_thumb(&self.ctx, gen, path, size, rgba),
                Msg::SkinScanDone(report) => {
                    let msg = format!("Scanned {} files · {} flagged", report.total_files, report.flagged.len());
                    self.skin_ed.scan_running = false;
                    self.skin_ed.scan_report = Some(report);
                    self.status = msg;
                }
                Msg::SkinReloadFiles { msg, modified, resolved_downscale } => {
                    self.skin_ed.after_disk_mutation(&modified, resolved_downscale.as_deref(), &self.tx, &self.ctx);
                    if self.skin_ed.dir.is_some() && self.skin_ed.dir == self.current_skin_folder() {
                        self.load_session = true;
                        self.reroll_map = false;
                    }
                    self.status = msg;
                }
                Msg::HistoryChanged => self.reload_history(),
                Msg::ImportDone(r) => {
                    self.import.running = false;
                    match r {
                        Ok(a) => {
                            let mut report = a.notes.join("\n");
                            if let Some(m) = a.matched {
                                report.push_str(&format!("\n\nMatch: {:.0}%", m * 100.0));
                            }
                            self.import.report = report;
                            self.import.result = Some(a.settings);
                        }
                        Err(e) => {
                            self.import.report = e;
                            self.import.result = None;
                        }
                    }
                }
                Msg::Picked(kind, p) => self.picked(kind, p),
            }
        }
        prepared
    }

    fn picked(&mut self, kind: PickKind, p: PathBuf) {
        let ps = p.to_string_lossy().to_string();
        match kind {
            PickKind::SkinEditSkin | PickKind::SkinEditSkinFolder => match self.skin_ed.open_skin(&p) {
                Ok(()) => {
                    let (tx, c) = (self.tx.clone(), self.ctx.clone());
                    self.skin_ed.load_thumbs(&tx, &c);
                    self.status = format!("{} pictures", self.skin_ed.files.len());
                }
                Err(e) => self.status = e,
            },
            PickKind::SkinEditPicture => {
                let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
                if let Err(e) = self.skin_ed.open_picture(&p, Some((&tx, &ctx))) {
                    self.status = e;
                }
            }
            PickKind::SkinEditSaveAs => match self.skin_ed.save_as(&p) {
                Ok(()) => self.status = format!("Saved {}", p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
                Err(e) => self.status = format!("Could not save: {e}"),
            },
            PickKind::SkinEditExportOsk => match self.skin_ed.export_osk(&p) {
                Ok(n) => self.status = format!("Packed {n} files into {}", p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
                Err(e) => self.status = format!("Could not pack the skin: {e}"),
            },
            PickKind::OpenSettings => match load_settings_file(&p) {
                Ok(v) => {
                    // Switching documents: clear undo/redo so Ctrl+Z cannot restore a previous file's JSON under the new path.
                    self.motion.clear_history();
                    self.settings = v;
                    self.path = Some(p.clone());
                    self.cfg.settings = ps;
                    self.cfg.save();
                    self.bufs.clear();
                    self.regen_pending = true;
                    self.dirty = false;
                }
                Err(e) => self.status = e,
            },
            PickKind::SaveSettings => self.save_to(p),
            PickKind::CustomImage => {
                self.motion.remember(&self.settings);
                self.settings["image"] = json!(ps);
                // the canvas follows the picture: room to slide across and for blur or outline around it
                if let Ok(sp) = engine::sources::load_png(&p) {
                    self.settings["width"] = json!((sp.w * 3).max(100) as i64);
                    self.settings["height"] = json!((sp.h * 2).max(50) as i64);
                }
                self.bufs.clear();
                self.edited();
            }
            PickKind::Skin | PickKind::SkinFolder => {
                if !self.skins.iter().any(|(_, q)| *q == ps) {
                    self.skins.insert(0, (label(&ps), ps.clone()));
                }
                self.cfg.skin = ps;
                self.cfg.save();
                self.load_session = true;
            }
            PickKind::SoundSkin | PickKind::SoundSkinFolder => {
                self.cfg.sound_skin = ps;
                self.cfg.save();
                self.load_session = true;
            }
            PickKind::Map => {
                self.cfg.map = ps;
                self.cfg.save();
                self.reroll_map = true;
                self.load_session = true;
            }
            PickKind::Songs => {
                self.cfg.songs = ps;
                self.cfg.save();
                self.reroll_map = true;
                self.load_session = true;
            }
            PickKind::ImportDir => self.import.folder = ps,
            PickKind::ExportDir => {
                let settings = self.settings.clone();
                let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
                std::thread::spawn(move || {
                    let msg = match export(&settings, &p) {
                        Ok(n) => {
                            if let Some(d) = paths::config_dir() {
                                state::history_add(&d, &settings, None);
                            }
                            let _ = tx.send(Msg::HistoryChanged);
                            format!("Exported {n} images to {}. Set AnimationFramerate: {} in skin.ini.", p.display(), state::fmt_num(&settings["fps"]))
                        }
                        Err(e) => e,
                    };
                    let _ = tx.send(Msg::Status(msg));
                    ctx.request_repaint();
                });
            }
        }
    }

    pub fn pick(&self, kind: PickKind) {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        let songs = (!self.cfg.songs.is_empty()).then(|| PathBuf::from(&self.cfg.songs));
        std::thread::spawn(move || {
            let r = match kind {
                PickKind::SkinEditSkin => rfd::FileDialog::new().set_title("Open a skin (.osk or .zip)").add_filter("skin", &["osk", "zip"]).pick_file(),
                PickKind::SkinEditSkinFolder => rfd::FileDialog::new().set_title("Open a skin folder").pick_folder(),
                PickKind::SkinEditPicture => rfd::FileDialog::new().set_title("Open a picture").add_filter("PNG image", &["png"]).pick_file(),
                PickKind::SkinEditSaveAs => rfd::FileDialog::new().set_title("Save the picture as").add_filter("PNG image", &["png"]).save_file(),
                PickKind::SkinEditExportOsk => rfd::FileDialog::new().set_title("Pack the skin into an .osk").set_file_name("skin.osk").add_filter("skin", &["osk"]).save_file(),
                PickKind::OpenSettings => rfd::FileDialog::new().set_title("Open settings").add_filter("settings", &["json"]).pick_file(),
                PickKind::SaveSettings => rfd::FileDialog::new().set_title("Save settings").set_file_name("followpoint-settings.json").add_filter("settings", &["json"]).save_file(),
                PickKind::CustomImage => rfd::FileDialog::new().set_title("Choose the image").add_filter("PNG image", &["png"]).pick_file(),
                PickKind::Skin => rfd::FileDialog::new().set_title("Choose a skin (.osk or .zip)").add_filter("skin", &["osk", "zip"]).pick_file(),
                PickKind::SkinFolder => rfd::FileDialog::new().set_title("Choose a skin folder").pick_folder(),
                PickKind::SoundSkin => rfd::FileDialog::new().set_title("Skin to take hit sounds from").add_filter("skin", &["osk", "zip"]).pick_file(),
                PickKind::SoundSkinFolder => rfd::FileDialog::new().set_title("Skin folder to take hit sounds from").pick_folder(),
                PickKind::Map => {
                    let mut d = rfd::FileDialog::new().set_title("Choose a beatmap").add_filter("osu! beatmap or archive", &["osu", "osz", "olz"]);
                    if let Some(p) = songs {
                        d = d.set_directory(p);
                    }
                    d.pick_file()
                }
                PickKind::Songs => rfd::FileDialog::new().set_title("Choose your osu! Songs folder").pick_folder(),
                PickKind::ExportDir => rfd::FileDialog::new().set_title("Export followpoint images to").pick_folder(),
                PickKind::ImportDir => rfd::FileDialog::new().set_title("Folder with followpoint-N.png files").pick_folder(),
            };
            if let Some(p) = r {
                let _ = tx.send(Msg::Picked(kind, p));
                ctx.request_repaint();
            }
        });
    }

    pub fn save(&mut self) {
        match self.path.clone() {
            Some(p) => self.save_to(p),
            None => self.pick(PickKind::SaveSettings),
        }
    }

    fn save_to(&mut self, p: PathBuf) {
        match save_settings_file(&p, &self.settings) {
            Ok(()) => {
                self.path = Some(p.clone());
                self.dirty = false;
                self.cfg.settings = p.to_string_lossy().to_string();
                self.cfg.save();
                if let Some(d) = paths::config_dir() {
                    state::history_add(&d, &self.settings, None);
                }
                self.reload_history();
                self.status = format!("Saved {}", p.display());
            }
            Err(e) => self.status = e,
        }
    }

    /// Play a different, random map.
    pub fn new_map(&mut self) {
        self.cfg.map = "random".into();
        self.cfg.save();
        self.reroll_map = true;
        self.load_session = true;
    }

    pub fn new_settings(&mut self) {
        self.motion.remember(&self.settings);
        self.settings = presets::default_settings();
        self.path = None;
        self.bufs.clear();
        self.edited();
        self.status = "Reset to new settings (Ctrl+Z to undo)".into();
    }

    pub fn export(&mut self) {
        self.pick(PickKind::ExportDir);
    }

    pub fn apply_effect(&mut self, name: &str) {
        self.motion.remember(&self.settings);
        presets::apply_effect(&mut self.settings, name);
        self.bufs.clear();
        self.edited();
    }

    pub fn set_source(&mut self, name: &str) {
        self.motion.remember(&self.settings);
        state::set_source(&mut self.settings, name);
        self.bufs.clear();
        self.edited();
    }

    pub fn is_seamless(&self) -> bool {
        self.settings["preset"].as_str() == Some(SEAMLESS)
    }

    pub fn run_import(&mut self) {
        let folder = self.import.folder.clone();
        let fps = self.import.fps.trim().parse::<f32>().ok();
        self.import.running = true;
        self.import.report = "Analyzing...".into();
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let sprite = paths::config_dir().unwrap_or_else(std::env::temp_dir).join("imported-sprite.png");
            if let Some(d) = sprite.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let r = engine::analyze::analyze(Path::new(&folder), fps, &sprite);
            let _ = tx.send(Msg::ImportDone(r));
            ctx.request_repaint();
        });
    }

    pub fn use_import(&mut self) {
        if let Some(v) = self.import.result.clone() {
            self.motion.remember(&self.settings);
            self.settings = state::migrate(v);
            self.path = None;
            self.bufs.clear();
            self.edited();
            self.tab = Tab::Animation;
        }
    }

    pub fn load_history(&mut self, id: &str) {
        if let Some(h) = self.history.iter().find(|h| h.id == id).cloned() {
            self.motion.remember(&self.settings);
            self.settings = state::migrate(h.settings);
            self.path = None;
            self.bufs.clear();
            self.edited();
            self.tab = Tab::Animation;
        }
    }

    pub fn delete_history(&mut self, id: &str) {
        if let Some(d) = paths::config_dir() {
            state::history_delete(&d, id);
        }
        self.reload_history();
    }

    pub fn add_history(&mut self) {
        if let Some(d) = paths::config_dir() {
            state::history_add(&d, &self.settings, None);
        }
        self.reload_history();
    }

    /// Set one top-level setting from edited text (and fix up the buffer when it was out of range).
    pub fn set_field(&mut self, id: &str, text: &str) {
        let before = self.settings.clone();
        if state::set_field(&mut self.settings, id, text) {
            self.motion.remember_coalesced(&before);
            if id == "fps" && self.settings["fps"].as_f64().unwrap_or(0.0) > engine::settings::MAX_FPS as f64 {
                self.settings["fps"] = json!(engine::settings::MAX_FPS as f64);
                self.bufs.remove(id);
            }
            self.edited();
        }
    }
}

pub fn label(p: &str) -> String {
    Path::new(p).file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string())
}

fn write_png(path: &Path, w: usize, h: usize, rgba: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().map_err(|e| e.to_string())?.write_image_data(rgba).map_err(|e| e.to_string())
}

pub fn export(settings: &Value, dir: &Path) -> Result<usize, String> {
    let parsed = Settings::from_json(settings);
    let sizes: &[(usize, &str)] = match settings["export"].as_str().unwrap_or("1x") {
        "2x" => &[(2, "@2x")],
        "both" => &[(1, ""), (2, "@2x")],
        _ => &[(1, "")],
    };
    let mut n = 0;
    for (scale, suffix) in sizes {
        let frames = engine::gen::build_frames(&parsed, *scale, &Default::default());
        for (i, f) in frames.iter().enumerate() {
            let path = dir.join(format!("followpoint-{i}{suffix}.png"));
            match f {
                Some(img) => write_png(&path, img.w, img.h, &img.to_straight_u8())?,
                None => write_png(&path, 1, 1, &[0, 0, 0, 0])?,
            }
            n += 1;
        }
    }
    Ok(n)
}

pub fn load_settings_file(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(state::migrate(v))
}

pub fn save_settings_file(path: &Path, v: &Value) -> Result<(), String> {
    std::fs::write(path, serde_json::to_string_pretty(v).map_err(|e| e.to_string())?).map_err(|e| format!("{}: {e}", path.display()))
}

/// The hit sound of a skin: its own `normal-hitnormal` when it has one, else the one of the skin the hit sounds fall back to.
fn find_hitsound(skin: &str, sound_skin: &str) -> Option<PathBuf> {
    let skin_path = PathBuf::from(if skin.is_empty() { "argon" } else { skin });
    let _ = paths::ensure_skin(&skin_path); // a skin of osu!lazer's database is copied out first
    let mut skins: Vec<game::skin::Skin> = vec![];
    if let Ok(s) = game::skin::Skin::open(&skin_path) {
        if s.style == game::skin::Style::Legacy {
            skins.push(s);
        }
    }
    if !sound_skin.is_empty() {
        if let Ok(s) = game::skin::Skin::open(std::path::Path::new(sound_skin)) {
            skins.push(s);
        }
    }
    skins.iter().find_map(|s| s.find_sound("normal-hitnormal"))
}
