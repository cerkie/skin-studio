//! Debanding on all four channels including alpha (f3kdb / libplacebo style), ported from the Python tool.
//! Random reference offsets and the dither pattern are fixed per pixel position, so identical frames give identical
//! output and moving shapes never shimmer.
use rayon::prelude::*;

use crate::img::Img;

/// What the grain that fills the steps in is like. `Params::grain` is its strength: the spread of the noise in 8-bit levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Grain {
    /// Nothing but the dither that rounding to 8 bits needs.
    None,
    /// Every level between the two limits equally likely.
    Uniform,
    /// Bell-shaped: mostly small, sometimes larger.
    Gaussian,
    /// Shot noise, as in film and photon-limited sensors: the brighter a pixel, the more it varies (in the dark it all but disappears).
    Poisson,
    /// Noise without the clumps of white noise: neighbouring pixels differ, so it has less of a visible texture (what vsdeband makes).
    Blue,
}

impl Grain {
    pub const ALL: [Grain; 5] = [Grain::None, Grain::Uniform, Grain::Gaussian, Grain::Poisson, Grain::Blue];
    pub fn name(self) -> &'static str {
        match self {
            Grain::None => "None",
            Grain::Uniform => "Uniform",
            Grain::Gaussian => "Gaussian",
            Grain::Poisson => "Poisson",
            Grain::Blue => "Blue",
        }
    }
}

/// How a pixel is compared with its reference pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Mode {
    /// Opposite pairs of references, each taken in by how close both are to the pixel (this program's own).
    Pairs,
    /// vszip's mode 6: three separate tests (the average of the references, the largest difference to one of them, and how far the pixel is
    /// from the line between two opposite ones), blended in smoothly.
    Smooth,
    /// vszip's mode 7: as `Smooth`, with the limits made wider where the picture slopes the same way around the pixel (a gradient).
    Angle,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Pairs, Mode::Smooth, Mode::Angle];
    pub fn name(self) -> &'static str {
        match self {
            Mode::Pairs => "Pairs",
            Mode::Smooth => "Smooth (vszip mode 6)",
            Mode::Angle => "Gradient aware (vszip mode 7)",
        }
    }
}

/// The settings that vszip's Deband has beyond neo_f3kdb's.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Advanced {
    pub mode: Mode,
    /// `thr1`: how different a reference may be from the pixel (levels; 0 = the same as the threshold). `Smooth` and `Angle` only.
    pub thr1: f32,
    /// `thr2`: how far the pixel may be from the line between two opposite references (levels; 0 = the same as the threshold). This is the
    /// test that tells banding (a step on a slope) from a detail, and it works on steep slopes however far the references are.
    pub thr2: f32,
    /// `Angle`: how much wider the limits are where the slope runs the same way (1.5 in vszip) and how much the direction may differ (0.15).
    pub angle_boost: f32,
    pub max_angle: f32,
    /// How far the references are placed: 0 = evenly over the radius, otherwise the spread of a Gaussian (1 is vszip's default; smaller keeps
    /// them nearer to the pixel).
    pub dist_sigma: f32,
}

impl Default for Advanced {
    fn default() -> Advanced {
        Advanced { mode: Mode::Pairs, thr1: 0.0, thr2: 0.0, angle_boost: 1.5, max_angle: 0.15, dist_sigma: 0.0 }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub radius: f32,
    pub thr_rgb: f32,
    pub thr_a: f32,
    pub iterations: usize,
    /// The spread of the grain, in 8-bit levels.
    pub grain: f32,
    pub kind: Grain,
    /// One noise value for red, green and blue together (grey grain) instead of one for each.
    pub mono: bool,
    /// Round the result to whole 8-bit levels, with the dither that needs (true), or give it as it is, in floating point, for the caller to
    /// round when the picture is saved (false: keeps the precision for the edits that follow).
    pub quantize: bool,
    /// vsdeband's `LimitFilter`: the change to a pixel is limited to this many 8-bit levels (0: no limit). Beyond `limit` the change is taken
    /// back softly, and from `limit * elast` on altogether (such a change is no banding, it is a detail).
    pub limit: f32,
    pub elast: f32,
    /// As `mdb_bilateral`: the radius narrows from 4/3 to 1/3 of `radius` over the iterations, and the first one uses half the threshold.
    pub narrowing: bool,
    pub adv: Advanced,
}

/// `LimitFilter` of vszip for one value: `flt` limited against `src`.
fn limit_value(flt: f32, src: f32, thr: f32, elast: f32) -> f32 {
    let d = flt - src;
    let a = d.abs();
    let elast = elast.max(1.01);
    if a <= thr {
        flt
    } else if a >= thr * elast {
        src
    } else {
        src + d * (thr * elast - a) / (thr * (elast - 1.0))
    }
}

/// Precomputed 4096-entry (cos, sin) table and exact unit-normal quantile table (mean 0, variance 1).
const LUT_BITS: u32 = 12;
const LUT_SIZE: usize = 1 << LUT_BITS;
const LUT_SHIFT: u32 = 64 - LUT_BITS;

fn trig_lut() -> &'static [(f32, f32); LUT_SIZE] {
    static LUT: std::sync::OnceLock<Box<[(f32, f32); LUT_SIZE]>> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        let mut t = vec![(0.0f32, 0.0f32); LUT_SIZE].into_boxed_slice();
        for (i, slot) in t.iter_mut().enumerate() {
            let a = (i as f32) * (std::f32::consts::TAU / LUT_SIZE as f32);
            *slot = (a.cos(), a.sin());
        }
        t.try_into().unwrap()
    })
}

