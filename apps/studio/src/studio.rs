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
    Motion,
    Look,
    Preview,
    Export,
}

pub const TABS: [(Tab, &str); 6] = [
    (Tab::Animation, "Animation"),
    (Tab::Timing, "Timing"),
    (Tab::Motion, "Motion"),
    (Tab::Look, "Look"),
    (Tab::Preview, "Preview"),
    (Tab::Export, "Export"),
];

#[derive(Clone, Copy, Debug)]
pub enum PickKind {
    OpenSettings,
    SaveSettings,
    CustomImage,
    Skin,
    SoundSkin,
    Map,
    Songs,
    ExportDir,
    ImportDir,
}

pub enum Msg {
    Frames { frames: Vec<Frame>, atlas: Arc<Atlas>, fps: f32 },
    Prepared(Result<Prepared, String>),
    Sheet(game::skin::Sheet, f32),
    Picked(PickKind, PathBuf),
    Status(String),
    ImportDone(Result<engine::analyze::Analysis, String>),
    HistoryChanged,
    /// A map for the lazer preview (resolved, with its audio next to it).
    LazerMap(Result<PathBuf, String>),
    /// A skin built for the lazer preview.
    LazerSkin(Result<PathBuf, String>),
}

#[derive(Default)]
pub struct ImportState {
    pub folder: String,
    pub fps: String,
    pub result: Option<Value>,
    pub report: String,
    pub running: bool,
}

