//! Import: read a folder of `followpoint-N.png` frames and work out the settings that produced them. A port of the
//! Python tool's `analyze_followpoints`: it recognises the seamless line, otherwise matches a drawn preset (or cuts
//! the sprite out as a custom image), measures position/scale/rotation/opacity/color per frame, and fits keyframes.
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::gen::{self, Options};
use crate::img::{self, Img, Sampler};
use crate::keys;
use crate::presets;
use crate::settings::{Settings, SEAMLESS};
use crate::sources::{self, Sprite};

pub struct Analysis {
    pub settings: Value,
    pub notes: Vec<String>,
    pub custom: Option<Sprite>,
    /// 0..1 overlap of the rebuilt frames with the originals.
    pub matched: Option<f32>,
}

struct Stats {
    cx: f32,
    cy: f32,
    cxx: f32,
    cyy: f32,
    l1: f32,
    l2: f32,
    theta: f32,
    peak: f32,
    col: Option<[f32; 3]>,
    a: Vec<f32>,
    w: usize,
    h: usize,
}

fn alpha_plane(im: &Img) -> Vec<f32> {
    im.px.iter().map(|p| p[3]).collect()
}

fn frame_stats(im: &Img) -> Option<Stats> {
    let a = alpha_plane(im);
    let max = a.iter().cloned().fold(0.0, f32::max);
    if (im.w, im.h) == (1, 1) || max < 0.02 {
        return None;
    }
    let (w, h) = (im.w, im.h);
    let m: f64 = a.iter().map(|&v| v as f64).sum();
    let (mut sx, mut sy) = (0.0f64, 0.0f64);
    for y in 0..h {
        for x in 0..w {
            let v = a[y * w + x] as f64;
            sx += (x as f64 + 0.5) * v;
            sy += (y as f64 + 0.5) * v;
        }
    }
    let (cx, cy) = (sx / m, sy / m);
    let (mut cxx, mut cyy, mut cxy) = (0.0f64, 0.0f64, 0.0f64);
    for y in 0..h {
        for x in 0..w {
            let v = a[y * w + x] as f64;
            let (dx, dy) = (x as f64 + 0.5 - cx, y as f64 + 0.5 - cy);
            cxx += dx * dx * v;
            cyy += dy * dy * v;
            cxy += dx * dy * v;
        }
    }
    let (cxx, cyy, cxy) = (cxx / m, cyy / m, cxy / m);
    let (tr, det) = (cxx + cyy, cxx * cyy - cxy * cxy);
    let disc = (tr * tr / 4.0 - det).max(0.0).sqrt();
    let (l1, l2) = (tr / 2.0 + disc, (tr / 2.0 - disc).max(1e-9));
    let theta = (0.5 * (2.0 * cxy).atan2(cxx - cyy)).to_degrees();
    let col = if max >= 0.3 {
        let mut acc = [0.0f64; 3];
        let mut n = 0.0;
        for p in &im.px {
            if p[3] > 0.5 * max {
                for k in 0..3 {
                    acc[k] += (p[k] / p[3] * 255.0) as f64;
                }
                n += 1.0;
            }
        }
        (n > 0.0).then(|| [(acc[0] / n) as f32, (acc[1] / n) as f32, (acc[2] / n) as f32])
    } else {
        None
    };
    Some(Stats { cx: cx as f32, cy: cy as f32, cxx: cxx as f32, cyy: cyy as f32, l1: l1 as f32, l2: l2 as f32, theta: theta as f32, peak: max, col, a, w, h })
}

type Plane = (Vec<f32>, usize, usize);

