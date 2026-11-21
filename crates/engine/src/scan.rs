//! Skin-wide image quality scanner: detects poor 1x downscales (by comparing against @2x),
//! gradient banding, bright/dark edge haloing, diagonal aliasing, and flat-region noise,
//! and synthesizes a starting `Vec<Op>` fix for each flagged asset.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::edit::{self, Grain, Op, Resample};
use crate::img::Img;
use crate::restore::{AntiAlias, Dehalo, Denoise, LumaScale, Mask, Target};

#[derive(Clone, Debug, PartialEq)]
pub enum IssueKind {
    /// Existing 1x diverges significantly (or rings) relative to area-averaged @2x.
    PoorDownscale,
    /// Smooth gradient shows quantized 8-bit stair-step plateaus (often in dark glows / alpha).
    Banding { dark: bool, avg_plateau: f32, avg_step: f32 },
    /// Bright overshoot halo along high-contrast edges.
    HaloBright,
    /// Dark undershoot halo along high-contrast edges.
    HaloDark,
    /// Staircase jaggedness along diagonal edges.
    Aliasing,
    /// High-frequency pixel noise in flat regions.
    Noise,
}

#[derive(Clone, Debug)]
pub struct Issue {
    pub kind: IssueKind,
    pub label: String,
    pub detail: String,
    /// 0.0..1.0 normalized severity for sorting and tuning suggested fixes.
    pub severity: f32,
}

#[derive(Clone, Debug)]
pub struct ScanItem {
    pub path: PathBuf,
    pub name: String,
    pub dims: (usize, usize),
    /// Matching `@2x` file if this is a `1x` asset and `@2x` exists.
    pub pair_2x: Option<PathBuf>,
    pub issues: Vec<Issue>,
    pub suggested_ops: Vec<Op>,
}

#[derive(Clone, Debug, Default)]
pub struct ScanReport {
    pub total_files: usize,
    pub skipped_blank: usize,
    pub flagged: Vec<ScanItem>,
}

/// Scan every PNG in `dir` (and one level of subfolders, skipping hidden folders like `.skin-studio-backup`).
pub fn scan_skin(dir: &Path) -> ScanReport {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let mut visit = |d: &Path, prefix: &str| {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                if p.is_file() && name.to_lowercase().ends_with(".png") {
                    let shown = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
                    files.push((shown, p));
                }
            }
        }
    };
    visit(dir, "");
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() && !name.starts_with('.') {
                visit(&p, &name);
            }
        }
    }
    files.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

    // Map lowercase relative stem -> path so 1x files can quickly find their @2x partner.
    let mut by_key: HashMap<String, PathBuf> = HashMap::with_capacity(files.len());
    for (shown, p) in &files {
        by_key.insert(shown.to_lowercase(), p.clone());
    }

    let total_files = files.len();
    let results: Vec<Option<Result<ScanItem, ()>>> = files
        .par_iter()
        .map(|(shown, path)| {
            let img = edit::load_png(path).ok()?;
            // Skip tiny 1x1 or 2x2 blank/sentinel PNGs commonly used in osu! skins.
            if img.w <= 2 && img.h <= 2 {
                return Some(Err(()));
            }
            let lower = shown.to_lowercase();
            let pair_2x = if !lower.ends_with("@2x.png") && lower.ends_with(".png") {
                let key_2x = format!("{}@2x.png", &lower[..lower.len() - 4]);
                by_key.get(&key_2x).cloned()
            } else {
                None
            };
            let item = analyze_image(path, shown, &img, pair_2x.as_deref());
            item.map(Ok)
        })
        .collect();

    let mut skipped_blank = 0usize;
    let mut flagged = Vec::new();
    for r in results.into_iter().flatten() {
        match r {
            Err(()) => skipped_blank += 1,
            Ok(item) => flagged.push(item),
        }
    }
    flagged.sort_by(|a, b| {
        let sa = a.issues.iter().map(|i| i.severity).fold(0.0f32, f32::max);
        let sb = b.issues.iter().map(|i| i.severity).fold(0.0f32, f32::max);
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.name.cmp(&b.name))
    });
    ScanReport { total_files, skipped_blank, flagged }
}

