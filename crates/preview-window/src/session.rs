//! A playing map: the scene, its skin sheet, the music and hit sounds, and the clock. It owns no window, so the same
//! code drives the full-speed preview window and the viewer inside the editor.
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use engine::atlas::Atlas;
use game::beatmap::Beatmap;
use game::scene::{FollowFrames, Frame, Scene};
use game::skin::{self, Rect, Sheet, Skin};
use preview::game_renderer::GameRenderer;

/// A title for the window while the map is still loading.
pub fn peek_title(p: &std::path::Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "preview".into())
}

/// The playfield plus margins, in osu! pixels.
pub const VIEW: (f32, f32) = (640.0, 480.0);

pub struct Sample {
    channels: rodio::ChannelCount,
    rate: rodio::SampleRate,
    data: Vec<f32>,
}

/// Counts the samples the output has pulled from the music, which is its position in the song whatever the speed (the
/// player's own `get_pos` is output time and cannot be trusted across speed changes and seeks).
struct CountSrc<S> {
    inner: S,
    n: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl<S: rodio::Source<Item = f32>> Iterator for CountSrc<S> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        self.n.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.inner.next()
    }
}

impl<S: rodio::Source<Item = f32>> rodio::Source for CountSrc<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }
    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        self.inner.try_seek(pos)?;
        let per_sec = self.inner.sample_rate().get() as f64 * self.inner.channels().get() as f64;
        self.n.store((pos.as_secs_f64() * per_sec) as u64, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
}

struct Audio {
    /// The player's speed. A seek is requested in output time (the player multiplies it by the speed), the song position is not.
    speed: f32,
    /// Eases the sound in after a map starts (and a seek or speed change), so there is no click.
    born: std::time::Instant,
    gain: f32,
    /// After a seek the music position is not trusted for a moment.
    ignore_until: std::time::Instant,
    count: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// Where the music comes from, to start it again when it has played to its end.
    path: std::path::PathBuf,
    /// Samples per second of the music (rate x channels).
    per_sec: f64,
    sink: rodio::MixerDeviceSink,
    player: rodio::Player,
    samples: std::collections::HashMap<String, Sample>,
    volume: f32,
    events: Vec<game::hitsound::HitEvent>,
    next_event: usize,
    loops: Vec<game::hitsound::LoopEvent>,
    /// The slider or spinner loop playing right now: its index and the players of its samples.
    active: Option<(usize, Vec<rodio::Player>)>,
}

fn decode(path: &std::path::Path) -> Option<Sample> {
    use rodio::Source;
    let file = std::fs::File::open(path).ok()?;
    let dec = rodio::Decoder::try_from(file).ok()?;
    let (channels, rate) = (dec.channels(), dec.sample_rate());
    Some(Sample { channels, rate, data: dec.collect() })
}

impl Drop for Audio {
    /// A short fade-out, so replacing a map while it plays does not click.
    fn drop(&mut self) {
        self.stop_loops();
        for i in 1..=8 {
            self.player.set_volume(self.volume * self.gain * (1.0 - i as f32 / 8.0));
            std::thread::sleep(Duration::from_millis(6));
        }
    }
}

impl Audio {
    /// Where the music is, in ms of the song.
    fn pos_ms(&self) -> f32 {
        (self.count.load(std::sync::atomic::Ordering::Relaxed) as f64 / self.per_sec * 1000.0) as f32
    }

    fn sample(&self, names: &[String]) -> Option<&Sample> {
        names.iter().find_map(|n| self.samples.get(n))
    }

    /// Play every hit sound due between the previous and the current map time.
    fn fire(&mut self, from: f32, to: f32) {
        use rodio::Source;
        while self.next_event < self.events.len() && self.events[self.next_event].time <= to {
            let e = self.events[self.next_event].clone();
            self.next_event += 1;
            if e.time <= from {
                continue;
            }
            for layer in game::hitsound::sample_names(&e) {
                if let Some(s) = self.sample(&layer) {
                    let src = rodio::buffer::SamplesBuffer::new(s.channels, s.rate, s.data.clone());
                    self.sink.mixer().add(src.amplify(0.8 * self.volume * self.gain * e.volume.clamp(0.05, 1.0)));
                }
            }
        }
    }

    fn stop_loops(&mut self) {
        if let Some((_, players)) = self.active.take() {
            for p in players {
                p.stop();
            }
        }
    }

