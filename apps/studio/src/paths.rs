//! Where things live on each platform, and the saved choices.
use std::path::PathBuf;

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

fn home() -> Option<PathBuf> {
    env_path("HOME").or_else(|| env_path("USERPROFILE"))
}

/// osu!stable's Songs folder, if it can be found in the usual places.
pub fn default_songs() -> Option<PathBuf> {
    let mut c: Vec<PathBuf> = Vec::new();
    if let Some(p) = env_path("OSU_SONGS") {
        c.push(p);
    }
    if cfg!(windows) {
        if let Some(l) = env_path("LOCALAPPDATA") {
            c.push(l.join("osu!").join("Songs"));
        }
    }
    if let Some(h) = home() {
        // Wine / osu-winello setups and a plain copy
        for rel in [".local/share/osu-wine/osu!/Songs", ".local/share/osu-wine/OSU/Songs", "osu!/Songs", ".osu/Songs", ".local/share/osu/Songs"] {
            c.push(h.join(rel));
        }
    }
    c.into_iter().find(|p| p.is_dir())
}

/// The newest skin export (`.osk`) from lazer or folder in stable's Skins directory, used as a starting point for
/// the hit sounds.
pub fn detect_skin() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(a) = env_path("APPDATA") {
        dirs.push(a.join("osu").join("exports"));
    }
    if let Some(h) = home() {
        dirs.push(h.join(".local/share/osu/exports"));
        dirs.push(h.join(".local/share/osu-wine/osu!/Skins"));
    }
    if cfg!(windows) {
        if let Some(l) = env_path("LOCALAPPDATA") {
            dirs.push(l.join("osu!").join("Skins"));
        }
    }
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let ok = p.extension().is_some_and(|x| x.eq_ignore_ascii_case("osk")) || (p.is_dir() && p.join("skin.ini").exists());
            if !ok {
                continue;
            }
            let t = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            if best.as_ref().is_none_or(|(bt, _)| t > *bt) {
                best = Some((t, p));
            }
        }
    }
    best.map(|(_, p)| p)
}

pub fn config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        Some(env_path("APPDATA")?.join("skin-studio"))
    } else {
        Some(env_path("XDG_CONFIG_HOME").or_else(|| home().map(|h| h.join(".config")))?.join("skin-studio"))
    }
}

fn config_file() -> Option<PathBuf> {
    Some(config_dir()?.join("config.json"))
}

/// The skins of osu!lazer's own database, by the folder they are copied into when one is picked (it does not exist before).
static LAZER_SKINS: std::sync::Mutex<Vec<(PathBuf, PathBuf, game::realm::LazerSkin)>> = std::sync::Mutex::new(Vec::new());

fn lazer_skins() -> Vec<(String, PathBuf)> {
    use std::hash::{Hash, Hasher};
    let Some(dir) = game::realm::lazer_dir() else { return Vec::new() };
    let Ok(mut skins) = game::realm::read_skins(&dir) else { return Vec::new() };
    skins.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    let mut reg = LAZER_SKINS.lock().unwrap();
    reg.clear();
    let mut out = Vec::new();
    for s in skins {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let mut files = s.files.clone();
        files.sort();
        files.hash(&mut h);
        let safe: String = s.name.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).take(40).collect();
        let folder = std::env::temp_dir().join("skinstudio").join("lazer-skins").join(format!("{}-{:08x}", safe.trim(), h.finish() as u32));
        out.push((format!("{} (osu!lazer)", s.name), folder.clone()));
        reg.push((folder, dir.clone(), s));
    }
    out
}

/// Make sure the folder of a skin picked from osu!lazer's database exists (copying its files there the first time).
pub fn ensure_skin(path: &std::path::Path) -> Result<(), String> {
    let reg = LAZER_SKINS.lock().unwrap();
    match reg.iter().find(|(f, _, _)| f == path) {
        Some((f, dir, s)) => game::realm::materialize_skin(dir, s, f),
        None => Ok(()),
    }
}

/// Every skin we can find: lazer exports and stable Skins folders, newest first.
pub fn detect_skins() -> Vec<(String, PathBuf)> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(a) = env_path("APPDATA") {
        dirs.push(a.join("osu").join("exports"));
    }
    if let Some(h) = home() {
        dirs.push(h.join(".local/share/osu/exports"));
        dirs.push(h.join(".local/share/osu-wine/osu!/Skins"));
    }
    if cfg!(windows) {
        if let Some(l) = env_path("LOCALAPPDATA") {
            dirs.push(l.join("osu!").join("Skins"));
        }
    }
    let mut found: Vec<(std::time::SystemTime, String, PathBuf)> = Vec::new();
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let is_osk = p.extension().is_some_and(|x| x.eq_ignore_ascii_case("osk"));
            let is_dir = p.is_dir() && p.join("skin.ini").exists();
            if !(is_osk || is_dir) {
                continue;
            }
            let label = if is_osk { p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default() } else { p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default() };
            let t = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            found.push((t, label, p));
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    // the skins of the game itself first: they are the ones in use
    let mut all = lazer_skins();
    all.extend(found.into_iter().map(|(_, l, p)| (l, p)));
    all
}