fn crop_alpha(a: &Plane) -> Plane {
    let (data, w, h) = a;
    let max = data.iter().cloned().fold(1e-6, f32::max);
    let (mut x0, mut y0, mut x1, mut y1) = (*w, *h, 0, 0);
    for y in 0..*h {
        for x in 0..*w {
            if data[y * w + x] > 0.08 * max {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x1 < x0 {
        return (vec![0.0], 1, 1);
    }
    let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut out = Vec::with_capacity(cw * ch);
    for y in y0..=y1 {
        out.extend_from_slice(&data[y * w + x0..y * w + x1 + 1]);
    }
    (out, cw, ch)
}

fn resize_plane(a: &Plane, nw: usize, nh: usize) -> Vec<f32> {
    let (d, w, h) = a;
    if nw <= *w && nh <= *h {
        let src = Img { w: *w, h: *h, px: d.iter().map(|&v| [v, v, v, v]).collect() };
        return alpha_plane(&img::resize_area(&src, nw, nh));
    }
    let mut out = Vec::with_capacity(nw * nh);
    for y in 0..nh {
        for x in 0..nw {
            let fx = ((x as f32 + 0.5) * *w as f32 / nw as f32 - 0.5).clamp(0.0, (*w - 1) as f32);
            let fy = ((y as f32 + 0.5) * *h as f32 / nh as f32 - 0.5).clamp(0.0, (*h - 1) as f32);
            let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
            let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
            let v = d[y0 * w + x0] * (1.0 - tx) * (1.0 - ty) + d[y0 * w + x1] * tx * (1.0 - ty) + d[y1 * w + x0] * (1.0 - tx) * ty + d[y1 * w + x1] * tx * ty;
            out.push(v);
        }
    }
    out
}

/// Scale-independent difference of two alpha shapes (0 = identical).
fn shape_error(a1: &Plane, a2: &Plane) -> f32 {
    let (c1, c2) = (crop_alpha(a1), crop_alpha(a2));
    let (ar1, ar2) = (c1.1 as f32 / c1.2 as f32, c2.1 as f32 / c2.2 as f32);
    if (ar1 / ar2).ln().abs() > 0.35 {
        return 9.0;
    }
    let m2 = c2.0.iter().cloned().fold(1e-6, f32::max);
    let n2: Plane = (c2.0.iter().map(|v| v / m2).collect(), c2.1, c2.2);
    let r2 = resize_plane(&n2, c1.1, c1.2);
    let m1 = c1.0.iter().cloned().fold(1e-6, f32::max);
    let mean1: f32 = c1.0.iter().map(|v| v / m1).sum::<f32>() / c1.0.len() as f32;
    let diff: f32 = c1.0.iter().zip(&r2).map(|(a, b)| (a / m1 - b).abs()).sum::<f32>() / c1.0.len() as f32;
    diff / mean1.max(1e-6)
}

/// PIL's `rotate(angle, expand=True)` of a plane (counter-clockwise degrees).
fn rotate_plane(a: &Plane, deg: f32) -> Plane {
    let (d, w, h) = a;
    let ang = -deg.to_radians();
    let (c, s) = (ang.cos(), ang.sin());
    let (cx, cy) = (*w as f32 / 2.0, *h as f32 / 2.0);
    let tf = |x: f32, y: f32, m: [f32; 6]| (m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5]);
    let mut m = [c, s, 0.0, -s, c, 0.0];
    let (t0, t1) = tf(-cx, -cy, m);
    m[2] = t0 + cx;
    m[5] = t1 + cy;
    let (mut xs, mut ys) = (vec![], vec![]);
    for (x, y) in [(0.0, 0.0), (*w as f32, 0.0), (*w as f32, *h as f32), (0.0, *h as f32)] {
        let (px, py) = tf(x, y, m);
        xs.push(px);
        ys.push(py);
    }
    let fmax = |v: &Vec<f32>| v.iter().cloned().fold(f32::MIN, f32::max);
    let fmin = |v: &Vec<f32>| v.iter().cloned().fold(f32::MAX, f32::min);
    let nw = (fmax(&xs).ceil() - fmin(&xs).floor()) as usize;
    let nh = (fmax(&ys).ceil() - fmin(&ys).floor()) as usize;
    let (nw, nh) = (nw.max(1), nh.max(1));
    let (u, v) = tf(-(nw as f32 - *w as f32) / 2.0, -(nh as f32 - *h as f32) / 2.0, m);
    m[2] = u;
    m[5] = v;
    let src = Img { w: *w, h: *h, px: d.iter().map(|&v| [v, v, v, v]).collect() };
    let out = img::affine(&src, nw, nh, m, Sampler::CatmullRom);
    (alpha_plane(&out), nw, nh)
}

fn upright(st: &Stats, nat: &Stats) -> Vec<Plane> {
    let a: Plane = (st.a.clone(), st.w, st.h);
    if nat.l1 / nat.l2 < 1.25 {
        return vec![a];
    }
    let rot = (st.theta - nat.theta + 90.0).rem_euclid(180.0) - 90.0;
    [0.0, 180.0].iter().map(|extra| rotate_plane(&a, rot + extra)).collect()
}

fn rdp(t: &[f32], v: &[f32], eps: f32) -> Vec<usize> {
    let mut keep = vec![0usize, t.len() - 1];
    let mut stack = vec![(0usize, t.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (mut best, mut bi) = (0.0f32, a);
        for i in a..=b {
            let line = v[a] + (v[b] - v[a]) * (t[i] - t[a]) / (t[b] - t[a]).max(1e-9);
            let e = (v[i] - line).abs();
            if e > best {
                best = e;
                bi = i;
            }
        }
        if best > eps {
            keep.push(bi);
            stack.push((a, bi));
            stack.push((bi, b));
        }
    }
    keep.sort_unstable();
    keep.dedup();
    keep
}

fn fit_keys(t: &[f32], v: &[f32], eps: f32) -> Vec<Value> {
    let idx = rdp(t, v, eps);
    let names = presets::easings();
    let mut out = Vec::new();
    for (j, &i0) in idx.iter().enumerate() {
        let mut e = "linear".to_string();
        if j + 1 < idx.len() {
            let i1 = idx[j + 1];
            let (tt, vv) = (&t[i0..=i1], &v[i0..=i1]);
            if tt.len() >= 4 && (vv[vv.len() - 1] - vv[0]).abs() > 1e-6 {
                let (t0, t1) = (tt[0], tt[tt.len() - 1]);
                let rms = |name: &str| -> f32 {
                    let mut s = 0.0;
                    for (x, y) in tt.iter().zip(vv) {
                        let u = (x - t0) / (t1 - t0);
                        let yn = (y - vv[0]) / (vv[vv.len() - 1] - vv[0]);
                        s += (keys::ease(name, u) - yn).powi(2);
                    }
                    (s / tt.len() as f32).sqrt()
                };
                let lin = rms("linear");
                let best = names.iter().map(|n| (rms(n), n)).min_by(|a, b| a.0.partial_cmp(&b.0).unwrap()).unwrap();
                if best.0 < 0.8 * lin && best.0 < 0.06 {
                    e = best.1.clone();
                }
            }
        }
        out.push(json!([(t[i0] * 10.0).round() / 10.0, round4(v[i0]), e]));
    }
    out
}

fn round4(v: f32) -> f64 {
    ((v as f64) * 10000.0).round() / 10000.0
}

fn seam_window(w: usize, ov: f32) -> Vec<f32> {
    let l = 32.0 * (1.0 + ov);
    let (a0, ramp) = (16.0 * (1.0 - ov), 32.0 * ov);
    (0..w)
        .map(|i| {
            let ax = ((i as f32 + 0.5) / w as f32 * l - l / 2.0).abs();
            if ax <= a0 {
                1.0
            } else if ax >= a0 + ramp {
                0.0
            } else {
                (std::f32::consts::FRAC_PI_2 * (ax - a0) / ramp.max(1e-9)).cos().powi(2)
            }
        })
        .collect()
}

fn hex(c: [f32; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0].round().clamp(0.0, 255.0) as u8, c[1].round().clamp(0.0, 255.0) as u8, c[2].round().clamp(0.0, 255.0) as u8)
}

fn load_frames(dir: &Path) -> Result<(Vec<Img>, usize, bool, bool), String> {
    let mut sets: [std::collections::BTreeMap<usize, PathBuf>; 2] = Default::default();
    for e in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(rest) = name.strip_prefix("followpoint-").and_then(|r| r.strip_suffix(".png")) else { continue };
        let (num, two) = match rest.strip_suffix("@2x") {
            Some(n) => (n, true),
            None => (rest, false),
        };
        if let Ok(i) = num.parse::<usize>() {
            sets[two as usize].insert(i, e.path());
        }
    }
    let (ratio, set) = if !sets[0].is_empty() { (1, &sets[0]) } else if !sets[1].is_empty() { (2, &sets[1]) } else { return Err("no followpoint-N.png files found in that folder".into()) };
    let n = set.keys().max().unwrap() + 1;
    if let Some(gap) = (0..n).find(|i| !set.contains_key(i)) {
        return Err(format!("frames are missing (first gap at followpoint-{gap}); lazer stops at the first gap"));
    }
    let mut frames = Vec::new();
    for i in 0..n {
        let sp = sources::load_png(&set[&i])?;
        frames.push(Img { w: sp.w, h: sp.h, px: sp.px.iter().map(|p| [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]]).collect() });
    }
    Ok((frames, ratio, !sets[0].is_empty(), !sets[1].is_empty()))
}