    /// Keep the slider-slide / spinner-spin loop in step with the map time.
    fn update_loops(&mut self, t: f32, playing: bool) {
        use rodio::Source;
        let idx = (0..self.loops.len()).rev().find(|&i| self.loops[i].start <= t && t < self.loops[i].end);
        if idx != self.active.as_ref().map(|a| a.0) {
            self.stop_loops();
            if let Some(i) = idx {
                let l = self.loops[i].clone();
                let mut players = Vec::new();
                for name in &l.names {
                    let plain = name.trim_end_matches(|c: char| c.is_ascii_digit()).to_string();
                    let Some(s) = self.sample(&[name.clone(), plain]) else { continue };
                    let player = rodio::Player::connect_new(self.sink.mixer());
                    player.append(rodio::buffer::SamplesBuffer::new(s.channels, s.rate, s.data.clone()).repeat_infinite());
                    players.push(player);
                }
                self.active = Some((i, players));
            }
        }
        if let Some((i, players)) = &self.active {
            let l = &self.loops[*i];
            let speed = match &l.spinner {
                // lazer: frequency 0.5 + progress * 2, at most 2.5
                Some(sp) => (0.5 + sp.progress_unclamped(t) * 2.0).min(2.5),
                None => 1.0,
            };
            for p in players {
                p.set_volume(0.8 * self.volume * self.gain * l.volume.clamp(0.05, 1.0));
                p.set_speed(speed);
                if playing {
                    p.play();
                } else {
                    p.pause();
                }
            }
        }
    }

    fn set_volume(&mut self, v: f32) {
        self.volume = v.clamp(0.0, 1.0);
        self.player.set_volume(self.volume * self.gain);
    }

    /// Fade in over a third of a second.
    fn update_gain(&mut self) {
        let g = (self.born.elapsed().as_secs_f32() / 0.35).min(1.0);
        let g = g * g;
        if (g - self.gain).abs() > 1e-4 {
            self.gain = g;
            self.player.set_volume(self.volume * g);
        }
    }

    /// Put the music at `ms` right away; its reported position is ignored until the player has really moved.
    fn seek_music(&mut self, ms: f32) {
        let ms = ms.max(0.0);
        // a song that has played to its end has nothing left in the player to seek in: queue it again
        if self.player.empty() {
            if let Some(dec) = std::fs::File::open(&self.path).ok().and_then(|f| rodio::Decoder::try_from(f).ok()) {
                self.player.append(CountSrc { inner: dec, n: self.count.clone() });
            }
        }
        self.count.store((ms as f64 / 1000.0 * self.per_sec) as u64, std::sync::atomic::Ordering::Relaxed);
        self.ignore_until = std::time::Instant::now() + Duration::from_millis(250);
        let _ = self.player.try_seek(Duration::from_secs_f32(ms / 1000.0 / self.speed.max(0.05)));
    }

    fn seek_events(&mut self, t: f32) {
        self.next_event = self.events.partition_point(|e| e.time <= t);
        self.stop_loops();
    }
}

/// Decode every sample the map can use. The beatmap folder wins over the skin, as in the game.
pub fn load_samples(map: &Beatmap, skins: &[&Skin], events: &[game::hitsound::HitEvent], loops: &[game::hitsound::LoopEvent]) -> std::collections::HashMap<String, Sample> {
    let mut in_map: std::collections::HashMap<String, PathBuf> = Default::default();
    if let Ok(rd) = std::fs::read_dir(&map.dir) {
        for e in rd.flatten() {
            let p = e.path();
            let ext = p.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
            if matches!(ext.as_str(), "wav" | "ogg" | "mp3") {
                if let Some(stem) = p.file_stem() {
                    in_map.insert(stem.to_string_lossy().to_lowercase(), p.clone());
                }
            }
        }
    }
    let mut out = std::collections::HashMap::new();
    for name in game::hitsound::all_names(events, loops) {
        let path = in_map.get(&name.to_lowercase()).cloned().or_else(|| skins.iter().find_map(|s| s.find_sound(&name)));
        if let Some(s) = path.and_then(|p| decode(&p)) {
            out.insert(name, s);
        }
    }
    out
}

