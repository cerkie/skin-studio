//! Legacy-format skins (folders or .osk/.zip): skin.ini, element lookup with @2x and animation frames, and packing
//! of the gameplay elements into one texture sheet.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct Ini {
    /// section (lowercase) -> key (lowercase) -> value
    pub sections: HashMap<String, HashMap<String, String>>,
}

impl Ini {
    pub fn parse(text: &str) -> Ini {
        let mut ini = Ini::default();
        let mut sec = String::new();
        for line in text.lines() {
            let line = line.trim_start_matches('\u{feff}').trim();
            if line.is_empty() || line.starts_with("//") {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                sec = line[1..line.len() - 1].to_lowercase();
                continue;
            }
            if let Some((k, v)) = line.split_once(':') {
                let v = v.split("//").next().unwrap_or("").trim();
                ini.sections.entry(sec.clone()).or_default().insert(k.trim().to_lowercase(), v.to_string());
            }
        }
        ini
    }

    pub fn get(&self, sec: &str, key: &str) -> Option<&str> {
        self.sections.get(sec)?.get(&key.to_lowercase()).map(|s| s.as_str())
    }

    pub fn color(&self, key: &str) -> Option<[f32; 3]> {
        let v: Vec<f32> = self.get("colours", key)?.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        (v.len() >= 3).then(|| [v[0], v[1], v[2]])
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Style {
    #[default]
    Legacy,
    Argon { pro: bool },
}

#[derive(Clone, Debug, Default)]
pub struct Skin {
    pub style: Style,
    pub name: String,
    pub root: PathBuf,
    pub ini: Ini,
    /// Fallback skins, searched after this one.
    pub fallbacks: Vec<Skin>,
    files: HashMap<String, PathBuf>,
}

fn index(dir: &Path, base: &Path, out: &mut HashMap<String, PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            index(&p, base, out);
        } else if let Ok(rel) = p.strip_prefix(base) {
            out.insert(rel.to_string_lossy().replace('\\', "/").to_lowercase(), p.clone());
        }
    }
}

impl Skin {
    /// A skin folder, or an .osk/.zip (extracted once into the cache directory).
    /// lazer's built-in Argon skin (drawn procedurally, see `argon.rs`); `pro` hides the Great judgement.
    pub fn argon(pro: bool) -> Skin {
        Skin { style: Style::Argon { pro }, name: if pro { "Argon Pro".into() } else { "Argon".into() }, ..Default::default() }
    }

    pub fn open(path: &Path) -> Result<Skin, String> {
        match path.to_string_lossy().to_lowercase().as_str() {
            "argon" => return Ok(Skin::argon(true)),
            "argon-pro" | "argon pro" | "argonpro" => return Ok(Skin::argon(true)),
            _ => {}
        }
        let root = if path.is_dir() {
            path.to_path_buf()
        } else {
            let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "skin".into());
            let dest = std::env::temp_dir().join("skinstudio").join("skins").join(stem);
            let stamp = dest.join(".extracted");
            let src_time = std::fs::metadata(path).and_then(|m| m.modified()).ok();
            let fresh = std::fs::metadata(&stamp).and_then(|m| m.modified()).ok().zip(src_time).is_some_and(|(a, b)| a >= b);
            if !fresh {
                let _ = std::fs::remove_dir_all(&dest);
                let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
                let mut zip = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
                zip.extract(&dest).map_err(|e| e.to_string())?;
                let _ = std::fs::write(&stamp, b"");
            }
            // some archives wrap everything in one folder
            let mut root = dest.clone();
            if !dest.join("skin.ini").exists() {
                if let Ok(rd) = std::fs::read_dir(&dest) {
                    for e in rd.flatten() {
                        if e.path().join("skin.ini").exists() {
                            root = e.path();
                        }
                    }
                }
            }
            root
        };
        let mut files = HashMap::new();
        index(&root, &root, &mut files);
        let ini = files.get("skin.ini").and_then(|p| std::fs::read(p).ok()).map(|b| Ini::parse(&String::from_utf8_lossy(&b))).unwrap_or_default();
        let name = ini.get("general", "name").map(|s| s.to_string()).unwrap_or_else(|| root.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
        Ok(Skin { style: Style::Legacy, name, root, ini, fallbacks: vec![], files })
    }

    pub fn flag(&self, key: &str, alt: &str, default: bool) -> bool {
        let v = self.ini.get("general", key).or_else(|| self.ini.get("general", alt));
        v.map(|s| s.trim() != "0").unwrap_or(default)
    }

    pub fn version(&self) -> f32 {
        match self.ini.get("general", "version") {
            Some("latest") => 2.7,
            Some(v) => v.parse().unwrap_or(1.0),
            None => 1.0,
        }
    }

    /// Path and image ratio (2 for @2x) of an element, searching fallbacks.
    pub fn find(&self, name: &str) -> Option<(PathBuf, f32)> {
        let n = name.to_lowercase().replace('\\', "/");
        for (suffix, ratio) in [("@2x.png", 2.0), (".png", 1.0)] {
            if let Some(p) = self.files.get(&format!("{n}{suffix}")) {
                return Some((p.clone(), ratio));
            }
        }
        self.fallbacks.iter().find_map(|f| f.find(name))
    }

