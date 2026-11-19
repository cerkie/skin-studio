//! Editing a picture: resize without ringing, flips and turns, crop and trim, colour, blur and sharpen, debanding.
//!
//! Pictures are the engine's [`Img`]: floating point, premultiplied. Every operation works on those and gives a new one, so an edit is a
//! list of [`Op`]s applied in order to the picture that was loaded ([`apply_all`]) and nothing is lost along the way until it is saved.
use std::path::Path;

use crate::deband;
use crate::restore;
use crate::img::{self, Img};

/// How a picture is resampled when it is resized. Making a picture smaller always averages the area each new pixel covers (nothing
/// can ring or leave a halo that way); the choice matters when it is made larger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resample {
    /// Bilinear: smooth, never overshoots.
    Smooth,
    /// Catmull-Rom, kept inside the range of the four pixels around each new one: sharper than smooth, and still without ringing.
    Crisp,
    /// The nearest pixel: for pixel art.
    Pixels,
}

impl Resample {
    pub const ALL: [Resample; 3] = [Resample::Smooth, Resample::Crisp, Resample::Pixels];
    pub fn name(self) -> &'static str {
        match self {
            Resample::Smooth => "Smooth",
            Resample::Crisp => "Crisp",
            Resample::Pixels => "Pixels",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Resize { w: usize, h: usize, mode: Resample },
    Flip { horizontal: bool },
    /// Turn clockwise by this many quarter turns.
    Rotate { quarter_turns: u8 },
    Crop { x: usize, y: usize, w: usize, h: usize },
    /// Cut away the empty border: every pixel with an alpha of at most `threshold` (0 to 1) counts as empty.
    Trim { threshold: f32 },
    /// Brightness and contrast (-1 to 1), saturation (-1 grey to 1 doubled) and hue (degrees).
    Adjust { brightness: f32, contrast: f32, saturation: f32, hue: f32 },
    /// Mix the colour towards `rgb` (0 to 1 each) by `strength`.
    Tint { rgb: [f32; 3], strength: f32 },
    Opacity { factor: f32 },
    Invert,
    Blur { sigma: f32 },
    /// Unsharp mask, kept inside the range of the pixels around each one so that it leaves no halos.
    Sharpen { amount: f32, sigma: f32 },
    /// Halo removal (`dehalo_alpha` / `fine_dehalo` of vs-jetpack).
    Dehalo(restore::Dehalo),
    /// Noise removal (non-local means or bilateral).
    Denoise(restore::Denoise),
    /// Anti-aliasing of edges.
    AntiAlias(restore::AntiAlias),
    /// Add an empty (or filled) border: `amount` pixels on every side.
    Expand { amount: usize, fill: Fill },
    /// Remove banding from gradients. `radius`: how far away (in pixels) it looks for pixels to average with; `threshold`: how different they may
    /// be (in 8-bit levels) to still count as the same band; `iterations`: how many times it goes over the picture; `grain`: the spread of the
    /// noise that fills the steps in (8-bit levels), of the given `kind`, grey when `mono`; `room`: give a glow that the picture cuts off
    /// the room to fade out.
    Deband { radius: f32, threshold: f32, iterations: usize, grain: f32, kind: Grain, mono: bool, room: bool, luma: restore::LumaScale, limit: f32, elast: f32, narrowing: bool, prefilter: f32, protect: restore::Mask, adv: deband::Advanced },
}

/// What a border that is added is filled with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fill {
    /// Empty (transparent).
    Clear,
    /// The pixel at the edge, repeated.
    Edge,
    /// The pixel at the edge, fading out smoothly to nothing over the border.
    Fade,
}

impl Fill {
    pub const ALL: [Fill; 3] = [Fill::Clear, Fill::Edge, Fill::Fade];
    pub fn name(self) -> &'static str {
        match self {
            Fill::Clear => "Empty",
            Fill::Edge => "Repeat edge",
            Fill::Fade => "Fade out",
        }
    }
}

pub use crate::deband::{Advanced, Grain, Mode as DebandMode};

impl Op {
    /// A deband made from the single slider of the first version: from a light touch (0) to a heavy one (1), with `grain` levels of uniform noise.
    pub fn deband(strength: f32, grain: f32) -> Op {
        let s = strength.clamp(0.0, 1.0);
        Op::Deband { radius: 8.0 + 80.0 * s, threshold: 2.0 + 8.0 * s, iterations: 1 + (5.0 * s).round() as usize, grain, kind: if grain > 0.0 { Grain::Uniform } else { Grain::None }, mono: false, room: true, luma: restore::LumaScale::off(), limit: 0.0, elast: 3.0, narrowing: false, prefilter: 0.0, protect: restore::Mask { invert: true, ..restore::Mask::off() }, adv: deband::Advanced::default() }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Op::Resize { .. } => "Resize",
            Op::Flip { .. } => "Flip",
            Op::Rotate { .. } => "Rotate",
            Op::Crop { .. } => "Crop",
            Op::Trim { .. } => "Trim empty border",
            Op::Adjust { .. } => "Colour",
            Op::Tint { .. } => "Tint",
            Op::Opacity { .. } => "Opacity",
            Op::Invert => "Invert",
            Op::Blur { .. } => "Blur",
            Op::Sharpen { .. } => "Sharpen",
            Op::Deband { .. } => "Deband",
            Op::Expand { .. } => "Expand border",
            Op::Dehalo(_) => "Dehalo",
            Op::Denoise(_) => "Denoise",
            Op::AntiAlias(_) => "Anti-alias",
        }
    }
}

/// The picture with every edit applied in order.
pub fn apply_all(src: &Img, ops: &[Op]) -> Img {
    ops.iter().fold(src.clone(), |im, op| apply(&im, op))
}