fn open_audio(map: &Beatmap, samples: std::collections::HashMap<String, Sample>, events: Vec<game::hitsound::HitEvent>, loops: Vec<game::hitsound::LoopEvent>, start_ms: f32, volume: f32) -> Option<Audio> {
    let path = map.audio_path();
    let sink = match rodio::DeviceSinkBuilder::open_default_sink() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("audio: no output device ({e})");
            return None;
        }
    };
    let file = std::fs::File::open(&path).map_err(|e| eprintln!("audio: {}: {e}", path.display())).ok()?;
    let dec = rodio::Decoder::try_from(file).map_err(|e| eprintln!("audio: cannot decode {}: {e}", path.display())).ok()?;
    let player = rodio::Player::connect_new(sink.mixer());
    // silent while it seeks, so any burst of the song's first samples is not heard; the gain then fades in
    player.set_volume(0.0);
    let count = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    use rodio::Source as _;
    let per_sec = dec.sample_rate().get() as f64 * dec.channels().get() as f64;
    player.append(CountSrc { inner: dec, n: count.clone() });
    if start_ms > 0.0 {
        if let Err(e) = player.try_seek(Duration::from_secs_f32(start_ms / 1000.0)) {
            eprintln!("audio: seek failed ({e:?})");
        }
    }
    let mut a = Audio { path: path.clone(), speed: 1.0, born: std::time::Instant::now(), gain: 0.0, ignore_until: std::time::Instant::now(), count, per_sec, sink, player, samples, volume, events, next_event: 0, loops, active: None };
    a.set_volume(volume);
    a.seek_events(start_ms);
    a.player.play();
    Some(a)
}

fn base_sheet(skin: &Skin, map: &Beatmap, ss: f32) -> Sheet {
    skin::base_sheet(skin, map.cs, ss)
}

/// The skin sheet with the follow-point animation appended below it.
fn with_follow(base: &Sheet, skin: &Skin, follow: Option<&Atlas>) -> (Sheet, Option<FollowFrames>) {
    let mut sheet = base.clone();
    let mut ff = None;
    if let Some(atlas) = follow {
        let y0 = sheet.h;
        let w = sheet.w.max(atlas.width);
        let mut rgba = vec![0u8; (w * (y0 + atlas.height) * 4) as usize];
        for row in 0..sheet.h {
            let n = (sheet.w * 4) as usize;
            rgba[(row * w * 4) as usize..][..n].copy_from_slice(&sheet.rgba[(row * sheet.w * 4) as usize..][..n]);
        }
        for row in 0..atlas.height {
            let n = (atlas.width * 4) as usize;
            rgba[((y0 + row) * w * 4) as usize..][..n].copy_from_slice(&atlas.rgba[(row * atlas.width * 4) as usize..][..n]);
        }
        let rects = atlas.frames.iter().map(|f| f.origin.map(|(ox, oy)| Rect { x: ox, y: y0 + oy, w: f.w, h: f.h })).collect();
        ff = Some(FollowFrames { rects, fps: atlas.fps, ratio: 1.0, ini_set: true, additive: false });
        sheet.rgba = rgba;
        sheet.w = w;
        sheet.h += atlas.height;
    }
    if ff.is_none() && matches!(skin.style, skin::Style::Argon { .. }) {
        // no animation to preview: show Argon's own follow points
        if let Some(e) = sheet.get("ar_followpoint") {
            ff = Some(FollowFrames { rects: vec![Some(e.frames[0])], fps: 60.0, ratio: e.ratio, ini_set: false, additive: true });
        }
    }
    (sheet, ff)
}


pub struct SessionConfig {
    pub map: PathBuf,
    pub skin: PathBuf,
    /// A legacy skin whose sounds are used when the visual skin has none (Argon).
    pub sound_skin: Option<PathBuf>,
    pub follow: Option<Arc<Atlas>>,
    pub audio: bool,
    pub volume: f32,
    /// Moves the music against the map, in ms (positive = the music comes later). Hit sounds are not affected.
    pub offset_ms: f32,
    pub cursor_scale: f32,
    pub hit_lighting: bool,
    pub snake_in: bool,
    pub snake_out: bool,
    /// Playback speed, 1.0 = as played.
    pub speed: f32,
    /// Optional preview overrides for Circle Size (0..10), Approach Rate (0..11), Hard Rock, and Hidden.
    pub override_cs: Option<f32>,
    pub override_ar: Option<f32>,
    pub hard_rock: bool,
    pub hidden: bool,
    /// Carry on from this map time (and play or pause state) instead of the start, as after a skin change.
    pub resume: Option<(f32, bool)>,
}

