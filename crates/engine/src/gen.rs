//! Frame generation: the Rust port of the Python tool's `build_frames` (sources, keyframed transforms, the seamless
//! line, appear/disappear windows, loop padding, outline and shadow). Frames are independent, so they are built in
//! parallel. `None` is a blank 1x1 frame.
use rayon::prelude::*;

use crate::img::{self, Img, Sampler};
use crate::keys;
use crate::lazer::{preempt_for, FADE};
use crate::settings::{Settings, PROPS};
use crate::sources::{self, Sprite};

pub type Frame = Option<Img>;

/// Python's `round` (ties to even), which the reference uses for frame counts.
fn round_f(v: f32) -> i64 {
    v.round_ties_even() as i64
}

pub struct Options {
    /// Kernel used when frames are transformed. Hermite never overshoots (no ringing); Catmull-Rom reproduces the
    /// Python reference for comparison.
    pub sampler: Sampler,
}

impl Default for Options {
    fn default() -> Self {
        Options { sampler: Sampler::Hermite }
    }
}

// ---------------------------------------------------------------------------------------------- one transformed layer
#[allow(clippy::too_many_arguments)]
fn render_layer(base: &Sprite, cw: usize, ch: usize, cx: f32, cy: f32, sx: f32, sy: f32, rot: f32, alpha: f32, blur: f32,
                color: Option<[f32; 3]>, strength: f32, sampler: Sampler) -> Img {
    let mut pm = Img { w: base.w, h: base.h, px: Vec::with_capacity(base.px.len()) };
    for p in &base.px {
        let mut c = [p[0], p[1], p[2]];
        if let Some(col) = color {
            if strength > 0.0 {
                for i in 0..3 {
                    c[i] = c[i] * (1.0 - strength) + col[i] / 255.0 * strength;
                }
            }
        }
        pm.px.push([c[0] * p[3], c[1] * p[3], c[2] * p[3], p[3]]);
    }
    let (w, h) = (base.w as f32, base.h as f32);
    let (sx, sy) = (sx.abs().max(1e-3), sy.abs().max(1e-3));
    let rw = (round_f(w * sx.min(1.0)).max(1)) as usize;
    let rh = (round_f(h * sy.min(1.0)).max(1)) as usize;
    let src = img::resize_area(&pm, rw, rh); // shrinking: exact area average, no ringing
    let (ex, ey) = (sx * w / rw as f32, sy * h / rh as f32);
    let (c, s_) = (rot.to_radians().cos(), rot.to_radians().sin());
    let (a, b, d, e) = (c / ex, s_ / ex, -s_ / ey, c / ey);
    let c0 = rw as f32 / 2.0 - a * cx - b * cy;
    let f0 = rh as f32 / 2.0 - d * cx - e * cy;
    let mut out = img::affine(&src, cw, ch, [a, b, c0, d, e, f0], sampler);
    if blur > 0.05 {
        out = img::blur(&out, blur);
    }
    out.scale_alpha(alpha);
    out
}

fn base_sprite(s: &Settings, scale: usize) -> Sprite {
    if s.preset == "Custom image..." {
        if let Ok(sp) = sources::load_png(std::path::Path::new(&s.image)) {
            if scale == 2 {
                let up = img::affine(
                    &Img { w: sp.w, h: sp.h, px: sp.px.iter().map(|p| [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]]).collect() },
                    sp.w * 2, sp.h * 2, [0.5, 0.0, 0.0, 0.0, 0.5, 0.0], Sampler::Hermite);
                return Sprite { w: up.w, h: up.h, px: up.px.iter().map(|p| if p[3] > 1e-6 { [p[0] / p[3], p[1] / p[3], p[2] / p[3], p[3]] } else { [1.0; 4] }).collect() };
            }
            return sp;
        }
    }
    sources::preset(&s.preset, scale).or_else(|| sources::preset("Arrow", scale)).unwrap()
}

