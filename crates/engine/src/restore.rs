//! Dehalo, denoise and anti-aliasing for pictures, after the VapourSynth filters of vs-jetpack (vsdehalo, vsdenoise, vsaa, vsmasktools):
//! `dehalo_alpha`, `fine_dehalo` (with its edge masks), non-local means and a bilateral filter, and an anti-aliaser guided by an edge mask
//! like `based_aa`. The neural scalers and EEDI3/NNEDI3 of the originals are not used: the anti-aliaser here smooths along the direction of
//! each edge instead.
//!
//! All of them work on single planes of 0 to 255 (the way the originals are written in 8 bit): the luma of the (premultiplied) colour,
//! which changes the colours as little as possible, and the alpha. Every one can be limited by an edge mask ([`Mask`]).
use rayon::prelude::*;

use crate::edit::Resample;
use crate::img::Img;

// ------------------------------------------------------------------------------------------------------------------------ options
/// Which planes of the picture a filter works on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Target {
    /// The brightness of the colour (the hue is kept).
    Colour,
    Alpha,
    Both,
}

impl Target {
    pub const ALL: [Target; 3] = [Target::Both, Target::Colour, Target::Alpha];
    pub fn name(self) -> &'static str {
        match self {
            Target::Both => "Color and alpha",
            Target::Colour => "Color",
            Target::Alpha => "Alpha (transparency)",
        }
    }
}

/// An edge detector (vsmasktools).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Edge {
    Sobel,
    Prewitt,
    Scharr,
    Kirsch,
    Robinson3,
    Robinson5,
}

impl Edge {
    pub const ALL: [Edge; 6] = [Edge::Robinson3, Edge::Prewitt, Edge::Sobel, Edge::Scharr, Edge::Kirsch, Edge::Robinson5];
    pub fn name(self) -> &'static str {
        match self {
            Edge::Sobel => "Sobel",
            Edge::Prewitt => "Prewitt",
            Edge::Scharr => "Scharr",
            Edge::Kirsch => "Kirsch",
            Edge::Robinson3 => "Robinson3",
            Edge::Robinson5 => "Robinson5",
        }
    }
}

/// Luma scaling, after adaptivegrain (used by vodesfunc's `adaptive_grain`): how much of a filter is applied depends on the brightness of the
/// pixel. The weight is `(1 - f(luma))^e`, with the polynomial `f` of adaptivegrain: 1 in the dark, falling to 0 in the light. Banding and
/// noise show in the dark parts, where there is less to work with, so a positive value applies the filter there and barely at all in the light.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LumaScale {
    /// The exponent `e` (adaptivegrain's `luma_scaling`): larger = the weight falls off faster towards the light. 0 = off (the filter is applied
    /// everywhere equally). Negative: the same for the bright parts instead (vodesfunc inverts the picture first).
    pub scaling: f32,
    /// Multiply `e` by the square of the average brightness of the picture, as adaptivegrain does (dark pictures then get it more evenly).
    pub by_average: bool,
}

impl LumaScale {
    pub fn off() -> LumaScale {
        LumaScale { scaling: 0.0, by_average: false }
    }
}

/// The weight of adaptivegrain's mask for a brightness `x` (0 to 1) and exponent `e`.
pub fn luma_weight(x: f32, e: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let f = x * (1.124 + x * (-9.466 + x * (36.624 + x * (-45.47 + x * 18.188))));
    (1.0 - f).clamp(0.0, 1.0).powf(e)
}

/// The weight of every pixel of a picture (0 to 1; all 1 when the scaling is off). Brightness is that of the colour as it looks over black.
pub fn luma_weights(src: &Img, l: &LumaScale) -> Vec<f32> {
    if l.scaling == 0.0 {
        return vec![1.0; src.px.len()];
    }
    let invert = l.scaling < 0.0;
    let lum = |p: &[f32; 4]| {
        let y = (p[0] * LUMA[0] + p[1] * LUMA[1] + p[2] * LUMA[2]).clamp(0.0, 1.0);
        if invert { 1.0 - y } else { y }
    };
    let e = if l.by_average {
        // the average over what is visible: a picture that is mostly empty would otherwise count as dark
        let seen: Vec<f32> = src.px.iter().filter(|p| p[3] > 1.0 / 255.0).map(lum).collect();
        let avg = if seen.is_empty() { 0.0 } else { seen.iter().sum::<f32>() / seen.len() as f32 };
        avg * avg * l.scaling.abs()
    } else {
        l.scaling.abs()
    };
    src.px.par_iter().map(|p| luma_weight(lum(p), e)).collect()
}

/// The weights (0 to 1) of an edge mask for a whole picture, made from its brightness and its alpha (all 1 when the mask is off).
pub fn mask_weights(src: &Img, m: &Mask) -> Vec<f32> {
    if !m.enabled || src.w == 0 || src.h == 0 {
        return vec![1.0; src.px.len()];
    }
    let (w, h) = (src.w, src.h);
    let y = Plane { w, h, v: src.px.par_iter().map(|p| (p[0] * LUMA[0] + p[1] * LUMA[1] + p[2] * LUMA[2]) * 255.0).collect() };
    let a = Plane { w, h, v: src.px.par_iter().map(|p| p[3] * 255.0).collect() };
    build_mask(&[&y, &a], m).v.iter().map(|v| v / 255.0).collect()
}

/// The mask itself as a grey picture.
pub fn mask_image(src: &Img, m: &Mask) -> Img {
    let w = mask_weights(src, &Mask { enabled: true, ..*m });
    Img { w: src.w, h: src.h, px: w.iter().map(|&v| [v, v, v, 1.0]).collect() }
}

/// `src` moved towards `filtered` by the weights.
pub fn merge_weighted(src: &Img, filtered: &Img, w: &[f32]) -> Img {
    let px = (0..src.px.len())
        .into_par_iter()
        .map(|i| {
            let (a, b) = (src.px[i], filtered.px[i]);
            [a[0] + (b[0] - a[0]) * w[i], a[1] + (b[1] - a[1]) * w[i], a[2] + (b[2] - a[2]) * w[i], a[3] + (b[3] - a[3]) * w[i]]
        })
        .collect();
    Img { w: src.w, h: src.h, px }
}

/// Limits a filter to where it is wanted: near edges (halos, aliasing) or, inverted, away from them (noise).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Mask {
    pub enabled: bool,
    pub edge: Edge,
    /// Edges weaker than this (0 to 255) are not counted.
    pub threshold: f32,
    /// How many pixels the edges are grown by.
    pub grow: u8,
    /// How many times the mask is smoothed (3 x 3 box blur).
    pub soften: u8,
    /// Filter everywhere except at the edges.
    pub invert: bool,
    /// Give the mask itself (grey) instead of the filtered picture.
    pub show: bool,
    /// Applies the filter more in the dark than in the light (or the other way round).
    pub luma: LumaScale,
}

