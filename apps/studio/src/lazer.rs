//! The osu!lazer preview: a separate program (`lazer-preview`) that plays the map with lazer's own renderer. Skin Studio starts
//! it, tells it what to play with JSON lines on its standard input, and reads what it reports from its standard output.
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};

use engine::gen::Frame;
use serde_json::{json, Value};

/// Where `lazer-preview` is looked for: an environment variable, next to the executable (in `lazer-preview/`), in the settings
/// folder, and, for a build from source, in the project's own build folder.
pub fn find_host() -> Option<PathBuf> {
    let name = if cfg!(windows) { "lazer-preview.exe" } else { "lazer-preview" };
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("SKIN_STUDIO_LAZER_PREVIEW") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
        dirs.push(p);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            dirs.push(d.join("lazer-preview"));
            dirs.push(d.to_path_buf());
            // target/release/studio -> the project's apps/lazer-preview build
            for up in [2, 3] {
                let mut root = d.to_path_buf();
                for _ in 0..up {
                    root.pop();
                }
                dirs.push(root.join("apps").join("lazer-preview").join("bin").join("Release").join("net10.0"));
            }
        }
    }
    if let Some(c) = crate::paths::config_dir() {
        dirs.push(c.join("lazer-preview"));
    }
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

/// What the preview last said about the playing map.
#[derive(Clone, Copy, Default, Debug)]
pub struct Time {
    pub ms: f32,
    pub start: f32,
    pub end: f32,
    pub playing: bool,
}

pub struct Link {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Value>,
    pub ready: bool,
    pub time: Option<Time>,
    pub title: String,
    /// The id of the last skin the preview confirmed.
    pub skin_ack: u32,
    pub error: Option<String>,
    next_skin_id: u32,
}

impl Link {
    pub fn spawn(exe: &Path) -> Result<Link, String> {
        let mut cmd = Command::new(exe);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        if let Some(d) = exe.parent() {
            cmd.current_dir(d);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // no console window
        }
        let mut child = cmd.spawn().map_err(|e| format!("{}: {e}", exe.display()))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    if tx.send(v).is_err() {
                        break;
                    }
                }
            }
        });
        Ok(Link { child, stdin, rx, ready: false, time: None, title: String::new(), skin_ack: 0, error: None, next_skin_id: 0 })
    }

    pub fn send(&mut self, v: Value) {
        let _ = writeln!(self.stdin, "{v}");
        let _ = self.stdin.flush();
    }

    /// Take in everything the preview has said since the last call. Returns true when something changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(v) = self.rx.try_recv() {
            changed = true;
            match v["event"].as_str().unwrap_or("") {
                "ready" => self.ready = true,
                "loaded" => self.title = format!("{} [{}]", v["title"].as_str().unwrap_or(""), v["version"].as_str().unwrap_or("")),
                "time" => {
                    self.time = Some(Time { ms: v["ms"].as_f64().unwrap_or(0.0) as f32, start: v["start"].as_f64().unwrap_or(0.0) as f32, end: v["end"].as_f64().unwrap_or(0.0) as f32, playing: v["playing"].as_bool().unwrap_or(false) });
                }
                "skin" => {
                    if v["ok"].as_bool().unwrap_or(false) {
                        self.skin_ack = v["id"].as_u64().unwrap_or(0) as u32;
                    } else {
                        self.error = Some(format!("skin: {}", v["message"].as_str().unwrap_or("could not be applied")));
                    }
                }
                "error" => self.error = Some(v["message"].as_str().unwrap_or("").to_string()),
                _ => {}
            }
        }
        changed
    }

    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn map(&mut self, path: &Path, resume: Option<f32>) {
        let mut v = json!({ "cmd": "map", "path": path.to_string_lossy() });
        if let Some(r) = resume {
            v["resume"] = json!(r);
        }
        self.send(v);
    }

    pub fn skin(&mut self, path: &Path) -> u32 {
        self.next_skin_id += 1;
        self.send(json!({ "cmd": "skin", "path": path.to_string_lossy(), "id": self.next_skin_id }));
        self.next_skin_id
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        let _ = writeln!(self.stdin, "{}", json!({ "cmd": "quit" }));
        let _ = self.stdin.flush();
        // give it a moment to close its window, then make sure it is gone
        for _ in 0..20 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = self.child.kill();
    }
}