/// Analyze a single loaded image (plus optional `@2x` partner) and return `Some(ScanItem)` if any issues are spotted.
pub fn analyze_image(path: &Path, name: &str, img: &Img, pair_2x: Option<&Path>) -> Option<ScanItem> {
    let mut issues = Vec::new();

    // 1. Compare 1x against @2x local neighborhood min/max to catch genuine ringing/overshoot (e.g. Photoshop Bicubic Sharper)
    //    without false-positiving on clean independently-rendered 1x assets like Skin Studio's own followpoints.
    if let Some(p2) = pair_2x {
        if let Ok(hi) = edit::load_png(p2) {
            let w_ok = hi.w >= img.w.saturating_mul(2).saturating_sub(2) && hi.w <= img.w.saturating_mul(2).saturating_add(2);
            let h_ok = hi.h >= img.h.saturating_mul(2).saturating_sub(2) && hi.h <= img.h.saturating_mul(2).saturating_add(2);
            if w_ok && h_ok && hi.w > 4 && hi.h > 4 {
                if let Some(issue) = detect_poor_downscale(img, &hi) {
                    issues.push(issue);
                }
            }
        }
    }

    // Build luma (0..255) and alpha (0..255) planes once for the remaining detectors.
    let n = img.w * img.h;
    let mut luma = vec![0.0f32; n];
    let mut alpha = vec![0.0f32; n];
    let mut opaque_count = 0usize;
    for (i, p) in img.px.iter().enumerate() {
        luma[i] = (0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]).clamp(0.0, 1.0) * 255.0;
        alpha[i] = p[3].clamp(0.0, 1.0) * 255.0;
        if p[3] > 0.02 {
            opaque_count += 1;
        }
    }
    if opaque_count < 16 {
        return if issues.is_empty() { None } else {
            let suggested_ops = build_suggested_ops((img.w, img.h), &issues);
            Some(ScanItem { path: path.to_path_buf(), name: name.to_string(), dims: (img.w, img.h), pair_2x: pair_2x.map(Path::to_path_buf), issues, suggested_ops })
        };
    }

    if let Some(issue) = detect_banding(img.w, img.h, &luma, &alpha) {
        issues.push(issue);
    }
    let (hb, hd) = detect_haloing(img.w, img.h, &luma, &alpha);
    if let Some(i) = hb {
        issues.push(i);
    }
    if let Some(i) = hd {
        issues.push(i);
    }
    if let Some(issue) = detect_aliasing(img.w, img.h, &luma, &alpha) {
        issues.push(issue);
    }
    if let Some(issue) = detect_noise(img.w, img.h, &luma, &alpha) {
        issues.push(issue);
    }

    if issues.is_empty() {
        return None;
    }
    let suggested_ops = build_suggested_ops((img.w, img.h), &issues);
    Some(ScanItem {
        path: path.to_path_buf(),
        name: name.to_string(),
        dims: (img.w, img.h),
        pair_2x: pair_2x.map(Path::to_path_buf),
        issues,
        suggested_ops,
    })
}