impl Mask {
    pub fn off() -> Mask {
        Mask { enabled: false, edge: Edge::Prewitt, threshold: 60.0, grow: 2, soften: 1, invert: false, show: false, luma: LumaScale::off() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Dehalo {
    /// `fine_dehalo` (with its own mask that spares lines and details) instead of plain `dehalo_alpha`.
    pub fine: bool,
    /// Spread of the Gaussian blur (1.4 in the original).
    pub blur: f32,
    /// Dehalo is fully applied below this edge-change sensitivity (0 to 100) and not at all above `highsens`; -1 for both: no mask.
    pub lowsens: f32,
    pub highsens: f32,
    /// Supersampling factor of the clamp that limits the result to what was there (1 = none).
    pub ss: f32,
    /// How much of the dark halos (0 to 1) and bright halos is taken away.
    pub darkstr: f32,
    pub brightstr: f32,
    pub iterations: u8,
    // fine_dehalo
    pub rx: u8,
    pub ry: u8,
    pub edge: Edge,
    pub thmi: f32,
    pub thma: f32,
    pub thlimi: f32,
    pub thlima: f32,
    pub exclude: bool,
    pub edgeproc: f32,
    pub target: Target,
    pub mask: Mask,
}

impl Default for Dehalo {
    fn default() -> Dehalo {
        Dehalo { fine: true, blur: 1.4, lowsens: 50.0, highsens: 50.0, ss: 1.5, darkstr: 0.0, brightstr: 1.0, iterations: 1, rx: 2, ry: 2, edge: Edge::Robinson3, thmi: 80.0, thma: 128.0, thlimi: 50.0, thlima: 100.0, exclude: true, edgeproc: 0.0, target: Target::Both, mask: Mask::off() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DenoiseKind {
    /// Non-local means: pixels are averaged with the ones in the neighbourhood whose surroundings look alike.
    NlMeans,
    /// Pixels are averaged with near ones that are also close in value.
    Bilateral,
}

impl DenoiseKind {
    pub const ALL: [DenoiseKind; 2] = [DenoiseKind::NlMeans, DenoiseKind::Bilateral];
    pub fn name(self) -> &'static str {
        match self {
            DenoiseKind::NlMeans => "Non-local means",
            DenoiseKind::Bilateral => "Bilateral",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Denoise {
    pub kind: DenoiseKind,
    /// Strength (non-local means) in 8-bit levels: about the spread of the noise that is to go.
    pub h: f32,
    /// Search radius `a` and similarity radius `s` of non-local means.
    pub search: u8,
    pub patch: u8,
    /// Bilateral: how far (pixels) and how different (levels) pixels may be to be averaged.
    pub sigma_s: f32,
    pub sigma_r: f32,
    pub target: Target,
    pub mask: Mask,
}

impl Default for Denoise {
    fn default() -> Denoise {
        Denoise { kind: DenoiseKind::NlMeans, h: 4.0, search: 2, patch: 2, sigma_s: 2.0, sigma_r: 12.0, target: Target::Both, mask: Mask { enabled: true, invert: true, grow: 1, ..Mask::off() } }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AntiAlias {
    /// How much larger the picture is made while the edges are smoothed (it is shrunk again after).
    pub rfactor: f32,
    /// How long (in pixels of the larger picture) the smoothing along an edge reaches, and how many times it is repeated.
    pub reach: f32,
    pub passes: u8,
    /// Clean up what the smoothing leaves (median of the result, the original and a bilateral blur), as `based_aa` does.
    pub postfilter: bool,
    pub target: Target,
    pub mask: Mask,
}

impl Default for AntiAlias {
    fn default() -> AntiAlias {
        AntiAlias { rfactor: 2.0, reach: 3.0, passes: 2, postfilter: true, target: Target::Both, mask: Mask { enabled: true, ..Mask::off() } }
    }
}

// ------------------------------------------------------------------------------------------------------------------------- planes
#[derive(Clone)]
pub struct Plane {
    pub w: usize,
    pub h: usize,
    pub v: Vec<f32>,
}

impl Plane {
    fn new(w: usize, h: usize) -> Plane {
        Plane { w, h, v: vec![0.0; w * h] }
    }
    /// Direct single-channel resize (area-average for shrinking, Catmull-Rom clamped or bilinear for growing).
    fn resized(&self, w: usize, h: usize, mode: Resample) -> Plane {
        let (w, h) = (w.clamp(1, 16384), h.clamp(1, 16384));
        if (w, h) == (self.w, self.h) || self.w == 0 || self.h == 0 {
            return self.clone();
        }
        let area_weights = |src_n: usize, dst_n: usize| -> Vec<Vec<(usize, f32)>> {
            let scale = src_n as f32 / dst_n as f32;
            (0..dst_n)
                .map(|o| {
                    let (a, b) = (o as f32 * scale, (o + 1) as f32 * scale);
                    let mut ws = Vec::new();
                    let (i0, i1) = (a.floor() as usize, (b.ceil() as usize).min(src_n));
                    for i in i0..i1 {
                        let lo = a.max(i as f32);
                        let hi = b.min((i + 1) as f32);
                        if hi > lo {
                            ws.push((i, (hi - lo) / scale));
                        }
                    }
                    ws
                })
                .collect()
        };
        let shrink = |p: &Plane, dw: usize, dh: usize| -> Plane {
            let tmp_owned: Plane;
            let mid: &Plane = if dw != p.w {
                let wx = area_weights(p.w, dw);
                let mut tmp = Plane::new(dw, p.h);
                tmp.v.par_chunks_mut(dw).enumerate().for_each(|(y, row_out)| {
                    let row_in = &p.v[y * p.w..(y + 1) * p.w];
                    for (x, ws) in wx.iter().enumerate() {
                        let mut acc = 0.0f32;
                        for &(i, k) in ws {
                            acc += row_in[i] * k;
                        }
                        row_out[x] = acc;
                    }
                });
                if dh == p.h {
                    return tmp;
                }
                tmp_owned = tmp;
                &tmp_owned
            } else {
                p
            };
            let wy = area_weights(mid.h, dh);
            let mut out = Plane::new(dw, dh);
            out.v.par_chunks_mut(dw).enumerate().for_each(|(y, row_out)| {
                for &(j, k) in &wy[y] {
                    let row_in = &mid.v[j * dw..(j + 1) * dw];
                    for (dst, &v) in row_out.iter_mut().zip(row_in) {
                        *dst += v * k;
                    }
                }
            });
            out
        };
        let (dw, dh) = (w.min(self.w), h.min(self.h));
        let shrunk_owned: Plane;
        let src: &Plane = if (dw, dh) == (w, h) {
            return shrink(self, dw, dh);
        } else if (dw, dh) != (self.w, self.h) {
            shrunk_owned = shrink(self, dw, dh);
            &shrunk_owned
        } else {
            self
        };
        let (kx, ky) = (src.w as f32 / w as f32, src.h as f32 / h as f32);
        let (sw_i, sh_i) = (src.w as i64 - 1, src.h as i64 - 1);
        let mut out = Plane::new(w, h);
        if mode == Resample::Crisp {
            let catmull = |t: f32| -> [f32; 4] {
                let (t2, t3) = (t * t, t * t * t);
                [0.5 * (-t3 + 2.0 * t2 - t), 0.5 * (3.0 * t3 - 5.0 * t2 + 2.0), 0.5 * (-3.0 * t3 + 4.0 * t2 + t), 0.5 * (t3 - t2)]
            };
            let x_taps: Vec<([usize; 4], [f32; 4])> = (0..w)
                .map(|x| {
                    let u = (x as f32 + 0.5) * kx - 0.5;
                    let ix = u.floor();
                    let tx = u - ix;
                    let ix = ix as i64;
                    (
                        [
                            (ix - 1).clamp(0, sw_i) as usize,
                            ix.clamp(0, sw_i) as usize,
                            (ix + 1).clamp(0, sw_i) as usize,
                            (ix + 2).clamp(0, sw_i) as usize,
                        ],
                        catmull(tx),
                    )
                })
                .collect();
            out.v.par_chunks_mut(w).enumerate().for_each(|(y, row_out)| {
                let v = (y as f32 + 0.5) * ky - 0.5;
                let iy = v.floor();
                let ty = v - iy;
                let iy = iy as i64;
                let wy = catmull(ty);
                let y0 = (iy - 1).clamp(0, sh_i) as usize;
                let y1 = iy.clamp(0, sh_i) as usize;
                let y2 = (iy + 1).clamp(0, sh_i) as usize;
                let y3 = (iy + 2).clamp(0, sh_i) as usize;
                let r0 = &src.v[y0 * src.w..(y0 + 1) * src.w];
                let r1 = &src.v[y1 * src.w..(y1 + 1) * src.w];
                let r2 = &src.v[y2 * src.w..(y2 + 1) * src.w];
                let r3 = &src.v[y3 * src.w..(y3 + 1) * src.w];
                for (dst, &(cols, wx)) in row_out.iter_mut().zip(&x_taps) {
                    let s0 = r0[cols[0]] * wx[0] + r0[cols[1]] * wx[1] + r0[cols[2]] * wx[2] + r0[cols[3]] * wx[3];
                    let p11 = r1[cols[1]];
                    let p12 = r1[cols[2]];
                    let s1 = r1[cols[0]] * wx[0] + p11 * wx[1] + p12 * wx[2] + r1[cols[3]] * wx[3];
                    let p21 = r2[cols[1]];
                    let p22 = r2[cols[2]];
                    let s2 = r2[cols[0]] * wx[0] + p21 * wx[1] + p22 * wx[2] + r2[cols[3]] * wx[3];
                    let s3 = r3[cols[0]] * wx[0] + r3[cols[1]] * wx[1] + r3[cols[2]] * wx[2] + r3[cols[3]] * wx[3];
                    let val = s0 * wy[0] + s1 * wy[1] + s2 * wy[2] + s3 * wy[3];
                    let lo = p11.min(p12).min(p21).min(p22);
                    let hi = p11.max(p12).max(p21).max(p22);
                    *dst = val.clamp(lo, hi);
                }
            });
        } else {
            let x_taps: Vec<(usize, usize, f32)> = (0..w)
                .map(|x| {
                    let u = (x as f32 + 0.5) * kx - 0.5;
                    let ix = u.floor();
                    let tx = u - ix;
                    let ix = ix as i64;
                    (ix.clamp(0, sw_i) as usize, (ix + 1).clamp(0, sw_i) as usize, tx)
                })
                .collect();
            out.v.par_chunks_mut(w).enumerate().for_each(|(y, row_out)| {
                let v = (y as f32 + 0.5) * ky - 0.5;
                let iy = v.floor();
                let ty = v - iy;
                let iy = iy as i64;
                let y0 = iy.clamp(0, sh_i) as usize;
                let y1 = (iy + 1).clamp(0, sh_i) as usize;
                let r0 = &src.v[y0 * src.w..(y0 + 1) * src.w];
                let r1 = &src.v[y1 * src.w..(y1 + 1) * src.w];
                let ity = 1.0 - ty;
                for (dst, &(x0, x1, tx)) in row_out.iter_mut().zip(&x_taps) {
                    let itx = 1.0 - tx;
                    *dst = (r0[x0] * itx + r0[x1] * tx) * ity + (r1[x0] * itx + r1[x1] * tx) * ty;
                }
            });
        }
        out
    }
    fn zip(&self, o: &Plane, f: impl Fn(f32, f32) -> f32 + Sync) -> Plane {
        Plane { w: self.w, h: self.h, v: self.v.par_iter().zip(o.v.par_iter()).map(|(&a, &b)| f(a, b)).collect() }
    }
    fn map(&self, f: impl Fn(f32) -> f32 + Sync) -> Plane {
        Plane { w: self.w, h: self.h, v: self.v.par_iter().map(|&a| f(a)).collect() }
    }
}

pub fn gauss_blur(p: &Plane, sigma: f32) -> Plane {
    if sigma <= 0.05 || p.w == 0 || p.h == 0 {
        return p.clone();
    }
    let r = (sigma * 3.0).ceil().max(1.0) as usize;
    let ri = r as i64;
    let k: Vec<f32> = (-ri..=ri).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = k.iter().sum();
    let k: Vec<f32> = k.iter().map(|v| v / sum).collect();
    let (w, h) = (p.w, p.h);
    let (wi, hi) = (w as i64 - 1, h as i64 - 1);

    let mut tmp = vec![0.0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row_out)| {
        let row_in = &p.v[y * w..(y + 1) * w];
        let left_end = r.min(w);
        let right_start = w.saturating_sub(r).max(left_end);
        for x in 0..left_end {
            let mut acc = 0.0f32;
            for (ki, &wk) in k.iter().enumerate() {
                let xx = (x as i64 + ki as i64 - ri).clamp(0, wi) as usize;
                acc += row_in[xx] * wk;
            }
            row_out[x] = acc;
        }
        for x in left_end..right_start {
            let mut acc = 0.0f32;
            for (&v, &wk) in row_in[x - r..x + r + 1].iter().zip(&k) {
                acc += v * wk;
            }
            row_out[x] = acc;
        }
        for x in right_start..w {
            let mut acc = 0.0f32;
            for (ki, &wk) in k.iter().enumerate() {
                let xx = (x as i64 + ki as i64 - ri).clamp(0, wi) as usize;
                acc += row_in[xx] * wk;
            }
            row_out[x] = acc;
        }
    });

    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row_out)| {
        for (ki, &wk) in k.iter().enumerate() {
            let yy = (y as i64 + ki as i64 - ri).clamp(0, hi) as usize;
            let row_in = &tmp[yy * w..(yy + 1) * w];
            for (dst, &v) in row_out.iter_mut().zip(row_in) {
                *dst += v * wk;
            }
        }
    });
    Plane { w, h, v: out }
}

/// 3 x 3 box blur, `passes` times (separable 1D passes).
fn box3(p: &Plane, passes: usize) -> Plane {
    if passes == 0 || p.w == 0 || p.h == 0 {
        return p.clone();
    }
    let (w, h) = (p.w, p.h);
    let mut cur = p.v.clone();
    let mut tmp = vec![0.0f32; w * h];
    const INV3: f32 = 1.0 / 3.0;
    for _ in 0..passes {
        let src = &cur;
        tmp.par_chunks_mut(w).enumerate().for_each(|(y, row_out)| {
            let row_in = &src[y * w..(y + 1) * w];
            if w == 1 {
                row_out[0] = row_in[0];
                return;
            }
            row_out[0] = (row_in[0] + row_in[0] + row_in[1]) * INV3;
            for x in 1..w - 1 {
                row_out[x] = (row_in[x - 1] + row_in[x] + row_in[x + 1]) * INV3;
            }
            row_out[w - 1] = (row_in[w - 2] + row_in[w - 1] + row_in[w - 1]) * INV3;
        });
        let mid = &tmp;
        cur.par_chunks_mut(w).enumerate().for_each(|(y, row_out)| {
            let r0 = &mid[y.saturating_sub(1) * w..(y.saturating_sub(1) + 1) * w];
            let r1 = &mid[y * w..(y + 1) * w];
            let y2 = (y + 1).min(h - 1);
            let r2 = &mid[y2 * w..(y2 + 1) * w];
            for x in 0..w {
                row_out[x] = (r0[x] + r1[x] + r2[x]) * INV3;
            }
        });
    }
    Plane { w, h, v: cur }
}

/// Grows (`max`) or shrinks the bright parts by `rx` and `ry` pixels, with a rectangle or an ellipse.
fn morph(p: &Plane, rx: usize, ry: usize, max: bool, ellipse: bool) -> Plane {
    if (rx == 0 && ry == 0) || p.w == 0 || p.h == 0 {
        return p.clone();
    }
    let (w, h) = (p.w, p.h);
    let (rxi, ryi) = (rx as i64, ry as i64);
    let (wi, hi) = (w as i64 - 1, h as i64 - 1);
    if !ellipse {
        let mut tmp = vec![0.0f32; w * h];
        if rx > 0 {
            tmp.par_chunks_mut(w).enumerate().for_each(|(y, row_out)| {
                let row_in = &p.v[y * w..(y + 1) * w];
                let left_end = rx.min(w);
                let right_start = w.saturating_sub(rx).max(left_end);
                for x in 0..left_end {
                    let mut acc = row_in[0];
                    for dx in -rxi..=rxi {
                        let v = row_in[(x as i64 + dx).clamp(0, wi) as usize];
                        acc = if max { acc.max(v) } else { acc.min(v) };
                    }
                    row_out[x] = acc;
                }
                for x in left_end..right_start {
                    let s = &row_in[x - rx..=x + rx];
                    let mut acc = s[0];
                    for &v in &s[1..] {
                        acc = if max { acc.max(v) } else { acc.min(v) };
                    }
                    row_out[x] = acc;
                }
                for x in right_start..w {
                    let mut acc = row_in[w - 1];
                    for dx in -rxi..=rxi {
                        let v = row_in[(x as i64 + dx).clamp(0, wi) as usize];
                        acc = if max { acc.max(v) } else { acc.min(v) };
                    }
                    row_out[x] = acc;
                }
            });
        } else {
            tmp.copy_from_slice(&p.v);
        }
        if ry == 0 {
            return Plane { w, h, v: tmp };
        }
        let mut out = vec![0.0f32; w * h];
        out.par_chunks_mut(w).enumerate().for_each(|(y, row_out)| {
            let y0 = (y as i64 - ryi).clamp(0, hi) as usize;
            row_out.copy_from_slice(&tmp[y0 * w..(y0 + 1) * w]);
            for dy in -ryi + 1..=ryi {
                let yy = (y as i64 + dy).clamp(0, hi) as usize;
                let row_in = &tmp[yy * w..(yy + 1) * w];
                if max {
                    for (dst, &v) in row_out.iter_mut().zip(row_in) {
                        *dst = dst.max(v);
                    }
                } else {
                    for (dst, &v) in row_out.iter_mut().zip(row_in) {
                        *dst = dst.min(v);
                    }
                }
            }
        });
        return Plane { w, h, v: out };
    }
    let offs: Vec<(i64, i64)> = (-ryi..=ryi)
        .flat_map(|dy| (-rxi..=rxi).map(move |dx| (dx, dy)))
        .filter(|&(dx, dy)| (dx * dx) as f32 / ((rx * rx).max(1)) as f32 + (dy * dy) as f32 / ((ry * ry).max(1)) as f32 <= 1.0 + 1e-3)
        .collect();
    let wi_is = w as isize;
    let offs_delta: Vec<isize> = offs.iter().map(|&(dx, dy)| dy as isize * wi_is + dx as isize).collect();
    let mut out = vec![0.0f32; w * h];
    let x_safe_end = w.saturating_sub(rx);
    let y_safe_end = h.saturating_sub(ry);
    out.par_chunks_mut(w).enumerate().for_each(|(y, row_out)| {
        let yi = y as i64;
        let row_base = y * w;
        let y_int = y >= ry && y < y_safe_end;
        for (x, dst) in row_out.iter_mut().enumerate() {
            if y_int && x >= rx && x < x_safe_end {
                let ii = (row_base + x) as isize;
                let mut acc = p.v[(ii + offs_delta[0]) as usize];
                if max {
                    for &d in &offs_delta[1..] {
                        acc = acc.max(p.v[(ii + d) as usize]);
                    }
                } else {
                    for &d in &offs_delta[1..] {
                        acc = acc.min(p.v[(ii + d) as usize]);
                    }
                }
                *dst = acc;
            } else {
                let xi = x as i64;
                let mut acc = p.v[(yi + offs[0].1).clamp(0, hi) as usize * w + (xi + offs[0].0).clamp(0, wi) as usize];
                if max {
                    for &(dx, dy) in &offs[1..] {
                        let v = p.v[(yi + dy).clamp(0, hi) as usize * w + (xi + dx).clamp(0, wi) as usize];
                        acc = acc.max(v);
                    }
                } else {
                    for &(dx, dy) in &offs[1..] {
                        let v = p.v[(yi + dy).clamp(0, hi) as usize * w + (xi + dx).clamp(0, wi) as usize];
                        acc = acc.min(v);
                    }
                }
                *dst = acc;
            }
        }
    });
    Plane { w, h, v: out }
}

fn binarize(p: &Plane, thr: f32) -> Plane {
    p.map(|v| if v > thr { 255.0 } else { 0.0 })
}

fn median3(a: f32, b: f32, c: f32) -> f32 {
    a.max(b).min(a.min(b).max(c))
}

// ------------------------------------------------------------------------------------------------------------------------- masks
const ROB3: [[f32; 9]; 4] = [[1., 1., 1., 0., 0., 0., -1., -1., -1.], [1., 1., 0., 1., 0., -1., 0., -1., -1.], [1., 0., -1., 1., 0., -1., 1., 0., -1.], [0., -1., -1., 1., 0., -1., 1., 1., 0.]];
const ROB5: [[f32; 9]; 4] = [[1., 2., 1., 0., 0., 0., -1., -2., -1.], [2., 1., 0., 1., 0., -1., 0., -1., -2.], [1., 0., -1., 2., 0., -2., 1., 0., -1.], [0., -1., -2., 1., 0., -1., 2., 1., 0.]];
const KIRSCH: [[f32; 9]; 8] = [
    [5., 5., 5., -3., 0., -3., -3., -3., -3.],
    [5., 5., -3., 5., 0., -3., -3., -3., -3.],
    [5., -3., -3., 5., 0., -3., 5., -3., -3.],
    [-3., -3., -3., 5., 0., -3., 5., 5., -3.],
    [-3., -3., -3., -3., 0., -3., 5., 5., 5.],
    [-3., -3., -3., -3., 0., 5., -3., 5., 5.],
    [-3., -3., 5., -3., 0., 5., -3., -3., 5.],
    [-3., 5., 5., -3., 0., 5., -3., -3., -3.],
];

#[inline(always)]
fn dot9(n: &[f32; 9], k: &[f32; 9]) -> f32 {
    n[0] * k[0] + n[1] * k[1] + n[2] * k[2] + n[3] * k[3] + n[4] * k[4] + n[5] * k[5] + n[6] * k[6] + n[7] * k[7] + n[8] * k[8]
}

/// The edges of a plane, 0 to 255 (strong edges saturate).
pub fn edge_mask(p: &Plane, kind: Edge) -> Plane {
    let (w, h) = (p.w, p.h);
    if w == 0 || h == 0 {
        return Plane::new(w, h);
    }
    let run_3x3 = |eval: &(dyn Fn([f32; 9]) -> f32 + Sync)| -> Plane {
        let mut out = vec![0.0f32; w * h];
        out.par_chunks_mut(w).enumerate().for_each(|(y, row_out)| {
            let ym = y.saturating_sub(1);
            let yp = (y + 1).min(h - 1);
            let r0 = &p.v[ym * w..(ym + 1) * w];
            let r1 = &p.v[y * w..(y + 1) * w];
            let r2 = &p.v[yp * w..(yp + 1) * w];
            for (x, dst) in row_out.iter_mut().enumerate() {
                let xm = x.saturating_sub(1);
                let xp = (x + 1).min(w - 1);
                *dst = eval([r0[xm], r0[x], r0[xp], r1[xm], r1[x], r1[xp], r2[xm], r2[x], r2[xp]]);
            }
        });
        Plane { w, h, v: out }
    };
    let pair = |gx: [f32; 9], gy: [f32; 9], div: f32| {
        let inv = 1.0 / div;
        run_3x3(&|n| {
            let a = dot9(&n, &gx) * inv;
            let b = dot9(&n, &gy) * inv;
            (a * a + b * b).sqrt().min(255.0)
        })
    };
    let compass = |ks: &[[f32; 9]]| {
        run_3x3(&|n| {
            let mut m = 0.0f32;
            for k in ks {
                m = m.max(dot9(&n, k).abs());
            }
            m.min(255.0)
        })
    };
    match kind {
        Edge::Sobel => pair([1., 0., -1., 2., 0., -2., 1., 0., -1.], [1., 2., 1., 0., 0., 0., -1., -2., -1.], 1.0),
        Edge::Prewitt => pair([1., 0., -1., 1., 0., -1., 1., 0., -1.], [1., 1., 1., 0., 0., 0., -1., -1., -1.], 1.0),
        Edge::Scharr => pair([-3., 0., 3., -10., 0., 10., -3., 0., 3.], [-3., -10., -3., 0., 0., 0., 3., 10., 3.], 3.0),
        Edge::Kirsch => compass(&KIRSCH),
        Edge::Robinson3 => compass(&ROB3),
        Edge::Robinson5 => compass(&ROB5),
    }
}

/// The mask of `m` for the planes of a picture (0 to 255; 255 = the filter is applied in full).
fn build_mask(planes: &[&Plane], m: &Mask) -> Plane {
    let (w, h) = (planes[0].w, planes[0].h);
    let mut edges = Plane::new(w, h);
    for p in planes {
        edges = edges.zip(&edge_mask(p, m.edge), f32::max);
    }
    let mut mask = binarize(&edges, m.threshold);
    for _ in 0..m.grow {
        mask = morph(&mask, 1, 1, true, false);
    }
    mask = box3(&mask, m.soften as usize);
    if m.invert {
        mask = mask.map(|v| 255.0 - v);
    }
    mask
}

// ------------------------------------------------------------------------------------------------------------------------- dehalo
/// `dehalo_alpha` (vsdehalo): blurs the picture and takes the blur in where an edge got weaker by it (the halo around an edge, not the edge
/// itself), keeps the result between the extremes of what was there and then takes away the wanted part of the difference.
fn dehalo_alpha(src: &Plane, o: &Dehalo, rounds: usize) -> Plane {
    let mut work = src.clone();
    for _ in 0..rounds {
        let mut dehalo = gauss_blur(&work, o.blur);
        if o.lowsens >= 0.0 && o.highsens >= 0.0 {
            let gx = morph(&work, 1, 1, true, false).zip(&morph(&work, 1, 1, false, false), |a, b| a - b);
            let gy_clip = morph(&dehalo, 1, 1, true, false).zip(&morph(&dehalo, 1, 1, false, false), |a, b| a - b);
            let (low, high) = (o.lowsens * 255.0 / 100.0, o.highsens / 100.0);
            // where the edge is weakened by the blur a lot, the picture is kept
            let mask = gx.zip(&gy_clip, |x, y| {
                let ratio = if x == 0.0 { 0.0 } else { (x - y) / x * 255.0 };
                let m = (ratio - low) * ((x + 256.0) / 512.0 + high);
                m.clamp(0.0, 255.0) / 255.0
            });
            let blurred = &dehalo;
            dehalo = Plane { w: work.w, h: work.h, v: (0..work.v.len()).into_par_iter().map(|i| blurred.v[i] + (work.v[i] - blurred.v[i]) * mask.v[i]).collect() };
        }
        // keep it between the extremes of the dehaloed picture, at a larger size to avoid aliasing
        let lo = morph(&dehalo, 1, 1, false, false);
        let hi = morph(&dehalo, 1, 1, true, false);
        let clamped = if (o.ss - 1.0).abs() < 1e-3 {
            Plane { w: work.w, h: work.h, v: (0..work.v.len()).into_par_iter().map(|i| work.v[i].clamp(lo.v[i], hi.v[i])).collect() }
        } else {
            let (sw, sh) = (((work.w as f32 * o.ss).round() as usize).max(1), ((work.h as f32 * o.ss).round() as usize).max(1));
            let c = work.resized(sw, sh, Resample::Crisp);
            let (l, h) = (lo.resized(sw, sh, Resample::Smooth), hi.resized(sw, sh, Resample::Smooth));
            let r = Plane { w: sw, h: sh, v: (0..c.v.len()).into_par_iter().map(|i| c.v[i].clamp(l.v[i].min(h.v[i]), l.v[i].max(h.v[i]))).collect() };
            r.resized(work.w, work.h, Resample::Crisp)
        };
        // the wanted part of what is taken away: bright halos (the picture is brighter than the result) and dark ones
        work = Plane {
            w: work.w,
            h: work.h,
            v: (0..work.v.len())
                .into_par_iter()
                .map(|i| {
                    let d = work.v[i] - clamped.v[i];
                    work.v[i] - d * if d > 0.0 { o.brightstr } else { o.darkstr }
                })
                .collect(),
        };
    }
    work
}

/// The mask of `fine_dehalo` (vsdehalo.FineDehalo.Masks): where halos are, without the lines and details next to them.
fn fine_dehalo_mask(y: &Plane, o: &Dehalo) -> Plane {
    let edges = edge_mask(y, o.edge);
    let scale = |v: f32, lo: f32, hi: f32| ((v - lo) / (hi - lo).max(1e-3) * 255.0).clamp(0.0, 255.0);
    let strong = edges.map(|v| scale(v, o.thmi, o.thma));
    let (rx, ry) = (o.rx as usize, o.ry as usize);
    let large = morph(&strong, rx, ry, true, false);
    let light = edges.map(|v| scale(v, o.thlimi, o.thlima));
    // two edges that are close (both sides of a line) are not touched: grow, make solid, shrink back, smooth
    let mut shrink = morph(&light, rx, ry, true, true);
    shrink = binarize(&shrink, 0.25 * 255.0);
    shrink = morph(&shrink, rx, ry, false, true);
    shrink = box3(&box3(&shrink, 1), 1);
    let base = Plane {
        w: y.w,
        h: y.h,
        v: (0..y.v.len())
            .into_par_iter()
            .map(|i| {
                let m = if o.exclude { strong.v[i].max(shrink.v[i]) } else { strong.v[i] };
                let d = (large.v[i] - m) * 2.0;
                let d = if o.edgeproc > 0.0 { d + d * strong.v[i] / 255.0 * o.edgeproc * 2.0 / 3.0 } else { d };
                d.clamp(0.0, 255.0)
            })
            .collect(),
    };
    box3(&base, 1).map(|v| (v * 2.0).clamp(0.0, 255.0))
}

impl Plane {
    /// `orig` moved towards `filtered` by `mask` (0 to 255).
    fn merged(orig: &Plane, filtered: &Plane, mask: &Plane) -> Plane {
        Plane { w: orig.w, h: orig.h, v: (0..orig.v.len()).into_par_iter().map(|i| orig.v[i] + (filtered.v[i] - orig.v[i]) * mask.v[i] / 255.0).collect() }
    }
}

// ------------------------------------------------------------------------------------------------------------------------ denoise
/// Non-local means with the Welsch weight of KNLMeansCL: for every shift in the search window, the squared difference between the picture and
/// the shifted one is averaged over the similarity window (a box blur, so the cost does not grow with the window), turned into a weight and
/// accumulated.
fn nl_means(p: &Plane, h: f32, a: usize, s: usize) -> Plane {
    let (w, hh) = (p.w, p.h);
    if w == 0 || hh == 0 {
        return p.clone();
    }
    let exp_lut = neg_exp_lut();
    let inv_h2 = 1.0 / (2.0 * h * h).max(1e-3); // h is about the spread of the noise to be taken away
    let n = w * hh;
    let (wi, hi) = (w as i64 - 1, hh as i64 - 1);
    let mut acc = vec![0.0f32; n];
    let mut wsum = vec![0.0f32; n];
    let mut wmax = vec![0.0f32; n];
    // Reuse scratch buffers across all search shifts instead of reallocating 3 planes per shift.
    let mut diff = vec![0.0f32; n];
    let mut tmp = vec![0.0f32; n];
    let mut dist = vec![0.0f32; n];
    for dy in -(a as i64)..=a as i64 {
        for dx in -(a as i64)..=a as i64 {
            if dx == 0 && dy == 0 {
                continue;
            }
            diff.par_chunks_mut(w).enumerate().for_each(|(y, row_out)| {
                let sy = (y as i64 + dy).clamp(0, hi) as usize;
                let r_cur = &p.v[y * w..(y + 1) * w];
                let r_sh = &p.v[sy * w..(sy + 1) * w];
                for (x, dst) in row_out.iter_mut().enumerate() {
                    let sx = (x as i64 + dx).clamp(0, wi) as usize;
                    let d = r_cur[x] - r_sh[sx];
                    *dst = d * d;
                }
            });
            box_blur_r_into(&diff, &mut tmp, &mut dist, w, hh, s);
            let dist_slice = if s == 0 { &diff } else { &dist };
            acc.par_chunks_mut(w)
                .zip(wsum.par_chunks_mut(w))
                .zip(wmax.par_chunks_mut(w))
                .enumerate()
                .for_each(|(y, ((ac_row, ws_row), wm_row))| {
                    let sy = (y as i64 + dy).clamp(0, hi) as usize;
                    let r_sh = &p.v[sy * w..(sy + 1) * w];
                    let r_dist = &dist_slice[y * w..(y + 1) * w];
                    for x in 0..w {
                        let sx = (x as i64 + dx).clamp(0, wi) as usize;
                        let wgt = fast_neg_exp(r_dist[x] * inv_h2, exp_lut);
                        ac_row[x] += wgt * r_sh[sx];
                        ws_row[x] += wgt;
                        wm_row[x] = wm_row[x].max(wgt);
                    }
                });
        }
    }
    Plane {
        w,
        h: hh,
        v: (0..n).into_par_iter().map(|i| (acc[i] + wmax[i].max(1e-3) * p.v[i]) / (wsum[i] + wmax[i].max(1e-3))).collect(),
    }
}

/// Separable box blur of radius `r` (edges repeat) writing into `out` via `tmp`, using an O(w) sliding sum horizontally
/// and sequential row-major accumulation vertically.
fn box_blur_r_into(src: &[f32], tmp: &mut [f32], out: &mut [f32], w: usize, h: usize, r: usize) {
    if r == 0 || w == 0 || h == 0 {
        return;
    }
    let ri = r as i64;
    let inv_n = 1.0 / (2 * r + 1) as f32;
    let (wi, hi) = (w as i64 - 1, h as i64 - 1);
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row_out)| {
        let row_in = &src[y * w..(y + 1) * w];
        let mut s = 0.0f32;
        for i in -ri..=ri {
            s += row_in[i.clamp(0, wi) as usize];
        }
        row_out[0] = s * inv_n;
        for x in 1..w {
            let sub = row_in[(x as i64 - ri - 1).clamp(0, wi) as usize];
            let add = row_in[(x as i64 + ri).clamp(0, wi) as usize];
            s += add - sub;
            row_out[x] = s * inv_n;
        }
    });
    let mid: &[f32] = tmp;
    out.par_chunks_mut(w).enumerate().for_each(|(y, row_out)| {
        let y0 = (y as i64 - ri).clamp(0, hi) as usize;
        row_out.copy_from_slice(&mid[y0 * w..(y0 + 1) * w]);
        for dy in -ri + 1..=ri {
            let yy = (y as i64 + dy).clamp(0, hi) as usize;
            let row_in = &mid[yy * w..(yy + 1) * w];
            for (dst, &v) in row_out.iter_mut().zip(row_in) {
                *dst += v;
            }
        }
        for dst in row_out.iter_mut() {
            *dst *= inv_n;
        }
    });
}

/// Precomputed 2048-entry table for `exp(-t)` over `t in [0, 12]` (max error < 5e-4, zero libm `expf` calls).
const EXP_LUT_MAX: f32 = 12.0;
const EXP_LUT_SIZE: usize = 2048;
const EXP_LUT_SCALE: f32 = (EXP_LUT_SIZE - 1) as f32 / EXP_LUT_MAX;

fn neg_exp_lut() -> &'static [f32; EXP_LUT_SIZE] {
    static LUT: std::sync::OnceLock<Box<[f32; EXP_LUT_SIZE]>> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        let t: Box<[f32]> = (0..EXP_LUT_SIZE).map(|i| (-(i as f32) / EXP_LUT_SCALE).exp()).collect();
        t.try_into().unwrap()
    })
}

#[inline(always)]
fn fast_neg_exp(t: f32, lut: &[f32; EXP_LUT_SIZE]) -> f32 {
    if t >= EXP_LUT_MAX {
        0.0
    } else {
        lut[(t * EXP_LUT_SCALE) as usize]
    }
}

fn bilateral(p: &Plane, sigma_s: f32, sigma_r: f32, reference: &Plane) -> Plane {
    let (w, h) = (p.w, p.h);
    if w == 0 || h == 0 {
        return p.clone();
    }
    let exp_lut = neg_exp_lut();
    let r = ((sigma_s * 2.0).ceil() as usize).clamp(1, 12);
    let ri = r as i64;
    let inv_s2 = 1.0 / (2.0 * sigma_s * sigma_s);
    let inv_r2 = 1.0 / (2.0 * sigma_r.max(0.05) * sigma_r.max(0.05));
    let wi_is = w as isize;
    let sp_w: Vec<(i64, i64, isize, f32)> = (-ri..=ri)
        .flat_map(|dy| {
            (-ri..=ri).map(move |dx| {
                let sw = (-((dx * dx + dy * dy) as f32) * inv_s2).exp();
                (dx, dy, dy as isize * wi_is + dx as isize, sw)
            })
        })
        .collect();
    let (wi, hi) = (w as i64 - 1, h as i64 - 1);
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row_out)| {
        let yi = y as i64;
        let row_base = y * w;
        let y_int = y >= r && y + r < h;
        let x_safe_end = w.saturating_sub(r);
        let row_ref = &reference.v[row_base..row_base + w];
        for (x, dst) in row_out.iter_mut().enumerate() {
            let xi = x as i64;
            let c = row_ref[x];
            let (mut sum, mut ws) = (0.0f32, 0.0f32);
            if y_int && x >= r && x < x_safe_end {
                let ii = (row_base + x) as isize;
                for &(_, _, delta, sw) in &sp_w {
                    let idx = (ii + delta) as usize;
                    let d = reference.v[idx] - c;
                    let rw = fast_neg_exp(d * d * inv_r2, exp_lut);
                    let wgt = sw * rw;
                    sum += wgt * p.v[idx];
                    ws += wgt;
                }
            } else {
                for &(dx, dy, _, sw) in &sp_w {
                    let idx = (yi + dy).clamp(0, hi) as usize * w + (xi + dx).clamp(0, wi) as usize;
                    let d = reference.v[idx] - c;
                    let rw = fast_neg_exp(d * d * inv_r2, exp_lut);
                    let wgt = sw * rw;
                    sum += wgt * p.v[idx];
                    ws += wgt;
                }
            }
            *dst = sum / ws;
        }
    });
    Plane { w, h, v: out }
}