pub fn apply(src: &Img, op: &Op) -> Img {
    match op {
        Op::Resize { w, h, mode } => resize(src, *w, *h, *mode),
        Op::Flip { horizontal } => flip(src, *horizontal),
        Op::Rotate { quarter_turns } => rotate(src, *quarter_turns),
        Op::Crop { x, y, w, h } => crop(src, *x, *y, *w, *h),
        Op::Trim { threshold } => trim(src, *threshold),
        Op::Adjust { brightness, contrast, saturation, hue } => adjust(src, *brightness, *contrast, *saturation, *hue),
        Op::Tint { rgb, strength } => map_straight(src, |c, a| (std::array::from_fn(|k| c[k] + (rgb[k] - c[k]) * strength.clamp(0.0, 1.0)), a)),
        Op::Opacity { factor } => {
            let mut out = src.clone();
            out.scale_alpha(factor.clamp(0.0, 1.0));
            out
        }
        Op::Invert => map_straight(src, |c, a| ([1.0 - c[0], 1.0 - c[1], 1.0 - c[2]], a)),
        Op::Blur { sigma } => img::blur(src, *sigma),
        Op::Sharpen { amount, sigma } => sharpen(src, *amount, *sigma),
        Op::Expand { amount, fill } => expand(src, *amount, *fill),
        Op::Dehalo(o) => restore::dehalo(src, o),
        Op::Denoise(o) => restore::denoise(src, o),
        Op::AntiAlias(o) => restore::antialias(src, o),
        Op::Deband { radius, threshold, iterations, grain, kind, mono, room, luma, limit, elast, narrowing, prefilter, protect, adv } => {
            let p = deband::Params { radius: *radius, thr_rgb: *threshold, thr_a: *threshold, iterations: (*iterations).clamp(1, 12), grain: *grain, kind: *kind, mono: *mono, quantize: false, limit: *limit, elast: *elast, narrowing: *narrowing, adv: *adv };
            let base = if *room { room_for_the_fade(src) } else { src.clone() };
            if protect.show {
                return restore::mask_image(&base, protect);
            }
            // pfdeband: the banding is looked for in a slightly blurred copy (noise would hide it), and what the blur took away is put back
            let work = if *prefilter > 0.05 { img::blur(&base, *prefilter) } else { base.clone() };
            let mut out = deband::deband(&work, &p);
            if *prefilter > 0.05 {
                for ((o, b), w) in out.px.iter_mut().zip(&base.px).zip(&work.px) {
                    let a = (o[3] + b[3] - w[3]).clamp(0.0, 1.0);
                    *o = [(o[0] + b[0] - w[0]).clamp(0.0, a), (o[1] + b[1] - w[1]).clamp(0.0, a), (o[2] + b[2] - w[2]).clamp(0.0, a), a];
                }
            }
            if luma.scaling == 0.0 && !protect.enabled {
                out
            } else {
                // the dark parts get all of it and the light ones hardly any; the mask keeps it off edges and details
                let lw = restore::luma_weights(&base, luma);
                let mw = restore::mask_weights(&base, protect);
                let w: Vec<f32> = lw.iter().zip(&mw).map(|(a, b)| a * b).collect();
                restore::merge_weighted(&base, &out, &w)
            }
        }
    }
}

/// `amount` pixels added on every side.
fn expand(src: &Img, amount: usize, fill: Fill) -> Img {
    let (w, h) = (src.w, src.h);
    if amount == 0 || w == 0 || h == 0 {
        return src.clone();
    }
    let (nw, nh) = (w + 2 * amount, h + 2 * amount);
    let mut out = Img::new(nw, nh);
    for y in 0..nh {
        for x in 0..nw {
            let (ix, iy) = (x as i64 - amount as i64, y as i64 - amount as i64);
            let inside = ix >= 0 && iy >= 0 && ix < w as i64 && iy < h as i64;
            if inside {
                out.px[y * nw + x] = src.px[iy as usize * w + ix as usize];
                continue;
            }
            if fill == Fill::Clear {
                continue;
            }
            let (cx, cy) = (ix.clamp(0, w as i64 - 1), iy.clamp(0, h as i64 - 1));
            let q = src.px[cy as usize * w + cx as usize];
            let k = if fill == Fill::Edge {
                1.0
            } else {
                let d = (((ix - cx) * (ix - cx) + (iy - cy) * (iy - cy)) as f32).sqrt();
                0.5 * (1.0 + (std::f32::consts::PI * (d / (amount as f32 + 1.0)).min(1.0)).cos())
            };
            out.px[y * nw + x] = [q[0] * k, q[1] * k, q[2] * k, q[3] * k];
        }
    }
    out
}