/// Everything about a session that needs no GPU or audio device, so it can be built on a worker thread.
pub struct Prepared {
    raw_map: Beatmap,
    map: Arc<Beatmap>,
    skin: Arc<Skin>,
    base: Sheet,
    ss: f32,
    samples: std::collections::HashMap<String, Sample>,
    events: Vec<game::hitsound::HitEvent>,
    loops: Vec<game::hitsound::LoopEvent>,
    cfg: SessionConfig,
    load_ms: f64,
}

impl Prepared {
    /// The .osu file this session was loaded from.
    pub fn map_path(&self) -> &std::path::Path {
        &self.cfg.map
    }
}

/// Load the map and skin, rasterize the sheet at the given pixels-per-view-unit and decode the hit sounds.
pub fn prepare(cfg: SessionConfig, ss: f32) -> Result<Prepared, String> {
    let t = std::time::Instant::now();
    let raw_map = Beatmap::parse_file(&cfg.map)?;
    let mut mod_map = raw_map.clone();
    mod_map.override_difficulty(cfg.override_cs, cfg.override_ar, cfg.hard_rock);
    let map = Arc::new(mod_map);
    let skin = Arc::new(Skin::open(&cfg.skin)?);
    let base = base_sheet(&skin, &map, ss);
    let events = game::hitsound::events(&map);
    let loops = game::hitsound::loops(&map);
    let samples = if cfg.audio {
        let sound_skin = cfg.sound_skin.as_ref().and_then(|p| Skin::open(p).ok());
        let mut sound_skins: Vec<&Skin> = Vec::new();
        if skin.style == skin::Style::Legacy {
            sound_skins.push(&skin);
        }
        if let Some(s) = &sound_skin {
            sound_skins.push(s);
        }
        load_samples(&map, &sound_skins, &events, &loops)
    } else {
        Default::default()
    };
    Ok(Prepared { raw_map, map, skin, base, ss, samples, events, loops, cfg, load_ms: t.elapsed().as_secs_f64() * 1000.0 })
}

pub struct Session {
    raw_map: Beatmap,
    pub map: Arc<Beatmap>,
    pub skin: Arc<Skin>,
    scene: Scene,
    base: Sheet,
    renderer: GameRenderer,
    frame: Frame,
    audio: Option<Audio>,
    follow: Option<Arc<Atlas>>,
    /// The pixels-per-view-unit the skin sheet was rasterized for.
    pub built_ss: f32,
    pub clock_ms: f32,
    pub start_ms: f32,
    pub end_ms: f32,
    /// The map has played to its end; whoever owns the session decides what comes next (see `restart`).
    pub ended: bool,
    pub playing: bool,
    offset_ms: f32,
    speed: f32,
    pub info: String,
    pub sprites: usize,
    pub sliders: usize,
}

impl Session {
    pub fn finish(p: Prepared, device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat, size: (u32, u32)) -> Session {
        let Prepared { raw_map, map, skin, base, ss, samples, events, loops, cfg, load_ms } = p;
        let (sheet, follow) = with_follow(&base, &skin, cfg.follow.as_deref());
        let sheet = Arc::new(sheet);
        let mut scene = Scene::new(map.clone(), skin.clone(), sheet.clone(), follow);
        scene.cursor_scale = cfg.cursor_scale;
        scene.hit_lighting = cfg.hit_lighting;
        scene.snake_in = cfg.snake_in;
        scene.snake_out = cfg.snake_out;
        scene.hidden = cfg.hidden;
        let renderer = GameRenderer::new(device, queue, format, size, &sheet.rgba, (sheet.w, sheet.h), &scene.geoms, ss);
        let first = map.objects.first().map(|o| o.time).unwrap_or(0.0);
        let last = map.objects.last().map(|o| o.end_time).unwrap_or(first + 1000.0);
        let start_ms = (first - 2500.0).max(0.0);
        let end_ms = last + 2500.0;
        let n_samples = samples.len();
        let (n_events, n_loops) = (events.len(), loops.len());
        let clock0 = cfg.resume.map(|(c, _)| c.clamp(start_ms, end_ms)).unwrap_or(start_ms);
        let audio = if cfg.audio { open_audio(&map, samples, events, loops, (clock0 - cfg.offset_ms).max(0.0), cfg.volume) } else { None };
        let mut audio = audio;
        if let Some(a) = &mut audio {
            a.player.set_speed(cfg.speed.clamp(0.05, 4.0));
            a.speed = cfg.speed.clamp(0.05, 4.0);
        }
        let _ = (load_ms, n_samples, n_events, n_loops);
        let info = format_info(&map, &skin);
        let s = Session { raw_map, map, skin, scene, base, renderer, frame: Frame::default(), audio, follow: cfg.follow.clone(), built_ss: ss, clock_ms: clock0, start_ms, end_ms, ended: false, playing: cfg.resume.map(|(_, p)| p).unwrap_or(true), offset_ms: cfg.offset_ms, speed: cfg.speed.clamp(0.05, 4.0), info, sprites: 0, sliders: 0 };
        if !s.playing {
            if let Some(a) = &s.audio {
                a.player.pause();
            }
        }
        s
    }