pub struct Studio {
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
    pub map_cs_ar: Option<(f32, f32)>,
    pub history: Vec<HistoryEntry>,
    pub import: ImportState,
    /// Where the animation card is in its loop (ms), how fast it plays, and the last time it moved.
    pub anim_pos: f32,
    pub anim_speed: f32,
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
    /// The osu!lazer preview process, when it is the one drawing the preview.
    pub lazer: Option<crate::lazer::Link>,
    pub lazer_skin_dirty: bool,
    lazer_skin_building: bool,
    lazer_skin_sent: u32,
    lazer_resume: Option<f32>,
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
        let mut s = Studio {
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
            tab: Tab::Animation,
            bufs: HashMap::new(),
            status: String::new(),
            status_shown: String::new(),
            status_at: std::time::Instant::now(),
            regen_pending: true,
            regen_busy: false,
            map_cs_ar: None,
            history: vec![],
            import: ImportState::default(),
            anim_pos: 0.0,
            anim_speed: 1.0,
            anim_last: Instant::now(),
            anim_scrub: false,
            anim_playing: true,
            anim_tex: None,
            anim_idx: usize::MAX,
            anim_gen: 0,
            load_session: true,
            lazer: None,
            lazer_skin_dirty: false,
            lazer_skin_building: false,
            lazer_skin_sent: 0,
            lazer_resume: None,
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
            resume: None,
        }
    }

    /// Whether the osu!lazer preview is the one drawing the preview.
    pub fn lazer_active(&self) -> bool {
        self.lazer.is_some()
    }

    /// Hand the preview to osu!lazer's own renderer (a separate window), or take it back.
    pub fn set_renderer(&mut self, lazer: bool) {
        if lazer == self.lazer.is_some() {
            return;
        }
        if lazer {
            let Some(exe) = crate::lazer::find_host() else {
                self.status = "The lazer preview component was not found. See the README for where to put it.".into();
                self.cfg.renderer = "builtin".into();
                return;
            };
            match crate::lazer::Link::spawn(&exe) {
                Ok(mut l) => {
                    l.send(serde_json::json!({ "cmd": "volume", "value": self.effective_volume() }));
                    l.send(serde_json::json!({ "cmd": "speed", "value": self.cfg.speed }));
                    l.send(serde_json::json!({ "cmd": "config", "hitLighting": self.cfg.hit_lighting, "snakingIn": self.cfg.snake_in, "snakingOut": self.cfg.snake_out, "cursorSize": self.cfg.cursor_scale }));
                    self.lazer = Some(l);
                    self.cfg.renderer = "lazer".into();
                    self.reroll_map = false;
                    self.lazer_request_map();
                }
                Err(e) => {
                    self.status = format!("The lazer preview could not be started: {e}");
                    self.cfg.renderer = "builtin".into();
                }
            }
        } else {
            self.lazer = None; // closes it
            self.cfg.renderer = "builtin".into();
            self.load_session = true;
        }
        self.cfg.save();
    }

    /// Resolve the map for the lazer preview (a new one when asked for, else the one already playing) and send it.
    pub fn lazer_request_map(&mut self) {
        self.load_session = false;
        self.session_error.clear();
        self.lazer_skin_dirty = true;
        let keep = (!self.reroll_map).then(|| self.resolved_map.clone()).flatten();
        if keep.is_some() && self.lazer.as_ref().is_some_and(|l| l.time.is_some()) {
            // a skin change: the map carries on
            return;
        }
        self.reroll_map = false;
        self.loading = true;
        let spec = keep.map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| self.cfg.map.clone());
        let songs = self.cfg.songs.clone();
        let library = self.cfg.library.clone();
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let r = paths::resolve_map(&spec, if songs.is_empty() { None } else { Some(&songs) }, &library);
            let _ = tx.send(Msg::LazerMap(r));
            ctx.request_repaint();
        });
    }

    /// Called every frame while the lazer preview runs: take in what it says, and send it the animation when that changed.
    pub fn lazer_tick(&mut self) {
        let Some(l) = &mut self.lazer else { return };
        if !l.alive() {
            // its window was closed: back to the built-in preview
            self.lazer = None;
            self.cfg.renderer = "builtin".into();
            self.cfg.save();
            self.load_session = true;
            return;
        }
        l.poll();
        if let Some(e) = l.error.take() {
            self.status = format!("lazer preview: {e}");
        }
        let acked = l.skin_ack >= self.lazer_skin_sent;
        if self.lazer_skin_dirty && !self.lazer_skin_building && acked && l.ready && !self.frames.is_empty() {
            self.lazer_skin_dirty = false;
            self.lazer_skin_building = true;
            let frames = self.frames.clone();
            let fps = self.fps;
            let base = self.cfg.skin.clone();
            let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
            std::thread::spawn(move || {
                let out = std::env::temp_dir().join("skinstudio").join(format!("lazer-skin-{}.osk", std::process::id() as u64 * 1_000_000 + std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_micros() as u64).unwrap_or(0)));
                let _ = std::fs::create_dir_all(out.parent().unwrap());
                let r = if base.is_empty() || base == "argon" {
                    Err("the lazer preview needs a legacy skin to show a custom animation (Argon Pro draws its own follow points)".to_string())
                } else {
                    crate::lazer::build_skin(std::path::Path::new(&base), &frames, fps, &out).map(|_| out)
                };
                let _ = tx.send(Msg::LazerSkin(r));
                ctx.request_repaint();
            });
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
            let r = paths::resolve_map(&spec, if songs.is_empty() { None } else { Some(&songs) }, &library).and_then(|map| {
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
        self.regen_pending = false;
        self.regen_busy = true;
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
                    self.lazer_skin_dirty = true;
                }
                Msg::Prepared(Ok(p)) => {
                    self.resolved_map = Some(p.map_path().to_path_buf());
                    prepared = Some(p);
                }
                Msg::Prepared(Err(e)) => {
                    self.loading = false;
                    self.session_error = e;
                }
                Msg::LazerMap(Ok(p)) => {
                    self.loading = false;
                    self.resolved_map = Some(p.clone());
                    let resume = self.lazer_resume.take();
                    if let Some(l) = &mut self.lazer {
                        l.map(&p, resume);
                    }
                    self.lazer_skin_dirty = true;
                }
                Msg::LazerMap(Err(e)) => {
                    self.loading = false;
                    self.session_error = e;
                }
                Msg::LazerSkin(Ok(p)) => {
                    self.lazer_skin_building = false;
                    if let Some(l) = &mut self.lazer {
                        self.lazer_skin_sent = l.skin(&p);
                    }
                }
                Msg::LazerSkin(Err(e)) => {
                    self.lazer_skin_building = false;
                    self.status = format!("The skin for the lazer preview could not be built: {e}");
                }
                Msg::Sheet(..) => {}
                Msg::Status(s) => self.status = s,
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
            PickKind::OpenSettings => match load_settings_file(&p) {
                Ok(v) => {
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
                self.settings["image"] = json!(ps);
                // the canvas follows the picture: room to slide across and for blur or outline around it
                if let Ok(sp) = engine::sources::load_png(&p) {
                    self.settings["width"] = json!((sp.w * 3).max(100) as i64);
                    self.settings["height"] = json!((sp.h * 2).max(50) as i64);
                }
                self.bufs.clear();
                self.edited();
            }
            PickKind::Skin => {
                if !self.skins.iter().any(|(_, q)| *q == ps) {
                    self.skins.insert(0, (label(&ps), ps.clone()));
                }
                self.cfg.skin = ps;
                self.cfg.save();
                self.load_session = true;
            }
            PickKind::SoundSkin => {
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
                PickKind::OpenSettings => rfd::FileDialog::new().set_title("Open settings").add_filter("settings", &["json"]).pick_file(),
                PickKind::SaveSettings => rfd::FileDialog::new().set_title("Save settings").set_file_name("followpoint-settings.json").add_filter("settings", &["json"]).save_file(),
                PickKind::CustomImage => rfd::FileDialog::new().set_title("Choose the image").add_filter("PNG image", &["png"]).pick_file(),
                PickKind::Skin => rfd::FileDialog::new().set_title("Choose a skin (.osk or .zip)").add_filter("skin", &["osk", "zip"]).pick_file().or_else(|| rfd::FileDialog::new().set_title("...or choose a skin folder").pick_folder()),
                PickKind::SoundSkin => rfd::FileDialog::new().set_title("Skin to take hit sounds from").add_filter("skin", &["osk", "zip"]).pick_file().or_else(|| rfd::FileDialog::new().set_title("...or a skin folder").pick_folder()),
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
        self.settings = presets::default_settings();
        self.path = None;
        self.bufs.clear();
        self.edited();
    }

    pub fn export(&mut self) {
        self.pick(PickKind::ExportDir);
    }

    pub fn apply_effect(&mut self, name: &str) {
        presets::apply_effect(&mut self.settings, name);
        self.bufs.clear();
        self.edited();
    }

    pub fn set_source(&mut self, name: &str) {
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
            self.settings = state::migrate(v);
            self.path = None;
            self.bufs.clear();
            self.edited();
            self.tab = Tab::Animation;
        }
    }

    pub fn load_history(&mut self, id: &str) {
        if let Some(h) = self.history.iter().find(|h| h.id == id).cloned() {
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
        if state::set_field(&mut self.settings, id, text) {
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