    pub fn find_sound(&self, name: &str) -> Option<PathBuf> {
        let n = name.to_lowercase();
        for ext in ["wav", "ogg", "mp3"] {
            if let Some(p) = self.files.get(&format!("{n}.{ext}")) {
                return Some(p.clone());
            }
        }
        self.fallbacks.iter().find_map(|f| f.find_sound(name))
    }

    /// Animation frames `name-0`, `name-1`... (separator `-`), or `name0`... (empty separator), else the single
    /// still image `name`.
    pub fn frames(&self, name: &str, sep: &str) -> Vec<(PathBuf, f32)> {
        let mut v = Vec::new();
        while let Some(f) = self.find(&format!("{name}{sep}{}", v.len())) {
            v.push(f);
        }
        if v.is_empty() {
            if let Some(f) = self.find(name) {
                v.push(f);
            }
        }
        v
    }

    pub fn combo_colors(&self) -> Vec<[f32; 3]> {
        (1..=8).filter_map(|i| self.ini.color(&format!("combo{i}"))).collect()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Clone, Debug)]
pub struct Elem {
    pub frames: Vec<Rect>,
    /// Image ratio: displayed size is pixel size / ratio.
    pub ratio: f32,
}

/// All gameplay elements in one premultiplied RGBA8 sheet.
#[derive(Default, Clone)]
pub struct Sheet {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
    pub elems: HashMap<String, Elem>,
    cur: (u32, u32, u32),
}

impl Sheet {
    pub const WIDTH: u32 = 8192;

    /// Adds one premultiplied RGBA8 image (`ratio` = pixels per display unit) after what is already packed.
    pub fn add_image(&mut self, key: &str, w: u32, h: u32, px: &[u8], ratio: f32) {
        if self.w == 0 {
            self.w = Self::WIDTH;
        }
        let (mut x, mut y, mut sh) = self.cur;
        if x + w + 1 > Self::WIDTH {
            x = 0;
            y += sh + 1;
            sh = 0;
        }
        sh = sh.max(h);
        let need = (y + sh + 1).max(self.h);
        self.rgba.resize((Self::WIDTH * need * 4) as usize, 0);
        self.h = need;
        for row in 0..h {
            let dst = (((y + row) * Self::WIDTH + x) * 4) as usize;
            let src = (row * w * 4) as usize;
            self.rgba[dst..dst + (w * 4) as usize].copy_from_slice(&px[src..src + (w * 4) as usize]);
        }
        self.elems.entry(key.to_string()).or_insert_with(|| Elem { frames: vec![], ratio }).frames.push(Rect { x, y, w, h });
        self.cur = (x + w + 1, y, sh);
    }

    pub fn get(&self, name: &str) -> Option<&Elem> {
        self.elems.get(name)
    }

    /// Packs `(key, frames)` entries, where each frame is an image file and its ratio, with a shelf packer.
    pub fn pack(entries: &[(String, Vec<(PathBuf, f32)>, Option<f32>)]) -> Sheet {
        struct Item {
            key: usize,
            w: u32,
            h: u32,
            px: Vec<u8>,
        }
        let mut items: Vec<Item> = Vec::new();
        let mut ratios: Vec<f32> = Vec::new();
        for (k, (_, frames, target)) in entries.iter().enumerate() {
            let src_ratio = frames.first().map(|f| f.1).unwrap_or(1.0);
            // never upscale: only shrink textures that are denser than the screen can show
            let ratio = match target {
                Some(tr) if *tr < src_ratio => *tr,
                _ => src_ratio,
            };
            ratios.push(ratio);
            for (path, fr) in frames {
                let Ok(sp) = engine::sources::load_png(path) else { continue };
                let mut img = engine::img::Img { w: sp.w, h: sp.h, px: sp.px.iter().map(|p| [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]]).collect() };
                if ratio < *fr {
                    let f = ratio / fr;
                    let (nw, nh) = (((sp.w as f32 * f).round() as usize).max(1), ((sp.h as f32 * f).round() as usize).max(1));
                    img = engine::img::resize_area(&img, nw, nh);
                }
                let mut px = Vec::with_capacity(img.w * img.h * 4);
                for p in &img.px {
                    px.extend_from_slice(&[(p[0] * 255.0 + 0.5) as u8, (p[1] * 255.0 + 0.5) as u8, (p[2] * 255.0 + 0.5) as u8, (p[3] * 255.0 + 0.5) as u8]);
                }
                items.push(Item { key: k, w: img.w as u32, h: img.h as u32, px });
            }
        }
        // tallest first for tidy shelves, but keep per-element frame order via the original index
        let mut order: Vec<usize> = (0..items.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(items[i].h));
        let (mut x, mut y, mut shelf_h) = (0u32, 0u32, 0u32);
        let mut placed: Vec<(usize, Rect)> = Vec::new();
        for &i in &order {
            let it = &items[i];
            if x + it.w + 1 > Self::WIDTH {
                x = 0;
                y += shelf_h + 1;
                shelf_h = 0;
            }
            placed.push((i, Rect { x, y, w: it.w, h: it.h }));
            x += it.w + 1;
            shelf_h = shelf_h.max(it.h);
        }
        let h = y + shelf_h + 1;
        let mut rgba = vec![0u8; (Self::WIDTH * h * 4) as usize];
        let mut sheet = Sheet { w: Self::WIDTH, h, rgba: vec![], elems: HashMap::new(), cur: (x, y, shelf_h) };
        placed.sort_by_key(|(i, _)| *i);
        for (i, r) in &placed {
            let it = &items[*i];
            for row in 0..it.h {
                let dst = (((r.y + row) * Self::WIDTH + r.x) * 4) as usize;
                let src = (row * it.w * 4) as usize;
                rgba[dst..dst + (it.w * 4) as usize].copy_from_slice(&it.px[src..src + (it.w * 4) as usize]);
            }
            sheet.elems.entry(entries[it.key].0.clone()).or_insert_with(|| Elem { frames: vec![], ratio: ratios[it.key] }).frames.push(*r);
        }
        sheet.rgba = rgba;
        sheet
    }
}

