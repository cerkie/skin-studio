//! Debanding on all four channels including alpha (f3kdb / libplacebo style), ported from the Python tool.
//! Random reference offsets and the dither pattern are fixed per pixel position, so identical frames give identical
//! output and moving shapes never shimmer.
use rayon::prelude::*;

use crate::img::Img;

#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub radius: f32,
    pub thr_rgb: f32,
    pub thr_a: f32,
    pub iterations: usize,
    pub grain: f32,
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

pub fn deband(src: &Img, p: &Params) -> Img {
    let (w, h) = (src.w, src.h);
    // premultiplied color and alpha, 0..255
    let mut c: Vec<[f32; 4]> = src.px.iter().map(|q| [q[0] * 255.0, q[1] * 255.0, q[2] * 255.0, q[3] * 255.0]).collect();
    let r = p.radius.round().max(1.0) as i64;
    let at = |c: &Vec<[f32; 4]>, x: i64, y: i64| c[(y.clamp(0, h as i64 - 1) as usize) * w + x.clamp(0, w as i64 - 1) as usize];
    for it in 0..p.iterations.max(1) {
        let next: Vec<[f32; 4]> = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let (x, y) = ((i % w) as i64, (i / w) as i64);
                let hs = hash(((i as u64) << 8) ^ (it as u64) ^ 0x9e3779b97f4a7c15);
                let dist = 1 + (unit(hs) * r as f32) as i64;
                let ang = unit(hash(hs)) * std::f32::consts::TAU;
                let dx = (dist as f32 * ang.cos()).round_ties_even() as i64;
                let dy = (dist as f32 * ang.sin()).round_ties_even() as i64;
                let cur = c[i];
                let refs = [at(&c, x + dx, y + dy), at(&c, x - dx, y - dy), at(&c, x - dy, y + dx), at(&c, x + dy, y - dx)];
                let mut avg = cur;
                for q in &refs {
                    for k in 0..4 {
                        avg[k] += q[k];
                    }
                }
                for v in &mut avg {
                    *v /= 5.0;
                }
                let (mut m_rgb, mut m_a) = (1.0f32, 1.0f32);
                for q in &refs {
                    let d = (q[0] - cur[0]).abs().max((q[1] - cur[1]).abs()).max((q[2] - cur[2]).abs());
                    m_rgb = m_rgb.min(((p.thr_rgb - d) / (0.5 * p.thr_rgb.max(1e-3))).clamp(0.0, 1.0));
                    m_a = m_a.min(((p.thr_a - (q[3] - cur[3]).abs()) / (0.5 * p.thr_a.max(1e-3))).clamp(0.0, 1.0));
                }
                [
                    cur[0] + (avg[0] - cur[0]) * m_rgb,
                    cur[1] + (avg[1] - cur[1]) * m_rgb,
                    cur[2] + (avg[2] - cur[2]) * m_rgb,
                    cur[3] + (avg[3] - cur[3]) * m_a,
                ]
            })
            .collect();
        c = next;
    }
    let px = c
        .par_iter()
        .enumerate()
        .map(|(i, q)| {
            let (x, y) = (i % w, i / w);
            let al = q[3];
            let extra = (ign(x, y, 17.0, 31.0) - 0.5) * p.grain;
            let out_a = if al < 0.25 {
                0.0
            } else if al > 254.75 {
                255.0
            } else {
                (al + ign(x, y, 0.0, 0.0) + extra).floor().clamp(0.0, 255.0)
            };
            let mut rgb = [0.0f32; 3];
            for k in 0..3 {
                let straight = if al > 0.5 { q[k] * 255.0 / al } else { 0.0 };
                rgb[k] = (straight + ign(x, y, 41.0 * k as f32 + 5.0, 13.0 * k as f32 + 3.0) + extra).floor().clamp(0.0, 255.0);
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
        let p = Params { radius: 12.0, thr_rgb: 9.0, thr_a: 9.0, iterations: 3, grain: 0.0 };
        let out = deband(&src, &p);
        let again = deband(&src, &p);
        assert_eq!(out.px, again.px);
        let row = |im: &Img| (0..w).map(|x| im.px[8 * w + x][3] * 255.0).collect::<Vec<f32>>();
        let (a, b) = (row(&src), row(&out));
        let max_step = |v: &[f32]| v.windows(2).map(|p| (p[1] - p[0]).abs()).fold(0.0, f32::max);
        assert!(max_step(&b) < max_step(&a), "steps {} -> {}", max_step(&a), max_step(&b));
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
        assert!((mean(&a) - mean(&b)).abs() < 2.0);
        assert_eq!(b[0], 0.0);
    }

    #[test]
    fn keeps_hard_edges() {
        let (w, h) = (64, 8);
        let px = (0..w * h).map(|i| if i % w < 32 { [0.0; 4] } else { [1.0; 4] });
        let src = Img { w, h, px: px.collect() };
        let out = deband(&src, &Params { radius: 8.0, thr_rgb: 3.0, thr_a: 3.0, iterations: 2, grain: 0.0 });
        assert_eq!(out.px[31], [0.0; 4]);
        assert_eq!(out.px[32], [1.0; 4]);
    }
}
