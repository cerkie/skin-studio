//! Float RGBA images and the resampling / blur / distance-transform primitives the frame generator is built from.
//! Images are stored premultiplied; straight-alpha conversions happen only at the edges.
use rayon::prelude::*;

/// Pixel-count threshold above which rayon parallelization pays off over single-threaded execution.
const PAR_THRESHOLD: usize = 8192;

/// Which kernel `affine` uses when it samples. Catmull-Rom matches the Python reference (and rings at hard edges);
/// Hermite never overshoots and is the default.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sampler {
    Hermite,
    CatmullRom,
}

#[derive(Clone, Debug)]
pub struct Img {
    pub w: usize,
    pub h: usize,
    /// Premultiplied RGBA, 0..1.
    pub px: Vec<[f32; 4]>,
}

/// Round 0..1 to 8 bits with interleaved-gradient noise: the pattern is fixed per pixel position, so the same frame
/// always gives the same bytes, and 0 and 1 stay exact. Without it a soft shadow collapses into visible bands.
fn quantize(v: f32, x: usize, y: usize, channel: usize) -> u8 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.0 || v >= 1.0 {
        return (v * 255.0) as u8;
    }
    let (ox, oy) = (channel as f32 * 41.0 + 5.0, channel as f32 * 13.0 + 3.0);
    let t = (0.06711056 * (x as f32 + ox) + 0.00583715 * (y as f32 + oy)).fract();
    let noise = (52.9829189 * t).fract();
    ((v * 255.0 + noise).floor()).clamp(0.0, 255.0) as u8
}

impl Img {
    pub fn new(w: usize, h: usize) -> Img {
        Img { w, h, px: vec![[0.0; 4]; w * h] }
    }

    /// From straight-alpha 8-bit RGBA.
    pub fn from_straight_u8(w: usize, h: usize, rgba: &[u8]) -> Img {
        let px = rgba
            .chunks_exact(4)
            .map(|c| {
                let a = c[3] as f32 / 255.0;
                [c[0] as f32 / 255.0 * a, c[1] as f32 / 255.0 * a, c[2] as f32 / 255.0 * a, a]
            })
            .collect();
        Img { w, h, px }
    }

    /// Straight-alpha RGBA, rounded to 8 bits.
    pub fn to_straight_u8(&self) -> Vec<u8> {
        let w = self.w.max(1);
        let mut out = vec![0u8; self.px.len() * 4];
        let row_fn = |(y, row_out): (usize, &mut [u8])| {
            let row_px = &self.px[y * w..y * w + row_out.len() / 4];
            for (x, p) in row_px.iter().enumerate() {
                let a = p[3];
                let (r, g, b) = if a > 1e-6 {
                    let inv = 1.0 / a;
                    (p[0] * inv, p[1] * inv, p[2] * inv)
                } else {
                    (0.0, 0.0, 0.0)
                };
                let o = x * 4;
                row_out[o] = quantize(r, x, y, 0);
                row_out[o + 1] = quantize(g, x, y, 1);
                row_out[o + 2] = quantize(b, x, y, 2);
                row_out[o + 3] = quantize(a, x, y, 3);
            }
        };
        if self.px.len() >= PAR_THRESHOLD {
            out.par_chunks_mut(w * 4).enumerate().for_each(row_fn);
        } else {
            out.chunks_mut(w * 4).enumerate().for_each(row_fn);
        }
        out
    }

    /// Premultiplied RGBA rounded to 8 bits (what the GPU atlas stores).
    pub fn to_premult_u8(&self) -> Vec<u8> {
        let w = self.w.max(1);
        let mut out = vec![0u8; self.px.len() * 4];
        let row_fn = |(y, row_out): (usize, &mut [u8])| {
            let row_px = &self.px[y * w..y * w + row_out.len() / 4];
            for (x, p) in row_px.iter().enumerate() {
                let o = x * 4;
                row_out[o] = quantize(p[0], x, y, 0);
                row_out[o + 1] = quantize(p[1], x, y, 1);
                row_out[o + 2] = quantize(p[2], x, y, 2);
                row_out[o + 3] = quantize(p[3], x, y, 3);
            }
        };
        if self.px.len() >= PAR_THRESHOLD {
            out.par_chunks_mut(w * 4).enumerate().for_each(row_fn);
        } else {
            out.chunks_mut(w * 4).enumerate().for_each(row_fn);
        }
        out
    }

    pub fn scale_alpha(&mut self, k: f32) {
        for p in &mut self.px {
            for v in p.iter_mut() {
                *v *= k;
            }
        }
    }