/// A soft glow that is cut off by the edge of its picture (something is still visible at the border) cannot be smoothed to nothing: the
/// cut stays as a hard square. Such a picture is given room on all four sides (so it stays centred) and what is at the border fades out over it.
fn room_for_the_fade(src: &Img) -> Img {
    let (w, h) = (src.w, src.h);
    if w == 0 || h == 0 {
        return src.clone();
    }
    let (mut edge, mut sum, mut n) = (0.0f32, 0.0f32, 0usize);
    let mut see = |a: f32| {
        edge = edge.max(a);
        sum += a;
        n += 1;
    };
    for x in 0..w {
        see(src.px[x][3]);
        see(src.px[(h - 1) * w + x][3]);
    }
    for y in 0..h {
        see(src.px[y * w][3]);
        see(src.px[y * w + w - 1][3]);
    }
    // a picture that is solid at its border (a photo, a panel) is not a glow and keeps its size
    if edge * 255.0 < 1.5 || sum / n as f32 > 0.9 {
        return src.clone();
    }
    let pad = ((w.max(h) as f32 * 0.15).round() as usize).clamp(16, 96);
    let (nw, nh) = (w + 2 * pad, h + 2 * pad);
    let mut out = Img::new(nw, nh);
    for y in 0..nh {
        for x in 0..nw {
            let (ix, iy) = (x as i64 - pad as i64, y as i64 - pad as i64);
            let (cx, cy) = (ix.clamp(0, w as i64 - 1), iy.clamp(0, h as i64 - 1));
            let q = src.px[cy as usize * w + cx as usize];
            let d = (((ix - cx) * (ix - cx) + (iy - cy) * (iy - cy)) as f32).sqrt();
            let k = if d == 0.0 { 1.0 } else { 0.5 * (1.0 + (std::f32::consts::PI * (d / pad as f32).min(1.0)).cos()) };
            out.px[y * nw + x] = [q[0] * k, q[1] * k, q[2] * k, q[3] * k];
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------- geometry
/// Resample to `w` x `h` pixels (see [`Resample`]).
pub fn resize(src: &Img, w: usize, h: usize, mode: Resample) -> Img {
    let (w, h) = (w.clamp(1, 16384), h.clamp(1, 16384));
    if (w, h) == (src.w, src.h) || src.w == 0 || src.h == 0 {
        return src.clone();
    }
    if mode == Resample::Pixels {
        let mut out = Img::new(w, h);
        for y in 0..h {
            let sy = (y * src.h / h).min(src.h - 1);
            for x in 0..w {
                out.px[y * w + x] = src.px[sy * src.w + (x * src.w / w).min(src.w - 1)];
            }
        }
        return out;
    }
    // an axis that gets smaller is averaged exactly, then an axis that gets larger is interpolated
    let mut cur = src.clone();
    if w < cur.w {
        cur = img::resize_area(&cur, w, cur.h);
    }
    if h < cur.h {
        cur = img::resize_area(&cur, cur.w, h);
    }
    if (cur.w, cur.h) == (w, h) {
        return cur;
    }
    upscale(&cur, w, h, mode)
}

fn clamp_px(img: &Img, x: i64, y: i64) -> [f32; 4] {
    img.px[(y.clamp(0, img.h as i64 - 1) as usize) * img.w + x.clamp(0, img.w as i64 - 1) as usize]
}

/// Interpolate onto a larger grid (an axis that does not grow is passed through). Edges repeat the outermost pixel.
fn upscale(src: &Img, w: usize, h: usize, mode: Resample) -> Img {
    let (kx, ky) = (src.w as f32 / w as f32, src.h as f32 / h as f32);
    let mut out = Img::new(w, h);
    for y in 0..h {
        let v = (y as f32 + 0.5) * ky - 0.5;
        let (iy, ty) = (v.floor(), v - v.floor());
        for x in 0..w {
            let u = (x as f32 + 0.5) * kx - 0.5;
            let (ix, tx) = (u.floor(), u - u.floor());
            let (ix, iy_) = (ix as i64, iy as i64);
            out.px[y * w + x] = match mode {
                Resample::Crisp => {
                    let (wx, wy) = (catmull(tx), catmull(ty));
                    let mut acc = [0.0f32; 4];
                    for (j, wyj) in wy.iter().enumerate() {
                        for (i, wxi) in wx.iter().enumerate() {
                            let p = clamp_px(src, ix + i as i64 - 1, iy_ + j as i64 - 1);
                            for c in 0..4 {
                                acc[c] += p[c] * wxi * wyj;
                            }
                        }
                    }
                    // never beyond the four pixels the new one lies between: that is what rings are made of
                    let near = [clamp_px(src, ix, iy_), clamp_px(src, ix + 1, iy_), clamp_px(src, ix, iy_ + 1), clamp_px(src, ix + 1, iy_ + 1)];
                    for c in 0..4 {
                        let lo = near.iter().fold(f32::MAX, |m, p| m.min(p[c]));
                        let hi = near.iter().fold(f32::MIN, |m, p| m.max(p[c]));
                        acc[c] = acc[c].clamp(lo, hi);
                    }
                    acc
                }
                _ => {
                    let (a, b, c, d) = (clamp_px(src, ix, iy_), clamp_px(src, ix + 1, iy_), clamp_px(src, ix, iy_ + 1), clamp_px(src, ix + 1, iy_ + 1));
                    std::array::from_fn(|k| (a[k] * (1.0 - tx) + b[k] * tx) * (1.0 - ty) + (c[k] * (1.0 - tx) + d[k] * tx) * ty)
                }
            };
        }
    }
    out
}

fn catmull(t: f32) -> [f32; 4] {
    let (t2, t3) = (t * t, t * t * t);
    [0.5 * (-t3 + 2.0 * t2 - t), 0.5 * (3.0 * t3 - 5.0 * t2 + 2.0), 0.5 * (-3.0 * t3 + 4.0 * t2 + t), 0.5 * (t3 - t2)]
}

pub fn flip(src: &Img, horizontal: bool) -> Img {
    let mut out = Img::new(src.w, src.h);
    for y in 0..src.h {
        for x in 0..src.w {
            let (sx, sy) = if horizontal { (src.w - 1 - x, y) } else { (x, src.h - 1 - y) };
            out.px[y * src.w + x] = src.px[sy * src.w + sx];
        }
    }
    out
}

pub fn rotate(src: &Img, quarter_turns: u8) -> Img {
    match quarter_turns % 4 {
        0 => src.clone(),
        1 => {
            // clockwise: the left column becomes the top row
            let mut out = Img::new(src.h, src.w);
            for y in 0..src.h {
                for x in 0..src.w {
                    out.px[x * src.h + (src.h - 1 - y)] = src.px[y * src.w + x];
                }
            }
            out
        }
        2 => flip(&flip(src, true), false),
        _ => {
            let mut out = Img::new(src.h, src.w);
            for y in 0..src.h {
                for x in 0..src.w {
                    out.px[(src.w - 1 - x) * src.h + y] = src.px[y * src.w + x];
                }
            }
            out
        }
    }
}

pub fn crop(src: &Img, x: usize, y: usize, w: usize, h: usize) -> Img {
    let x = x.min(src.w.saturating_sub(1));
    let y = y.min(src.h.saturating_sub(1));
    let w = w.clamp(1, src.w - x);
    let h = h.clamp(1, src.h - y);
    let mut out = Img::new(w, h);
    for row in 0..h {
        out.px[row * w..(row + 1) * w].copy_from_slice(&src.px[(y + row) * src.w + x..(y + row) * src.w + x + w]);
    }
    out
}

/// The box around everything that is not empty: `(x, y, w, h)`, or `None` for a picture with nothing in it.
pub fn content_box(src: &Img, threshold: f32) -> Option<(usize, usize, usize, usize)> {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for y in 0..src.h {
        for x in 0..src.w {
            if src.px[y * src.w + x][3] > threshold {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    (x0 != usize::MAX).then_some((x0, y0, x1 - x0 + 1, y1 - y0 + 1))
}

pub fn trim(src: &Img, threshold: f32) -> Img {
    match content_box(src, threshold) {
        Some((x, y, w, h)) => crop(src, x, y, w, h),
        None => Img::new(1, 1),
    }
}

// ---------------------------------------------------------------------------------------------------------------- colour
/// Change the colour of every pixel on its straight (not premultiplied) colour; `f` gets the colour and the alpha.
fn map_straight(src: &Img, f: impl Fn([f32; 3], f32) -> ([f32; 3], f32)) -> Img {
    let mut out = src.clone();
    for p in &mut out.px {
        let a = p[3];
        let c = if a > 1e-6 { [p[0] / a, p[1] / a, p[2] / a] } else { [0.0; 3] };
        let (c, a) = f(c, a);
        let a = a.clamp(0.0, 1.0);
        *p = [c[0].clamp(0.0, 1.0) * a, c[1].clamp(0.0, 1.0) * a, c[2].clamp(0.0, 1.0) * a, a];
    }
    out
}

pub fn adjust(src: &Img, brightness: f32, contrast: f32, saturation: f32, hue: f32) -> Img {
    if brightness == 0.0 && contrast == 0.0 && saturation == 0.0 && hue.rem_euclid(360.0) == 0.0 {
        return src.clone();
    }
    // a rotation of the colour around the grey axis (1, 1, 1): turns of 120 degrees swap the channels, and turns add up exactly
    let (s, c) = hue.to_radians().sin_cos();
    let (k, r3) = ((1.0 - c) / 3.0, s / 3.0f32.sqrt());
    let (a, b, d) = (c + k, k - r3, k + r3);
    let m = [[a, b, d], [d, a, b], [b, d, a]];
    map_straight(src, |col, a| {
        let rot: [f32; 3] = std::array::from_fn(|i| m[i][0] * col[0] + m[i][1] * col[1] + m[i][2] * col[2]);
        let luma = 0.2126 * rot[0] + 0.7152 * rot[1] + 0.0722 * rot[2];
        let out: [f32; 3] = std::array::from_fn(|i| {
            let v = luma + (rot[i] - luma) * (1.0 + saturation);
            (v - 0.5) * (1.0 + contrast) + 0.5 + brightness
        });
        (out, a)
    })
}

/// Unsharp mask with the result held between the smallest and the largest value of the 3x3 pixels around each one.
pub fn sharpen(src: &Img, amount: f32, sigma: f32) -> Img {
    if amount == 0.0 {
        return src.clone();
    }
    let soft = img::blur(src, sigma.max(0.1));
    let mut out = src.clone();
    for y in 0..src.h {
        for x in 0..src.w {
            let (mut lo, mut hi) = ([f32::MAX; 4], [f32::MIN; 4]);
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let p = clamp_px(src, x as i64 + dx, y as i64 + dy);
                    for c in 0..4 {
                        lo[c] = lo[c].min(p[c]);
                        hi[c] = hi[c].max(p[c]);
                    }
                }
            }
            let (s, b) = (src.px[y * src.w + x], soft.px[y * src.w + x]);
            let mut v: [f32; 4] = std::array::from_fn(|c| (s[c] + amount * (s[c] - b[c])).clamp(lo[c], hi[c]));
            // a premultiplied colour never exceeds its alpha
            for c in 0..3 {
                v[c] = v[c].min(v[3]);
            }
            out.px[y * src.w + x] = v;
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------- files
/// Load a PNG (any colour type) as a picture.
pub fn load_png(path: &Path) -> Result<Img, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    // palettes and short grey levels are expanded; 16-bit pictures are read as they are, so none of their precision is lost
    dec.set_transformations(png::Transformations::EXPAND);
    let mut r = dec.read_info().map_err(|e| format!("{}: {e}", path.display()))?;
    let mut buf = vec![0; r.output_buffer_size().ok_or("the picture is too large")?];
    let info = r.next_frame(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
    let (w, h) = (info.width as usize, info.height as usize);
    let n = w * h;
    let wide = info.bit_depth == png::BitDepth::Sixteen;
    let channels = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Indexed => return Err("indexed pictures could not be expanded".into()),
    };
    let sample = |i: usize| -> f32 {
        if wide {
            u16::from_be_bytes([buf[i * 2], buf[i * 2 + 1]]) as f32 / 65535.0
        } else {
            buf[i] as f32 / 255.0
        }
    };
    let mut px = Vec::with_capacity(n);
    for i in 0..n {
        let c = |k: usize| sample(i * channels + k);
        let (r, g, b, a) = match channels {
            4 => (c(0), c(1), c(2), c(3)),
            3 => (c(0), c(1), c(2), 1.0),
            2 => (c(0), c(0), c(0), c(1)),
            _ => (c(0), c(0), c(0), 1.0),
        };
        px.push([r * a, g * a, b * a, a]);
    }
    Ok(Img { w, h, px })
}

/// How a picture is stored when it is saved. Everything before that is done in 32-bit floating point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    Bits8,
    Bits16,
}

impl Depth {
    pub const ALL: [Depth; 2] = [Depth::Bits8, Depth::Bits16];
    pub fn name(self) -> &'static str {
        match self {
            Depth::Bits8 => "8 bit",
            Depth::Bits16 => "16 bit",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Output {
    pub depth: Depth,
    /// Dither when rounding to 8 bit (a fixed pattern, so the same picture always gives the same file). Values that are already whole levels
    /// (an untouched picture) are not changed by it.
    pub dither: bool,
}

impl Default for Output {
    fn default() -> Output {
        Output { depth: Depth::Bits8, dither: true }
    }
}

/// Straight 8-bit RGBA of a picture. Rounded, with dither if asked: a value within a thousandth of a level of a whole level is rounded
/// to it, so that an untouched picture comes back exactly as it was loaded.
pub fn to_rgba8_with(img: &Img, dither: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(img.px.len() * 4);
    let w = img.w.max(1);
    for (i, p) in img.px.iter().enumerate() {
        let (x, y) = (i % w, i / w);
        let a = p[3];
        let c = if a > 1e-6 { [p[0] / a, p[1] / a, p[2] / a] } else { [0.0; 3] };
        for (k, v) in [c[0], c[1], c[2], a].into_iter().enumerate() {
            let lv = v.clamp(0.0, 1.0) * 255.0;
            let near = lv.round();
            let q = if !dither || (lv - near).abs() < 1e-3 || lv <= 0.25 || lv >= 254.75 {
                near
            } else {
                // interleaved gradient noise, a different offset for each channel
                let (ox, oy) = (k as f32 * 41.0 + 5.0, k as f32 * 13.0 + 3.0);
                let t = (0.06711056 * (x as f32 + ox) + 0.00583715 * (y as f32 + oy)).fract();
                (lv + (52.9829189 * t).fract()).floor()
            };
            out.push(q.clamp(0.0, 255.0) as u8);
        }
    }
    out
}

/// Straight 8-bit RGBA of a picture, rounded (no dither: an untouched picture comes back exactly as it was loaded).
pub fn to_rgba8(img: &Img) -> Vec<u8> {
    to_rgba8_with(img, false)
}

/// Straight 16-bit RGBA, big-endian as a PNG stores it.
pub fn to_rgba16(img: &Img) -> Vec<u8> {
    let mut out = Vec::with_capacity(img.px.len() * 8);
    for p in &img.px {
        let a = p[3];
        let c = if a > 1e-9 { [p[0] / a, p[1] / a, p[2] / a] } else { [0.0; 3] };
        for v in [c[0], c[1], c[2], a] {
            out.extend_from_slice(&((v.clamp(0.0, 1.0) * 65535.0).round() as u16).to_be_bytes());
        }
    }
    out
}

pub fn save_png(path: &Path, img: &Img) -> Result<(), String> {
    save_png_with(path, img, Output::default())
}

pub fn save_png_with(path: &Path, img: &Img, output: Output) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), img.w as u32, img.h as u32);
    enc.set_color(png::ColorType::Rgba);
    let data = match output.depth {
        Depth::Bits8 => {
            enc.set_depth(png::BitDepth::Eight);
            to_rgba8_with(img, output.dither)
        }
        Depth::Bits16 => {
            enc.set_depth(png::BitDepth::Sixteen);
            to_rgba16(img)
        }
    };
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(&data).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step_image(w: usize, h: usize) -> Img {
        // a hard edge between dark and bright, and a hard edge in alpha
        let mut im = Img::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let bright = x >= w / 2;
                let a = if y >= h / 2 { 1.0 } else { 0.6 };
                let v = if bright { 0.9 } else { 0.1 };
                im.px[y * w + x] = [v * a, v * a, v * a, a];
            }
        }
        im
    }

    fn range(im: &Img) -> ([f32; 4], [f32; 4]) {
        let (mut lo, mut hi) = ([f32::MAX; 4], [f32::MIN; 4]);
        for p in &im.px {
            for c in 0..4 {
                lo[c] = lo[c].min(p[c]);
                hi[c] = hi[c].max(p[c]);
            }
        }
        (lo, hi)
    }

    #[test]
    fn no_mode_rings_when_resizing_a_hard_edge() {
        let src = step_image(24, 20);
        let (lo, hi) = range(&src);
        for mode in Resample::ALL {
            for (w, h) in [(7, 5), (48, 40), (100, 19), (13, 61), (24, 20), (240, 200)] {
                let out = resize(&src, w, h, mode);
                assert_eq!((out.w, out.h), (w, h));
                let (olo, ohi) = range(&out);
                for c in 0..4 {
                    assert!(olo[c] >= lo[c] - 1e-5 && ohi[c] <= hi[c] + 1e-5, "{mode:?} {w}x{h}: channel {c} left [{}, {}] -> [{}, {}]", lo[c], hi[c], olo[c], ohi[c]);
                }
            }
        }
    }

    #[test]
    fn a_flat_picture_stays_flat_and_the_average_survives_shrinking() {
        let mut flat = Img::new(10, 10);
        flat.px.iter_mut().for_each(|p| *p = [0.2, 0.3, 0.4, 0.5]);
        for mode in Resample::ALL {
            let out = resize(&flat, 33, 7, mode);
            assert!(out.px.iter().all(|p| (0..4).all(|c| (p[c] - flat.px[0][c]).abs() < 1e-5)), "{mode:?}");
        }
        let src = step_image(32, 32);
        let mean = |im: &Img| im.px.iter().map(|p| p[3] as f64).sum::<f64>() / im.px.len() as f64;
        assert!((mean(&src) - mean(&resize(&src, 8, 8, Resample::Smooth))).abs() < 1e-3);
    }

    #[test]
    fn crisp_is_sharper_than_smooth_on_an_edge() {
        let src = step_image(16, 16);
        // the steepest step between neighbouring pixels along a row of the result
        let steepest = |im: &Img| {
            let row = im.h / 2 + 1;
            (1..im.w).map(|x| (im.px[row * im.w + x][0] - im.px[row * im.w + x - 1][0]).abs()).fold(0.0f32, f32::max)
        };
        let (smooth, crisp) = (resize(&src, 128, 128, Resample::Smooth), resize(&src, 128, 128, Resample::Crisp));
        assert!(steepest(&crisp) > steepest(&smooth) * 1.2, "crisp {} smooth {}", steepest(&crisp), steepest(&smooth));
    }

    #[test]
    fn flips_turns_crops_and_trims_move_the_right_pixels() {
        let mut im = Img::new(3, 2);
        for (i, p) in im.px.iter_mut().enumerate() {
            *p = [i as f32 / 10.0, 0.0, 0.0, 1.0];
        }
        // row 0: 0 1 2 / row 1: 3 4 5
        let v = |im: &Img| im.px.iter().map(|p| (p[0] * 10.0).round() as i32).collect::<Vec<_>>();
        assert_eq!(v(&flip(&im, true)), vec![2, 1, 0, 5, 4, 3]);
        assert_eq!(v(&flip(&im, false)), vec![3, 4, 5, 0, 1, 2]);
        let r1 = rotate(&im, 1);
        assert_eq!((r1.w, r1.h), (2, 3));
        assert_eq!(v(&r1), vec![3, 0, 4, 1, 5, 2], "clockwise");
        assert_eq!(v(&rotate(&im, 3)), vec![2, 5, 1, 4, 0, 3], "counter-clockwise");
        assert_eq!(v(&rotate(&im, 2)), vec![5, 4, 3, 2, 1, 0]);
        assert_eq!(v(&rotate(&rotate(&im, 1), 3)), v(&im));
        assert_eq!(v(&crop(&im, 1, 0, 2, 1)), vec![1, 2]);
        // trim
        let mut t = Img::new(6, 5);
        t.px[2 * 6 + 3] = [1.0, 1.0, 1.0, 1.0];
        t.px[3 * 6 + 4] = [1.0, 1.0, 1.0, 0.5];
        assert_eq!(content_box(&t, 0.0), Some((3, 2, 2, 2)));
        let tr = trim(&t, 0.0);
        assert_eq!((tr.w, tr.h), (2, 2));
        assert_eq!(trim(&Img::new(4, 4), 0.0).w, 1);
    }

    #[test]
    fn colour_edits() {
        let mut im = Img::new(2, 1);
        im.px[0] = [0.8 * 0.5, 0.2 * 0.5, 0.1 * 0.5, 0.5];
        im.px[1] = [0.0, 0.0, 0.0, 0.0];
        // nothing set: the same picture
        assert_eq!(adjust(&im, 0.0, 0.0, 0.0, 0.0).px, im.px);
        assert_eq!(adjust(&im, 0.0, 0.0, 0.0, 360.0).px, im.px);
        // no saturation: grey, with the alpha untouched
        let g = adjust(&im, 0.0, 0.0, -1.0, 0.0);
        assert!((g.px[0][0] - g.px[0][1]).abs() < 1e-5 && (g.px[0][1] - g.px[0][2]).abs() < 1e-5 && (g.px[0][3] - 0.5).abs() < 1e-6);
        // a turn of the hue and back
        let there = adjust(&im, 0.0, 0.0, 0.0, 120.0);
        let back = adjust(&there, 0.0, 0.0, 0.0, 240.0);
        assert!((0..3).all(|c| (back.px[0][c] - im.px[0][c]).abs() < 0.01), "{:?} vs {:?}", back.px[0], im.px[0]);
        // brightness lifts the colour, not the alpha, and invert twice is the picture again
        let b = adjust(&im, 0.2, 0.0, 0.0, 0.0);
        assert!(b.px[0][0] > im.px[0][0] && (b.px[0][3] - 0.5).abs() < 1e-6);
        let inv2 = apply(&apply(&im, &Op::Invert), &Op::Invert);
        assert!((0..4).all(|c| (inv2.px[0][c] - im.px[0][c]).abs() < 1e-5));
        // opacity and tint
        let half = apply(&im, &Op::Opacity { factor: 0.5 });
        assert!((half.px[0][3] - 0.25).abs() < 1e-6);
        let tinted = apply(&im, &Op::Tint { rgb: [0.0, 0.0, 1.0], strength: 1.0 });
        assert!((tinted.px[0][2] - 0.5).abs() < 1e-5 && tinted.px[0][0].abs() < 1e-5);
    }

    #[test]
    fn sharpen_adds_no_halos() {
        let src = step_image(32, 16);
        let (lo, hi) = range(&src);
        let out = sharpen(&src, 2.0, 1.5);
        let (olo, ohi) = range(&out);
        for c in 0..4 {
            assert!(olo[c] >= lo[c] - 1e-5 && ohi[c] <= hi[c] + 1e-5, "channel {c}");
        }
        // and it did change the picture where it is soft
        let soft = img::blur(&src, 1.2);
        let sharper = sharpen(&soft, 1.5, 1.2);
        let edge = |im: &Img| (im.px[8 * 32 + 17][0] - im.px[8 * 32 + 14][0]).abs();
        assert!(edge(&sharper) > edge(&soft));
    }

    #[test]
    fn debanding_works_on_the_bands_of_a_gradient_and_keeps_the_picture() {
        // a slow gradient stored in 8 bits: a band of 5 pixels for every level, which is what banding is
        let (w, h) = (700usize, 32usize);
        let mut im = Img::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = (40.0 + (x / 5) as f32) / 255.0;
                im.px[y * w + x] = [v, v, v, 1.0];
            }
        }
        let out = apply(&im, &Op::deband(1.0, 0.0));
        let level = |i: &Img, x: usize, y: usize| (i.px[y * w + x][0] * 255.0).round() as i32;
        let changed = (0..w * h).filter(|i| level(&out, i % w, i / w) != level(&im, i % w, i / w)).count();
        assert!(changed > w * h / 10, "only {changed} pixels changed");
        // (towards the sides of a picture there is only a neighbour on one side, which tilts the average of a slope)
        let worst = (0..w * h).filter(|i| (300..400).contains(&(i % w))).map(|i| (level(&out, i % w, i / w) - level(&im, i % w, i / w)).abs()).max().unwrap();
        assert!(worst <= 3, "a pixel moved by {worst} levels");
        assert!(out.px.iter().all(|p| (p[3] - 1.0).abs() < 1e-6), "opaque stays opaque");
        // a hard edge is not banding and stays
        let mut edge = Img::new(32, 8);
        for y in 0..8 {
            for x in 0..32 {
                let v = if x < 16 { 0.2 } else { 0.8 };
                edge.px[y * 32 + x] = [v, v, v, 1.0];
            }
        }
        let e = apply(&edge, &Op::deband(1.0, 0.0));
        assert!((e.px[4 * 32 + 15][0] - 0.2).abs() < 0.02 && (e.px[4 * 32 + 16][0] - 0.8).abs() < 0.02);
    }

    /// How much of a band edge is left: the jump of the 3 pixel average across the places where the original had a step (1 level over 10
    /// pixels), less the same jump anywhere else, which is the dither's own noise. 1.0 is a full step, 0.0 is a smooth ramp.
    fn edge_jump(im: &Img, band: usize) -> f32 {
        let (w, y) = (im.w, im.h / 2);
        let v = |x: usize| im.px[y * w + x][0] * 255.0;
        let jump = |x: usize| ((x..x + 3).map(v).sum::<f32>() - (x - 3..x).map(v).sum::<f32>()).abs() / 3.0;
        let (mut on, mut non, mut off, mut noff) = (0.0, 0, 0.0, 0);
        for x in 40..w - 40 {
            if x % band == 0 {
                on += jump(x);
                non += 1;
            } else if x % band == band / 2 {
                off += jump(x);
                noff += 1;
            }
        }
        on / non as f32 - off / noff as f32
    }

    #[test]
    fn stronger_debanding_takes_more_of_the_steps_out_of_a_gradient() {
        // a ramp of 40 levels over 400 pixels, stored in 8 bits: a band of 10 pixels for every level
        let (w, h) = (400usize, 24usize);
        let ramp = |x: usize| 100.0 + x as f32 * 40.0 / w as f32;
        let mut im = Img::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = ramp(x).floor() / 255.0;
                im.px[y * w + x] = [v, v, v, 1.0];
            }
        }
        let before = edge_jump(&im, 10);
        let mut last = before;
        let mut report = vec![format!("none {before:.2}")];
        for strength in [0.2f32, 0.5, 1.0] {
            let out = apply(&im, &Op::deband(strength, 0.0));
            let now = edge_jump(&out, 10);
            report.push(format!("{strength} {now:.2}"));
            // (what is left at these strengths is the dither's noise: the step is gone)
            assert!(now < before * 0.4, "{report:?}");
            last = now;
        }
        eprintln!("band edge left after deband: {report:?}");
        // the full strength takes most of the step out of every band edge
        assert!(last < before * 0.4, "{report:?}");
        // wide bands (60 pixels for every level, as in a slow fade) need the long reach
        let (w2, h2) = (992usize, 12usize);
        let ramp2 = |x: usize| 100.0 + x as f32 * 16.0 / w2 as f32;
        let mut wide = Img::new(w2, h2);
        for y in 0..h2 {
            for x in 0..w2 {
                let v = ramp2(x).floor() / 255.0;
                wide.px[y * w2 + x] = [v, v, v, 1.0];
            }
        }
        let (b0, b1) = (edge_jump(&wide, 62), edge_jump(&apply(&wide, &Op::deband(1.0, 0.0)), 62));
        eprintln!("wide bands: {b0:.2} -> {b1:.2}");
        assert!(b1 < b0 * 0.5, "{b0} -> {b1}");
        // and a hard edge is still a hard edge at full strength
        let mut edge = Img::new(32, 8);
        for y in 0..8 {
            for x in 0..32 {
                let v = if x < 16 { 0.2 } else { 0.8 };
                edge.px[y * 32 + x] = [v, v, v, 1.0];
            }
        }
        let e = apply(&edge, &Op::deband(1.0, 0.0));
        assert!((e.px[4 * 32 + 15][0] - 0.2).abs() < 0.02 && (e.px[4 * 32 + 16][0] - 0.8).abs() < 0.02);
    }

    #[test]
    fn png_files_round_trip_exactly() {
        let dir = std::env::temp_dir().join(format!("ss-edit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut rgba = Vec::new();
        for i in 0..(7 * 5) {
            rgba.extend([(i * 7) as u8, (i * 13) as u8, (255 - i * 5) as u8, 20 + (i * 6) as u8]);
        }
        let im = Img::from_straight_u8(7, 5, &rgba);
        let file = dir.join("t.png");
        save_png(&file, &im).unwrap();
        let back = load_png(&file).unwrap();
        assert_eq!((back.w, back.h), (7, 5));
        assert_eq!(to_rgba8(&back), rgba);
        assert!(load_png(&dir.join("nothing.png")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_edit_list_applies_in_order() {
        let src = step_image(20, 10);
        let ops = vec![Op::Resize { w: 40, h: 20, mode: Resample::Smooth }, Op::Flip { horizontal: true }, Op::Crop { x: 0, y: 0, w: 10, h: 10 }];
        let out = apply_all(&src, &ops);
        assert_eq!((out.w, out.h), (10, 10));
        // flipped: the left of the result is what was the right (bright) side
        assert!(out.px[5 * 10][0] > 0.4);
        assert_eq!(apply_all(&src, &[]).px, src.px);
    }
}

#[cfg(test)]
mod timing {
    use super::*;

    /// `cargo test --release -p engine edit::timing -- --ignored --nocapture`: how long each edit takes on a 2048 x 2048 picture.
    #[test]
    #[ignore]
    fn how_long_each_edit_takes() {
        let (w, h) = (2048usize, 2048usize);
        let mut rgba = vec![];
        for y in 0..h {
            for x in 0..w {
                rgba.extend([(x * 255 / w) as u8, (y * 255 / h) as u8, ((x + y) * 255 / (w + h)) as u8, 255]);
            }
        }
        let im = Img::from_straight_u8(w, h, &rgba);
        let ops = [
            Op::Resize { w: 1024, h: 1024, mode: Resample::Smooth },
            Op::Resize { w: 4096, h: 4096, mode: Resample::Smooth },
            Op::Resize { w: 4096, h: 4096, mode: Resample::Crisp },
            Op::Flip { horizontal: true },
            Op::Rotate { quarter_turns: 1 },
            Op::Crop { x: 10, y: 10, w: 1500, h: 1500 },
            Op::Trim { threshold: 0.0 },
            Op::Adjust { brightness: 0.1, contrast: 0.1, saturation: 0.1, hue: 20.0 },
            Op::Tint { rgb: [1.0, 0.0, 0.0], strength: 0.3 },
            Op::Blur { sigma: 3.0 },
            Op::Sharpen { amount: 1.0, sigma: 1.5 },
            Op::deband(0.6, 0.0),
        ];
        for op in &ops {
            let t = std::time::Instant::now();
            let out = apply(&im, op);
            eprintln!("{:<22} {:>7.0} ms  ({}x{})", format!("{op:?}").chars().take(22).collect::<String>(), t.elapsed().as_secs_f64() * 1000.0, out.w, out.h);
        }
        let t = std::time::Instant::now();
        let _ = im.to_premult_u8();
        eprintln!("to_premult_u8 (dithered) {:>7.0} ms", t.elapsed().as_secs_f64() * 1000.0);
        let t = std::time::Instant::now();
        let _ = to_rgba8(&im);
        eprintln!("to_rgba8                 {:>7.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    }
}

#[cfg(test)]
mod deband_pictures {
    use super::*;
    /// Writes a banded glow before and after debanding: `DEBAND_OUT=dir cargo test --release -p engine deband_pictures -- --ignored`
    #[test]
    #[ignore]
    fn write_pictures() {
        let Some(dir) = std::env::var_os("DEBAND_OUT") else { return };
        let dir = std::path::PathBuf::from(dir);
        if let Some(p) = std::env::var_os("DEBAND_IN") {
            let im = load_png(std::path::Path::new(&p)).unwrap();
            save_png(&dir.join("src.png"), &im).unwrap();
            for s in [0.5f32, 1.0] {
                save_png(&dir.join(format!("out{}.png", (s * 10.0) as i32)), &apply(&im, &Op::deband(s, 0.0))).unwrap();
            }
            let mk = |pre: f32, limit: f32, name: &str| {
                let op = Op::Deband { radius: 64.0, threshold: 8.0, iterations: 4, grain: 0.0, kind: Grain::None, mono: true, room: false, luma: restore::LumaScale::off(), limit, elast: 3.0, narrowing: false, prefilter: pre, protect: restore::Mask::off(), adv: deband::Advanced::default() };
                save_png(&dir.join(name), &apply(&im, &op)).unwrap();
            };
            let mm = |mode: deband::Mode, thr1: f32, thr2: f32, name: &str| {
                let op = Op::Deband { radius: 64.0, threshold: 4.0, iterations: 4, grain: 0.0, kind: Grain::None, mono: true, room: false, luma: restore::LumaScale::off(), limit: 0.0, elast: 3.0, narrowing: false, prefilter: 0.0, protect: restore::Mask::off(), adv: deband::Advanced { mode, thr1, thr2, ..deband::Advanced::default() } };
                let t = std::time::Instant::now();
                save_png(&dir.join(name), &apply(&im, &op)).unwrap();
                eprintln!("{name}: {:?}", t.elapsed());
            };
            for (t1, t2) in [(40.0, 2.0), (40.0, 4.0), (40.0, 8.0), (80.0, 4.0), (80.0, 8.0)] {
                mm(deband::Mode::Angle, t1, t2, &format!("g_angle_{t1}_{t2}.png"));
                mm(deband::Mode::Smooth, t1, t2, &format!("g_smooth_{t1}_{t2}.png"));
            }
            mm(deband::Mode::Pairs, 0.0, 0.0, "m_pairs.png");
            mm(deband::Mode::Smooth, 0.0, 0.0, "m_smooth0.png");
            mm(deband::Mode::Smooth, 24.0, 4.0, "m_smooth1.png");
            mm(deband::Mode::Angle, 24.0, 4.0, "m_angle1.png");
            mk(0.0, 0.0, "v_plain.png");
            mk(1.5, 0.0, "v_pre.png");
            mk(0.0, 3.0, "v_limit.png");
            mk(1.5, 3.0, "v_both.png");
            return;
        }
        let n = 512usize;
        let mut im = Img::new(n, n);
        for y in 0..n {
            for x in 0..n {
                let d = ((x as f32 - 256.0).powi(2) + (y as f32 - 256.0).powi(2)).sqrt() / 250.0;
                let a = (1.0 - d).clamp(0.0, 1.0).powf(1.6) * 0.9;
                // quantised to 8 bits like a saved picture: a smooth glow with steps
                let q = |v: f32| (v * 255.0).round() / 255.0;
                let (r, g, b) = (q(0.55 * a), q(0.4 * a), q(a));
                let al = q(a);
                im.px[y * n + x] = [r.min(al), g.min(al), b.min(al), al];
            }
        }
        save_png(&dir.join("src.png"), &im).unwrap();
        for s in [0.5f32, 1.0] {
            save_png(&dir.join(format!("out{}.png", (s * 10.0) as i32)), &apply(&im, &Op::deband(s, 0.0))).unwrap();
        }
    }
}

#[cfg(test)]
mod glow_room {
    use super::*;

    #[test]
    fn a_glow_cut_by_the_border_gets_room_and_fades_out_while_a_clean_picture_keeps_its_size() {
        let n = 64;
        let mut glow = Img::new(n, n);
        for y in 0..n {
            for x in 0..n {
                let d = (((x as f32 - 31.5).powi(2) + (y as f32 - 31.5).powi(2)).sqrt() / 31.5).min(1.5);
                let a = ((1.0 - d * 0.5).max(0.0) * 0.8).min(0.8); // still 0.4 at the border
                glow.px[y * n + x] = [0.0, a, 0.0, a];
            }
        }
        let out = apply(&glow, &Op::deband(0.5, 0.0));
        assert!(out.w > n && out.h > n && out.w == out.h, "grown: {}x{}", out.w, out.h);
        let pad = (out.w - n) / 2;
        // the new outer edge is empty, the middle is where it was
        assert!(out.px[0][3] < 0.004, "{}", out.px[0][3]);
        assert!(out.px[(out.h / 2) * out.w][3] < 0.02);
        assert!(out.px[(out.h / 2) * out.w + out.w / 2][3] > 0.6);
        assert!(pad >= 16);
        let mut clean = Img::new(n, n);
        clean.px[32 * n + 32] = [1.0, 1.0, 1.0, 1.0];
        let same = apply(&clean, &Op::deband(0.5, 0.0));
        assert_eq!((same.w, same.h), (n, n));
    }
}

#[cfg(test)]
mod expand_tests {
    use super::*;

    #[test]
    fn expanding_adds_the_border_on_every_side_and_fills_it_as_asked() {
        let mut im = Img::new(4, 3);
        for p in im.px.iter_mut() {
            *p = [0.5, 0.25, 0.0, 0.5];
        }
        let clear = apply(&im, &Op::Expand { amount: 5, fill: Fill::Clear });
        assert_eq!((clear.w, clear.h), (14, 13));
        assert_eq!(clear.px[0], [0.0; 4]);
        assert_eq!(clear.px[5 * 14 + 5], [0.5, 0.25, 0.0, 0.5]);
        let edge = apply(&im, &Op::Expand { amount: 5, fill: Fill::Edge });
        assert_eq!(edge.px[0], [0.5, 0.25, 0.0, 0.5]);
        let fade = apply(&im, &Op::Expand { amount: 5, fill: Fill::Fade });
        assert!(fade.px[0][3] < 0.01 && fade.px[5 * 14 + 4][3] > 0.4, "{:?} {:?}", fade.px[0], fade.px[5 * 14 + 4]);
        assert_eq!(apply(&im, &Op::Expand { amount: 0, fill: Fill::Clear }).w, 4);
        // trimming it away again gives the picture back
        let back = apply(&clear, &Op::Trim { threshold: 0.0 });
        assert_eq!((back.w, back.h), (4, 3));
    }

    #[test]
    fn deband_with_luma_scaling_works_on_the_dark_parts_and_leaves_the_light_ones() {
        // the same banded ramp twice: once dark (levels 20 to 60) and once light (levels 190 to 230)
        let (w, h) = (400usize, 16usize);
        let make = |base: f32| {
            let mut im = Img::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    let v = (base + (x / 10) as f32) / 255.0;
                    im.px[y * w + x] = [v, v, v, 1.0];
                }
            }
            im
        };
        let luma = restore::LumaScale { scaling: 6.0, by_average: false };
        let go = |im: &Img, l: restore::LumaScale| apply(im, &Op::Deband { radius: 40.0, threshold: 4.0, iterations: 3, grain: 0.0, kind: Grain::None, mono: true, room: false, luma: l, limit: 0.0, elast: 3.0, narrowing: false, prefilter: 0.0, protect: restore::Mask::off(), adv: deband::Advanced::default() });
        let changed = |a: &Img, b: &Img| a.px.iter().zip(&b.px).map(|(p, q)| (p[0] - q[0]).abs()).sum::<f32>() * 255.0 / a.px.len() as f32;
        let (dark, light) = (make(20.0), make(190.0));
        let (d_on, l_on) = (changed(&dark, &go(&dark, luma)), changed(&light, &go(&light, luma)));
        let l_off = changed(&light, &go(&light, restore::LumaScale::off()));
        assert!(d_on > 0.2, "the dark ramp is worked on: {d_on}");
        assert!(l_on < l_off * 0.2, "the light one hardly: {l_on} against {l_off}");
    }

    #[test]
    fn the_work_is_done_in_floating_point_and_only_the_saving_rounds() {
        // a ramp of 0.3 levels per pixel, stored as it would be in 8 bits (steps of one level every 3 or 4 pixels)
        let (w, h) = (600usize, 8usize);
        let truth = |x: usize| (20.0 + x as f32 * 0.3) / 255.0;
        let mut im = Img::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = (truth(x) * 255.0).round() / 255.0;
                im.px[y * w + x] = [v, v, v, 1.0];
            }
        }
        let out = apply(&im, &Op::Deband { radius: 24.0, threshold: 4.0, iterations: 3, grain: 0.0, kind: Grain::None, mono: true, room: false, luma: restore::LumaScale::off(), limit: 0.0, elast: 3.0, narrowing: false, prefilter: 0.0, protect: restore::Mask::off(), adv: deband::Advanced::default() });
        // the result is not rounded: it holds the ramp between the levels
        let off_grid = out.px.iter().filter(|p| ((p[0] * 255.0).fract() - 0.0).abs() > 0.1 && (p[0] * 255.0).fract() < 0.9).count();
        assert!(off_grid > w * h / 3, "{off_grid} values between levels");
        // saving with dither keeps the average of the ramp closer to the truth than plain rounding does
        let err = |bytes: &[u8]| (150..450).map(|x| (0..h).map(|y| bytes[(y * w + x) * 4] as f32).sum::<f32>() / h as f32 - truth(x) * 255.0).map(|d| d.abs()).sum::<f32>() / 300.0;
        let (plain, dithered) = (err(&to_rgba8_with(&out, false)), err(&to_rgba8_with(&out, true)));
        assert!(dithered < plain * 0.8 || dithered < 0.2, "plain {plain} dithered {dithered}");
        // 16 bit keeps it
        let dir = std::env::temp_dir().join(format!("deep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("deep.png");
        save_png_with(&file, &out, Output { depth: Depth::Bits16, dither: false }).unwrap();
        let back = load_png(&file).unwrap();
        assert!(back.px.iter().zip(&out.px).all(|(a, b)| (a[0] - b[0]).abs() < 2.0 / 65535.0), "16 bit round trip");
        // and an untouched 8-bit picture is not changed by the dither
        let mut flat = Img::new(64, 64);
        for (i, p) in flat.px.iter_mut().enumerate() {
            let v = (i % 251) as f32 / 255.0;
            *p = [v * 0.6, v * 0.4, v * 0.2, 0.6];
        }
        let rgba = to_rgba8_with(&flat, false);
        let again = Img::from_straight_u8(64, 64, &rgba);
        assert_eq!(to_rgba8_with(&again, true), rgba);
        std::fs::remove_dir_all(&dir).ok();
    }
}