fn denoise_plane(p: &Plane, o: &Denoise) -> Plane {
    match o.kind {
        DenoiseKind::NlMeans => nl_means(p, o.h, o.search.clamp(1, 8) as usize, o.patch.clamp(0, 6) as usize),
        DenoiseKind::Bilateral => bilateral(p, o.sigma_s, o.sigma_r, p),
    }
}

// ----------------------------------------------------------------------------------------------------------------------- antialias
/// Smooths along the direction of every edge: the stair-steps of an aliased edge lie along it, so averaging there takes them out while the
/// edge keeps its place. (An edge-directed interpolation in the spirit of EEDI2/SangNom.)
fn smooth_along_edges(p: &Plane, reach: f32, passes: usize) -> Plane {
    let (w, h) = (p.w, p.h);
    if w == 0 || h == 0 {
        return p.clone();
    }
    let mut cur = p.v.clone();
    let mut next = vec![0.0f32; w * h];
    let sigma = (reach / 2.0).max(0.5);
    let taps = reach.ceil().max(1.0) as i64;
    let tap_weights: Vec<(f32, f32)> = (-taps..=taps)
        .map(|t| (t as f32, (-((t * t) as f32) / (2.0 * sigma * sigma)).exp()))
        .collect();
    let inv_ws: f32 = 1.0 / tap_weights.iter().map(|&(_, wgt)| wgt).sum::<f32>();
    let (wi, hi) = (w as i64 - 1, h as i64 - 1);
    let max_x = (w as f32 - 2.0).max(0.0);
    let max_y = (h as f32 - 2.0).max(0.0);
    for _ in 0..passes {
        let c = &cur;
        next.par_chunks_mut(w).enumerate().for_each(|(y, row_out)| {
            let ym = y.saturating_sub(1);
            let yp = (y + 1).min(h - 1);
            let r0 = &c[ym * w..(ym + 1) * w];
            let r1 = &c[y * w..(y + 1) * w];
            let r2 = &c[yp * w..(yp + 1) * w];
            let yf = y as f32;
            for (x, dst) in row_out.iter_mut().enumerate() {
                let xm = x.saturating_sub(1);
                let xp = (x + 1).min(w - 1);
                let centre = r1[x];
                // Sobel gradient from the 8 neighbours loaded once
                let gx = (r0[xp] + 2.0 * r1[xp] + r2[xp]) - (r0[xm] + 2.0 * r1[xm] + r2[xm]);
                let gy = (r2[xm] + 2.0 * r2[x] + r2[xp]) - (r0[xm] + 2.0 * r0[x] + r0[xp]);
                let mag2 = gx * gx + gy * gy;
                if mag2 < 64.0 {
                    *dst = centre;
                    continue;
                }
                let mag = mag2.sqrt();
                let inv_mag = 1.0 / mag;
                let (tx, ty) = (-gy * inv_mag, gx * inv_mag);
                let xf = x as f32;
                let mut sum = 0.0f32;
                for &(tf, wgt) in &tap_weights {
                    let sx = xf + tx * tf;
                    let sy = yf + ty * tf;
                    let val = if sx >= 0.0 && sx <= max_x && sy >= 0.0 && sy <= max_y {
                        let x0 = sx as usize;
                        let y0 = sy as usize;
                        let fx = sx - x0 as f32;
                        let fy = sy - y0 as f32;
                        let b0 = y0 * w + x0;
                        let b1 = b0 + w;
                        let a = c[b0] + (c[b0 + 1] - c[b0]) * fx;
                        let b = c[b1] + (c[b1 + 1] - c[b1]) * fx;
                        a + (b - a) * fy
                    } else {
                        let x0 = sx.floor();
                        let y0 = sy.floor();
                        let fx = sx - x0;
                        let fy = sy - y0;
                        let xi = x0 as i64;
                        let yi = y0 as i64;
                        let x_a = xi.clamp(0, wi) as usize;
                        let x_b = (xi + 1).clamp(0, wi) as usize;
                        let y_a = yi.clamp(0, hi) as usize * w;
                        let y_b = (yi + 1).clamp(0, hi) as usize * w;
                        let a = c[y_a + x_a] + (c[y_a + x_b] - c[y_a + x_a]) * fx;
                        let b = c[y_b + x_a] + (c[y_b + x_b] - c[y_b + x_a]) * fx;
                        a + (b - a) * fy
                    };
                    sum += wgt * val;
                }
                let k = (mag * (1.0 / 48.0)).min(1.0);
                *dst = centre + (sum * inv_ws - centre) * k;
            }
        });
        std::mem::swap(&mut cur, &mut next);
    }
    Plane { w, h, v: cur }
}