fn guess_fps(n: usize) -> Option<f32> {
    [60, 30, 120, 144, 240, 90, 50, 24, 48, 75, 100].into_iter().find(|fps| (1.2 * *fps as f32).ceil() as usize + 1 == n).map(|f| f as f32)
}

/// AnimationFramerate from a skin.ini next to the frames or up to three folders above.
fn skin_fps(dir: &Path) -> Option<(f32, PathBuf)> {
    let mut d = Some(dir);
    for _ in 0..4 {
        let cur = d?;
        let ini = cur.join("skin.ini");
        if let Ok(text) = std::fs::read_to_string(&ini) {
            for line in text.lines() {
                let line = line.trim();
                if let Some((k, v)) = line.split_once(':') {
                    if k.trim().eq_ignore_ascii_case("AnimationFramerate") {
                        if let Ok(f) = v.trim().parse::<f32>() {
                            if f > 0.0 {
                                return Some((f, ini));
                            }
                        }
                    }
                }
            }
        }
        d = cur.parent();
    }
    None
}

fn infer_seamless(frames: &[Img], ratio: usize, fps: f32, notes: &mut Vec<String>) -> Option<Value> {
    let flen = 1000.0 / fps;
    let vis: Vec<usize> = (0..frames.len()).filter(|&i| (frames[i].w, frames[i].h) != (1, 1)).collect();
    if vis.len() < 8 || vis.iter().any(|&i| (frames[i].w, frames[i].h) != (frames[vis[0]].w, frames[vis[0]].h)) {
        return None;
    }
    let ip = *vis.iter().min_by(|&&a, &&b| (((a as f32 + 0.5) * flen - 600.0).abs()).partial_cmp(&(((b as f32 + 0.5) * flen - 600.0).abs())).unwrap()).unwrap();
    let a = alpha_plane(&frames[ip]);
    let (w, h) = (frames[ip].w, frames[ip].h);
    let d: Vec<f32> = a.iter().map(|&v| -(1.0 - v).clamp(1e-3, 1.0).ln()).collect();
    let p: Vec<f32> = (0..w).map(|x| (0..h).map(|y| d[y * w + x]).fold(0.0, f32::max)).collect();
    let pmax = p.iter().cloned().fold(0.0, f32::max);
    if pmax < 0.05 {
        return None;
    }
    let cols: Vec<usize> = (0..w).filter(|&x| p[x] > 0.02 * pmax).collect();
    let (x0, x1) = (*cols.first()?, *cols.last()? + 1);
    let wt = x1 - x0;
    if (wt as f32) < 40.0 * ratio as f32 || h > wt {
        return None;
    }
    let pfull: Vec<f32> = p.iter().map(|v| v / pmax).collect();
    let xcen = (x0 + x1) as f32 / 2.0;
    let mut best: Option<(f32, f32, usize, Vec<f32>)> = None;
    for lp in wt..wt + 10 {
        let xs0 = (xcen - lp as f32 / 2.0).round() as i64;
        let seg: Vec<f32> = (0..lp).map(|j| { let x = xs0 + j as i64; if x >= 0 && (x as usize) < w { pfull[x as usize] } else { 0.0 } }).collect();
        for ov in [1.0, 0.9, 0.75, 0.6, 0.5, 0.35, 0.25, 0.1, 0.0] {
            let win = seam_window(lp, ov);
            let rms = (seg.iter().zip(&win).map(|(a, b)| (a - b).powi(2)).sum::<f32>() / lp as f32).sqrt();
            if best.as_ref().is_none_or(|b| rms < b.0) {
                best = Some((rms, ov, lp, seg.clone()));
            }
        }
    }
    let (rms, ov, wt, pn) = best?;
    if rms > 0.07 || pn[0] > 0.2 || pn[pn.len() - 1] > 0.2 {
        return None;
    }
    let x0 = (xcen - wt as f32 / 2.0).round() as i64;
    let sc = 32.0 * (1.0 + ov) * ratio as f32 / wt as f32;
    let cs = (5.0 + (1.0 - 2.0 * sc / 1.00041) * 5.0 / 0.7).clamp(0.0, 10.0);
    let xc = (x0 + (wt / 2) as i64).clamp(0, w as i64 - 1) as usize;
    let col: Vec<f32> = (0..h).map(|y| d[y * w + xc]).collect();
    let gm = col.iter().cloned().fold(1e-6, f32::max);
    let thick = col.iter().map(|v| v / gm).sum::<f32>() * sc / ratio as f32;
    let amax = frames[ip].px.iter().map(|p| p[3]).fold(0.0, f32::max).min(0.999);
    let t0_full = (400.0 * amax).max(350.0);
    let t_on = vis[0] as f32 * flen;
    let mut st = presets::default_settings();
    let o = st.as_object_mut().unwrap();
    o.insert("preset".into(), json!(SEAMLESS));
    o.insert("sl_cs".into(), json!((cs * 100.0).round() / 100.0));
    o.insert("sl_thick".into(), json!((thick * 100.0).round() / 100.0));
    o.insert("sl_alpha".into(), json!((amax * 100.0).round() / 100.0));
    o.insert("sl_overlap".into(), json!(ov));
    o.insert("width".into(), json!((w as f32 / ratio as f32).round() as i64));
    o.insert("height".into(), json!((h as f32 / ratio as f32).round() as i64));
    o.insert("fps".into(), json!(fps));
    if t_on < 30.0 {
        o.insert("sl_tight".into(), json!(0.0));
    } else {
        o.insert("sl_tight".into(), json!(((t_on / t0_full).min(1.0) * 100.0).round() / 100.0));
        let full = vis.iter().copied().find(|&i| frames[i].px.iter().map(|p| p[3]).fold(0.0, f32::max) >= 0.97 * amax).unwrap_or(vis[0]);
        o.insert("sl_soft".into(), json!((full as f32 * flen - t_on).max(60.0).round()));
    }
    let stats: Vec<[f32; 3]> = vis.iter().filter_map(|&i| frame_stats(&frames[i]).and_then(|s| s.col)).collect();
    if !stats.is_empty() {
        let pick: Vec<[f32; 3]> = (0..6.min(stats.len())).map(|k| stats[k * (stats.len() - 1) / (6.min(stats.len()) - 1).max(1)]).collect();
        let mean_min = (0..3).map(|k| pick.iter().map(|c| c[k]).sum::<f32>() / pick.len() as f32).fold(255.0, f32::min);
        if mean_min < 235.0 {
            o.insert("color_on".into(), json!(true));
            o.insert("color_keys".into(), json!([[0, hex(pick[0]), "linear"], ["end", hex(pick[pick.len() - 1]), "linear"]]));
            notes.push("Color keyframes: only the first and last colors were read (a 2-key estimate).".into());
        }
    }
    notes.push(format!("Recognised the seamless line: CS about {cs:.1}, thickness {thick:.1}, opacity {amax:.2}, overlap {ov}."));
    notes.push("Only the line's own timing was recovered. If it was also animated (spin, slide, ...) those keyframes are not inferred; the match score shows how much that matters.".into());
    Some(st)
}