/// Acklam's rational approximation to the inverse standard normal CDF (accurate to ~1e-7).
fn inv_norm_cdf(p: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969683028665376e1,
        2.209460984245205e2,
        -2.759285104469687e2,
        1.383577518672690e2,
        -3.066479806614716e1,
        2.506628277459239e0,
    ];
    const B: [f64; 5] = [
        -5.447609879822406e1,
        1.615858368580409e2,
        -1.556989798598866e2,
        6.680131188771972e1,
        -1.328068155288572e1,
    ];
    const C: [f64; 6] = [
        -7.784894002430293e-3,
        -3.223964580411365e-1,
        -2.400758277161838e0,
        -2.549732539343734e0,
        4.374664141464968e0,
        2.938163982698783e0,
    ];
    const D: [f64; 4] = [7.784695709041462e-3, 3.224671290700398e-1, 2.445134137142996e0, 3.754408661907416e0];
    const PLOW: f64 = 0.02425;
    const PHIGH: f64 = 1.0 - PLOW;
    if p < PLOW {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p <= PHIGH {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        -(((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    }
}

fn gauss_lut() -> &'static [f32; LUT_SIZE] {
    static LUT: std::sync::OnceLock<Box<[f32; LUT_SIZE]>> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        let mut raw = vec![0.0f64; LUT_SIZE];
        for (i, slot) in raw.iter_mut().enumerate() {
            *slot = inv_norm_cdf((i as f64 + 0.5) / LUT_SIZE as f64);
        }
        let mean = raw.iter().sum::<f64>() / LUT_SIZE as f64;
        let var = raw.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / LUT_SIZE as f64;
        let inv_sd = 1.0 / var.sqrt();
        let t: Box<[f32]> = raw.into_iter().map(|v| ((v - mean) * inv_sd) as f32).collect();
        t.try_into().unwrap()
    })
}

/// A white noise value for a pixel and channel with an average of 0 and a spread of 1, the same every time.
#[inline(always)]
fn white_with_lut(kind: Grain, x: i64, y: i64, ch: u64, g_lut: &[f32; LUT_SIZE]) -> f32 {
    let h = hash(((y as u64) << 32) ^ (x as u64 & 0xffff_ffff) ^ (ch.wrapping_mul(0x9e3779b97f4a7c15)) ^ 0x5851f42d4c957f2d);
    match kind {
        Grain::Uniform => (unit(h) - 0.5) * 3.464_101_6, // sqrt(12)
        _ => g_lut[(h >> LUT_SHIFT) as usize],
    }
}

/// The grain at a pixel, spread 1 (`level` is how bright the pixel is, 0 to 255, for the kind that depends on it).
#[inline(always)]
fn grain_at_with_lut(kind: Grain, x: usize, y: usize, ch: u64, level: f32, g_lut: &[f32; LUT_SIZE]) -> f32 {
    let (xi, yi) = (x as i64, y as i64);
    match kind {
        Grain::None => 0.0,
        Grain::Uniform | Grain::Gaussian => white_with_lut(kind, xi, yi, ch, g_lut),
        Grain::Poisson => white_with_lut(Grain::Gaussian, xi, yi, ch, g_lut) * (level * (1.0 / 255.0)).clamp(0.0, 1.0).sqrt(),
        Grain::Blue => {
            // 8-neighbor high-pass blue-noise kernel: crisp high-frequency spectrum with minimal low-frequency clumping.
            const INV_NORM: f32 = 0.940_720_86; // 1 / sqrt(1.13)
            let g = |dx: i64, dy: i64| white_with_lut(Grain::Gaussian, xi + dx, yi + dy, ch, g_lut);
            let nbr = 0.15 * (g(-1, 0) + g(1, 0) + g(0, -1) + g(0, 1))
                + 0.10 * (g(-1, -1) + g(1, -1) + g(-1, 1) + g(1, 1));
            (g(0, 0) - nbr) * INV_NORM
        }
    }
}

#[cfg(test)]
fn grain_at(kind: Grain, x: usize, y: usize, ch: u64, level: f32) -> f32 {
    grain_at_with_lut(kind, x, y, ch, level, gauss_lut())
}

#[inline(always)]
fn hash(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58476d1ce4e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}

#[inline(always)]
fn unit(h: u64) -> f32 {
    (h >> 40) as f32 * (1.0 / (1u64 << 24) as f32)
}

/// Interleaved-gradient noise in [0, 1) without libm `truncf` calls (arguments are always >= 0).
#[inline(always)]
fn fast_fract_pos(v: f32) -> f32 {
    v - (v as u32) as f32
}

#[inline(always)]
fn ign(x: usize, y: usize, ox: f32, oy: f32) -> f32 {
    let t = fast_fract_pos(0.06711056 * (x as f32 + ox) + 0.00583715 * (y as f32 + oy));
    fast_fract_pos(52.9829189 * t)
}

/// Fast round-to-nearest integer using pure SSE2 conversion (avoids libm `rintf` calls).
#[inline(always)]
fn fast_round_isize(v: f32) -> isize {
    (v + 0.5f32.copysign(v)) as isize
}

#[inline(always)]
fn fast_floor_0_255(v: f32) -> f32 {
    if v <= 0.0 {
        0.0
    } else if v >= 255.0 {
        255.0
    } else {
        (v as u32) as f32
    }
}

/// The normalised gradient direction (0 to 1, vszip's `deband_angle`) of every pixel: a Sobel kernel whose taps are 20 pixels apart, so that
/// the direction is that of the slope around the pixel and not of the noise on it.
fn angle_plane(v: &[f32], w: usize, h: usize) -> Vec<f32> {
    const RD: i64 = 20;
    let (wi, hi) = (w as i64, h as i64);
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
        let yi = y as i64;
        let ym = (yi - RD).clamp(0, hi - 1) as usize * w;
        let y0 = y * w;
        let yp = (yi + RD).clamp(0, hi - 1) as usize * w;
        for (x, dst) in row.iter_mut().enumerate() {
            let xi = x as i64;
            let xm = (xi - RD).clamp(0, wi - 1) as usize;
            let xp = (xi + RD).clamp(0, wi - 1) as usize;
            let (p00, p10, p20) = (v[ym + xm], v[ym + x], v[ym + xp]);
            let (p01, p21) = (v[y0 + xm], v[y0 + xp]);
            let (p02, p12, p22) = (v[yp + xm], v[yp + x], v[yp + xp]);
            let gx = (p20 + 2.0 * p21 + p22) - (p00 + 2.0 * p01 + p02);
            let gy = (p00 + 2.0 * p10 + p20) - (p02 + 2.0 * p12 + p22);
            *dst = if gx.abs() < 0.03 { 1.0 } else { (gy / gx).atan() * std::f32::consts::FRAC_1_PI + 0.5 };
        }
    });
    out
}