fn antialias_plane(p: &Plane, o: &AntiAlias) -> Plane {
    let r = o.rfactor.clamp(1.0, 4.0);
    let (sw, sh) = (((p.w as f32 * r).round() as usize).max(1), ((p.h as f32 * r).round() as usize).max(1));
    let ss = p.resized(sw, sh, Resample::Crisp);
    let aa = smooth_along_edges(&ss, o.reach, o.passes.clamp(1, 6) as usize);
    // area-average back down (Box for whole factors, as in based_aa)
    let mut out = aa.resized(p.w, p.h, Resample::Smooth);
    if o.postfilter {
        let bl = bilateral(&out, 2.0, 1.0, &out);
        out = Plane { w: p.w, h: p.h, v: (0..out.v.len()).into_par_iter().map(|i| median3(out.v[i], p.v[i], bl.v[i])).collect() };
    }
    out
}

// ------------------------------------------------------------------------------------------------------------------------- driver
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// Runs `f` on the planes chosen by `target`, limits the result with the mask (a given `forced` mask replaces the computed one) and puts it
/// back into the picture.
fn run(src: &Img, target: Target, mask: &Mask, f: &(dyn Fn(&Plane) -> Plane + Sync)) -> Img {
    let (w, h) = (src.w, src.h);
    if w == 0 || h == 0 {
        return src.clone();
    }
    let y = Plane { w, h, v: src.px.par_iter().map(|p| (p[0] * LUMA[0] + p[1] * LUMA[1] + p[2] * LUMA[2]) * 255.0).collect() };
    let a = Plane { w, h, v: src.px.par_iter().map(|p| p[3] * 255.0).collect() };
    let y2 = if target != Target::Alpha { f(&y) } else { y.clone() };
    let a2 = if target != Target::Colour { f(&a) } else { a.clone() };
    let m = if mask.enabled {
        build_mask(&[&y, &a], mask)
    } else {
        Plane { w, h, v: vec![255.0; w * h] }
    };
    if mask.show {
        let mm = if mask.enabled { m } else { build_mask(&[&y, &a], &Mask { enabled: true, ..*mask }) };
        return Img { w, h, px: mm.v.iter().map(|&v| [v / 255.0, v / 255.0, v / 255.0, 1.0]).collect() };
    }
    let lw = luma_weights(src, &mask.luma);
    let px = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let k = m.v[i] / 255.0 * lw[i];
            let q = src.px[i];
            let na = (q[3] + (a2.v[i] - a.v[i]) * k / 255.0).clamp(0.0, 1.0);
            let dy = (y2.v[i] - y.v[i]) * k / 255.0;
            [(q[0] + dy).clamp(0.0, na), (q[1] + dy).clamp(0.0, na), (q[2] + dy).clamp(0.0, na), na]
        })
        .collect();
    Img { w, h, px }
}

