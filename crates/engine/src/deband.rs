//! Debanding on all four channels including alpha (f3kdb / libplacebo style), ported from the Python tool.
//! Random reference offsets and the dither pattern are fixed per pixel position, so identical frames give identical
//! output and moving shapes never shimmer.
use rayon::prelude::*;

use crate::img::Img;

/// What the grain that fills the steps in is like. `Params::grain` is its strength: the spread of the noise in 8-bit levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
#[derive(Clone, Copy, Debug, PartialEq)]
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

/// A white noise value for a pixel and channel with an average of 0 and a spread of 1, the same every time.
fn white(kind: Grain, x: i64, y: i64, ch: u64) -> f32 {
    let h = hash(((y as u64) << 32) ^ (x as u64 & 0xffff_ffff) ^ (ch.wrapping_mul(0x9e3779b97f4a7c15)) ^ 0x5851f42d4c957f2d);
    match kind {
        Grain::Uniform => (unit(h) - 0.5) * 12.0f32.sqrt(),
        _ => {
            // Box-Muller
            let u1 = unit(h).max(1e-7);
            let u2 = unit(hash(h ^ 0xa0761d6478bd642f));
            (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
        }
    }
}

/// The grain at a pixel, spread 1 (`level` is how bright the pixel is, 0 to 255, for the kind that depends on it).
fn grain_at(kind: Grain, x: usize, y: usize, ch: u64, level: f32) -> f32 {
    let (xi, yi) = (x as i64, y as i64);
    match kind {
        Grain::None => 0.0,
        Grain::Uniform | Grain::Gaussian => white(kind, xi, yi, ch),
        Grain::Poisson => white(Grain::Gaussian, xi, yi, ch) * (level / 255.0).clamp(0.0, 1.0).sqrt(),
        Grain::Blue => {
            let g = |dx: i64, dy: i64| white(Grain::Gaussian, xi + dx, yi + dy, ch);
            (g(0, 0) - 0.25 * (g(-1, 0) + g(1, 0) + g(0, -1) + g(0, 1))) / 1.25f32.sqrt()
        }
    }
}

fn hash(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58476d1ce4e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}

fn unit(h: u64) -> f32 {
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// Interleaved-gradient noise in [0, 1).
fn ign(x: usize, y: usize, ox: f32, oy: f32) -> f32 {
    let t = (0.06711056 * (x as f32 + ox) + 0.00583715 * (y as f32 + oy)).fract();
    (52.9829189 * t).fract()
}

/// The normalised gradient direction (0 to 1, vszip's `deband_angle`) of every pixel: a Sobel kernel whose taps are 20 pixels apart, so that
/// the direction is that of the slope around the pixel and not of the noise on it.
fn angle_plane(v: &[f32], w: usize, h: usize) -> Vec<f32> {
    const RD: i64 = 20;
    let at = |x: i64, y: i64| v[y.clamp(0, h as i64 - 1) as usize * w + x.clamp(0, w as i64 - 1) as usize];
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            let (p00, p10, p20) = (at(x - RD, y - RD), at(x, y - RD), at(x + RD, y - RD));
            let (p01, p21) = (at(x - RD, y), at(x + RD, y));
            let (p02, p12, p22) = (at(x - RD, y + RD), at(x, y + RD), at(x + RD, y + RD));
            let gx = (p20 + 2.0 * p21 + p22) - (p00 + 2.0 * p01 + p02);
            let gy = (p00 + 2.0 * p10 + p20) - (p02 + 2.0 * p12 + p22);
            if gx.abs() < 0.03 { 1.0 } else { (gy / gx).atan() / std::f32::consts::PI + 0.5 }
        })
        .collect()
}

/// One evaluation of vszip's mode 6 on four references (`p[0]`, `p[2]` opposite each other, `p[1]`, `p[3]` too): how far the pixel moves towards
/// their average. `cur` and the references hold `n` channels that share one weight. `t` = (average, largest, line) limits.
fn soft_delta<const N: usize>(cur: [f32; N], p: [[f32; N]; 4], t: [f32; 3]) -> [f32; N] {
    let comp = |d: f32, t: f32| (3.0 * (1.0 - d / t.max(1e-5))).clamp(0.0, 1.0);
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
    let product = comp(avg_dif, t[0]) * comp(max_dif, t[1]) * comp(mid_v, t[2]) * comp(mid_h, t[2]);
    let factor = product.powf(0.1);
    let mut out = [0.0f32; N];
    for k in 0..N {
        out[k] = avg[k] * factor;
    }
    out
}