/// One evaluation of vszip's mode 6 on four references (`p[0]`, `p[2]` opposite each other, `p[1]`, `p[3]` too): how far the pixel moves towards
/// their average. `cur` and the references hold `n` channels that share one weight. `inv_t` = reciprocals of (average, largest, line) limits.
#[inline(always)]
fn soft_delta<const N: usize>(cur: [f32; N], p: [[f32; N]; 4], inv_t: [f32; 3]) -> [f32; N] {
    let comp = |d: f32, inv: f32| (3.0 * (1.0 - d * inv)).clamp(0.0, 1.0);
    let mut avg = [0.0f32; N];
    let (mut avg_dif, mut max_dif, mut mid_v, mut mid_h) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for k in 0..N {
        avg[k] = (p[0][k] + p[1][k] + p[2][k] + p[3][k]) * 0.25 - cur[k];
        avg_dif = avg_dif.max(avg[k].abs());
        for q in &p {
            max_dif = max_dif.max((q[k] - cur[k]).abs());
        }
        mid_v = mid_v.max((p[0][k] + p[2][k] - 2.0 * cur[k]).abs());
        mid_h = mid_h.max((p[1][k] + p[3][k] - 2.0 * cur[k]).abs());
    }
    let product = comp(avg_dif, inv_t[0]) * comp(max_dif, inv_t[1]) * comp(mid_v, inv_t[2]) * comp(mid_h, inv_t[2]);
    if product <= 0.0 {
        return [0.0f32; N];
    }
    if product >= 1.0 {
        return avg;
    }
    let factor = product.powf(0.1);
    let mut out = [0.0f32; N];
    for k in 0..N {
        out[k] = avg[k] * factor;
    }
    out
}