fn sprite_img(sp: &Sprite) -> Img {
    Img { w: sp.w, h: sp.h, px: sp.px.iter().map(|p| [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]]).collect() }
}

fn infer_classic(frames: &[Img], ratio: usize, fps: f32, notes: &mut Vec<String>) -> Result<(Value, Option<Sprite>), String> {
    let flen = 1000.0 / fps;
    let stats: Vec<Option<Stats>> = frames.iter().map(frame_stats).collect();
    let mut vis: Vec<usize> = (0..frames.len()).filter(|&i| stats[i].is_some()).collect();
    if vis.is_empty() {
        return Err("every frame is blank".into());
    }
    let mut sizes: Vec<((usize, usize), usize)> = Vec::new();
    for &i in &vis {
        let sz = (frames[i].w, frames[i].h);
        match sizes.iter_mut().find(|s| s.0 == sz) {
            Some(s) => s.1 += 1,
            None => sizes.push((sz, 1)),
        }
    }
    if sizes.len() > 1 {
        let best = sizes.iter().max_by_key(|s| s.1).unwrap().0;
        notes.push(format!("Frames have different sizes ({} sizes); using {}x{} and ignoring the others.", sizes.len(), best.0, best.1));
        vis.retain(|&i| (frames[i].w, frames[i].h) == best);
    }
    let (w, h) = (frames[vis[0]].w, frames[vis[0]].h);
    let (first, last) = (vis[0], *vis.last().unwrap());
    let n = last - first + 1;
    let cand: Vec<usize> = (0..12.min(vis.len())).map(|k| vis[k * (vis.len() - 1) / (12.min(vis.len()) - 1).max(1)]).collect();
    // which drawn preset, if any?
    let mut errs: Vec<(String, f32, usize)> = Vec::new();
    let preset_stats: Vec<(String, Stats)> = sources::NAMES.iter().filter_map(|n| sources::preset(n, 1).and_then(|sp| frame_stats(&sprite_img(&sp))).map(|s| (n.to_string(), s))).collect();
    for (name, nat) in &preset_stats {
        let natp: Plane = (nat.a.clone(), nat.w, nat.h);
        let mut best = (9.0f32, cand[0]);
        for &i in &cand {
            for au in upright(stats[i].as_ref().unwrap(), nat) {
                let e = shape_error(&au, &natp);
                if e < best.0 {
                    best = (e, i);
                }
            }
        }
        errs.push((name.clone(), best.0, best.1));
    }
    errs.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    let second = errs.get(1).map(|e| e.1).unwrap_or(9.0);
    let chosen = errs.first().filter(|b| b.1 < 0.3 && (b.1 < 0.1 || second > 1.35 * b.1)).cloned();
    let mut st = presets::default_settings();
    let mut custom = None;
    let nat_stats: Stats;
    match chosen {
        Some((name, e, _)) => {
            nat_stats = preset_stats.into_iter().find(|(n, _)| *n == name).unwrap().1;
            st["preset"] = json!(name);
            notes.push(format!("Source image: '{name}' (shape difference {e:.2})."));
        }
        None => {
            let i = cand[cand.len() / 2];
            let s = stats[i].as_ref().unwrap();
            let (mut x0, mut y0, mut x1, mut y1) = (s.w, s.h, 0, 0);
            for y in 0..s.h {
                for x in 0..s.w {
                    if s.a[y * s.w + x] > 0.04 {
                        x0 = x0.min(x);
                        y0 = y0.min(y);
                        x1 = x1.max(x);
                        y1 = y1.max(y);
                    }
                }
            }
            let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
            let mut px = Vec::new();
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let p = frames[i].px[y * s.w + x];
                    px.push(if p[3] > 1e-6 { [p[0] / p[3], p[1] / p[3], p[2] / p[3], p[3]] } else { [1.0; 4] });
                }
            }
            let mut sp = Sprite { w: cw, h: ch, px };
            if ratio == 2 {
                let small = img::resize_area(&sprite_img(&sp), (cw / 2).max(1), (ch / 2).max(1));
                sp = Sprite { w: small.w, h: small.h, px: small.px.iter().map(|p| if p[3] > 1e-6 { [p[0] / p[3], p[1] / p[3], p[2] / p[3], p[3]] } else { [1.0; 4] }).collect() };
            }
            nat_stats = frame_stats(&sprite_img(&sp)).ok_or("the cut-out sprite is empty")?;
            custom = Some(sp);
            st["preset"] = json!("Custom image...");
            notes.push("Source image: no preset matched, so the sprite was cut out of a frame and saved as a custom image.".into());
        }
    }
    let nat = &nat_stats;
    let sx_major = nat.cxx >= nat.cyy;
    let isotropic = nat.l1 / nat.l2 < 1.25;
    let nat_theta = if isotropic { 0.0 } else { nat.theta };
    let (ox, oy) = (nat.cx - nat.w as f32 / 2.0, nat.cy - nat.h as f32 / 2.0);
    let (wd, hd) = (w as f32 / ratio as f32, h as f32 / ratio as f32);
    let (mut ts, mut xs, mut ys, mut sxs, mut sys, mut rots, mut als, mut cols) = (vec![], vec![], vec![], vec![], vec![], vec![], vec![], Vec::<[f32; 3]>::new());
    for i in first..=last {
        let Some(s) = &stats[i] else { continue };
        if (frames[i].w, frames[i].h) != (w, h) {
            continue;
        }
        let (mut sx, mut sy, rot);
        if isotropic {
            let sc = ((s.l1 + s.l2) / (nat.l1 + nat.l2)).sqrt();
            sx = sc;
            sy = sc;
            rot = 0.0;
        } else {
            if sx_major {
                sx = (s.l1 / nat.l1).sqrt();
                sy = (s.l2 / nat.l2).sqrt();
            } else {
                sy = (s.l1 / nat.l1).sqrt();
                sx = (s.l2 / nat.l2).sqrt();
            }
            rot = (s.theta - nat_theta + 90.0).rem_euclid(180.0) - 90.0;
        }
        sx /= ratio as f32;
        sy /= ratio as f32;
        let th = rot.to_radians();
        let dxo = ox * sx * th.cos() - oy * sy * th.sin();
        let dyo = ox * sx * th.sin() + oy * sy * th.cos();
        let (cx, cy) = (s.cx / ratio as f32 - dxo, s.cy / ratio as f32 - dyo);
        let trav = wd - nat.w as f32;
        ts.push((i - first) as f32);
        xs.push(if trav > 2.0 { 0.5 + (cx - wd / 2.0) / trav } else { 0.5 });
        ys.push(cy - hd / 2.0);
        sxs.push(sx);
        sys.push(sy);
        rots.push(rot);
        als.push(s.peak / nat.peak.max(1e-6));
        cols.push(s.col.unwrap_or_else(|| cols.last().copied().unwrap_or([255.0; 3])));
    }
    if !isotropic && rots.len() > 2 {
        // the axis angle repeats every 180 degrees: follow it continuously
        let pi = std::f32::consts::PI;
        let rad: Vec<f32> = rots.iter().map(|r| 2.0 * r.to_radians()).collect();
        let mut un: Vec<f32> = vec![rad[0].to_degrees() / 2.0];
        let mut corr = 0.0f32;
        for k in 1..rad.len() {
            let dd = rad[k] - rad[k - 1];
            let mut ddmod = (dd + pi).rem_euclid(2.0 * pi) - pi;
            if ddmod == -pi && dd > 0.0 {
                ddmod = pi;
            }
            if dd.abs() >= pi {
                corr += ddmod - dd;
            }
            un.push((rad[k] + corr).to_degrees() / 2.0);
        }
        let shift = 180.0 * (un[0] / 180.0).round();
        rots = un.iter().map(|v| v - shift).collect();
    }
    let r_len = n as f32 * flen;
    let dlen = r_len.max(500.0);
    let spd = dlen / r_len;
    let tau: Vec<f32> = ts.iter().map(|t| t * dlen / (n - 1).max(1) as f32).collect();
    let mut changed: Vec<&str> = Vec::new();
    let tol = [("x", 0.012f32, 0.5f32), ("y", 0.5, 0.0), ("sx", 0.03, 1.0), ("sy", 0.03, 1.0), ("rot", 1.5, 0.0), ("alpha", 0.03, 1.0)];
    let series: [&Vec<f32>; 6] = [&xs, &ys, &sxs, &sys, &rots, &als];
    for (k, (key, t, base)) in tol.iter().enumerate() {
        let vals = series[k];
        let (mn, mx) = vals.iter().fold((f32::MAX, f32::MIN), |a, v| (a.0.min(*v), a.1.max(*v)));
        let mut sorted = vals.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = sorted[sorted.len() / 2];
        let slack = if *key == "rot" { 6.0 } else { 2.0 };
        if mx - mn <= t * 1.5 && (median - base).abs() <= t * slack {
            st["props"][*key] = json!([[0, base, "linear"], [(dlen * 10.0).round() / 10.0, base, "linear"]]);
        } else {
            let mut keys = fit_keys(&tau, vals, *t);
            if keys.last().and_then(|k| k[0].as_f64()).unwrap_or(0.0) < dlen as f64 - 1e-6 {
                let lastv = keys.last().unwrap()[1].clone();
                keys.push(json!([(dlen * 10.0).round() / 10.0, lastv, "linear"]));
            }
            st["props"][*key] = Value::Array(keys);
            changed.push(*key);
        }
    }
    let colmin = cols.iter().flat_map(|c| c.iter().copied()).fold(255.0, f32::min);
    let nat_col_min = nat.col.map(|c| c.iter().cloned().fold(255.0, f32::min)).unwrap_or(255.0);
    if st["preset"].as_str().is_some_and(|p| p != "Custom image...") && colmin < 235.0 && nat_col_min > 235.0 {
        st["color_on"] = json!(true);
        let dist: Vec<f32> = cols.iter().map(|c| (0..3).map(|k| (c[k] - cols[0][k]).abs()).sum()).collect();
        let ik = rdp(&tau, &dist, 12.0);
        st["color_keys"] = Value::Array(ik.iter().map(|&j| json!([(tau[j] * 10.0).round() / 10.0, hex(cols[j]), "linear"])).collect());
        changed.push("color");
    }
    st["width"] = json!(wd.round() as i64);
    st["height"] = json!(hd.round() as i64);
    st["fps"] = json!(fps);
    st["speed"] = json!((spd as f64 * 10000.0).round() / 10000.0);
    st["lead"] = json!(((first as f32 * flen) as f64 * 10.0).round() / 10.0);
    st["window_mode"] = json!("shift");
    st["fade_in"] = json!(0.0);
    st["fade_out"] = json!(0.0);
    st["no_loop"] = json!(frames.len() as f32 * flen >= 1200.0);
    notes.push(format!("Timing: starts {:.0} ms after the point begins fading in, runs {:.0} ms ({n} frames) -> appear {}, speed {}.", first as f32 * flen, r_len, st["lead"], st["speed"]));
    notes.push(format!("Animated properties found: {}.", if changed.is_empty() { "none (a still image)".to_string() } else { changed.join(", ") }));
    if changed.contains(&"rot") {
        notes.push("Rotation is read modulo 180 degrees, so very fast spins cannot be told apart.".into());
    }
    Ok((st, custom))
}