    /// `self` over `below` (both premultiplied).
    pub fn over(&mut self, below: &Img) {
        for (t, b) in self.px.iter_mut().zip(&below.px) {
            let k = 1.0 - t[3];
            for c in 0..4 {
                t[c] += b[c] * k;
            }
        }
    }

    pub fn max_alpha(&self) -> f32 {
        self.px.iter().fold(0.0, |m, p| m.max(p[3]))
    }
}

// ---------------------------------------------------------------------------------------------- kernels
/// Weights for the 4 taps at offsets -1, 0, 1, 2 around floor(x), for fractional position t in [0, 1).
fn taps(t: f32, s: Sampler) -> [f32; 4] {
    match s {
        Sampler::CatmullRom => {
            let (t2, t3) = (t * t, t * t * t);
            [
                0.5 * (-t3 + 2.0 * t2 - t),
                0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
                0.5 * (-3.0 * t3 + 4.0 * t2 + t),
                0.5 * (t3 - t2),
            ]
        }
        Sampler::Hermite => {
            let w = t * t * (3.0 - 2.0 * t); // smoothstep: interpolating, C1, never overshoots
            [0.0, 1.0 - w, w, 0.0]
        }
    }
}

/// Sample at continuous coordinates (pixel i spans [i, i+1]); outside the image is transparent.
fn sample(img: &Img, u: f32, v: f32, s: Sampler) -> [f32; 4] {
    let (fx, fy) = (u - 0.5, v - 0.5);
    let (ix, iy) = (fx.floor(), fy.floor());
    let (wx, wy) = (taps(fx - ix, s), taps(fy - iy, s));
    let (ix, iy) = (ix as i64, iy as i64);
    let mut acc = [0.0f32; 4];
    for (j, wyj) in wy.iter().enumerate() {
        if *wyj == 0.0 {
            continue;
        }
        let y = iy + j as i64 - 1;
        if y < 0 || y >= img.h as i64 {
            continue;
        }
        for (i, wxi) in wx.iter().enumerate() {
            if *wxi == 0.0 {
                continue;
            }
            let x = ix + i as i64 - 1;
            if x < 0 || x >= img.w as i64 {
                continue;
            }
            let p = &img.px[y as usize * img.w + x as usize];
            let wgt = wxi * wyj;
            for c in 0..4 {
                acc[c] += p[c] * wgt;
            }
        }
    }
    acc
}

/// Resample `src` onto a `w`x`h` canvas. `m = [a, b, c, d, e, f]` maps output pixel centers (x+0.5, y+0.5)
/// to source coordinates: sx = a*x + b*y + c, sy = d*x + e*y + f (the same convention PIL's AFFINE uses).
pub fn affine(src: &Img, w: usize, h: usize, m: [f32; 6], s: Sampler) -> Img {
    let mut out = Img::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let (u, v) = (m[0] * px + m[1] * py + m[2], m[3] * px + m[4] * py + m[5]);
            let mut p = sample(src, u, v, s);
            if s == Sampler::CatmullRom {
                for c in p.iter_mut() {
                    *c = c.clamp(0.0, 1.0);
                }
            }
            out.px[y * w + x] = p;
        }
    }
    out
}

fn area_weights(n_in: usize, n_out: usize) -> Vec<Vec<(usize, f32)>> {
    let s = n_in as f64 / n_out as f64;
    (0..n_out)
        .map(|o| {
            let (a, b) = (o as f64 * s, (o + 1) as f64 * s);
            let mut v = Vec::new();
            let mut tot = 0.0;
            for i in (a.floor() as usize)..((b.ceil() as usize).min(n_in)) {
                let cov = (b.min(i as f64 + 1.0) - a.max(i as f64)).max(0.0);
                if cov > 0.0 {
                    v.push((i, cov as f32));
                    tot += cov;
                }
            }
            v.iter_mut().for_each(|e| e.1 /= tot as f32);
            v
        })
        .collect()
}