/// Detect genuine poor 1x downscales against `@2x` using three complementary physical checks:
/// 1. Tight 3×3 `@2x` overshoot/undershoot (catches Photoshop Bicubic Sharper / Lanczos ringing halos).
/// 2. Semi-transparent edge darkening (catches non-premultiplied transparent downscaling where black bleeds into edges).
/// 3. Subpixel-tolerant mismatch against the 3×3 envelope of clean `area_average(@2x)` (catches Nearest-Neighbor,
///    over-blurred, or stale/mismatched 1x files while allowing tiny subpixel alignment differences on clean procedural 1x renders).
fn detect_poor_downscale(actual: &Img, hi_2x: &Img) -> Option<Issue> {
    if actual.w < 6 || actual.h < 6 || hi_2x.w < 12 || hi_2x.h < 12 {
        return None;
    }
    let ref_1x = edit::resize(hi_2x, actual.w, actual.h, Resample::Smooth);
    let sx = hi_2x.w as f32 / actual.w as f32;
    let sy = hi_2x.h as f32 / actual.h as f32;

    let mut active_px = 0usize;
    let mut ring_px = 0usize;
    let mut max_ring = 0.0f32;
    let mut edge_semi_px = 0usize;
    let mut dark_fringe_px = 0usize;
    let mut env_bad_px = 0usize;
    let mut env_sq_sum = 0.0f64;

    for y in 1..actual.h - 1 {
        for x in 1..actual.w - 1 {
            let i = y * actual.w + x;
            let a = actual.px[i];
            let r = ref_1x.px[i];
            if a[3] < 0.03 && r[3] < 0.03 {
                continue;
            }
            active_px += 1;

            // 1. Symmetric 4x4 neighborhood in @2x centered on the exact mapped 2x2 footprint `[2x, 2x+1]` (±1 @2x px margin)
            //    so all four sides of circular edges (e.g. sliderstartcircle) are treated symmetrically.
            let x0 = (x as f32 * sx).floor() as isize - 1;
            let y0 = (y as f32 * sy).floor() as isize - 1;
            let mut lo2 = [1.0f32; 4];
            let mut hi2 = [0.0f32; 4];
            for dy in 0..4isize {
                let hy = (y0 + dy).clamp(0, hi_2x.h as isize - 1) as usize;
                for dx in 0..4isize {
                    let hx = (x0 + dx).clamp(0, hi_2x.w as isize - 1) as usize;
                    let q = hi_2x.px[hy * hi_2x.w + hx];
                    for k in 0..4 {
                        lo2[k] = lo2[k].min(q[k]);
                        hi2[k] = hi2[k].max(q[k]);
                    }
                }
            }
            let mut over = 0.0f32;
            for k in 0..4 {
                over = over.max(a[k] - hi2[k]).max(lo2[k] - a[k]);
            }
            if over > max_ring {
                max_ring = over;
            }
            if over > 0.045 {
                ring_px += 1;
            }

            // 2. Dark alpha-fringe check (non-premultiplied downscale bleeds black into semi-transparent edges)
            if r[3] > 0.08 && r[3] < 0.85 && a[3] > 0.08 {
                edge_semi_px += 1;
                let rl = (0.2126 * r[0] + 0.7152 * r[1] + 0.0722 * r[2]) / r[3].max(1e-4);
                let al = (0.2126 * a[0] + 0.7152 * a[1] + 0.0722 * a[2]) / a[3].max(1e-4);
                if rl - al > 0.08 {
                    dark_fringe_px += 1;
                }
            }

            // 3. Subpixel-tolerant 3x3 envelope check against clean area-averaged ref_1x
            let mut rlo = [1.0f32; 4];
            let mut rhi = [0.0f32; 4];
            for dy in -1..=1isize {
                let ry = (y as isize + dy) as usize;
                for dx in -1..=1isize {
                    let rx = (x as isize + dx) as usize;
                    let q = ref_1x.px[ry * actual.w + rx];
                    for k in 0..4 {
                        rlo[k] = rlo[k].min(q[k]);
                        rhi[k] = rhi[k].max(q[k]);
                    }
                }
            }
            let mut e = 0.0f32;
            for k in 0..4 {
                e = e.max(a[k] - rhi[k]).max(rlo[k] - a[k]);
            }
            env_sq_sum += (e * e) as f64;
            if e > 0.035 {
                env_bad_px += 1;
            }
        }
    }

    if active_px < 24 {
        return None;
    }
    let ring_ratio = ring_px as f32 / active_px as f32;
    let fringe_ratio = if edge_semi_px >= 16 { dark_fringe_px as f32 / edge_semi_px as f32 } else { 0.0 };
    let env_rmse = (env_sq_sum / active_px as f64).sqrt() as f32;
    let env_bad_ratio = env_bad_px as f32 / active_px as f32;

    if ring_ratio > 0.02 && max_ring > 0.09 {
        Some(Issue {
            kind: IssueKind::PoorDownscale,
            label: "Poor 1x downscale (ringing)".into(),
            detail: format!("Overshoot/ringing vs @2x on {:.1}% of pixels (peak {:.0}%)", ring_ratio * 100.0, max_ring * 100.0),
            severity: ((ring_ratio / 0.08).max(max_ring / 0.22)).clamp(0.45, 1.0),
        })
    } else if fringe_ratio > 0.14 {
        Some(Issue {
            kind: IssueKind::PoorDownscale,
            label: "Poor 1x downscale (dark fringe)".into(),
            detail: format!("Non-premultiplied dark edge fringe on {:.0}% of translucent pixels", fringe_ratio * 100.0),
            severity: (fringe_ratio * 1.4).clamp(0.45, 1.0),
        })
    } else if env_rmse > 0.045 || env_bad_ratio > 0.06 {
        Some(Issue {
            kind: IssueKind::PoorDownscale,
            label: "Poor 1x downscale".into(),
            detail: format!("Differs from clean @2x downscale ({:.1}% pixels outside local envelope)", env_bad_ratio * 100.0),
            severity: ((env_rmse / 0.08).max(env_bad_ratio / 0.12)).clamp(0.4, 1.0),
        })
    } else {
        None
    }
}

