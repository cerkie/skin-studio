//! The built-in source images, drawn procedurally. Each is a white shape whose alpha is the exact area coverage
//! (4x4 samples per pixel, box-averaged: no ringing), rendered natively at 1x or 2x so @2x files are crisp
//! instead of upscaled.
use std::path::Path;

/// A straight-alpha float image, the input to every animation.
#[derive(Clone, Debug)]
pub struct Sprite {
    pub w: usize,
    pub h: usize,
    /// Straight RGBA, 0..1.
    pub px: Vec<[f32; 4]>,
}

pub const NAMES: [&str; 11] = [
    "Arrow", "Chevron (thin)", "Chevron (bold)", "Double chevron", "Triangle", "Arrow with shaft", "Dot", "Diamond", "Ring",
    "Spark (glow dot)", "Tick (bar)",
];

/// The sources the editor offers. (`NAMES` also holds the older arrow shapes and the spark, which are still drawn for old
/// settings files and recognised when importing.)
pub const OFFERED: [&str; 8] = ["Chevron (thin)", "Chevron (bold)", "Double chevron", "Triangle", "Dot", "Diamond", "Ring", "Tick (bar)"];

/// Default canvas (1x design px) for each source, as in the Python tool.
pub fn default_canvas(name: &str) -> (usize, usize) {
    match name {
        "Double chevron" | "Arrow with shaft" => (110, 50),
        "Dot" | "Diamond" => (64, 32),
        "Ring" => (64, 64),
        "Spark (glow dot)" => (96, 64),
        "Tick (bar)" => (48, 48),
        _ => (100, 50),
    }
}

