//! Renders every source image and every effect preset as a contact sheet of frames, to check them by eye.
//! usage: audit OUT_DIR
use engine::gen::build_frames;
use engine::presets;
use engine::settings::Settings;
use serde_json::{json, Value};

const CELLS: usize = 8;

fn row(s: &Value, scale: usize) -> (usize, usize, Vec<u8>) {
    let parsed = Settings::from_json(s);
    let frames = build_frames(&parsed, 1, &Default::default());
    let drawn: Vec<usize> = (0..frames.len()).filter(|&i| frames[i].is_some()).collect();
    let (cw, ch) = (parsed.width * scale, parsed.height * scale);
    let mut out = vec![0u8; cw * CELLS * ch * 4];
    for px in out.chunks_mut(4) {
        px.copy_from_slice(&[27, 26, 32, 255]);
    }
    for c in 0..CELLS {
        let Some(&i) = drawn.get(c * drawn.len().saturating_sub(1) / (CELLS - 1).max(1)) else { continue };
        let f = frames[i].as_ref().unwrap();
        let (ox, oy) = ((parsed.width.saturating_sub(f.w)) / 2 * scale, (parsed.height.saturating_sub(f.h)) / 2 * scale);
        for y in 0..f.h * scale {
            for x in 0..f.w * scale {
                let q = f.px[(y / scale) * f.w + x / scale];
                let (tx, ty) = (c * cw + ox + x, oy + y);
                if tx >= cw * CELLS || ty >= ch {
                    continue;
                }
                let o = (ty * cw * CELLS + tx) * 4;
                for k in 0..3 {
                    out[o + k] = (q[k] * 255.0 + out[o + k] as f32 * (1.0 - q[3])).min(255.0) as u8;
                }
            }
        }
        // cell boundary
        for y in 0..ch {
            let o = (y * cw * CELLS + c * cw) * 4;
            out[o..o + 3].copy_from_slice(&[70, 68, 80]);
        }
    }
    (cw * CELLS, ch, out)
}

fn save(path: &std::path::Path, rows: Vec<(usize, usize, Vec<u8>)>) {
    let w = rows.iter().map(|r| r.0).max().unwrap_or(1);
    let h: usize = rows.iter().map(|r| r.1 + 4).sum();
    let mut img = vec![0u8; w * h * 4];
    for px in img.chunks_mut(4) {
        px.copy_from_slice(&[10, 10, 14, 255]);
    }
    let mut y0 = 0;
    for (rw, rh, data) in rows {
        for y in 0..rh {
            img[((y0 + y) * w) * 4..((y0 + y) * w + rw) * 4].copy_from_slice(&data[y * rw * 4..(y + 1) * rw * 4]);
        }
        y0 += rh + 4;
    }
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&img).unwrap();
}

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut rows = Vec::new();
    for (i, name) in presets::source_names().iter().enumerate() {
        if name == "Custom image..." {
            continue;
        }
        let mut s = presets::base_settings();
        s["fps"] = json!(60);
        s["props"]["x"] = json!([[0, 0.2, "linear"], ["end", 0.8, "linear"]]);
        engine_set_source(&mut s, name);
        println!("source {i}: {name} canvas {}x{}", s["width"], s["height"]);
        rows.push(row(&s, 2));
    }
    save(&dir.join("sources.png"), rows);
    let names = presets::effect_names();
    for (page, chunk) in names.chunks(8).enumerate() {
        let mut rows = Vec::new();
        for (i, name) in chunk.iter().enumerate() {
            let mut s = presets::base_settings();
            s["fps"] = json!(60);
            presets::apply_effect(&mut s, name);
            println!("effect {}: {name} canvas {}x{}", page * 8 + i, s["width"], s["height"]);
            rows.push(row(&s, 2));
        }
        save(&dir.join(format!("effects{page}.png")), rows);
    }
}

fn engine_set_source(s: &mut Value, name: &str) {
    s["preset"] = json!(name);
    if name == engine::settings::SEAMLESS {
        s["width"] = json!(128);
        s["height"] = json!(32);
    } else {
        let (w, h) = engine::sources::default_canvas(name);
        s["width"] = json!(w);
        s["height"] = json!(h);
    }
}