/// Detect quantized 8-bit stair-stepping in smooth gradient regions (luma or alpha).
/// Requires wide flat plateaus (>= 5 px) that stay coherent perpendicular to the ramp (filtering out already-dithered gradients).
fn detect_banding(w: usize, h: usize, luma: &[f32], alpha: &[f32]) -> Option<Issue> {
    if w < 24 || h < 24 {
        return None;
    }
    let check_plane = |plane: &[f32], mask: &[f32]| -> (f32, bool, f32, f32) {
        let mut grad_runs = 0usize;
        let mut banded_runs = 0usize;
        let mut dark_banded = 0usize;
        let mut plateau_sum = 0.0f32;
        let mut plateau_cnt = 0usize;
        let mut step_sum = 0.0f32;
        let mut step_cnt = 0usize;
        let mut scan_line = |len: usize, get: &dyn Fn(usize) -> (f32, f32, f32)| {
            let mut run_len = 1usize;
            let mut coherent_px = 0usize;
            let (v0, _, n0) = get(0);
            let mut prev = v0.round();
            if (v0 - n0).abs() < 0.5 {
                coherent_px += 1;
            }
            let mut steps_in_ramp = 0usize;
            let mut flat_plateaus = 0usize;
            let mut ramp_sum = 0.0f32;
            let mut dir = 0i32;
            let mut finish = |steps: usize, plats: usize, rsum: f32| {
                if steps >= 5 {
                    grad_runs += 1;
                    if plats >= 4 {
                        banded_runs += 1;
                        if rsum / (steps as f32) < 96.0 {
                            dark_banded += 1;
                        }
                    }
                }
            };
            for k in 1..len {
                let (v, a, nb) = get(k);
                if a < 6.0 {
                    finish(steps_in_ramp, flat_plateaus, ramp_sum);
                    run_len = 1;
                    coherent_px = 0;
                    steps_in_ramp = 0;
                    flat_plateaus = 0;
                    ramp_sum = 0.0;
                    dir = 0;
                    prev = v.round();
                    continue;
                }
                let q = v.round();
                let d = (q - prev) as i32;
                if d == 0 {
                    run_len += 1;
                    if (v - nb).abs() < 0.5 {
                        coherent_px += 1;
                    }
                } else if d.abs() <= 3 && (dir == 0 || d.signum() == dir) {
                    // A genuine un-dithered band is both wide (>= 5 px) and coherent across neighboring rows/cols.
                    if run_len >= 5 && coherent_px * 4 >= run_len * 3 {
                        flat_plateaus += 1;
                        plateau_sum += run_len as f32;
                        plateau_cnt += 1;
                    }
                    step_sum += d.abs() as f32;
                    step_cnt += 1;
                    steps_in_ramp += 1;
                    ramp_sum += q;
                    dir = d.signum();
                    run_len = 1;
                    coherent_px = usize::from((v - nb).abs() < 0.5);
                } else {
                    finish(steps_in_ramp, flat_plateaus, ramp_sum);
                    run_len = 1;
                    coherent_px = usize::from((v - nb).abs() < 0.5);
                    steps_in_ramp = 0;
                    flat_plateaus = 0;
                    ramp_sum = 0.0;
                    dir = 0;
                }
                prev = q;
            }
            finish(steps_in_ramp, flat_plateaus, ramp_sum);
        };
        let step_y = (h / 64).max(1);
        for y in (1..h - 1).step_by(step_y) {
            scan_line(w, &|x| (plane[y * w + x], mask[y * w + x], plane[(y + 1) * w + x]));
        }
        let step_x = (w / 64).max(1);
        for x in (1..w - 1).step_by(step_x) {
            scan_line(h, &|y| (plane[y * w + x], mask[y * w + x], plane[y * w + x + 1]));
        }
        if banded_runs < 8 || grad_runs == 0 {
            return (0.0, false, 6.0, 1.5);
        }
        let ratio = banded_runs as f32 / grad_runs as f32;
        let avg_plat = if plateau_cnt > 0 { plateau_sum / plateau_cnt as f32 } else { 6.0 };
        let avg_step = if step_cnt > 0 { step_sum / step_cnt as f32 } else { 1.5 };
        (if ratio > 0.48 { (ratio * 0.85).clamp(0.3, 1.0) } else { 0.0 }, dark_banded * 2 >= banded_runs, avg_plat, avg_step)
    };

    let (sl, dl, pl, stl) = check_plane(luma, alpha);
    let (sa, da, pa, sta) = check_plane(alpha, alpha);
    let score = sl.max(sa);
    if score > 0.0 {
        let (dark, avg_plateau, avg_step) = if sl >= sa { (dl, pl, stl) } else { (da, pa, sta) };
        Some(Issue {
            kind: IssueKind::Banding { dark, avg_plateau, avg_step },
            label: if dark { "Dark banding".into() } else { "Banding".into() },
            detail: format!("Quantized {:.0} px plateaus in smooth gradient", avg_plateau),
            severity: score,
        })
    } else {
        None
    }
}