fn inside_poly(poly: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let ((xi, yi), (xj, yj)) = (poly[i], poly[j]);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn in_rect(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
    x >= x0 && x <= x1 && y >= y0 && y <= y1
}

/// Distance from (x, y) to the segment a-b.
fn seg_dist(x: f32, y: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((x - a.0) * dx + (y - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    ((x - a.0 - t * dx).powi(2) + (y - a.1 - t * dy).powi(2)).sqrt()
}

/// Coverage of a shape defined in 1x design coordinates, sampled 4x4 per output pixel.
fn raster(w: usize, h: usize, scale: usize, shape: impl Fn(f32, f32) -> bool) -> Sprite {
    const SS: usize = 4;
    let (ow, oh) = (w * scale, h * scale);
    let step = 1.0 / (SS * scale) as f32;
    let mut px = Vec::with_capacity(ow * oh);
    for oy in 0..oh {
        for ox in 0..ow {
            let mut hit = 0;
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (ox * SS + sx) as f32 * step + step * 0.5;
                    let y = (oy * SS + sy) as f32 * step + step * 0.5;
                    if shape(x, y) {
                        hit += 1;
                    }
                }
            }
            px.push([1.0, 1.0, 1.0, hit as f32 / (SS * SS) as f32]);
        }
    }
    Sprite { w: ow, h: oh, px }
}

pub fn preset(name: &str, scale: usize) -> Option<Sprite> {
    let chev = |dx: f32| vec![(1.0 + dx, 0.0), (5.0 + dx, 0.0), (14.0 + dx, 11.5), (5.0 + dx, 23.0), (1.0 + dx, 23.0), (10.0 + dx, 11.5)];
    Some(match name {
        "Arrow" => {
            let p = [(1.0, 0.0), (6.0, 0.0), (15.0, 11.5), (6.0, 23.0), (1.0, 23.0), (10.0, 11.5)];
            raster(16, 23, scale, |x, y| inside_poly(&p, x, y))
        }
        "Chevron (thin)" => {
            let (a, m, b) = ((3.0, 2.0), (12.0, 11.5), (3.0, 21.0));
            raster(16, 23, scale, |x, y| seg_dist(x, y, a, m) <= 1.0 || seg_dist(x, y, m, b) <= 1.0)
        }
        "Chevron (bold)" => {
            let p = [(1.0, 0.0), (9.0, 0.0), (19.0, 13.0), (9.0, 26.0), (1.0, 26.0), (11.0, 13.0)];
            raster(20, 26, scale, |x, y| inside_poly(&p, x, y))
        }
        "Double chevron" => {
            let (p0, p1) = (chev(0.0), chev(9.0));
            raster(26, 23, scale, |x, y| inside_poly(&p0, x, y) || inside_poly(&p1, x, y))
        }
        "Triangle" => {
            let p = [(2.0, 1.0), (2.0, 22.0), (17.0, 11.5)];
            raster(18, 23, scale, |x, y| inside_poly(&p, x, y))
        }
        "Arrow with shaft" => {
            let p = [(18.0, 0.0), (23.0, 0.0), (33.0, 11.5), (23.0, 23.0), (18.0, 23.0), (27.0, 11.5)];
            raster(34, 23, scale, |x, y| in_rect(x, y, 0.0, 10.0, 24.0, 13.0) || inside_poly(&p, x, y))
        }
        "Dot" => raster(10, 10, scale, |x, y| ((x - 5.0).powi(2) + (y - 5.0).powi(2)).sqrt() <= 4.0),
        "Diamond" => {
            let p = [(8.0, 0.0), (16.0, 8.0), (8.0, 16.0), (0.0, 8.0)];
            raster(16, 16, scale, |x, y| inside_poly(&p, x, y))
        }
        "Ring" => raster(26, 26, scale, |x, y| {
            let d = ((x - 13.0).powi(2) + (y - 13.0).powi(2)).sqrt();
            d <= 11.0 && d >= 9.0
        }),
        "Spark (glow dot)" => {
            let n = 64 * scale;
            let mut px = Vec::with_capacity(n * n);
            for oy in 0..n {
                for ox in 0..n {
                    let (x, y) = ((ox as f32 + 0.5) / scale as f32 - 32.0, (oy as f32 + 0.5) / scale as f32 - 32.0);
                    let r = (x * x + y * y).sqrt();
                    let a = ((-(r / 4.5).powi(2)).exp() + 0.55 * (-(r / 10.0).powi(2)).exp()).clamp(0.0, 1.0);
                    px.push([1.0, 1.0, 1.0, a]);
                }
            }
            Sprite { w: n, h: n, px }
        }
        "Tick (bar)" => raster(6, 24, scale, |x, y| {
            // rounded bar: x in [1, 5], y in [0, 24], end caps are half circles of radius 2
            if !in_rect(x, y, 1.0, 0.0, 5.0, 24.0) {
                return false;
            }
            if y < 2.0 {
                (x - 3.0).powi(2) + (y - 2.0).powi(2) <= 4.0
            } else if y > 22.0 {
                (x - 3.0).powi(2) + (y - 22.0).powi(2) <= 4.0
            } else {
                true
            }
        }),
        _ => return None,
    })
}

/// Load a PNG (or fall back to a transparent 1x1 on error) as a sprite.
pub fn load_png(path: &Path) -> Result<Sprite, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; r.output_buffer_size().ok_or("image too large")?];
    let info = r.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let n = (info.width * info.height) as usize;
    let px = match info.color_type {
        png::ColorType::Rgba => buf[..n * 4].chunks(4).map(|c| [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0]).collect(),
        png::ColorType::Rgb => buf[..n * 3].chunks(3).map(|c| [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, 1.0]).collect(),
        png::ColorType::GrayscaleAlpha => buf[..n * 2].chunks(2).map(|c| [c[0] as f32 / 255.0, c[0] as f32 / 255.0, c[0] as f32 / 255.0, c[1] as f32 / 255.0]).collect(),
        png::ColorType::Grayscale => buf[..n].iter().map(|&g| [g as f32 / 255.0, g as f32 / 255.0, g as f32 / 255.0, 1.0]).collect(),
        other => return Err(format!("unsupported color type {other:?}")),
    };
    Ok(Sprite { w: info.width as usize, h: info.height as usize, px })
}