    /// Whether both sessions play the same difficulty.
    pub fn map_path_eq(&self, other: &Session) -> bool {
        self.map.dir == other.map.dir && self.map.title == other.map.title && self.map.version == other.map.version
    }

    pub fn queue(&self) -> &wgpu::Queue {
        self.renderer.queue()
    }

    /// Show a new follow-point animation (or the skin's own when `None`).
    /// Fast path uploads ONLY the followpoint atlas rows directly into the existing GPU sheet (~1 ms vs ~110 ms).
    pub fn set_follow(&mut self, atlas: Option<Arc<Atlas>>) {
        self.follow = atlas;
        if let Some(a) = &self.follow {
            let y0 = self.base.h;
            if self.renderer.update_sheet_region(y0, &a.rgba, (a.width, a.height)) {
                let rects = a.frames.iter().map(|f| f.origin.map(|(ox, oy)| Rect { x: ox, y: y0 + oy, w: f.w, h: f.h })).collect();
                self.scene.follow = Some(FollowFrames { rects, fps: a.fps, ratio: 1.0, ini_set: true, additive: false });
                return;
            }
        }
        let (sheet, ff) = with_follow(&self.base, &self.skin, self.follow.as_deref());
        let sheet = Arc::new(sheet);
        self.renderer.set_sheet(&sheet.rgba, (sheet.w, sheet.h));
        self.scene.sheet = sheet;
        self.scene.follow = ff;
    }

