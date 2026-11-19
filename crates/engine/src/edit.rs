//! Editing a picture: resize without ringing, flips and turns, crop and trim, colour, blur and sharpen, debanding.
//!
//! Pictures are the engine's [`Img`]: floating point, premultiplied. Every operation works on those and gives a new one, so an edit is a
//! list of [`Op`]s applied in order to the picture that was loaded ([`apply_all`]) and nothing is lost along the way until it is saved.
use std::path::Path;

use crate::deband;
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
    /// Remove banding from gradients: `strength` 0 to 1, `grain` in 8-bit levels of dither.
    Deband { strength: f32, grain: f32 },
}

impl Op {
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
        Op::Deband { strength, grain } => {
            let s = strength.clamp(0.0, 1.0);
            let p = deband::Params { radius: 4.0 + 12.0 * s, thr_rgb: 1.0 + 7.0 * s, thr_a: 1.0 + 7.0 * s, iterations: 2, grain: *grain };
            deband::deband(src, &p)
        }
    }
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
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().map_err(|e| format!("{}: {e}", path.display()))?;
    let mut buf = vec![0; r.output_buffer_size().ok_or("the picture is too large")?];
    let info = r.next_frame(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
    let (w, h) = (info.width as usize, info.height as usize);
    let n = w * h;
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => buf[..n * 4].to_vec(),
        png::ColorType::Rgb => buf[..n * 3].chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::Grayscale => buf[..n].iter().flat_map(|g| [*g, *g, *g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf[..n * 2].chunks(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Indexed => return Err("indexed pictures could not be expanded".into()),
    };
    Ok(Img::from_straight_u8(w, h, &rgba))
}

/// Straight 8-bit RGBA of a picture, rounded (no dither: an untouched picture comes back exactly as it was loaded).
pub fn to_rgba8(img: &Img) -> Vec<u8> {
    let mut out = Vec::with_capacity(img.px.len() * 4);
    for p in &img.px {
        let a = p[3];
        let c = if a > 1e-6 { [p[0] / a, p[1] / a, p[2] / a] } else { [0.0; 3] };
        for v in [c[0], c[1], c[2], a] {
            out.push((v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8);
        }
    }
    out
}

pub fn save_png(path: &Path, img: &Img) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), img.w as u32, img.h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(&to_rgba8(img)).map_err(|e| e.to_string())
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
        let (w, h) = (160usize, 32usize);
        let mut im = Img::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = (40.0 + (x / 5) as f32) / 255.0;
                im.px[y * w + x] = [v, v, v, 1.0];
            }
        }
        let out = apply(&im, &Op::Deband { strength: 1.0, grain: 0.0 });
        let level = |i: &Img, x: usize, y: usize| (i.px[y * w + x][0] * 255.0).round() as i32;
        let changed = (0..w * h).filter(|i| level(&out, i % w, i / w) != level(&im, i % w, i / w)).count();
        assert!(changed > w * h / 10, "only {changed} pixels changed");
        let worst = (0..w * h).map(|i| (level(&out, i % w, i / w) - level(&im, i % w, i / w)).abs()).max().unwrap();
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
        let e = apply(&edge, &Op::Deband { strength: 1.0, grain: 0.0 });
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