/// Exact area-average resize (every output pixel is the mean of the source area it covers): no ringing, no
/// halos. The right filter for shrinking.
pub fn resize_area(src: &Img, w: usize, h: usize) -> Img {
    if (w, h) == (src.w, src.h) || src.w == 0 || src.h == 0 || w == 0 || h == 0 {
        return src.clone();
    }
    let tmp_owned: Img;
    let mid: &Img = if w != src.w {
        let wx = area_weights(src.w, w);
        let mut tmp = Img::new(w, src.h);
        let row_fn = |(y, row_out): (usize, &mut [[f32; 4]])| {
            let row_in = &src.px[y * src.w..(y + 1) * src.w];
            for (x, ws) in wx.iter().enumerate() {
                let mut acc = [0.0f32; 4];
                for &(i, k) in ws {
                    let p = &row_in[i];
                    acc[0] += p[0] * k;
                    acc[1] += p[1] * k;
                    acc[2] += p[2] * k;
                    acc[3] += p[3] * k;
                }
                row_out[x] = acc;
            }
        };
        if w * src.h >= PAR_THRESHOLD {
            tmp.px.par_chunks_mut(w).enumerate().for_each(row_fn);
        } else {
            tmp.px.chunks_mut(w).enumerate().for_each(row_fn);
        }
        if h == src.h {
            return tmp;
        }
        tmp_owned = tmp;
        &tmp_owned
    } else {
        src
    };
    let wy = area_weights(mid.h, h);
    let mut out = Img::new(w, h);
    let row_fn = |(y, row_out): (usize, &mut [[f32; 4]])| {
        for &(j, k) in &wy[y] {
            let row_in = &mid.px[j * w..(j + 1) * w];
            for (dst, p) in row_out.iter_mut().zip(row_in) {
                dst[0] += p[0] * k;
                dst[1] += p[1] * k;
                dst[2] += p[2] * k;
                dst[3] += p[3] * k;
            }
        }
    };
    if w * h >= PAR_THRESHOLD {
        out.px.par_chunks_mut(w).enumerate().for_each(row_fn);
    } else {
        out.px.chunks_mut(w).enumerate().for_each(row_fn);
    }
    out
}

// ---------------------------------------------------------------------------------------------- blur
fn gauss_kernel(sigma: f32) -> Vec<f32> {
    let r = (sigma * 3.0).ceil().max(1.0) as i32;
    let mut k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= sum);
    k
}

/// Separable Gaussian blur of a premultiplied image (edges read as transparent).
pub fn blur(src: &Img, sigma: f32) -> Img {
    if sigma <= 0.05 || src.w == 0 || src.h == 0 {
        return src.clone();
    }
    let k = gauss_kernel(sigma);
    let r = k.len() / 2;
    let (w, h) = (src.w, src.h);
    let par = w * h >= PAR_THRESHOLD;

    let mut tmp = Img::new(w, h);
    let h_row = |(y, row_out): (usize, &mut [[f32; 4]])| {
        let row_in = &src.px[y * w..(y + 1) * w];
        let left_end = r.min(w);
        let right_start = w.saturating_sub(r).max(left_end);
        for x in 0..left_end {
            let mut acc = [0.0f32; 4];
            let k_start = r - x;
            let k_end = (r + w - x).min(k.len());
            for ki in k_start..k_end {
                let p = &row_in[x + ki - r];
                let wk = k[ki];
                acc[0] += p[0] * wk;
                acc[1] += p[1] * wk;
                acc[2] += p[2] * wk;
                acc[3] += p[3] * wk;
            }
            row_out[x] = acc;
        }
        for x in left_end..right_start {
            let mut acc = [0.0f32; 4];
            let base = &row_in[x - r..x + r + 1];
            for (p, &wk) in base.iter().zip(&k) {
                acc[0] += p[0] * wk;
                acc[1] += p[1] * wk;
                acc[2] += p[2] * wk;
                acc[3] += p[3] * wk;
            }
            row_out[x] = acc;
        }
        for x in right_start..w {
            let mut acc = [0.0f32; 4];
            let k_start = r.saturating_sub(x);
            let k_end = (r + w - x).min(k.len());
            for ki in k_start..k_end {
                let p = &row_in[x + ki - r];
                let wk = k[ki];
                acc[0] += p[0] * wk;
                acc[1] += p[1] * wk;
                acc[2] += p[2] * wk;
                acc[3] += p[3] * wk;
            }
            row_out[x] = acc;
        }
    };
    if par {
        tmp.px.par_chunks_mut(w).enumerate().for_each(h_row);
    } else {
        tmp.px.chunks_mut(w).enumerate().for_each(h_row);
    }

    let mut out = Img::new(w, h);
    let v_row = |(y, row_out): (usize, &mut [[f32; 4]])| {
        let k_start = r.saturating_sub(y);
        let k_end = (r + h - y).min(k.len());
        for ki in k_start..k_end {
            let yy = y + ki - r;
            let wk = k[ki];
            let row_in = &tmp.px[yy * w..(yy + 1) * w];
            for (dst, p) in row_out.iter_mut().zip(row_in) {
                dst[0] += p[0] * wk;
                dst[1] += p[1] * wk;
                dst[2] += p[2] * wk;
                dst[3] += p[3] * wk;
            }
        }
    };
    if par {
        out.px.par_chunks_mut(w).enumerate().for_each(v_row);
    } else {
        out.px.chunks_mut(w).enumerate().for_each(v_row);
    }
    out
}