pub fn dehalo(src: &Img, o: &Dehalo) -> Img {
    run(src, o.target, &o.mask, &|p| {
        let d = dehalo_alpha(p, o, o.iterations.max(1) as usize);
        if o.fine {
            // fine_dehalo: the result only where its own mask (halos, not lines and details) says so
            Plane::merged(p, &d, &fine_dehalo_mask(p, o))
        } else {
            d
        }
    })
}

pub fn denoise(src: &Img, o: &Denoise) -> Img {
    run(src, o.target, &o.mask, &|p| denoise_plane(p, o))
}

pub fn antialias(src: &Img, o: &AntiAlias) -> Img {
    run(src, o.target, &o.mask, &|p| antialias_plane(p, o))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dark band on a grey ground with a bright ring (the overshoot of a sharpened picture) on both sides of it.
    fn halo_band() -> Img {
        let n = 96;
        let mut im = Img::new(n, n);
        for y in 0..n {
            for x in 0..n {
                let v = match x {
                    36..=37 | 50..=51 => 0.68,
                    38..=49 => 0.1,
                    _ => 0.5,
                };
                im.px[y * n + x] = [v, v, v, 1.0];
            }
        }
        im
    }

    #[test]
    fn dehalo_takes_the_bright_ring_away_and_keeps_the_band() {
        let src = halo_band();
        let out = dehalo(&src, &Dehalo { fine: false, ..Dehalo::default() });
        let at = |im: &Img, x: usize| im.px[48 * 96 + x][0];
        assert!(at(&out, 36) < at(&src, 36) - 0.05, "{} -> {}", at(&src, 36), at(&out, 36));
        assert!((at(&out, 44) - 0.1).abs() < 0.03 && (at(&out, 10) - 0.5).abs() < 0.03);
        // with no strength nothing changes
        let none = dehalo(&src, &Dehalo { fine: false, brightstr: 0.0, darkstr: 0.0, ..Dehalo::default() });
        assert!(none.px.iter().zip(&src.px).all(|(a, b)| (a[0] - b[0]).abs() < 1e-4));
    }

    #[test]
    fn fine_dehalo_spares_a_thin_line() {
        let n = 64;
        let mut im = Img::new(n, n);
        for y in 0..n {
            for x in 0..n {
                let v = if x == 32 { 0.9 } else { 0.05 };
                im.px[y * n + x] = [v, v, v, 1.0];
            }
        }
        let out = dehalo(&im, &Dehalo::default());
        assert!(out.px[20 * n + 32][0] > 0.8, "the line is still there: {}", out.px[20 * n + 32][0]);
    }

    #[test]
    fn nl_means_takes_noise_out_of_flat_areas_and_leaves_an_edge() {
        let n = 64;
        let mut im = Img::new(n, n);
        for y in 0..n {
            for x in 0..n {
                let base = if x < 32 { 0.3 } else { 0.7 };
                let noise = ((((x * 7919 + y * 104729) % 97) as f32) / 97.0 - 0.5) * 0.06;
                im.px[y * n + x] = [base + noise, base + noise, base + noise, 1.0];
            }
        }
        let out = denoise(&im, &Denoise { mask: Mask::off(), ..Denoise::default() });
        let spread = |i: &Img| {
            let v: Vec<f32> = (10..22).flat_map(|y| (4..28).map(move |x| (x, y))).map(|(x, y)| i.px[y * n + x][0]).collect();
            let m = v.iter().sum::<f32>() / v.len() as f32;
            (v.iter().map(|a| (a - m).powi(2)).sum::<f32>() / v.len() as f32).sqrt()
        };
        assert!(spread(&out) < spread(&im) * 0.6, "{} -> {}", spread(&im), spread(&out));
        assert!(out.px[20 * n + 40][0] - out.px[20 * n + 24][0] > 0.35, "the edge stays");
    }

    #[test]
    fn antialias_smooths_a_stair_step_edge_and_leaves_flat_areas() {
        let n = 64;
        let mut im = Img::new(n, n);
        for y in 0..n {
            for x in 0..n {
                // a shallow diagonal edge, hard (every row a step)
                let v = if (x as f32) < 10.0 + y as f32 * 0.4 { 1.0 } else { 0.0 };
                im.px[y * n + x] = [v, v, v, 1.0];
            }
        }
        let out = antialias(&im, &AntiAlias::default());
        let grey = |i: &Img| i.px.iter().filter(|p| p[0] > 0.1 && p[0] < 0.9).count();
        assert!(grey(&out) > grey(&im) + 20, "intermediate pixels: {} -> {}", grey(&im), grey(&out));
        assert!((out.px[10 * n + 2][0] - 1.0).abs() < 0.02 && out.px[10 * n + 60][0] < 0.02);
    }

    #[test]
    fn a_mask_limits_a_filter_to_the_edges_and_can_be_shown() {
        let src = halo_band();
        let masked = Mask { enabled: true, ..Mask::off() };
        let out = denoise(&src, &Denoise { mask: Mask { invert: false, ..masked }, ..Denoise::default() });
        // far from any edge nothing may change
        assert_eq!(out.px[2 * 96 + 2], src.px[2 * 96 + 2]);
        let shown = denoise(&src, &Denoise { mask: Mask { show: true, ..masked }, ..Denoise::default() });
        assert!(shown.px[47 * 96 + 37][0] > 0.9 && shown.px[2 * 96 + 2][0] < 0.05);
    }

    /// `cargo test --release -p engine restore::tests::how_long -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn how_long() {
        let n = 1024;
        let mut im = Img::new(n, n);
        for (i, p) in im.px.iter_mut().enumerate() {
            let v = ((i % n) as f32 / n as f32 * 0.8 + (((i * 7919) % 101) as f32) / 2000.0).min(1.0);
            *p = [v, v, v, 1.0];
        }
        for (name, f) in [
            ("dehalo alpha", Box::new(|| dehalo(&im, &Dehalo { fine: false, ..Dehalo::default() })) as Box<dyn Fn() -> Img>),
            ("fine dehalo", Box::new(|| dehalo(&im, &Dehalo::default()))),
            ("nl means", Box::new(|| denoise(&im, &Denoise::default()))),
            ("bilateral", Box::new(|| denoise(&im, &Denoise { kind: DenoiseKind::Bilateral, ..Denoise::default() }))),
            ("antialias", Box::new(|| antialias(&im, &AntiAlias::default()))),
        ] {
            let t = std::time::Instant::now();
            let _ = f();
            eprintln!("{name}: {:?}", t.elapsed());
        }
    }

    #[test]
    fn luma_scaling_applies_a_filter_in_the_dark_and_hardly_in_the_light() {
        assert!((luma_weight(0.0, 6.0) - 1.0).abs() < 1e-6 && luma_weight(1.0, 6.0) < 1e-3);
        assert!(luma_weight(0.1, 6.0) > luma_weight(0.5, 6.0) && luma_weight(0.5, 6.0) > luma_weight(0.8, 6.0));
        let (dark, light) = (luma_weight(0.1, 6.0), luma_weight(0.9, 6.0));
        assert!(dark > 0.15 && light < 0.001, "{dark} {light}");
        // a negative value is the same the other way round, and 0 is off
        let mut im = Img::new(2, 1);
        im.px[0] = [0.05, 0.05, 0.05, 1.0];
        im.px[1] = [0.9, 0.9, 0.9, 1.0];
        let pos = luma_weights(&im, &LumaScale { scaling: 6.0, by_average: false });
        let neg = luma_weights(&im, &LumaScale { scaling: -6.0, by_average: false });
        assert!(pos[0] > 0.3 && pos[1] < 0.01 && neg[1] > 0.3 && neg[0] < 0.01);
        assert_eq!(luma_weights(&im, &LumaScale::off()), vec![1.0, 1.0]);
    }
}