/// Detect bright overshoot or dark undershoot ringing 1–2 px adjacent to genuine interior step edges.
/// Requires solid alpha (> 200) across the full probe so glowing outer silhouettes / hollow ring cursors are never flagged.
fn detect_haloing(w: usize, h: usize, luma: &[f32], alpha: &[f32]) -> (Option<Issue>, Option<Issue>) {
    if w < 16 || h < 16 {
        return (None, None);
    }
    let mut edge_px = 0usize;
    let mut bright_hits = 0usize;
    let mut dark_hits = 0usize;
    for y in 5..h.saturating_sub(5) {
        for x in 5..w.saturating_sub(5) {
            let i = y * w + x;
            if alpha[i] < 200.0 {
                continue;
            }
            let gx = luma[i + 1] - luma[i - 1];
            let gy = luma[i + w] - luma[i - w];
            let g = (gx * gx + gy * gy).sqrt();
            if g < 60.0 {
                continue;
            }
            let (dx, dy) = if gx.abs() >= gy.abs() { (gx.signum() as isize, 0) } else { (0, gy.signum() as isize) };
            let sample = |k: isize| -> (f32, f32) {
                let sx = (x as isize + dx * k) as usize;
                let sy = (y as isize + dy * k) as usize;
                let idx = sy * w + sx;
                (luma[idx], alpha[idx])
            };
            // Require solid opacity across the whole probe so glowing alpha edges / ring cursors are ignored.
            let mut all_opaque = true;
            for k in -5..=5 {
                if sample(k).1 < 200.0 {
                    all_opaque = false;
                    break;
                }
            }
            if !all_opaque {
                continue;
            }
            // Require genuinely flat plateaus at +3..+5 and -3..-5 so smooth radial glows (e.g. colored center + white core + tinted rim) are not flagged.
            let (b3, b4, b5) = (sample(3).0, sample(4).0, sample(5).0);
            let (d3, d4, d5) = (sample(-3).0, sample(-4).0, sample(-5).0);
            if (b3 - b4).abs() > 2.5 || (b4 - b5).abs() > 2.5 || (d3 - d4).abs() > 2.5 || (d4 - d5).abs() > 2.5 || (b3 - d3) < 45.0 {
                continue;
            }
            edge_px += 1;
            let b1 = sample(1).0;
            let b2 = sample(2).0;
            if (b1.max(b2) - b3) > 22.0 && b1.max(b2) > luma[i] + 6.0 {
                bright_hits += 1;
            }
            let d1 = sample(-1).0;
            let d2 = sample(-2).0;
            if (d3 - d1.min(d2)) > 22.0 && d1.min(d2) + 6.0 < luma[i] {
                dark_hits += 1;
            }
        }
    }
    if edge_px < 48 {
        return (None, None);
    }
    let br = bright_hits as f32 / edge_px as f32;
    let dr = dark_hits as f32 / edge_px as f32;
    let hb = (br > 0.30).then(|| Issue {
        kind: IssueKind::HaloBright,
        label: "Bright haloing".into(),
        detail: format!("Bright edge overshoot on {:.0}% of interior step edges", br * 100.0),
        severity: (br * 1.5).clamp(0.35, 1.0),
    });
    let hd = (dr > 0.30).then(|| Issue {
        kind: IssueKind::HaloDark,
        label: "Dark haloing".into(),
        detail: format!("Dark edge undershoot on {:.0}% of interior step edges", dr * 100.0),
        severity: (dr * 1.5).clamp(0.35, 1.0),
    });
    (hb, hd)
}