pub fn deband(src: &Img, p: &Params) -> Img {
    let (w, h) = (src.w, src.h);
    if w == 0 || h == 0 {
        return src.clone();
    }
    // Premultiplied color and alpha in Q8.8 fixed point (0..=65280, where 256 = 1.0 of an 8-bit level).
    // Half the size of [f32; 4] (8 bytes/pixel vs 16), doubling L2/L3 cache residency for scattered taps while
    // retaining 16-bit (1/256th level) precision.
    const SCALE_U16: f32 = 255.0 * 256.0;
    const INV_Q8: f32 = 1.0 / 256.0;
    let mut c: Vec<[u16; 4]> = src
        .px
        .par_iter()
        .map(|q| {
            [
                (q[0].clamp(0.0, 1.0) * SCALE_U16 + 0.5) as u16,
                (q[1].clamp(0.0, 1.0) * SCALE_U16 + 0.5) as u16,
                (q[2].clamp(0.0, 1.0) * SCALE_U16 + 0.5) as u16,
                (q[3].clamp(0.0, 1.0) * SCALE_U16 + 0.5) as u16,
            ]
        })
        .collect();
    let uniform_alpha = c.par_iter().all(|q| q[3] == c[0][3]);
    let trig = trig_lut();
    let g_lut = gauss_lut();
    let (wi, hi) = (w as isize, h as isize);
    let run_passes = p.thr_rgb > 0.0 || p.thr_a > 0.0;
    let iterations = p.iterations.max(1);
    if run_passes {
        // (vszip mode 7) the direction of the slope, from the brightness and from the alpha, once for the picture
        let angles = if p.adv.mode == Mode::Angle {
            let y: Vec<f32> = c.par_iter().map(|q| (q[0] as f32 * 0.2126 + q[1] as f32 * 0.7152 + q[2] as f32 * 0.0722) * INV_Q8).collect();
            let a: Vec<f32> = c.par_iter().map(|q| q[3] as f32 * INV_Q8).collect();
            Some((angle_plane(&y, w, h), angle_plane(&a, w, h)))
        } else {
            None
        };
        let mut next: Vec<[u16; 4]> = vec![[0; 4]; w * h];
        // cos(GOLDEN) and sin(GOLDEN) for GOLDEN = 2.3999631 rad
        const CG: f32 = -0.737_368_8;
        const SG: f32 = 0.675_490_3;
        // Scale rings per pass by iteration count so multi-pass runs always gather 16–32+ stratified taps total
        // without doing redundant 64-tap scattered reads on 4+ iterations.
        let rings: usize = match iterations {
            1 => 4,
            2 => 3,
            _ => 2,
        };
        let inv_rings = 1.0 / rings as f32;

        for it in 0..iterations {
            // (mdb_bilateral: 4/3, 2/3 and 1/3 of the radius in three stages, the first with half the threshold)
            let (r, thr_rgb, thr_a) = if p.narrowing && iterations > 1 {
                let t = it as f32 / (iterations - 1) as f32;
                let k = 4.0 / 3.0 + (1.0 / 3.0 - 4.0 / 3.0) * t;
                let half = if it == 0 { 0.5 } else { 1.0 };
                (((p.radius * k).round().max(1.0)) as isize, p.thr_rgb * half, p.thr_a * half)
            } else {
                (p.radius.round().max(1.0) as isize, p.thr_rgb, p.thr_a)
            };
            let it_seed = (it as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x517c_c1b7_2722_0a95;
            let src_c = &c;

            if p.adv.mode == Mode::Pairs {
                let thr_rgb_q = (thr_rgb * 256.0).clamp(0.0, 65535.0) as u16;
                let thr_a_q = (thr_a * 256.0).clamp(0.0, 65535.0) as u16;
                let inv_thr_rgb_q = 1.0 / (thr_rgb * 256.0).max(0.256);
                let inv_thr_a_q = 1.0 / (thr_a * 256.0).max(0.256);
                let do_rgb = thr_rgb > 0.0;
                // When alpha is uniform across the entire image (e.g., opaque backgrounds), alpha taps are a no-op.
                let do_a = thr_a > 0.0 && !uniform_alpha;
                let r_f = r as f32;
                let r_pad = (r + 2) as usize;
                let y_safe_end = h.saturating_sub(r_pad);
                let x_safe_end = w.saturating_sub(r_pad);

                const TILE_H: usize = 64;
                const TILE_W: usize = 128;
                next.par_chunks_mut(w * TILE_H).enumerate().for_each(|(band_idx, band)| {
                    let y0 = band_idx * TILE_H;
                    let rows = band.len() / w;
                    let mut x0 = 0;
                    while x0 < w {
                        let x1 = (x0 + TILE_W).min(w);
                        for ly in 0..rows {
                            let y = y0 + ly;
                            let yi = y as isize;
                            let row_base = y * w;
                            let out_row = &mut band[ly * w..(ly + 1) * w];
                            let y_interior = y >= r_pad && y < y_safe_end;

                            #[inline(always)]
                            fn accum_pair(
                                cur: [u16; 4],
                                q1: [u16; 4],
                                q2: [u16; 4],
                                do_rgb: bool,
                                do_a: bool,
                                thr_rgb_q: u16,
                                thr_a_q: u16,
                                inv_thr_rgb_q: f32,
                                inv_thr_a_q: f32,
                                sum0: &mut f32,
                                sum1: &mut f32,
                                sum2: &mut f32,
                                wsum: &mut f32,
                                sum_a: &mut f32,
                                wsum_a: &mut f32,
                            ) {
                                if do_rgb {
                                    let d_rgb = q1[0]
                                        .abs_diff(cur[0])
                                        .max(q1[1].abs_diff(cur[1]))
                                        .max(q1[2].abs_diff(cur[2]))
                                        .max(q2[0].abs_diff(cur[0]))
                                        .max(q2[1].abs_diff(cur[1]))
                                        .max(q2[2].abs_diff(cur[2]));
                                    if d_rgb < thr_rgb_q {
                                        let u = d_rgb as f32 * inv_thr_rgb_q;
                                        let wr = 1.0 - u * u;
                                        *sum0 += wr * (q1[0] as u32 + q2[0] as u32) as f32;
                                        *sum1 += wr * (q1[1] as u32 + q2[1] as u32) as f32;
                                        *sum2 += wr * (q1[2] as u32 + q2[2] as u32) as f32;
                                        *wsum += 2.0 * wr;
                                    }
                                }
                                if do_a {
                                    let d_a = q1[3].abs_diff(cur[3]).max(q2[3].abs_diff(cur[3]));
                                    if d_a < thr_a_q {
                                        let u = d_a as f32 * inv_thr_a_q;
                                        let wa = 1.0 - u * u;
                                        *sum_a += wa * (q1[3] as u32 + q2[3] as u32) as f32;
                                        *wsum_a += 2.0 * wa;
                                    }
                                }
                            }

                            let eval_border = |x: usize| -> [u16; 4] {
                                let i = row_base + x;
                                let cur = src_c[i];
                                let hs = hash(((i as u64) << 8) ^ it_seed);
                                let (mut ca, mut sa) = trig[(hs >> LUT_SHIFT) as usize];
                                let (mut sum0, mut sum1, mut sum2, mut wsum) = (cur[0] as f32, cur[1] as f32, cur[2] as f32, 1.0f32);
                                let (mut sum_a, mut wsum_a) = (cur[3] as f32, 1.0f32);
                                let xi = x as isize;
                                for ring in 0..rings {
                                    let j = ((hs >> (ring * 12)) & 0xfff) as f32 * (1.0 / 4096.0);
                                    let dist_r = 1.0 + ((ring as f32 + j) * inv_rings) * r_f;
                                    let ox0 = fast_round_isize(dist_r * ca);
                                    let oy0 = fast_round_isize(dist_r * sa);
                                    let nca = ca * CG - sa * SG;
                                    let nsa = sa * CG + ca * SG;
                                    ca = nca;
                                    sa = nsa;
                                    for (ox, oy) in [(ox0, oy0), (-oy0, ox0)] {
                                        let i1 = (yi + oy).clamp(0, hi - 1) as usize * w + (xi + ox).clamp(0, wi - 1) as usize;
                                        let i2 = (yi - oy).clamp(0, hi - 1) as usize * w + (xi - ox).clamp(0, wi - 1) as usize;
                                        accum_pair(
                                            cur, src_c[i1], src_c[i2], do_rgb, do_a, thr_rgb_q, thr_a_q, inv_thr_rgb_q, inv_thr_a_q,
                                            &mut sum0, &mut sum1, &mut sum2, &mut wsum, &mut sum_a, &mut wsum_a,
                                        );
                                    }
                                }
                                let inv_w = 1.0 / wsum;
                                [
                                    (sum0 * inv_w + 0.5) as u16,
                                    (sum1 * inv_w + 0.5) as u16,
                                    (sum2 * inv_w + 0.5) as u16,
                                    if do_a { (sum_a / wsum_a + 0.5) as u16 } else { cur[3] },
                                ]
                            };

                            if y_interior && r_pad < x_safe_end {
                                let int_a = x0.max(r_pad).min(x1);
                                let int_b = x1.min(x_safe_end).max(int_a);
                                for x in x0..int_a {
                                    out_row[x] = eval_border(x);
                                }
                                for x in int_a..int_b {
                                    let i = row_base + x;
                                    let cur = src_c[i];
                                    let hs = hash(((i as u64) << 8) ^ it_seed);
                                    let (mut ca, mut sa) = trig[(hs >> LUT_SHIFT) as usize];
                                    let (mut sum0, mut sum1, mut sum2, mut wsum) = (cur[0] as f32, cur[1] as f32, cur[2] as f32, 1.0f32);
                                    let (mut sum_a, mut wsum_a) = (cur[3] as f32, 1.0f32);
                                    let ii = i as isize;
                                    for ring in 0..rings {
                                        let j = ((hs >> (ring * 12)) & 0xfff) as f32 * (1.0 / 4096.0);
                                        let dist_r = 1.0 + ((ring as f32 + j) * inv_rings) * r_f;
                                        let ox0 = fast_round_isize(dist_r * ca);
                                        let oy0 = fast_round_isize(dist_r * sa);
                                        let nca = ca * CG - sa * SG;
                                        let nsa = sa * CG + ca * SG;
                                        ca = nca;
                                        sa = nsa;
                                        let d0 = oy0 * wi + ox0;
                                        let d1 = ox0 * wi - oy0;
                                        accum_pair(
                                            cur, src_c[(ii + d0) as usize], src_c[(ii - d0) as usize], do_rgb, do_a,
                                            thr_rgb_q, thr_a_q, inv_thr_rgb_q, inv_thr_a_q,
                                            &mut sum0, &mut sum1, &mut sum2, &mut wsum, &mut sum_a, &mut wsum_a,
                                        );
                                        accum_pair(
                                            cur, src_c[(ii + d1) as usize], src_c[(ii - d1) as usize], do_rgb, do_a,
                                            thr_rgb_q, thr_a_q, inv_thr_rgb_q, inv_thr_a_q,
                                            &mut sum0, &mut sum1, &mut sum2, &mut wsum, &mut sum_a, &mut wsum_a,
                                        );
                                    }
                                    let inv_w = 1.0 / wsum;
                                    out_row[x] = [
                                        (sum0 * inv_w + 0.5) as u16,
                                        (sum1 * inv_w + 0.5) as u16,
                                        (sum2 * inv_w + 0.5) as u16,
                                        if do_a { (sum_a / wsum_a + 0.5) as u16 } else { cur[3] },
                                    ];
                                }
                                for x in int_b..x1 {
                                    out_row[x] = eval_border(x);
                                }
                            } else {
                                for x in x0..x1 {
                                    out_row[x] = eval_border(x);
                                }
                            }
                        }
                        x0 = x1;
                    }
                });
            } else {
                // vszip's mode 6 / 7 on two sets of four references (opposite ones are 0 and 1, 2 and 3), the results averaged
                let (t1_rgb, t2_rgb) = (if p.adv.thr1 > 0.0 { p.adv.thr1 } else { p.thr_rgb }, if p.adv.thr2 > 0.0 { p.adv.thr2 } else { p.thr_rgb });
                let (t1_a, t2_a) = (if p.adv.thr1 > 0.0 { p.adv.thr1 } else { p.thr_a }, if p.adv.thr2 > 0.0 { p.adv.thr2 } else { p.thr_a });
                let half = if p.narrowing && iterations > 1 && it == 0 { 0.5 } else { 1.0 };
                let base_inv_rgb = [1.0 / thr_rgb.max(1e-5), 1.0 / (t1_rgb * half).max(1e-5), 1.0 / (t2_rgb * half).max(1e-5)];
                let base_inv_a = [1.0 / thr_a.max(1e-5), 1.0 / (t1_a * half).max(1e-5), 1.0 / (t2_a * half).max(1e-5)];
                let inv_boost = 1.0 / p.adv.angle_boost.max(1e-5);
                let r_f = r as f32;

                next.par_chunks_mut(w).enumerate().for_each(|(y, out_row)| {
                    let yi = y as isize;
                    let row_base = y * w;
                    let reach = |h1: u64| -> isize {
                        let f = if p.adv.dist_sigma > 0.0 {
                            let mut hh = h1;
                            loop {
                                let (a, b) = (unit(hh) * 2.0 - 1.0, unit(hash(hh)) * 2.0 - 1.0);
                                let r2 = a * a + b * b;
                                hh = hash(hh ^ 0x2545f4914f6cdd1d);
                                if r2 > 0.0 && r2 <= 1.0 {
                                    let v = p.adv.dist_sigma * b * (-2.0 * r2.ln() / r2).sqrt();
                                    if v > -1.0 && v < 1.0 {
                                        break v.abs();
                                    }
                                }
                            }
                        } else {
                            unit(h1)
                        };
                        1 + (f * r_f) as isize
                    };
                    let to_f = |q: [u16; 4]| [q[0] as f32 * INV_Q8, q[1] as f32 * INV_Q8, q[2] as f32 * INV_Q8, q[3] as f32 * INV_Q8];
                    for (x, dst) in out_row.iter_mut().enumerate() {
                        let i = row_base + x;
                        let xi = x as isize;
                        let cur = to_f(src_c[i]);
                        let hs = hash(((i as u64) << 8) ^ it_seed);
                        let dist = reach(hs) as f32;
                        let (c1, s1) = trig[(hash(hs) >> LUT_SHIFT) as usize];
                        let dx = fast_round_isize(dist * c1);
                        let dy = fast_round_isize(dist * s1);
                        let hs2 = hash(hs ^ 0xa5a5a5a5);
                        let dist2 = reach(hs2) as f32;
                        let (c2, s2) = trig[(hash(hs2) >> LUT_SHIFT) as usize];
                        let dx2 = fast_round_isize(dist2 * c2);
                        let dy2 = fast_round_isize(dist2 * s2);
                        let offs = [(dx, dy), (-dx, -dy), (-dy, dx), (dy, -dx), (dx2, dy2), (-dx2, -dy2), (-dy2, dx2), (dy2, -dx2)];
                        let idxs: [usize; 8] = std::array::from_fn(|k| {
                            (yi + offs[k].1).clamp(0, hi - 1) as usize * w + (xi + offs[k].0).clamp(0, wi - 1) as usize
                        });
                        let refs: [[f32; 4]; 8] = std::array::from_fn(|k| to_f(src_c[idxs[k]]));
                        let (mut inv_rgb, mut inv_a) = (base_inv_rgb, base_inv_a);
                        if let Some((ang_y, ang_a)) = &angles {
                            if (0..4).all(|k| (ang_y[idxs[k]] - ang_y[i]).abs() <= p.adv.max_angle) {
                                for v in &mut inv_rgb {
                                    *v *= inv_boost;
                                }
                            }
                            if (0..4).all(|k| (ang_a[idxs[k]] - ang_a[i]).abs() <= p.adv.max_angle) {
                                for v in &mut inv_a {
                                    *v *= inv_boost;
                                }
                            }
                        }
                        let rgb = |q: &[f32; 4]| [q[0], q[1], q[2]];
                        let cur_rgb = rgb(&cur);
                        let set = |s: usize| -> ([f32; 3], [f32; 1]) {
                            let pr = [rgb(&refs[s]), rgb(&refs[s + 2]), rgb(&refs[s + 1]), rgb(&refs[s + 3])];
                            let pa = [[refs[s][3]], [refs[s + 2][3]], [refs[s + 1][3]], [refs[s + 3][3]]];
                            (soft_delta(cur_rgb, pr, inv_rgb), soft_delta([cur[3]], pa, inv_a))
                        };
                        let (d1, a1) = set(0);
                        let (d2, a2) = set(4);
                        *dst = [
                            ((cur[0] + (d1[0] + d2[0]) * 0.5).clamp(0.0, 255.0) * 256.0 + 0.5) as u16,
                            ((cur[1] + (d1[1] + d2[1]) * 0.5).clamp(0.0, 255.0) * 256.0 + 0.5) as u16,
                            ((cur[2] + (d1[2] + d2[2]) * 0.5).clamp(0.0, 255.0) * 256.0 + 0.5) as u16,
                            ((cur[3] + (a1[0] + a2[0]) * 0.5).clamp(0.0, 255.0) * 256.0 + 0.5) as u16,
                        ];
                    }
                });
            }
            std::mem::swap(&mut c, &mut next);
        }
    }

    let do_rgb = p.thr_rgb > 0.0 || p.thr_a <= 0.0;
    let do_a = p.thr_a > 0.0 || p.thr_rgb <= 0.0;
    let do_limit = p.limit > 0.0;
    let do_grain = p.grain > 0.0 && p.kind != Grain::None;
    // Fast path when no grain or quantization is needed (typical Op::Deband pipeline with grain == 0):
    // avoids computing pixel coordinates, IGN, or pre-zeroing a 64 MB buffer.
    let px: Vec<[f32; 4]> = if !do_grain && !p.quantize && do_rgb && do_a {
        c.par_iter()
            .zip(src.px.par_iter())
            .map(|(cq, orig)| {
                let mut q = [cq[0] as f32 * INV_Q8, cq[1] as f32 * INV_Q8, cq[2] as f32 * INV_Q8, cq[3] as f32 * INV_Q8];
                if do_limit {
                    for k in 0..4 {
                        q[k] = limit_value(q[k], orig[k] * 255.0, p.limit, p.elast);
                    }
                }
                let a = q[3].clamp(0.0, 255.0) * (1.0 / 255.0);
                if q[3] > 0.5 {
                    let s = a / q[3];
                    [(q[0] * s).clamp(0.0, a), (q[1] * s).clamp(0.0, a), (q[2] * s).clamp(0.0, a), a]
                } else {
                    [0.0, 0.0, 0.0, a]
                }
            })
            .collect()
    } else {
        c.par_iter()
            .zip(src.px.par_iter())
            .enumerate()
            .map(|(i, (cq, orig))| {
                let (x, y) = (i % w, i / w);
                let mut q = [cq[0] as f32 * INV_Q8, cq[1] as f32 * INV_Q8, cq[2] as f32 * INV_Q8, cq[3] as f32 * INV_Q8];
                if do_limit {
                    for k in 0..4 {
                        q[k] = limit_value(q[k], orig[k] * 255.0, p.limit, p.elast);
                    }
                }
                let al = if do_a { q[3] } else { orig[3] * 255.0 };
                let n_a = if do_a && do_grain { p.grain * grain_at_with_lut(p.kind, x, y, 3, al, g_lut) } else { 0.0 };
                let mut out_a = if !do_a {
                    al.clamp(0.0, 255.0)
                } else if !p.quantize {
                    (al + n_a).clamp(0.0, 255.0)
                } else if al < 0.25 {
                    0.0
                } else if al > 254.75 {
                    255.0
                } else {
                    fast_floor_0_255(al + ign(x, y, 0.0, 0.0) + n_a)
                };
                if !do_rgb {
                    // Alpha-only deband: preserve original straight RGB untouched.
                    let oa = orig[3];
                    let a = out_a * (1.0 / 255.0);
                    return if oa > 1e-4 {
                        let inv_oa = 1.0 / oa;
                        [(orig[0] * inv_oa).clamp(0.0, 1.0) * a, (orig[1] * inv_oa).clamp(0.0, 1.0) * a, (orig[2] * inv_oa).clamp(0.0, 1.0) * a, a]
                    } else {
                        [0.0, 0.0, 0.0, a]
                    };
                }
                if !p.mono && do_grain && al > 0.5 {
                    let taper = ((al - 0.5) * (1.0 / 7.5)).clamp(0.0, 1.0);
                    let mut pr = [0.0f32; 3];
                    let scale_a = out_a / al;
                    let g_mul = p.grain * taper;
                    for k in 0..3 {
                        let base_k = (q[k] * scale_a).clamp(0.0, 255.0);
                        let n = g_mul * grain_at_with_lut(p.kind, x, y, k as u64, base_k, g_lut);
                        pr[k] = if p.quantize {
                            fast_floor_0_255(base_k + ign(x, y, 41.0 * k as f32 + 5.0, 13.0 * k as f32 + 3.0) + n)
                        } else {
                            (base_k + n).clamp(0.0, 255.0)
                        };
                    }
                    if do_a {
                        out_a = out_a.max(pr[0]).max(pr[1]).max(pr[2]).clamp(0.0, 255.0);
                    } else {
                        for v in &mut pr {
                            *v = v.min(out_a);
                        }
                    }
                    return [pr[0] * (1.0 / 255.0), pr[1] * (1.0 / 255.0), pr[2] * (1.0 / 255.0), out_a * (1.0 / 255.0)];
                }
                let mut rgb = [0.0f32; 3];
                let inv_q3 = if q[3] > 0.5 { 255.0 / q[3] } else { 0.0 };
                let mono_n = if !do_grain {
                    Some(0.0)
                } else if p.mono && p.kind != Grain::Poisson {
                    Some(p.grain * grain_at_with_lut(p.kind, x, y, 0, 0.0, g_lut))
                } else {
                    None
                };
                for k in 0..3 {
                    let straight = q[k] * inv_q3;
                    let n = match mono_n {
                        Some(v) => v,
                        None => p.grain * grain_at_with_lut(p.kind, x, y, if p.mono { 0 } else { k as u64 }, straight, g_lut),
                    };
                    rgb[k] = if p.quantize {
                        fast_floor_0_255(straight + ign(x, y, 41.0 * k as f32 + 5.0, 13.0 * k as f32 + 3.0) + n)
                    } else {
                        (straight + n).clamp(0.0, 255.0)
                    };
                }
                let a = out_a * (1.0 / 255.0);
                let a_div = a * (1.0 / 255.0);
                [rgb[0] * a_div, rgb[1] * a_div, rgb[2] * a_div, a]
            })
            .collect()
    };
    Img { w, h, px }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A smooth alpha ramp quantised to 8 bits has visible steps; debanding must reduce the step height without
    /// moving the average, leave empty/solid areas alone and be deterministic.
    #[test]
    fn smooths_alpha_ramp() {
        let (w, h) = (256, 16);
        let px = (0..w * h).map(|i| {
            let a = ((i % w) as f32 / 8.0).floor() * 8.0 / 255.0; // coarse 32-step staircase
            [a, a, a, a]
        });
        let src = Img { w, h, px: px.collect() };
        let p = Params { radius: 12.0, thr_rgb: 9.0, thr_a: 9.0, iterations: 3, grain: 0.0, kind: Grain::None, mono: false, quantize: true, limit: 0.0, elast: 3.0, narrowing: false, adv: Advanced::default() };
        let out = deband(&src, &p);
        let again = deband(&src, &p);
        assert_eq!(out.px, again.px);
        let row = |im: &Img| (0..w).map(|x| im.px[8 * w + x][3] * 255.0).collect::<Vec<f32>>();
        let (a, b) = (row(&src), row(&out));
        let max_step = |v: &[f32]| v.windows(2).map(|p| (p[1] - p[0]).abs()).fold(0.0, f32::max);
        assert!(max_step(&b) < max_step(&a), "steps {} -> {}", max_step(&a), max_step(&b));
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
        assert!((mean(&a) - mean(&b)).abs() < 2.0);
        assert!(b[0] <= 1.0, "an empty start stays empty: {}", b[0]);
    }

    #[test]
    fn keeps_hard_edges() {
        let (w, h) = (64, 8);
        let px = (0..w * h).map(|i| if i % w < 32 { [0.0; 4] } else { [1.0; 4] });
        let src = Img { w, h, px: px.collect() };
        let out = deband(&src, &Params { radius: 8.0, thr_rgb: 3.0, thr_a: 3.0, iterations: 2, grain: 0.0, kind: Grain::None, mono: false, quantize: true, limit: 0.0, elast: 3.0, narrowing: false, adv: Advanced::default() });
        assert_eq!(out.px[31], [0.0; 4]);
        assert_eq!(out.px[32], [1.0; 4]);
    }

    fn stats(kind: Grain, level: f32) -> (f32, f32, f32) {
        let v: Vec<f32> = (0..200 * 200).map(|i| grain_at(kind, i % 200, i / 200, 0, level)).collect();
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        let sd = (v.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / v.len() as f32).sqrt();
        // how much a pixel looks like its right-hand neighbour
        let corr = (0..200 * 200).filter(|i| i % 200 < 199).map(|i| (v[i] - mean) * (v[i + 1] - mean)).sum::<f32>() / (v.len() as f32 * sd * sd);
        (mean, sd, corr)
    }

    #[test]
    fn the_kinds_of_grain_have_the_spread_and_the_texture_they_are_named_for() {
        for kind in [Grain::Uniform, Grain::Gaussian, Grain::Blue] {
            let (m, sd, _) = stats(kind, 200.0);
            assert!(m.abs() < 0.03 && (sd - 1.0).abs() < 0.05, "{kind:?}: mean {m} spread {sd}");
        }
        assert_eq!(stats(Grain::None, 200.0).1, 0.0);
        // white noise has no pattern from pixel to pixel; blue noise pushes neighbours apart
        assert!(stats(Grain::Gaussian, 200.0).2.abs() < 0.03);
        assert!(stats(Grain::Blue, 200.0).2 < -0.2, "blue: {}", stats(Grain::Blue, 200.0).2);
        // shot noise: the brighter, the stronger (a quarter of the brightness is half the spread)
        let (bright, dark) = (stats(Grain::Poisson, 255.0).1, stats(Grain::Poisson, 255.0 / 4.0).1);
        assert!((bright - 1.0).abs() < 0.05 && (dark - 0.5).abs() < 0.03, "{bright} {dark}");
    }

    #[test]
    fn grey_grain_moves_red_green_and_blue_together_and_coloured_grain_does_not() {
        let (w, h) = (64, 64);
        let src = Img { w, h, px: vec![[0.2, 0.2, 0.2, 1.0]; w * h] };
        let go = |mono| deband(&src, &Params { radius: 2.0, thr_rgb: 0.0, thr_a: 0.0, iterations: 1, grain: 3.0, kind: Grain::Gaussian, mono, quantize: true, limit: 0.0, elast: 3.0, narrowing: false, adv: Advanced::default() });
        let spread = |im: &Img| im.px.iter().map(|p| ((p[0] - p[1]).abs() + (p[1] - p[2]).abs()) * 255.0).sum::<f32>() / im.px.len() as f32;
        let (grey, colour) = (spread(&go(true)), spread(&go(false)));
        // the dither that rounding needs differs per channel in both, the grain on top does so only in the coloured one
        assert!(colour > grey * 1.5, "grey {grey} colour {colour}");
        assert_eq!(go(true).px, go(true).px);
    }

    #[test]
    fn the_limit_takes_back_changes_that_are_no_banding() {
        // inside the limit the filter result stands, between limit and limit * elast it fades out, beyond that the source stays
        assert_eq!(limit_value(101.0, 100.0, 2.0, 3.0), 101.0);
        assert_eq!(limit_value(110.0, 100.0, 2.0, 3.0), 100.0);
        let mid = limit_value(103.0, 100.0, 2.0, 3.0);
        assert!(mid > 100.0 && mid < 103.0, "{mid}");
        // so a limited debanding does not move a pixel by (much) more than the limit
        let (w, h) = (256, 8);
        let src = Img { w, h, px: (0..w * h).map(|i| { let v = ((i % w) / 8) as f32 * 4.0 / 255.0; [v, v, v, 1.0] }).collect() };
        let go = |limit| deband(&src, &Params { radius: 24.0, thr_rgb: 20.0, thr_a: 20.0, iterations: 3, grain: 0.0, kind: Grain::None, mono: true, quantize: false, limit, elast: 3.0, narrowing: false, adv: Advanced::default() });
        let far = |im: &Img| im.px.iter().zip(&src.px).map(|(a, b)| (a[0] - b[0]).abs() * 255.0).fold(0.0f32, f32::max);
        assert!(far(&go(0.0)) > 1.5, "unlimited it moves things: {}", far(&go(0.0)));
        // (the soft edge of the limit lets a change reach 9/8 of it at most, with an elasticity of 3)
        assert!(far(&go(1.0)) <= 1.125 + 1e-3, "limited: {}", far(&go(1.0)));
    }

    #[test]
    fn narrowing_stages_still_smooth_a_staircase() {
        let (w, h) = (300, 8);
        let src = Img { w, h, px: (0..w * h).map(|i| { let v = (40.0 + ((i % w) / 10) as f32) / 255.0; [v, v, v, 1.0] }).collect() };
        let go = |narrowing| deband(&src, &Params { radius: 30.0, thr_rgb: 4.0, thr_a: 4.0, iterations: 3, grain: 0.0, kind: Grain::None, mono: true, quantize: false, limit: 0.0, elast: 3.0, narrowing, adv: Advanced::default() });
        let jump = |im: &Img| (60..240).step_by(10).map(|x| (im.px[4 * w + x][0] - im.px[4 * w + x - 1][0]).abs() * 255.0).sum::<f32>() / 18.0;
        assert!(jump(&go(true)) < jump(&src) * 0.7, "{} -> {}", jump(&src), jump(&go(true)));
    }

    fn steep_ramp() -> Img {
        // a slope of 1 level every 4 pixels, stored in 8 bits: banding on a steep gradient
        let (w, h) = (640, 8);
        Img { w, h, px: (0..w * h).map(|i| { let v = (20.0 + ((i % w) / 4) as f32) / 255.0; [v, v, v, 1.0] }).collect() }
    }

    fn step_left(im: &Img) -> f32 {
        // the average size of the jumps between neighbouring pixels in the middle, in levels, less the true slope (0.25 a pixel)
        let w = im.w;
        (100..540).map(|x| (im.px[4 * w + x][0] - im.px[4 * w + x - 1][0]).abs() * 255.0).map(|d| (d - 0.25).abs()).sum::<f32>() / 440.0
    }

    #[test]
    fn the_line_limit_lets_the_smooth_mode_work_on_a_steep_slope_that_the_pairs_mode_gives_up_on() {
        let src = steep_ramp();
        let base = Params { radius: 40.0, thr_rgb: 3.0, thr_a: 3.0, iterations: 3, grain: 0.0, kind: Grain::None, mono: true, quantize: false, limit: 0.0, elast: 3.0, narrowing: false, adv: Advanced::default() };
        let pairs = deband(&src, &base);
        let smooth = deband(&src, &Params { adv: Advanced { mode: Mode::Smooth, thr1: 40.0, thr2: 3.0, ..Advanced::default() }, ..base });
        let angle = deband(&src, &Params { adv: Advanced { mode: Mode::Angle, thr1: 40.0, thr2: 3.0, ..Advanced::default() }, ..base });
        let (a, b, c, d) = (step_left(&src), step_left(&pairs), step_left(&smooth), step_left(&angle));
        assert!(c < a * 0.6, "smooth takes the steps out: {a} -> {c}");
        assert!(c < b, "and does so where pairs does not manage: pairs {b}, smooth {c}");
        assert!(d <= c * 1.5 + 0.02, "the angle mode is as good: {d}");
        assert_eq!(angle.px, deband(&src, &Params { adv: Advanced { mode: Mode::Angle, thr1: 40.0, thr2: 3.0, ..Advanced::default() }, ..base }).px, "deterministic");
    }

    #[test]
    fn the_new_modes_keep_a_hard_edge_and_the_gaussian_spread_stays_inside_the_radius() {
        let (w, h) = (64, 8);
        let edge = Img { w, h, px: (0..w * h).map(|i| if i % w < 32 { [0.0; 4] } else { [1.0; 4] }).collect() };
        for mode in [Mode::Smooth, Mode::Angle] {
            let out = deband(&edge, &Params { radius: 8.0, thr_rgb: 3.0, thr_a: 3.0, iterations: 2, grain: 0.0, kind: Grain::None, mono: true, quantize: true, limit: 0.0, elast: 3.0, narrowing: false, adv: Advanced { mode, dist_sigma: 1.0, ..Advanced::default() } });
            assert_eq!(out.px[31], [0.0; 4], "{mode:?}");
            assert_eq!(out.px[32], [1.0; 4], "{mode:?}");
        }
    }
}
