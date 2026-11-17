//! Loads a folder of followpoint-N.png frames into one premultiplied-alpha atlas.
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

#[derive(Clone, Copy, Debug)]
pub struct FrameInfo {
    pub w: u32,
    pub h: u32,
    /// Top-left of this frame inside the atlas; `None` for blank (1x1) frames, which draw nothing.
    pub origin: Option<(u32, u32)>,
}

pub struct Atlas {
    pub width: u32,
    pub height: u32,
    /// RGBA8, premultiplied alpha, so bilinear sampling does not fringe.
    pub rgba: Vec<u8>,
    pub frames: Vec<FrameInfo>,
    pub fps: f32,
}

fn decode(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("image too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (w, h) = (info.width, info.height);
    let px = (w * h) as usize;
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf[..px * 4].to_vec(),
        png::ColorType::Rgb => buf[..px * 3].chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf[..px * 2].chunks(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Grayscale => buf[..px].iter().flat_map(|&g| [g, g, g, 255]).collect(),
        other => return Err(format!("unsupported PNG color type {other:?}")),
    };
    Ok((w, h, rgba))
}

pub fn load_dir(dir: &Path, fps: f32) -> Result<Atlas, String> {
    let mut raw = Vec::new();
    while let Ok(f) = decode(&dir.join(format!("followpoint-{}.png", raw.len()))) {
        raw.push(f);
    }
    if raw.is_empty() {
        return Err(format!("no followpoint-0.png in {}", dir.display()));
    }
    let drawn: Vec<usize> = (0..raw.len()).filter(|&i| raw[i].0 > 1 || raw[i].1 > 1).collect();
    let cw = drawn.iter().map(|&i| raw[i].0).max().unwrap_or(1);
    let ch = drawn.iter().map(|&i| raw[i].1).max().unwrap_or(1);
    let cols = (8192 / cw).max(1).min(drawn.len().max(1) as u32);
    let rows = (drawn.len() as u32).div_ceil(cols).max(1);
    let (width, height) = (cols * cw, rows * ch);
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    let mut frames = Vec::with_capacity(raw.len());
    let mut slot = 0u32;
    for (w, h, px) in &raw {
        if *w <= 1 && *h <= 1 {
            frames.push(FrameInfo { w: *w, h: *h, origin: None });
            continue;
        }
        let (ox, oy) = ((slot % cols) * cw, (slot / cols) * ch);
        slot += 1;
        for y in 0..*h {
            for x in 0..*w {
                let s = ((y * w + x) * 4) as usize;
                let d = (((oy + y) * width + ox + x) * 4) as usize;
                let a = px[s + 3] as u32;
                for c in 0..3 {
                    rgba[d + c] = ((px[s + c] as u32 * a + 127) / 255) as u8; // premultiply
                }
                rgba[d + 3] = a as u8;
            }
        }
        frames.push(FrameInfo { w: *w, h: *h, origin: Some((ox, oy)) });
    }
    Ok(Atlas { width, height, rgba, frames, fps })
}

impl Atlas {
    /// Write this atlas in a simple binary format so another process (the preview window) can load it.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        let mut w = std::io::BufWriter::new(File::create(path)?);
        w.write_all(b"FPAT")?;
        w.write_all(&1u32.to_le_bytes())?;
        w.write_all(&self.fps.to_le_bytes())?;
        for v in [self.width, self.height, self.frames.len() as u32] {
            w.write_all(&v.to_le_bytes())?;
        }
        for f in &self.frames {
            let (ox, oy) = f.origin.map(|(x, y)| (x as i32, y as i32)).unwrap_or((-1, -1));
            for v in [f.w as i32, f.h as i32, ox, oy] {
                w.write_all(&v.to_le_bytes())?;
            }
        }
        w.write_all(&(self.rgba.len() as u64).to_le_bytes())?;
        w.write_all(&self.rgba)?;
        w.flush()
    }

    pub fn load(path: &Path) -> Result<Atlas, String> {
        use std::io::Read;
        let mut r = BufReader::new(File::open(path).map_err(|e| e.to_string())?);
        let mut b4 = [0u8; 4];
        let mut b8 = [0u8; 8];
        r.read_exact(&mut b4).map_err(|e| e.to_string())?;
        if &b4 != b"FPAT" {
            return Err("not an atlas file".into());
        }
        let rd32 = |r: &mut BufReader<File>| -> Result<[u8; 4], String> {
            let mut b = [0u8; 4];
            r.read_exact(&mut b).map_err(|e| e.to_string())?;
            Ok(b)
        };
        let _version = u32::from_le_bytes(rd32(&mut r)?);
        let fps = f32::from_le_bytes(rd32(&mut r)?);
        let width = u32::from_le_bytes(rd32(&mut r)?);
        let height = u32::from_le_bytes(rd32(&mut r)?);
        let n = u32::from_le_bytes(rd32(&mut r)?) as usize;
        let mut frames = Vec::with_capacity(n);
        for _ in 0..n {
            let w = i32::from_le_bytes(rd32(&mut r)?);
            let h = i32::from_le_bytes(rd32(&mut r)?);
            let ox = i32::from_le_bytes(rd32(&mut r)?);
            let oy = i32::from_le_bytes(rd32(&mut r)?);
            frames.push(FrameInfo { w: w as u32, h: h as u32, origin: if ox < 0 { None } else { Some((ox as u32, oy as u32)) } });
        }
        r.read_exact(&mut b8).map_err(|e| e.to_string())?;
        let mut rgba = vec![0u8; u64::from_le_bytes(b8) as usize];
        r.read_exact(&mut rgba).map_err(|e| e.to_string())?;
        Ok(Atlas { width, height, rgba, frames, fps })
    }
}

impl Atlas {
    /// Pack generated frames (`None` = blank) into one premultiplied atlas.
    pub fn from_frames(frames: &[Option<crate::img::Img>], fps: f32) -> Atlas {
        let drawn: Vec<&crate::img::Img> = frames.iter().flatten().collect();
        let cw = drawn.iter().map(|f| f.w).max().unwrap_or(1) as u32;
        let ch = drawn.iter().map(|f| f.h).max().unwrap_or(1) as u32;
        let cols = (8192 / cw).max(1).min(drawn.len().max(1) as u32);
        let rows = (drawn.len() as u32).div_ceil(cols).max(1);
        let (width, height) = (cols * cw, rows * ch);
        let mut rgba = vec![0u8; (width * height * 4) as usize];
        let mut infos = Vec::with_capacity(frames.len());
        let mut slot = 0u32;
        for f in frames {
            let Some(im) = f else {
                infos.push(FrameInfo { w: 1, h: 1, origin: None });
                continue;
            };
            let (ox, oy) = ((slot % cols) * cw, (slot / cols) * ch);
            slot += 1;
            for y in 0..im.h {
                for x in 0..im.w {
                    let p = im.px[y * im.w + x];
                    let d = (((oy as usize + y) * width as usize + ox as usize + x) * 4) as usize;
                    for c in 0..4 {
                        rgba[d + c] = (p[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                    }
                }
            }
            infos.push(FrameInfo { w: im.w as u32, h: im.h as u32, origin: Some((ox, oy)) });
        }
        Atlas { width, height, rgba, frames: infos, fps }
    }
}