pub fn deband(src: &Img, p: &Params) -> Img {
    let (w, h) = (src.w, src.h);
    // premultiplied color and alpha, 0..255
    let mut c: Vec<[f32; 4]> = src.px.iter().map(|q| [q[0] * 255.0, q[1] * 255.0, q[2] * 255.0, q[3] * 255.0]).collect();
    let at = |c: &Vec<[f32; 4]>, x: i64, y: i64| c[(y.clamp(0, h as i64 - 1) as usize) * w + x.clamp(0, w as i64 - 1) as usize];
    let iterations = p.iterations.max(1);
    // (vszip mode 7) the direction of the slope, from the brightness and from the alpha, once for the picture
    let angles = if p.adv.mode == Mode::Angle {
        let y: Vec<f32> = c.iter().map(|q| q[0] * 0.2126 + q[1] * 0.7152 + q[2] * 0.0722).collect();
        let a: Vec<f32> = c.iter().map(|q| q[3]).collect();
        Some((angle_plane(&y, w, h), angle_plane(&a, w, h)))
    } else {
        None
    };
    for it in 0..iterations {
        // (mdb_bilateral: 4/3, 2/3 and 1/3 of the radius in three stages, the first with half the threshold)
        let (r, thr_rgb, thr_a) = if p.narrowing && iterations > 1 {
            let t = it as f32 / (iterations - 1) as f32;
            let k = 4.0 / 3.0 + (1.0 / 3.0 - 4.0 / 3.0) * t;
            let half = if it == 0 { 0.5 } else { 1.0 };
            (((p.radius * k).round().max(1.0)) as i64, p.thr_rgb * half, p.thr_a * half)
        } else {
            (p.radius.round().max(1.0) as i64, p.thr_rgb, p.thr_a)
        };
        let next: Vec<[f32; 4]> = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let (x, y) = ((i % w) as i64, (i / w) as i64);
                let hs = hash(((i as u64) << 8) ^ (it as u64) ^ 0x9e3779b97f4a7c15);
                // how far away a reference is: evenly over the radius, or (vszip's gaussian) mostly near the pixel
                let reach = |h1: u64| -> i64 {
                    let f = if p.adv.dist_sigma > 0.0 {
                        // polar Box-Muller until the value lies inside (-1, 1)
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
                    1 + (f * r as f32) as i64
                };
                let dist = reach(hs);
                let ang = unit(hash(hs)) * std::f32::consts::TAU;
                let dx = (dist as f32 * ang.cos()).round_ties_even() as i64;
                let dy = (dist as f32 * ang.sin()).round_ties_even() as i64;
                let cur = c[i];
                // eight neighbours at two random distances and angles
                let hs2 = hash(hs ^ 0xa5a5a5a5);
                let dist2 = reach(hs2);
                let ang2 = unit(hash(hs2)) * std::f32::consts::TAU;
                let dx2 = (dist2 as f32 * ang2.cos()).round_ties_even() as i64;
                let dy2 = (dist2 as f32 * ang2.sin()).round_ties_even() as i64;
                let offs = [(dx, dy), (-dx, -dy), (-dy, dx), (dy, -dx), (dx2, dy2), (-dx2, -dy2), (-dy2, dx2), (dy2, -dx2)];
                let refs: [[f32; 4]; 8] = std::array::from_fn(|k| at(&c, x + offs[k].0, y + offs[k].1));
                if p.adv.mode != Mode::Pairs {
                    // vszip's mode 6 / 7 on two sets of four references (opposite ones are 0 and 1, 2 and 3), the results averaged
                    let (t1_rgb, t2_rgb) = (if p.adv.thr1 > 0.0 { p.adv.thr1 } else { p.thr_rgb }, if p.adv.thr2 > 0.0 { p.adv.thr2 } else { p.thr_rgb });
                    let (t1_a, t2_a) = (if p.adv.thr1 > 0.0 { p.adv.thr1 } else { p.thr_a }, if p.adv.thr2 > 0.0 { p.adv.thr2 } else { p.thr_a });
                    let half = if p.narrowing && iterations > 1 && it == 0 { 0.5 } else { 1.0 };
                    let (mut b_rgb, mut b_a) = (1.0f32, 1.0f32);
                    if let Some((ang_y, ang_a)) = &angles {
                        // wider limits where the slope runs the same way at the pixel and at all four references (one set)
                        let idx = |k: usize| (y + offs[k].1).clamp(0, h as i64 - 1) as usize * w + (x + offs[k].0).clamp(0, w as i64 - 1) as usize;
                        let same = |a: &Vec<f32>| (0..4).all(|k| (a[idx(k)] - a[i]).abs() <= p.adv.max_angle);
                        if same(ang_y) {
                            b_rgb = p.adv.angle_boost;
                        }
                        if same(ang_a) {
                            b_a = p.adv.angle_boost;
                        }
                    }
                    let t_rgb = [thr_rgb * b_rgb, t1_rgb * half * b_rgb, t2_rgb * half * b_rgb];
                    let t_a = [thr_a * b_a, t1_a * half * b_a, t2_a * half * b_a];
                    let rgb = |q: &[f32; 4]| [q[0], q[1], q[2]];
                    let cur_rgb = rgb(&cur);
                    let set = |s: usize| -> ([f32; 3], [f32; 1]) {
                        let pr = [rgb(&refs[s]), rgb(&refs[s + 2]), rgb(&refs[s + 1]), rgb(&refs[s + 3])];
                        let pa = [[refs[s][3]], [refs[s + 2][3]], [refs[s + 1][3]], [refs[s + 3][3]]];
                        (soft_delta(cur_rgb, pr, t_rgb), soft_delta([cur[3]], pa, t_a))
                    };
                    let (d1, a1) = set(0);
                    let (d2, a2) = set(4);
                    return [cur[0] + (d1[0] + d2[0]) * 0.5, cur[1] + (d1[1] + d2[1]) * 0.5, cur[2] + (d1[2] + d2[2]) * 0.5, cur[3] + (a1[0] + a2[0]) * 0.5];
                }
                // the neighbours come in opposite pairs, taken in together by how close both are to the pixel: a straight ramp is its own
                // average (it stays as it is), a staircase becomes the ramp it was cut from, and an edge keeps its side
                let (mut sum, mut wsum) = ([cur[0], cur[1], cur[2]], 1.0f32);
                let (mut sum_a, mut wsum_a) = (cur[3], 1.0f32);
                for pair in refs.chunks(2) {
                    let (q1, q2) = (&pair[0], &pair[1]);
                    let dd = |q: &[f32; 4]| (q[0] - cur[0]).abs().max((q[1] - cur[1]).abs()).max((q[2] - cur[2]).abs());
                    let wr = ((thr_rgb - dd(q1).max(dd(q2))) / (0.5 * thr_rgb.max(1e-3))).clamp(0.0, 1.0);
                    let da = (q1[3] - cur[3]).abs().max((q2[3] - cur[3]).abs());
                    let wa = ((thr_a - da) / (0.5 * thr_a.max(1e-3))).clamp(0.0, 1.0);
                    for k in 0..3 {
                        sum[k] += wr * (q1[k] + q2[k]);
                    }
                    wsum += 2.0 * wr;
                    sum_a += wa * (q1[3] + q2[3]);
                    wsum_a += 2.0 * wa;
                }
                [sum[0] / wsum, sum[1] / wsum, sum[2] / wsum, sum_a / wsum_a]
            })
            .collect();
        c = next;
    }
    if p.limit > 0.0 {
        c = c
            .par_iter()
            .zip(src.px.par_iter())
            .map(|(q, s)| {
                let mut o = *q;
                for k in 0..4 {
                    o[k] = limit_value(q[k], s[k] * 255.0, p.limit, p.elast);
                }
                o
            })
            .collect();
    }
    let px = c
        .par_iter()
        .enumerate()
        .map(|(i, q)| {
            let (x, y) = (i % w, i / w);
            let al = q[3];
            let n_a = p.grain * grain_at(p.kind, x, y, 3, al);
            let out_a = if !p.quantize {
                (al + n_a).clamp(0.0, 255.0)
            } else if al < 0.25 {
                0.0
            } else if al > 254.75 {
                255.0
            } else {
                (al + ign(x, y, 0.0, 0.0) + n_a).floor().clamp(0.0, 255.0)
            };
            let mut rgb = [0.0f32; 3];
            for k in 0..3 {
                let straight = if al > 0.5 { q[k] * 255.0 / al } else { 0.0 };
                let n = p.grain * grain_at(p.kind, x, y, if p.mono { 0 } else { k as u64 }, straight);
                rgb[k] = if p.quantize { (straight + ign(x, y, 41.0 * k as f32 + 5.0, 13.0 * k as f32 + 3.0) + n).floor().clamp(0.0, 255.0) } else { (straight + n).clamp(0.0, 255.0) };
            }
            let a = out_a / 255.0;
            [rgb[0] / 255.0 * a, rgb[1] / 255.0 * a, rgb[2] / 255.0 * a, a]
        })
        .collect();
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