// ---------------------------------------------------------------------------------------------- classic animation
fn render_frames(s: &Settings, base: &Sprite, scale: usize, opt: &Options) -> Vec<Frame> {
    let (cw, ch) = (s.width * scale, s.height * scale);
    let ms = |v: f32| round_f(v * s.fps / 1000.0);
    let mut lead_f = ms(s.lead).max(0) as usize;
    let mut n = ms(s.real_length()).max(1) as usize;
    let mut tail_f = ms(s.tail).max(0) as usize;
    let (mut fi, mut fo) = (ms(s.fade_in), ms(s.fade_out));
    if s.clamp_mode {
        lead_f = 0;
        tail_f = 0;
        fi = 0;
        fo = 0;
        n = ms(s.real_length()).max(1) as usize;
    }
    let total = s.design_length();
    let hw = base.w as f32 / 2.0;
    let body: Vec<Frame> = (0..n)
        .into_par_iter()
        .map(|i| {
            let mut acc = Img::new(cw, ch);
            for k in 0..s.copies {
                let mut u = if s.copies > 1 {
                    (i as f32 / n as f32 + k as f32 / s.copies as f32) % 1.0
                } else if n > 1 {
                    i as f32 / (n - 1) as f32
                } else {
                    0.5
                };
                if s.pingpong {
                    u = 1.0 - (2.0 * u - 1.0).abs();
                }
                let tau = (u * total + s.shift).clamp(0.0, total);
                let v: Vec<f32> = PROPS.iter().map(|p| keys::eval(s.prop(p), tau, total)).collect();
                let (x, y, sx, sy, rot, alpha, blur) = (v[0], v[1], v[2], v[3], v[4], v[5], v[6]);
                let cx = cw as f32 / 2.0 + (x - 0.5) * (cw as f32 - 2.0 * hw).max(0.0);
                let cy = ch as f32 / 2.0 + y;
                let mut a = alpha.max(0.0);
                if fi > 0 {
                    a *= ((i as f32 + 1.0) / fi as f32).min(1.0);
                }
                if fo > 0 {
                    a *= ((n - i) as f32 / fo as f32).min(1.0);
                }
                if s.copies > 1 {
                    a *= (std::f32::consts::PI * ((i as f32 / n as f32 + k as f32 / s.copies as f32) % 1.0)).sin().sqrt();
                }
                let col = if s.color_on { Some(keys::eval_color(&s.color_keys, tau, total)) } else { None };
                let mut layer = render_layer(base, cw, ch, cx, cy, sx, sy, rot, a, blur.max(0.0), col, s.color_strength, opt.sampler);
                layer.over(&acc);
                acc = layer;
            }
            Some(acc)
        })
        .collect();
    let mut frames: Vec<Frame> = vec![None; lead_f];
    frames.extend(body);
    frames.extend(vec![None; tail_f]);
    frames
}

// ---------------------------------------------------------------------------------------------- seamless line
fn fade_phi(tt: f32, pre: f32) -> f32 {
    if tt < 0.0 || tt > pre + FADE {
        return 0.0;
    }
    let mut a = (tt / FADE).min(1.0);
    if tt > pre {
        a *= 1.0 - ((tt - pre) / FADE).min(1.0);
    }
    a
}