/// Infer settings from the frames in `dir`. `fps` overrides the guess; `custom_path` is where a cut-out sprite is saved
/// if the source has to be a custom image.
pub fn analyze(dir: &Path, fps: Option<f32>, custom_path: &Path) -> Result<Analysis, String> {
    let mut notes = Vec::new();
    let (frames, ratio, has1, has2) = load_frames(dir)?;
    let fps = if let Some(f) = fps {
        notes.push(format!("Frame rate {f} (given)."));
        f
    } else if let Some((f, p)) = skin_fps(dir) {
        notes.push(format!("Frame rate {f} from {}.", p.display()));
        f
    } else if let Some(f) = guess_fps(frames.len()) {
        notes.push(format!("Frame rate {f} guessed from the frame count ({}); no skin.ini was found. Check it against your AnimationFramerate.", frames.len()));
        f
    } else {
        notes.push("Frame rate unknown (no skin.ini, odd frame count): assuming 60. Images do not store it; set the real AnimationFramerate if you remember it.".into());
        60.0
    };
    notes.push(format!("{} frames read from the {} set{}.", frames.len(), if ratio == 2 { "@2x" } else { "1x" }, if has1 && has2 { " (both sizes present)" } else { "" }));
    let mut custom = None;
    let mut st = match infer_seamless(&frames, ratio, fps, &mut notes) {
        Some(s) => s,
        None => {
            let (s, c) = infer_classic(&frames, ratio, fps, &mut notes)?;
            custom = c;
            s
        }
    };
    st["export"] = json!(if has1 && has2 { "both" } else if ratio == 2 { "2x" } else { "1x" });
    st["hires"] = json!(st["export"] == "2x");
    if let Some(sp) = &custom {
        // write the cut-out sprite so the settings can point at it
        let mut raw = Vec::with_capacity(sp.w * sp.h * 4);
        for p in &sp.px {
            raw.extend_from_slice(&[(p[0] * 255.0 + 0.5) as u8, (p[1] * 255.0 + 0.5) as u8, (p[2] * 255.0 + 0.5) as u8, (p[3] * 255.0 + 0.5) as u8]);
        }
        let file = std::fs::File::create(custom_path).map_err(|e| e.to_string())?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), sp.w as u32, sp.h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().map_err(|e| e.to_string())?.write_image_data(&raw).map_err(|e| e.to_string())?;
        st["image"] = json!(custom_path.to_string_lossy());
    }
    // how well do the inferred settings reproduce the input?
    let back = gen::build_frames(&Settings::from_json(&st), ratio, &Options { sampler: Sampler::CatmullRom });
    let (mut inter, mut union) = (0.0f64, 0.0f64);
    for k in 0..back.len().max(frames.len()) {
        let a = frames.get(k).filter(|f| (f.w, f.h) != (1, 1));
        let b = back.get(k).and_then(|f| f.as_ref());
        match (a, b) {
            (Some(a), Some(b)) if (a.w, a.h) == (b.w, b.h) => {
                for (p, q) in a.px.iter().zip(&b.px) {
                    inter += p[3].min(q[3]) as f64;
                    union += p[3].max(q[3]) as f64;
                }
            }
            (Some(x), _) | (_, Some(x)) => union += x.px.iter().map(|p| p[3] as f64).sum::<f64>(),
            _ => {}
        }
    }
    let matched = (union > 0.0).then(|| (inter / union) as f32);
    if let Some(m) = matched {
        notes.push(format!("Rebuilding the frames from these settings overlaps the originals by {:.0}% (100% = identical alpha).", m * 100.0));
    }
    Ok(Analysis { settings: st, notes, custom, matched })
}