/// `random` / `random:SEED` picks a map from the Songs folder, anything else is a path to an .osu file.
pub fn resolve_map(spec: &str, songs: Option<&str>, library: &str) -> Result<PathBuf, String> {
    if let Some(rest) = spec.strip_prefix("random") {
        let seed = rest.trim_start_matches(':').parse().unwrap_or_else(|_| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1));
        // lazer's own library (read from its Realm database) unless stable was asked for
        if library != "stable" {
            if let Some(dir) = game::realm::lazer_dir() {
                match game::realm::random_map(&dir, seed) {
                    Ok(p) => return Ok(p),
                    Err(e) if library == "lazer" => return Err(e),
                    Err(_) => {} // automatic: fall back to the stable Songs folder
                }
            }
            if library == "lazer" {
                return Err("osu!lazer's data folder was not found (set OSU_LAZER_DIR to it).".into());
            }
        }
        let songs: PathBuf = songs.map(PathBuf::from).or_else(default_songs).ok_or("No Songs folder found: choose it, or pick a .osu, .osz or .olz file in the Preview section.")?;
        game::library::random_map(&songs, seed).map(|(p, _)| p).ok_or_else(|| "No suitable map found in that folder.".to_string())
    } else {
        let p = PathBuf::from(spec);
        let is_archive = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("osz") || e.eq_ignore_ascii_case("olz"));
        if is_archive {
            game::library::open_archive(&p)
        } else {
            Ok(p)
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub settings: String,
    /// "argon", "argon-pro" or a skin path
    pub skin: String,
    pub sound_skin: String,
    /// "random" or an .osu path
    pub map: String,
    pub songs: String,
    pub volume: f32,
    /// Audio is off until the user turns it on.
    pub muted: bool,
    /// Music offset in ms (positive = the song comes later than the map).
    pub offset_ms: f32,
    pub cursor_scale: f32,
    pub hit_lighting: bool,
    pub snake_in: bool,
    pub snake_out: bool,
    pub speed: f32,
    /// Where random maps come from: "auto", "lazer" or "stable".
    pub library: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            settings: String::new(),
            skin: "argon".into(),
            sound_skin: detect_skin().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
            map: "random".into(),
            songs: default_songs().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
            volume: 0.10,
            muted: true,
            offset_ms: 0.0,
            cursor_scale: 1.0,
            hit_lighting: false,
            snake_in: true,
            snake_out: true,
            speed: 1.0,
            library: "auto".into(),
        }
    }
}

impl Config {
    pub fn load() -> Config {
        let mut c = Config::default();
        let Some(text) = config_file().and_then(|f| std::fs::read_to_string(f).ok()) else { return c };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return c };
        let s = |k: &str, d: &mut String| {
            if let Some(x) = v.get(k).and_then(|x| x.as_str()) {
                *d = x.to_string();
            }
        };
        s("settings", &mut c.settings);
        s("skin", &mut c.skin);
        s("sound_skin", &mut c.sound_skin);
        s("map", &mut c.map);
        s("songs", &mut c.songs);
        s("library", &mut c.library);
        if let Some(x) = v.get("volume").and_then(|x| x.as_f64()) {
            c.volume = (x as f32).clamp(0.0, 1.0);
        }
        if let Some(b) = v.get("muted").and_then(|x| x.as_bool()) {
            c.muted = b;
        }
        if let Some(x) = v.get("offset_ms").and_then(|x| x.as_f64()) {
            c.offset_ms = (x as f32).clamp(-1000.0, 1000.0);
        }
        if let Some(x) = v.get("cursor_scale").and_then(|x| x.as_f64()) {
            c.cursor_scale = (x as f32).clamp(0.3, 3.0);
        }
        if let Some(b) = v.get("hit_lighting").and_then(|x| x.as_bool()) {
            c.hit_lighting = b;
        }
        if let Some(b) = v.get("snake_in").and_then(|x| x.as_bool()) {
            c.snake_in = b;
        }
        if let Some(b) = v.get("snake_out").and_then(|x| x.as_bool()) {
            c.snake_out = b;
        }
        if let Some(x) = v.get("speed").and_then(|x| x.as_f64()) {
            c.speed = (x as f32).clamp(0.1, 2.0);
        }
        c
    }

    pub fn save(&self) {
        let Some(f) = config_file() else { return };
        if let Some(d) = f.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let v = serde_json::json!({ "settings": self.settings, "skin": self.skin, "sound_skin": self.sound_skin, "map": self.map, "songs": self.songs, "volume": self.volume, "muted": self.muted, "offset_ms": self.offset_ms, "cursor_scale": self.cursor_scale, "hit_lighting": self.hit_lighting, "snake_in": self.snake_in, "snake_out": self.snake_out, "speed": self.speed, "library": self.library });
        let _ = std::fs::write(f, serde_json::to_string_pretty(&v).unwrap_or_default());
    }
}

#[cfg(test)]
mod lazer_skin_tests {
    #[test]
    #[ignore]
    fn lists_and_copies_the_cerkie_skin() {
        let skins = super::detect_skins();
        let (label, path) = skins.iter().find(|(l, _)| l.starts_with("Cerkie lazer (may 2026) (osu!lazer)")).expect("skin in the list").clone();
        println!("{label} -> {}", path.display());
        super::ensure_skin(&path).expect("copied");
        println!("copied: {} files, cursor1@2x: {}", std::fs::read_dir(&path).unwrap().count(), path.join("cursor1@2x.png").is_file());
    }
}