fn make_seamless_frames(s: &Settings, scale: usize, opt: &Options) -> Vec<Frame> {
    let ratio = scale as f32;
    let sc = (1.0 - 0.7 * (s.sl_cs - 5.0) / 5.0) / 2.0 * 1.00041;
    let want = s.sl_overlap.clamp(0.0, 1.0);
    let max_ov = (128.0 * sc / 32.0 - 1.0).max(0.0);
    let ov = want.min(max_ov);
    let length = 32.0 * (1.0 + ov);
    let pre = preempt_for(s.sl_ar);
    let life = pre + FADE;
    let flen = 1000.0 / s.fps;
    let n = (life / flen).ceil() as usize + 1;
    let amax = s.sl_alpha.clamp(0.0, 0.999);
    let tight = s.sl_tight.clamp(0.0, 1.0);
    let t0 = (400.0 * amax).max(350.0) * tight;
    let soft = (s.sl_soft * tight).min((life - 2.0 * t0) / 2.0).max(1.0);
    let sm = |x: f32| {
        let c = x.clamp(0.0, 1.0);
        c * c * (3.0 - 2.0 * c)
    };
    let env = |tt: f32| if tight <= 0.0 { 1.0 } else { sm((tt - t0) / soft) * (1.0 - sm((tt - (life - t0 - soft)) / soft)) };
    let lpx = ((length / sc * ratio).round() as usize).max(2);
    let hpx = (((s.sl_thick / sc * ratio + 4.0 * ratio).ceil() as usize) / 2 * 2) + 2;
    let a0 = 16.0 * (1.0 - ov);
    let ramp = 32.0 * ov;
    let w: Vec<f32> = (0..lpx)
        .map(|i| {
            let ax = (((i as f32 + 0.5) / lpx as f32 * length) - length / 2.0).abs();
            if ax <= a0 {
                1.0
            } else if ax >= a0 + ramp {
                0.0
            } else {
                ((std::f32::consts::FRAC_PI_2 * (ax - a0) / ramp.max(1e-9)).cos()).powi(2)
            }
        })
        .collect();
    let tpx = s.sl_thick / sc * ratio;
    let g: Vec<f32> = (0..hpx)
        .map(|j| {
            let yy = (j as f32 + 0.5 - hpx as f32 / 2.0).abs();
            let v = ((tpx / 2.0 - yy) / ratio + 0.5).clamp(0.0, 1.0);
            v * v * (3.0 - 2.0 * v)
        })
        .collect();
    let sy_max = s.prop("sy").iter().map(|k| k.v).fold(1.0f32, f32::max);
    let mut wc = lpx.max(s.width * scale);
    let mut hc = (((hpx as f32 * sy_max).ceil() as usize) + 4 * scale).max(s.height * scale);
    wc += wc % 2;
    hc += hc % 2;
    let spd = s.speed.max(0.05);
    (0..n)
        .into_par_iter()
        .map(|i| {
            let tq = (i as f32 + 0.5) * flen;
            let ph = fade_phi(tq, pre);
            let v: Vec<f32> = PROPS.iter().map(|p| keys::eval(s.prop(p), tq * spd, life)).collect();
            let a_t = ph.min(amax * v[5].max(0.0) * env(tq)).min(0.999);
            if ph <= 1e-4 || a_t <= 1e-4 {
                return None;
            }
            let big_a = -(1.0 - a_t).ln();
            let mut tile = Sprite { w: lpx, h: hpx, px: Vec::with_capacity(lpx * hpx) };
            for gj in &g {
                for wi in &w {
                    let alpha = ((1.0 - (-big_a * wi * gj).exp()) / ph).min(1.0);
                    // the Python tile is rounded to 8 bits before it is placed: keep that quantisation
                    tile.px.push([1.0, 1.0, 1.0, (alpha * 255.0 + 0.5).floor() / 255.0]);
                }
            }
            let col = if s.color_on { Some(keys::eval_color(&s.color_keys, tq * spd, life)) } else { None };
            let cx = wc as f32 / 2.0 + (v[0] - 0.5) * (wc as f32 - lpx as f32).max(0.0);
            Some(render_layer(&tile, wc, hc, cx, hc as f32 / 2.0 + v[1] * ratio, v[2], v[3], v[4], 1.0, v[6].max(0.0) * ratio,
                              col, s.color_strength, opt.sampler))
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------- window, padding
fn clamp_window(frames: Vec<Frame>, s: &Settings, use_lead: bool) -> Vec<Frame> {
    let a = if use_lead { s.lead } else { 0.0 };
    let b = s.disappear;
    if a <= 0.0 && b <= 0.0 {
        return frames;
    }
    let flen = 1000.0 / s.fps;
    frames
        .into_iter()
        .enumerate()
        .map(|(i, f)| {
            let mut f = f?;
            let tc = (i as f32 + 0.5) * flen;
            if tc < a || (b > 0.0 && tc >= b) {
                return None;
            }
            let mut k = 1.0f32;
            if a > 0.0 && s.fade_in > 0.0 {
                k = k.min((tc - a) / s.fade_in);
            }
            if b > 0.0 && s.fade_out > 0.0 {
                k = k.min((b - tc) / s.fade_out);
            }
            if k < 1.0 {
                f.scale_alpha(k.max(0.0));
            }
            Some(f)
        })
        .collect()
}

fn windowed(frames: Vec<Frame>, s: &Settings) -> Vec<Frame> {
    if s.clamp_mode || s.is_seamless() {
        clamp_window(frames, s, true)
    } else if s.disappear > 0.0 {
        clamp_window(frames, s, false)
    } else {
        frames
    }
}

fn pad_to_life(mut frames: Vec<Frame>, s: &Settings) -> Vec<Frame> {
    if !s.no_loop {
        return frames;
    }
    let life = preempt_for(s.sl_ar) + FADE;
    let need = (life * s.fps / 1000.0).ceil() as usize + 1;
    while frames.len() < need {
        frames.push(None);
    }
    frames
}

// ---------------------------------------------------------------------------------------------- outline and shadow
fn bilinear(plane: &[f32], w: usize, h: usize, u: f32, v: f32) -> f32 {
    let (fx, fy) = (u - 0.5, v - 0.5);
    let (ix, iy) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - ix, fy - iy);
    let at = |x: i64, y: i64| -> f32 {
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            0.0
        } else {
            plane[y as usize * w + x as usize]
        }
    };
    let (ix, iy) = (ix as i64, iy as i64);
    (at(ix, iy) * (1.0 - tx) + at(ix + 1, iy) * tx) * (1.0 - ty) + (at(ix, iy + 1) * (1.0 - tx) + at(ix + 1, iy + 1) * tx) * ty
}

/// Round, anti-aliased dilation of an alpha plane by `r` pixels (exact distance transform at 3x).
fn dilate(a: &[f32], w: usize, h: usize, r: f32) -> Vec<f32> {
    if r <= 0.01 {
        return a.to_vec();
    }
    const K: usize = 3;
    let (bw, bh) = (w * K, h * K);
    let mut big = vec![0.0f32; bw * bh];
    for y in 0..bh {
        for x in 0..bw {
            big[y * bw + x] = bilinear(a, w, h, (x as f32 + 0.5) / K as f32, (y as f32 + 0.5) / K as f32);
        }
    }
    let inside: Vec<bool> = big.iter().map(|&v| v > 0.5).collect();
    let d2 = img::edt_sq(&inside, bw, bh);
    let amax = a.iter().cloned().fold(0.0, f32::max);
    let rr = r * K as f32;
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut sum = 0.0;
            for j in 0..K {
                for i in 0..K {
                    let idx = (y * K + j) * bw + x * K + i;
                    let grown = (rr + 0.5 - d2[idx].sqrt()).clamp(0.0, 1.0) * amax;
                    sum += big[idx].max(grown);
                }
            }
            out[y * w + x] = sum / (K * K) as f32;
        }
    }
    out
}