/// Detect staircase aliasing along diagonal edges (high contrast with abrupt 0->255 binary transitions).
fn detect_aliasing(w: usize, h: usize, luma: &[f32], alpha: &[f32]) -> Option<Issue> {
    if w < 32 || h < 32 {
        return None;
    }
    let mut diag_edges = 0usize;
    let mut jagged = 0usize;
    for y in 2..h - 2 {
        for x in 2..w - 2 {
            let i = y * w + x;
            let gx = (alpha[i + 1] - alpha[i - 1]).abs().max((luma[i + 1] - luma[i - 1]).abs());
            let gy = (alpha[i + w] - alpha[i - w]).abs().max((luma[i + w] - luma[i - w]).abs());
            if gx > 80.0 && gy > 80.0 {
                diag_edges += 1;
                let a = alpha[i];
                let l = luma[i];
                let hard_a = a < 8.0 || a > 247.0;
                let hard_l = (l - luma[i - 1]).abs() > 100.0 || (l - luma[i + 1]).abs() > 100.0;
                if hard_a && hard_l {
                    jagged += 1;
                }
            }
        }
    }
    if diag_edges < 48 {
        return None;
    }
    let ratio = jagged as f32 / diag_edges as f32;
    if ratio > 0.72 {
        Some(Issue {
            kind: IssueKind::Aliasing,
            label: "Aliasing".into(),
            detail: format!("Hard stair-stepped transitions on {:.0}% of diagonal edges", ratio * 100.0),
            severity: (ratio * 0.9).clamp(0.3, 1.0),
        })
    } else {
        None
    }
}

/// Detect excessive high-frequency noise in flat, solidly-opaque interior regions using a 3x3 Laplacian
/// plus sign-alternation check so thin rings/strokes (like `spinner-approachcircle`) are never flagged as noise.
fn detect_noise(w: usize, h: usize, luma: &[f32], alpha: &[f32]) -> Option<Issue> {
    if w < 24 || h < 24 {
        return None;
    }
    let mut flat_px = 0usize;
    let mut lap_sum = 0.0f64;
    let mut flips = 0usize;
    for y in 2..h - 2 {
        let mut prev_sign = 0i32;
        for x in 2..w - 2 {
            let i = y * w + x;
            // Require solid opacity across the full 5x5 patch so thin strokes/rings on transparent are skipped.
            let mut solid = true;
            'chk: for dy in -2..=2isize {
                let row = (y as isize + dy) as usize * w;
                for dx in -2..=2isize {
                    if alpha[row + (x as isize + dx) as usize] < 200.0 {
                        solid = false;
                        break 'chk;
                    }
                }
            }
            if !solid {
                prev_sign = 0;
                continue;
            }
            let span = (luma[i + 2] - luma[i - 2]).abs().max((luma[i + 2 * w] - luma[i - 2 * w]).abs());
            if span > 16.0 {
                prev_sign = 0;
                continue;
            }
            let signed_lap = 4.0 * luma[i] - luma[i - 1] - luma[i + 1] - luma[i - w] - luma[i + w];
            let s = if signed_lap > 3.0 { 1 } else if signed_lap < -3.0 { -1 } else { 0 };
            if s != 0 && prev_sign != 0 && s != prev_sign {
                flips += 1;
            }
            if s != 0 {
                prev_sign = s;
            }
            flat_px += 1;
            lap_sum += signed_lap.abs() as f64;
        }
    }
    if flat_px < 160 {
        return None;
    }
    let mean_lap = (lap_sum / flat_px as f64) as f32;
    let flip_ratio = flips as f32 / flat_px as f32;
    if mean_lap > 14.0 && flip_ratio > 0.25 {
        Some(Issue {
            kind: IssueKind::Noise,
            label: "Excessive noise".into(),
            detail: format!("High-frequency grain in flat areas (score {mean_lap:.1})"),
            severity: ((mean_lap - 10.0) / 25.0).clamp(0.3, 1.0),
        })
    } else {
        None
    }
}

