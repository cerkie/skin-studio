//! Float RGBA images and the resampling / blur / distance-transform primitives the frame generator is built from.
//! Images are stored premultiplied; straight-alpha conversions happen only at the edges.

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
        let mut out = Vec::with_capacity(self.px.len() * 4);
        for (i, p) in self.px.iter().enumerate() {
            let a = p[3];
            let (r, g, b) = if a > 1e-6 { (p[0] / a, p[1] / a, p[2] / a) } else { (0.0, 0.0, 0.0) };
            let (x, y) = (i % self.w.max(1), i / self.w.max(1));
            for (k, v) in [r, g, b, a].into_iter().enumerate() {
                out.push(quantize(v, x, y, k));
            }
        }
        out
    }

    /// Premultiplied RGBA rounded to 8 bits (what the GPU atlas stores).
    pub fn to_premult_u8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.px.len() * 4);
        for (i, p) in self.px.iter().enumerate() {
            let (x, y) = (i % self.w.max(1), i / self.w.max(1));
            for (k, v) in p.iter().enumerate() {
                out.push(quantize(*v, x, y, k));
            }
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
    if (w, h) == (src.w, src.h) {
        return src.clone();
    }
    let (wx, wy) = (area_weights(src.w, w), area_weights(src.h, h));
    let mut tmp = Img::new(w, src.h);
    for y in 0..src.h {
        for (x, ws) in wx.iter().enumerate() {
            let mut acc = [0.0f32; 4];
            for &(i, k) in ws {
                let p = &src.px[y * src.w + i];
                for c in 0..4 {
                    acc[c] += p[c] * k;
                }
            }
            tmp.px[y * w + x] = acc;
        }
    }
    let mut out = Img::new(w, h);
    for (y, ws) in wy.iter().enumerate() {
        for x in 0..w {
            let mut acc = [0.0f32; 4];
            for &(j, k) in ws {
                let p = &tmp.px[j * w + x];
                for c in 0..4 {
                    acc[c] += p[c] * k;
                }
            }
            out.px[y * w + x] = acc;
        }
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
    if sigma <= 0.05 {
        return src.clone();
    }
    let k = gauss_kernel(sigma);
    let r = (k.len() / 2) as i64;
    let (w, h) = (src.w as i64, src.h as i64);
    let mut tmp = Img::new(src.w, src.h);
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f32; 4];
            for (i, wk) in k.iter().enumerate() {
                let xx = x + i as i64 - r;
                if xx >= 0 && xx < w {
                    let p = &src.px[(y * w + xx) as usize];
                    for c in 0..4 {
                        acc[c] += p[c] * wk;
                    }
                }
            }
            tmp.px[(y * w + x) as usize] = acc;
        }
    }
    let mut out = Img::new(src.w, src.h);
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f32; 4];
            for (i, wk) in k.iter().enumerate() {
                let yy = y + i as i64 - r;
                if yy >= 0 && yy < h {
                    let p = &tmp.px[(yy * w + x) as usize];
                    for c in 0..4 {
                        acc[c] += p[c] * wk;
                    }
                }
            }
            out.px[(y * w + x) as usize] = acc;
        }
    }
    out
}

/// Blur a single float plane (used for shadows).
pub fn blur_plane(a: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let img = Img { w, h, px: a.iter().map(|&v| [0.0, 0.0, 0.0, v]).collect() };
    blur(&img, sigma).px.iter().map(|p| p[3]).collect()
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