fn is_follow_point(name: &str) -> bool {
    let n = name.rsplit(['/', '\\']).next().unwrap_or(name).to_ascii_lowercase();
    n.starts_with("followpoint")
}

/// Put `AnimationFramerate` into the `[General]` section of a skin.ini, keeping everything else the skin says.
fn patch_ini(ini: &str, fps: f32) -> String {
    let line = format!("AnimationFramerate: {}", fps.round() as i64);
    let mut out: Vec<String> = Vec::new();
    let (mut in_general, mut done, mut seen_general) = (false, false, false);
    for l in ini.lines() {
        let t = l.trim();
        if t.starts_with('[') {
            if in_general && !done {
                out.push(line.clone());
                done = true;
            }
            in_general = t.eq_ignore_ascii_case("[general]");
            seen_general |= in_general;
        } else if in_general && t.to_ascii_lowercase().starts_with("animationframerate") {
            if !done {
                out.push(line.clone());
                done = true;
            }
            continue;
        }
        out.push(l.to_string());
    }
    if in_general && !done {
        out.push(line.clone());
        done = true;
    }
    if !seen_general {
        out.insert(0, "[General]".to_string());
        out.insert(1, line);
    } else if !done {
        out.push(line);
    }
    out.join("\n") + "\n"
}

fn png_bytes(w: usize, h: usize, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut buf, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().map_err(|e| e.to_string())?.write_image_data(rgba).map_err(|e| e.to_string())?;
    }
    Ok(buf)
}

/// A copy of the skin at `base` (an .osk or a folder) whose follow point is the animation `frames`, written to `out`.
pub fn build_skin(base: &Path, frames: &[Frame], fps: f32, out: &Path) -> Result<(), String> {
    use zip::write::SimpleFileOptions;
    let file = std::fs::File::create(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut zw = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut ini: Option<String> = None;

    let mut add = |zw: &mut zip::ZipWriter<std::fs::File>, name: &str, data: &[u8]| -> Result<(), String> {
        zw.start_file(name, opts).map_err(|e| e.to_string())?;
        zw.write_all(data).map_err(|e| e.to_string())
    };

    if base.is_dir() {
        let mut stack = vec![base.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).map_err(|e| e.to_string())?.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let name = p.strip_prefix(base).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                if is_follow_point(&name) {
                    continue;
                }
                let data = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
                if name.eq_ignore_ascii_case("skin.ini") {
                    ini = Some(String::from_utf8_lossy(&data).trim_start_matches('\u{feff}').to_string());
                } else {
                    add(&mut zw, &name, &data)?;
                }
            }
        }
    } else {
        let f = std::fs::File::open(base).map_err(|e| format!("{}: {e}", base.display()))?;
        let mut zr = zip::ZipArchive::new(f).map_err(|e| format!("{}: {e}", base.display()))?;
        for i in 0..zr.len() {
            let mut e = zr.by_index(i).map_err(|e| e.to_string())?;
            if e.is_dir() {
                continue;
            }
            let name = e.name().replace('\\', "/");
            if is_follow_point(&name) {
                continue;
            }
            let mut data = Vec::new();
            std::io::Read::read_to_end(&mut e, &mut data).map_err(|e| e.to_string())?;
            if name.eq_ignore_ascii_case("skin.ini") {
                ini = Some(String::from_utf8_lossy(&data).trim_start_matches('\u{feff}').to_string());
            } else {
                add(&mut zw, &name, &data)?;
            }
        }
    }

    let ini = patch_ini(&ini.unwrap_or_else(|| "[General]\nName: Skin Studio preview\nVersion: latest\n".to_string()), fps);
    add(&mut zw, "skin.ini", ini.as_bytes())?;
    for (i, f) in frames.iter().enumerate() {
        let data = match f {
            Some(img) => png_bytes(img.w, img.h, &img.to_straight_u8())?,
            None => png_bytes(1, 1, &[0, 0, 0, 0])?,
        };
        add(&mut zw, &format!("followpoint-{i}.png"), &data)?;
    }
    zw.finish().map_err(|e| e.to_string())?;
    Ok(())
}