fn build_suggested_ops((w, h): (usize, usize), issues: &[Issue]) -> Vec<Op> {
    let mut ops = Vec::new();
    let mut need_bright = 0.0f32;
    let mut need_dark = 0.0f32;
    let min_dim = w.min(h) as f32;
    for iss in issues {
        match iss.kind {
            IssueKind::PoorDownscale => {}
            IssueKind::Noise => {
                let mut d = Denoise::default();
                d.h = (2.0 + iss.severity * 2.5).round();
                ops.push(Op::Denoise(d));
            }
            IssueKind::HaloBright => need_bright = need_bright.max((0.35 + iss.severity * 0.3).min(0.7)),
            IssueKind::HaloDark => need_dark = need_dark.max((0.35 + iss.severity * 0.3).min(0.7)),
            IssueKind::Aliasing => {
                ops.push(Op::AntiAlias(AntiAlias::default()));
            }
            IssueKind::Banding { dark, avg_plateau, avg_step } => {
                // Scale radius & threshold gently to the measured plateau width and asset size so small elements are never smeared.
                let max_r = (min_dim * 0.25).clamp(12.0, 36.0);
                let radius = (avg_plateau * 2.5).round().clamp(12.0, max_r);
                let threshold = (avg_step * 1.6 + 1.5).clamp(3.0, 7.0);
                ops.push(Op::Deband {
                    radius,
                    threshold: (threshold * 2.0).round() * 0.5,
                    iterations: if iss.severity > 0.75 { 3 } else { 2 },
                    grain: 0.35,
                    kind: Grain::Gaussian,
                    mono: true,
                    room: false,
                    luma: LumaScale { scaling: if dark { 2.0 } else { 0.0 }, by_average: false },
                    limit: 3.5,
                    elast: 3.0,
                    narrowing: true,
                    prefilter: 0.0,
                    protect: Mask { invert: true, ..Mask::off() },
                    adv: edit::Advanced::default(),
                    target: Target::Both,
                });
            }
        }
    }
    if need_bright > 0.0 || need_dark > 0.0 {
        let mut dh = Dehalo::default();
        dh.brightstr = need_bright;
        dh.darkstr = need_dark;
        ops.push(Op::Dehalo(dh));
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_flat_image_has_no_issues() {
        let mut im = Img::new(64, 64);
        for p in &mut im.px {
            *p = [0.5, 0.5, 0.5, 1.0];
        }
        assert!(analyze_image(Path::new("clean.png"), "clean.png", &im, None).is_none());
    }

    #[test]
    fn detects_banding_in_stepped_gradient() {
        let mut im = Img::new(96, 32);
        for y in 0..32 {
            for x in 0..96 {
                // 6-px wide plateaus stepping by 2/255 in the dark
                let v = ((x / 6) * 2 + 10) as f32 / 255.0;
                im.px[y * 96 + x] = [v, v, v, 1.0];
            }
        }
        let item = analyze_image(Path::new("glow.png"), "glow.png", &im, None).expect("flagged");
        assert!(item.issues.iter().any(|i| matches!(i.kind, IssueKind::Banding { dark: true, .. })));
        assert!(item.suggested_ops.iter().any(|o| matches!(o, Op::Deband { .. })));
    }

    #[test]
    fn detects_bright_haloing_next_to_edge() {
        let mut im = Img::new(64, 48);
        for y in 0..48 {
            for x in 0..64 {
                let v = if x < 30 {
                    0.15
                } else if x == 30 || x == 31 {
                    0.98 // bright overshoot ring
                } else {
                    0.75
                };
                im.px[y * 64 + x] = [v, v, v, 1.0];
            }
        }
        let item = analyze_image(Path::new("halo.png"), "halo.png", &im, None).expect("flagged");
        assert!(item.issues.iter().any(|i| matches!(i.kind, IssueKind::HaloBright)));
        assert!(item.suggested_ops.iter().any(|o| matches!(o, Op::Dehalo(_))));
    }

    #[test]
    fn detects_flat_region_noise() {
        let mut im = Img::new(48, 48);
        for y in 0..48 {
            for x in 0..48 {
                let n = if (x + y) % 2 == 0 { 0.06 } else { -0.06 };
                let v = 0.5 + n;
                im.px[y * 48 + x] = [v, v, v, 1.0];
            }
        }
        let item = analyze_image(Path::new("noisy.png"), "noisy.png", &im, None).expect("flagged");
        assert!(item.issues.iter().any(|i| matches!(i.kind, IssueKind::Noise)));
        assert!(item.suggested_ops.iter().any(|o| matches!(o, Op::Denoise(_))));
    }

    #[test]
    fn downscale_detector_spares_clean_and_flags_ringing_and_fringe() {
        // Build a crisp 64x64 @2x white disc on transparent
        let mut hi = Img::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                let d = (((x as f32 - 31.5).powi(2) + (y as f32 - 31.5).powi(2)).sqrt() - 20.0).clamp(0.0, 2.0);
                let a = (1.0 - d / 2.0).clamp(0.0, 1.0);
                hi.px[y * 64 + x] = [a, a, a, a];
            }
        }
        // Clean area-averaged 1x -> NOT flagged
        let clean_1x = edit::resize(&hi, 32, 32, Resample::Smooth);
        assert!(detect_poor_downscale(&clean_1x, &hi).is_none());

        // Non-premultiplied dark-fringed 1x -> FLAGGED
        let mut fringed = clean_1x.clone();
        for p in &mut fringed.px {
            if p[3] > 0.05 && p[3] < 0.9 {
                p[0] *= 0.4;
                p[1] *= 0.4;
                p[2] *= 0.4;
            }
        }
        assert!(detect_poor_downscale(&fringed, &hi).is_some());

        // Ringed 1x (overshoot outside edge) -> FLAGGED
        let mut ringed = clean_1x.clone();
        for y in 2..30 {
            for x in 2..30 {
                let d = ((x as f32 - 15.5).powi(2) + (y as f32 - 15.5).powi(2)).sqrt();
                if (d - 12.0).abs() < 1.0 {
                    ringed.px[y * 32 + x] = [0.2, 0.2, 0.2, 0.2];
                }
            }
        }
        assert!(detect_poor_downscale(&ringed, &hi).is_some());
    }

    #[test]
    fn thin_ring_and_glowing_cursor_are_not_false_positives() {
        // Thin white ring (like spinner-approachcircle) must not trigger noise or haloing
        let mut ring = Img::new(128, 128);
        for y in 0..128 {
            for x in 0..128 {
                let d = (((x as f32 - 63.5).powi(2) + (y as f32 - 63.5).powi(2)).sqrt() - 50.0).abs();
                let a = (1.0 - (d - 1.5).max(0.0) / 1.5).clamp(0.0, 1.0);
                ring.px[y * 128 + x] = [a, a, a, a];
            }
        }
        let res = analyze_image(Path::new("spinner-approachcircle.png"), "spinner-approachcircle.png", &ring, None);
        assert!(res.is_none() || !res.unwrap().issues.iter().any(|i| matches!(i.kind, IssueKind::Noise | IssueKind::HaloBright | IssueKind::HaloDark)));

        // Hollow glowing cursor (like cursor2@2x.png) must not trigger bright haloing
        let mut cur = Img::new(96, 96);
        for y in 0..96 {
            for x in 0..96 {
                let r = ((x as f32 - 47.5).powi(2) + (y as f32 - 47.5).powi(2)).sqrt();
                let (v, a) = if r < 14.0 {
                    (0.6, 0.85)
                } else if r < 20.0 {
                    (1.0, 1.0)
                } else {
                    let g = (1.0 - (r - 20.0) / 14.0).clamp(0.0, 1.0);
                    (g, g)
                };
                cur.px[y * 96 + x] = [v * a, v * a, v * a, a];
            }
        }
        let res = analyze_image(Path::new("cursor2@2x.png"), "cursor2@2x.png", &cur, None);
        assert!(res.is_none() || !res.unwrap().issues.iter().any(|i| matches!(i.kind, IssueKind::HaloBright | IssueKind::HaloDark)));
    }
}