fn decorate_one(fr: &Img, s: &Settings, scale: usize, m: usize) -> Img {
    let (w, h) = (fr.w + 2 * m, fr.h + 2 * m);
    let mut shape = Img::new(w, h);
    for y in 0..fr.h {
        for x in 0..fr.w {
            shape.px[(y + m) * w + x + m] = fr.px[y * fr.w + x];
        }
    }
    let alpha: Vec<f32> = shape.px.iter().map(|p| p[3]).collect();
    let sil = alpha.clone();
    let mut layers: Vec<Img> = vec![shape];
    if s.outline_on {
        let oc = [s.outline_color[0] / 255.0, s.outline_color[1] / 255.0, s.outline_color[2] / 255.0];
        let oa: Vec<f32> = dilate(&alpha, w, h, s.outline_w * scale as f32).iter().map(|v| (v * s.outline_opacity).clamp(0.0, 1.0)).collect();
        layers.push(Img { w, h, px: oa.iter().map(|&a| [oc[0] * a, oc[1] * a, oc[2] * a, a]).collect() });
    }
    if s.shadow_on {
        let (dx, dy) = (s.shadow_dx * scale as f32, s.shadow_dy * scale as f32);
        let shifted: Vec<f32> = (0..w * h).map(|i| bilinear(&sil, w, h, (i % w) as f32 + 0.5 - dx, (i / w) as f32 + 0.5 - dy)).collect();
        let sa: Vec<f32> = img::blur_plane(&shifted, w, h, s.shadow_blur * scale as f32).iter().map(|v| (v * s.shadow_opacity).clamp(0.0, 1.0)).collect();
        let sc = [s.shadow_color[0] / 255.0, s.shadow_color[1] / 255.0, s.shadow_color[2] / 255.0];
        layers.push(Img { w, h, px: sa.iter().map(|&a| [sc[0] * a, sc[1] * a, sc[2] * a, a]).collect() });
    }
    let mut acc = layers.pop().unwrap();
    while let Some(mut top) = layers.pop() {
        top.over(&acc);
        acc = top;
    }
    acc
}

fn decorate(frames: Vec<Frame>, s: &Settings, scale: usize) -> Vec<Frame> {
    if !(s.outline_on || s.shadow_on) {
        return frames;
    }
    let mut m = 0.0f32;
    if s.outline_on {
        m += s.outline_w * scale as f32;
    }
    if s.shadow_on {
        m += 2.5 * s.shadow_blur * scale as f32 + s.shadow_dx.abs().max(s.shadow_dy.abs()) * scale as f32;
    }
    let m = m.ceil() as usize + if m > 0.0 { 2 } else { 0 };
    frames.into_par_iter().map(|f| f.map(|im| decorate_one(&im, s, scale, m))).collect()
}