/// Blur a single float plane (used for shadows).
pub fn blur_plane(a: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    if sigma <= 0.05 || w == 0 || h == 0 {
        return a.to_vec();
    }
    let k = gauss_kernel(sigma);
    let r = k.len() / 2;
    let par = w * h >= PAR_THRESHOLD;

    let mut tmp = vec![0.0f32; w * h];
    let h_row = |(y, row_out): (usize, &mut [f32])| {
        let row_in = &a[y * w..(y + 1) * w];
        let left_end = r.min(w);
        let right_start = w.saturating_sub(r).max(left_end);
        for x in 0..left_end {
            let mut acc = 0.0f32;
            let k_start = r - x;
            let k_end = (r + w - x).min(k.len());
            for ki in k_start..k_end {
                acc += row_in[x + ki - r] * k[ki];
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
            let k_start = r.saturating_sub(x);
            let k_end = (r + w - x).min(k.len());
            for ki in k_start..k_end {
                acc += row_in[x + ki - r] * k[ki];
            }
            row_out[x] = acc;
        }
    };
    if par {
        tmp.par_chunks_mut(w).enumerate().for_each(h_row);
    } else {
        tmp.chunks_mut(w).enumerate().for_each(h_row);
    }

    let mut out = vec![0.0f32; w * h];
    let v_row = |(y, row_out): (usize, &mut [f32])| {
        let k_start = r.saturating_sub(y);
        let k_end = (r + h - y).min(k.len());
        for ki in k_start..k_end {
            let yy = y + ki - r;
            let wk = k[ki];
            let row_in = &tmp[yy * w..(yy + 1) * w];
            for (dst, &v) in row_out.iter_mut().zip(row_in) {
                *dst += v * wk;
            }
        }
    };
    if par {
        out.par_chunks_mut(w).enumerate().for_each(v_row);
    } else {
        out.chunks_mut(w).enumerate().for_each(v_row);
    }
    out
}

// ---------------------------------------------------------------------------------------------- distance transform
/// Exact squared Euclidean distance from every pixel to the nearest `true` pixel of `inside`
/// (Felzenszwalb & Huttenlocher). `f32::INFINITY` everywhere if nothing is inside.
pub fn edt_sq(inside: &[bool], w: usize, h: usize) -> Vec<f32> {
    const INF: f32 = 1e20;
    let mut d: Vec<f32> = inside.iter().map(|&b| if b { 0.0 } else { INF }).collect();
    let mut f = vec![0.0f32; w.max(h)];
    let mut v = vec![0usize; w.max(h)];
    let mut z = vec![0.0f32; w.max(h) + 1];
    let dt1d = |f: &[f32], n: usize, v: &mut [usize], z: &mut [f32], out: &mut [f32]| {
        let mut k = 0usize;
        v[0] = 0;
        z[0] = -INF;
        z[1] = INF;
        for q in 1..n {
            let mut s;
            loop {
                let p = v[k];
                s = ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * q as f32 - 2.0 * p as f32);
                if s <= z[k] && k > 0 {
                    k -= 1;
                } else {
                    break;
                }
            }
            if s <= z[k] && k == 0 {
                v[0] = q;
                z[0] = -INF;
                z[1] = INF;
            } else {
                k += 1;
                v[k] = q;
                z[k] = s;
                z[k + 1] = INF;
            }
        }
        k = 0;
        for q in 0..n {
            while z[k + 1] < q as f32 {
                k += 1;
            }
            let p = v[k];
            out[q] = ((q as f32 - p as f32).powi(2)) + f[p];
        }
    };
    let mut out = vec![0.0f32; w.max(h)];
    for x in 0..w {
        for y in 0..h {
            f[y] = d[y * w + x];
        }
        dt1d(&f, h, &mut v, &mut z, &mut out);
        for y in 0..h {
            d[y * w + x] = out[y];
        }
    }
    for y in 0..h {
        for x in 0..w {
            f[x] = d[y * w + x];
        }
        dt1d(&f, w, &mut v, &mut z, &mut out);
        for x in 0..w {
            d[y * w + x] = out[x];
        }
    }
    d
}