    /// A job that rasterizes the skin sheet for a new pixels-per-view-unit; run it on a worker, then `apply_base`.
    pub fn rescale_job(&self, ss: f32) -> impl FnOnce() -> (Sheet, f32) + Send + 'static {
        let (skin, map) = (self.skin.clone(), self.map.clone());
        move || (base_sheet(&skin, &map, ss), ss)
    }

    pub fn apply_base(&mut self, base: Sheet, ss: f32) {
        self.base = base;
        self.built_ss = ss;
        let f = self.follow.clone();
        self.set_follow(f);
    }

    pub fn resize(&mut self, size: (u32, u32)) {
        self.renderer.resize(size);
    }

    /// Where the 640x480 view goes in the target, in pixels. Everything scales with it.
    pub fn set_viewport(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.renderer.set_viewport(x, y, w, h);
    }

    /// Pixels per view unit right now.
    pub fn scale(&self) -> f32 {
        self.renderer.scale
    }
    fn seek_audio(&mut self) {
        if let Some(a) = &mut self.audio {
            a.seek_music(self.clock_ms - self.offset_ms);
            a.seek_events(self.clock_ms);
        }
    }

    pub fn seek_to(&mut self, ms: f32) {
        self.clock_ms = ms.clamp(self.start_ms, self.end_ms);
        self.ended = false;
        self.seek_audio();
        if self.playing {
            if let Some(a) = &self.audio {
                a.player.play();
            }
        }
    }

    /// Play the same map again from the start.
    pub fn restart(&mut self) {
        self.seek_to(self.start_ms);
        self.set_playing(true);
    }

    pub fn seek_by(&mut self, ms: f32) {
        self.seek_to(self.clock_ms + ms);
    }

    pub fn set_playing(&mut self, p: bool) {
        self.playing = p;
        if let Some(a) = &mut self.audio {
            if p {
                a.player.play();
            } else {
                a.player.pause();
                a.stop_loops();
            }
        }
    }

    pub fn toggle_play(&mut self) {
        self.set_playing(!self.playing);
    }

    pub fn volume(&self) -> f32 {
        self.audio.as_ref().map(|a| a.volume).unwrap_or(0.0)
    }

    pub fn set_volume(&mut self, v: f32) {
        if let Some(a) = &mut self.audio {
            a.set_volume(v);
        }
    }

    pub fn set_offset(&mut self, ms: f32) {
        if (ms - self.offset_ms).abs() > f32::EPSILON {
            self.offset_ms = ms;
            self.seek_audio();
        }
    }

    /// Play slower or faster; the music follows (and changes pitch with it, as it does with half or double time).
    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed.clamp(0.05, 4.0);
        if let Some(a) = &mut self.audio {
            a.player.set_speed(self.speed);
            a.speed = self.speed;
        }
        self.seek_audio();
    }

    pub fn set_snaking(&mut self, snake_in: bool, snake_out: bool) {
        self.scene.snake_in = snake_in;
        self.scene.snake_out = snake_out;
    }

    pub fn set_hidden(&mut self, on: bool) {
        self.scene.hidden = on;
    }

    /// Update CS/AR/HR live in-memory without re-reading disk or restarting audio.
    pub fn set_difficulty(&mut self, cs: Option<f32>, ar: Option<f32>, hard_rock: bool) {
        let old_cs = self.map.cs;
        let mut mod_map = self.raw_map.clone();
        mod_map.override_difficulty(cs, ar, hard_rock);
        let new_cs = mod_map.cs;
        self.map = Arc::new(mod_map);
        if matches!(self.skin.style, game::skin::Style::Argon { .. }) && (new_cs - old_cs).abs() > 1e-4 {
            self.base = base_sheet(&self.skin, &self.map, self.built_ss);
            let (sheet, ff) = with_follow(&self.base, &self.skin, self.follow.as_deref());
            let sheet = Arc::new(sheet);
            self.renderer.set_sheet(&sheet.rgba, (sheet.w, sheet.h));
            self.scene.sheet = sheet;
            self.scene.follow = ff;
        }
        let mut scene = Scene::new(self.map.clone(), self.skin.clone(), self.scene.sheet.clone(), self.scene.follow.take());
        scene.cursor_scale = self.scene.cursor_scale;
        scene.hit_lighting = self.scene.hit_lighting;
        scene.snake_in = self.scene.snake_in;
        scene.snake_out = self.scene.snake_out;
        scene.hidden = self.scene.hidden;
        self.renderer.set_geoms(&scene.geoms);
        self.scene = scene;
        self.info = format_info(&self.map, &self.skin);
    }

    pub fn set_hit_lighting(&mut self, on: bool) {
        self.scene.hit_lighting = on;
    }

    pub fn set_cursor_scale(&mut self, s: f32) {
        self.scene.cursor_scale = s;
    }

    /// Advance the clock by `dt` ms, following the music, and play the hit sounds that came due.
    pub fn advance(&mut self, dt: f32) {
        if !self.playing || self.ended {
            return;
        }
        if let Some(a) = &mut self.audio {
            a.update_gain();
        }
        let prev = self.clock_ms;
        self.clock_ms += dt.min(50.0) * self.speed;
        // the audio device is the master clock while there is music left to play; once it has ended the clock runs on its own
        if let Some(a) = self.audio.as_ref().filter(|a| !a.player.empty()) {
            // follow it gently so frames stay smooth
            let ap = a.pos_ms() + self.offset_ms;
            let err = ap - self.clock_ms;
            if std::time::Instant::now() < a.ignore_until {
                // just sought: the clock leads
            } else if err.abs() > 120.0 {
                self.clock_ms = ap;
            } else {
                self.clock_ms += err * 0.02;
            }
        }
        if self.clock_ms >= self.end_ms {
            self.clock_ms = self.end_ms;
            self.ended = true;
        } else if let Some(a) = &mut self.audio {
            a.update_loops(self.clock_ms, true);
            // a hit sound belongs to the moment it is hit, so a jump must not replay old ones
            if self.clock_ms - prev < 100.0 {
                a.fire(prev, self.clock_ms);
            } else {
                a.seek_events(self.clock_ms);
            }
        }
    }

    pub fn render(&mut self, view: &wgpu::TextureView) {
        self.scene.build(self.clock_ms, &mut self.frame);
        self.sprites = self.frame.sprites.len();
        self.sliders = self.frame.sliders.len();
        self.renderer.render(view, &self.frame, &self.scene.geoms, [0.0, 0.0, 0.0, 1.0]);
    }
}

fn format_info(map: &Beatmap, skin: &Skin) -> String {
    let cs = format!("{:.1}", map.cs);
    let cs = cs.trim_end_matches(".0");
    let ar = format!("{:.1}", map.ar);
    let ar = ar.trim_end_matches(".0");
    format!("{} [{}]\nCS {} · AR {} · {}", map.title, map.version, cs, ar, skin.name)
}