fn debanded(frames: Vec<Frame>, s: &Settings, scale: usize) -> Vec<Frame> {
    if !s.deband_on {
        return frames;
    }
    let p = crate::deband::Params { radius: s.deband_radius * scale as f32, thr_rgb: s.deband_thr_rgb, thr_a: s.deband_thr_a, iterations: s.deband_iter, grain: s.deband_grain / 12.0f32.sqrt(), kind: if s.deband_grain > 0.0 { crate::deband::Grain::Uniform } else { crate::deband::Grain::None }, mono: true, quantize: true, limit: 0.0, elast: 3.0, narrowing: false, adv: crate::deband::Advanced::default() };
    frames.into_par_iter().map(|f| f.map(|im| crate::deband::deband(&im, &p))).collect()
}

// ---------------------------------------------------------------------------------------------- time of a frame
/// The motion time (in ms of the design, the axis the keyframes are on) that frame `idx` of the animation shows, or `None` for
/// the blank frames before and after the animation itself. This is what `render_frames` and `make_seamless_frames` evaluate the
/// keys at, so a playhead drawn from it is where the picture really is on the motion graph.
pub fn frame_tau(s: &Settings, idx: usize) -> Option<f32> {
    if s.is_seamless() {
        let flen = 1000.0 / s.fps;
        let life = preempt_for(s.sl_ar) + FADE;
        let tq = (idx as f32 + 0.5) * flen;
        return (tq <= life).then(|| (tq * s.speed.max(0.05)).min(life));
    }
    let ms = |v: f32| round_f(v * s.fps / 1000.0);
    let lead_f = if s.clamp_mode { 0 } else { ms(s.lead).max(0) as usize };
    let n = ms(s.real_length()).max(1) as usize;
    if idx < lead_f || idx >= lead_f + n {
        return None;
    }
    let b = idx - lead_f;
    let total = s.design_length();
    let mut u = if s.copies > 1 {
        (b as f32 / n as f32) % 1.0
    } else if n > 1 {
        b as f32 / (n - 1) as f32
    } else {
        0.5
    };
    if s.pingpong {
        u = 1.0 - (2.0 * u - 1.0).abs();
    }
    Some((u * total + s.shift).clamp(0.0, total))
}

// ---------------------------------------------------------------------------------------------- entry point
/// Frames at 1x (`scale` = 1) or @2x (`scale` = 2). Settings are the 1x design; everything is scaled for @2x.
pub fn build_frames(s: &Settings, scale: usize, opt: &Options) -> Vec<Frame> {
    if s.is_seamless() {
        let f = make_seamless_frames(s, scale, opt);
        return debanded(decorate(windowed(f, s), s, scale), s, scale);
    }
    let base = base_sprite(s, scale);
    let mut t = s.clone();
    if scale == 2 {
        // y offset and blur are pixel measures: they double with the resolution
        for name in ["y", "blur"] {
            let i = PROPS.iter().position(|p| *p == name).unwrap();
            t.props[i].iter_mut().for_each(|k| k.v *= 2.0);
        }
    }
    let f = render_frames(&t, &base, scale, opt);
    pad_to_life(debanded(decorate(windowed(f, s), s, scale), s, scale), s)
}

#[cfg(test)]
mod tau_tests {
    use super::*;

    #[test]
    fn the_default_animation_shows_the_blur_key_where_the_graph_says() {
        let s = Settings::from_json(&crate::presets::default_settings());
        let frames = build_frames(&s, 1, &Default::default());
        // frames that show a moment of the animation, and the padding blank after them
        let real: Vec<usize> = (0..frames.len()).filter(|i| frame_tau(&s, *i).is_some()).collect();
        assert!(!real.is_empty());
        let (first, last) = (real[0], *real.last().unwrap());
        assert_eq!(real.len(), last - first + 1, "the frames of the animation are in one run");
        assert!(frames[last + 1..].iter().all(|f| f.is_none()));
        // time grows with the frames
        let taus: Vec<f32> = real.iter().map(|i| frame_tau(&s, *i).unwrap()).collect();
        assert!(taus.windows(2).all(|w| w[1] >= w[0]));
        // the blur key sits at 250 ms of the animation: the frame that shows it is where the graph says, counted from the start of the
        // animation, moved by the window shift (the default has none)
        let at250 = real[taus.iter().position(|t| *t >= 250.0).unwrap()];
        let loop_ms = (at250 - first) as f32 * 1000.0 / s.fps;
        let expect = 250.0 - s.shift.min(0.0);
        assert!((loop_ms - expect).abs() < 40.0, "{loop_ms} vs {expect}");
    }
}