/// Every element the gameplay scene can draw, resolved against the skin. `object_scale` is the circle scale and
/// `ss` the view-to-pixel factor; textures are shrunk (area average, no ringing) to 1.5x what the screen can show.
pub fn gameplay_entries(skin: &Skin, object_scale: f32, ss: f32) -> Vec<(String, Vec<(PathBuf, f32)>, Option<f32>)> {
    let mut e: Vec<(String, Vec<(PathBuf, f32)>, Option<f32>)> = Vec::new();
    let obj = Some(object_scale * ss * 1.5);
    let one = Some(ss * 1.5);
    let mut add = |key: &str, frames: Vec<(PathBuf, f32)>, target: Option<f32>| {
        if !frames.is_empty() {
            e.push((key.to_string(), frames, target));
        }
    };
    for n in ["hitcircle", "hitcircleoverlay", "sliderstartcircle", "sliderstartcircleoverlay", "sliderendcircle", "sliderendcircleoverlay", "reversearrow", "sliderscorepoint", "sliderb-nd", "sliderb-spec"] {
        add(n, skin.frames(n, "-").into_iter().take(1).collect(), obj);
    }
    add("approachcircle", skin.frames("approachcircle", "-").into_iter().take(1).collect(), Some(object_scale * ss * 2.0));
    for n in ["cursor", "cursormiddle", "cursortrail"] {
        add(n, skin.frames(n, "-").into_iter().take(1).collect(), one);
    }
    add("sliderb", skin.frames("sliderb", ""), obj);
    for n in ["spinner-background", "spinner-circle", "spinner-metre", "spinner-approachcircle", "spinner-spin", "spinner-clear", "spinner-rpm", "spinner-glow", "spinner-bottom", "spinner-top", "spinner-middle", "spinner-middle2"] {
        add(n, skin.frames(n, "-").into_iter().take(1).collect(), one);
    }
    let score = skin.ini.get("fonts", "scoreprefix").unwrap_or("score").to_string();
    for d in 0..10 {
        add(&format!("sc{d}"), skin.frames(&format!("{score}-{d}"), "").into_iter().take(1).collect(), one);
    }
    add("sliderfollowcircle", skin.frames("sliderfollowcircle", "-"), Some(object_scale * ss * 1.5 * 2.0));
    for n in ["hit300", "hit300g", "hit300k", "hit100", "hit100k", "hit50", "hit0"] {
        add(n, skin.frames(n, "-"), Some(object_scale * ss * 1.5));
    }
    let prefix = skin.ini.get("fonts", "hitcircleprefix").unwrap_or("default").to_string();
    for d in 0..10 {
        add(&format!("num{d}"), skin.frames(&format!("{prefix}-{d}"), "").into_iter().take(1).collect(), Some(object_scale * ss * 0.8 * 1.5));
    }
    e
}

pub fn hitcircle_overlap(skin: &Skin) -> f32 {
    skin.ini.get("fonts", "hitcircleoverlap").and_then(|s| s.parse().ok()).unwrap_or(0.0)
}

/// lazer falls back to a plain approach ring (128 units across) when the skin has none.
pub fn add_default_approach(sheet: &mut Sheet, object_scale: f32, ss: f32) {
    crate::argon::add_approach_ring(sheet, "approachcircle", object_scale * ss * 2.0);
}

/// The sheet of everything the gameplay draws from a skin: the Argon set drawn in code, or the skin's own images packed, with lazer's
/// default approach ring for a skin that has none (osu!lazer falls back to its default skin for it).
pub fn base_sheet(skin: &Skin, cs: f32, ss: f32) -> Sheet {
    let scale = engine::lazer::circle_scale(cs);
    if matches!(skin.style, Style::Argon { .. }) {
        let mut s = Sheet::default();
        crate::argon::add_to_sheet(&mut s, scale, ss);
        s
    } else {
        let mut sheet = Sheet::pack(&gameplay_entries(skin, scale, ss));
        if sheet.get("approachcircle").is_none() {
            add_default_approach(&mut sheet, scale, ss);
        }
        sheet
    }
}
