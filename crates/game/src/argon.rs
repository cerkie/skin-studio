//! The Argon skin, drawn from the same geometry and colors osu!lazer uses (ArgonMainCirclePiece, ArgonSliderBall,
//! ArgonFollowCircle, ArgonSliderScorePoint, ArgonReverseArrow, ArgonCursor). Pieces are rendered procedurally into
//! white/gray sprites that the scene tints with the combo color, so the shapes stay sharp at any size.
//!
//! All sizes are in lazer's object space, where a hit circle is 128 units across.
use crate::skin::Sheet;

pub const RADIUS: f32 = 64.0;
pub const BORDER: f32 = 128.0 * (2.0 / 58.0);
pub const GRADIENT: f32 = BORDER * 2.5;
pub const OUTER_GRADIENT: f32 = 128.0 - BORDER * 4.0;
pub const INNER_GRADIENT: f32 = OUTER_GRADIENT - GRADIENT * 2.0;
pub const INNER_FILL: f32 = INNER_GRADIENT - GRADIENT * 2.0;
pub const FOLLOW_AREA: f32 = 2.4;

/// Argon's six default combo colors, in the order lazer cycles them.
pub const COMBO: [[f32; 3]; 6] = [
    [241.0, 116.0, 0.0],
    [0.0, 241.0, 53.0],
    [0.0, 82.0, 241.0],
    [241.0, 0.0, 0.0],
    [232.0, 235.0, 0.0],
    [92.0, 0.0, 241.0],
];

/// `color.Darken(x)` as lazer defines it.
pub fn darken(c: [f32; 3], x: f32) -> [f32; 3] {
    [c[0] / (1.0 + x), c[1] / (1.0 + x), c[2] / (1.0 + x)]
}

fn smooth_edge(d: f32) -> f32 {
    // coverage of a 1-sample-wide edge, used only for analytic shapes
    (0.5 - d).clamp(0.0, 1.0)
}

/// Renders a w x h unit shape (centerd on 0,0, y down) at `density` pixels per unit. `shade(x, y)` returns straight
/// RGBA; 4x4 samples per pixel are averaged (premultiplied), so edges are exact area coverage.
fn raster(w: f32, h: f32, density: f32, shade: impl Fn(f32, f32) -> [f32; 4]) -> (usize, usize, Vec<u8>) {
    const SS: usize = 4;
    let (pw, ph) = ((w * density).ceil() as usize, (h * density).ceil() as usize);
    // the image covers exactly pw/density units; keep it centerd
    let (uw, uh) = (pw as f32 / density, ph as f32 / density);
    let mut out = Vec::with_capacity(pw * ph * 4);
    for py in 0..ph {
        for px in 0..pw {
            let mut acc = [0.0f32; 4];
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (px as f32 + (sx as f32 + 0.5) / SS as f32) / density - uw * 0.5;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) / density - uh * 0.5;
                    let c = shade(x, y);
                    acc[0] += c[0] * c[3];
                    acc[1] += c[1] * c[3];
                    acc[2] += c[2] * c[3];
                    acc[3] += c[3];
                }
            }
            let n = (SS * SS) as f32;
            out.extend_from_slice(&[(acc[0] / n * 255.0 + 0.5) as u8, (acc[1] / n * 255.0 + 0.5) as u8, (acc[2] / n * 255.0 + 0.5) as u8, (acc[3] / n * 255.0 + 0.5) as u8]);
        }
    }
    (pw, ph, out)
}

fn seg_dist(x: f32, y: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((x - a.0) * dx + (y - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    ((x - a.0 - t * dx).powi(2) + (y - a.1 - t * dy).powi(2)).sqrt()
}

/// Stroked chevron pointing right, `n` of them spaced `gap` apart, centerd.
fn chevron(x: f32, y: f32, half_w: f32, half_h: f32, stroke: f32, n: usize, gap: f32) -> bool {
    for i in 0..n {
        let ox = (i as f32 - (n as f32 - 1.0) * 0.5) * gap;
        let (a, m, b) = ((ox - half_w, -half_h), (ox + half_w, 0.0), (ox - half_w, half_h));
        if seg_dist(x, y, a, m) <= stroke * 0.5 || seg_dist(x, y, m, b) <= stroke * 0.5 {
            return true;
        }
    }
    false
}

fn font_bytes() -> Option<Vec<u8>> {
    // Inter SemiBold, embedded: lazer uses Torus and Inter, and this keeps the numbers identical on every system
    Some(include_bytes!("Inter-SemiBold.otf").to_vec())
}

/// Rounded bold digits in the spirit of Torus (which cannot be redistributed): M PLUS Rounded 1c Bold, OFL, digits only.
fn digit_font_bytes() -> Vec<u8> {
    include_bytes!("Digits-Bold.ttf").to_vec()
}

/// An approach circle: a plain ring 128 units across and 5 thick, as lazer's default one looks.
pub fn add_approach_ring(sheet: &mut Sheet, key: &str, density: f32) {
    let (pw, ph, px) = raster(128.0, 128.0, density, |x, y| {
        let r = (x * x + y * y).sqrt();
        [1.0, 1.0, 1.0, if r <= 64.0 && r >= 59.0 { 1.0 } else { 0.0 }]
    });
    sheet.add_image(key, pw as u32, ph as u32, &px, density);
}

/// Adds every Argon piece to `sheet`. `scale` is the object scale, `ss` the view-to-pixel factor.
pub fn add_to_sheet(sheet: &mut Sheet, scale: f32, ss: f32) {
    let d = scale * ss * 2.0; // pixels per object unit: twice what the screen shows, for headroom when pieces grow
    let white = |a: f32| [1.0, 1.0, 1.0, a];
    let gray = |b: f32, a: f32| [b, b, b, a];
    let put = |sheet: &mut Sheet, key: &str, w: f32, h: f32, density: f32, f: &dyn Fn(f32, f32) -> [f32; 4]| {
        let (pw, ph, px) = raster(w, h, density, f);
        sheet.add_image(key, pw as u32, ph as u32, &px, density);
    };
    let disc = |r: f32, x: f32, y: f32| smooth_edge(((x * x + y * y).sqrt() - r) * 1.0);
    let _ = disc;
    let inside = |r: f32, x: f32, y: f32| ((x * x + y * y).sqrt() <= r) as u8 as f32;

    // brightness gradients are vertical, top to bottom, tinted by the combo color afterwards
    let grad = |h: f32, y: f32, top: f32, bottom: f32| top + (bottom - top) * ((y + h * 0.5) / h).clamp(0.0, 1.0);

    put(sheet, "ar_outerfill", 128.0, 128.0, d, &|x, y| gray(1.0, inside(63.5, x, y)));
    put(sheet, "ar_outergrad", OUTER_GRADIENT, OUTER_GRADIENT, d, &|x, y| gray(grad(OUTER_GRADIENT, y, 1.0, 1.0 / 1.1), inside(OUTER_GRADIENT * 0.5, x, y)));
    put(sheet, "ar_innergrad", INNER_GRADIENT, INNER_GRADIENT, d, &|x, y| gray(grad(INNER_GRADIENT, y, 1.0 / 1.5, 1.0 / 1.6), inside(INNER_GRADIENT * 0.5, x, y)));
    put(sheet, "ar_innerfill", INNER_FILL, INNER_FILL, d, &|x, y| gray(1.0, inside(INNER_FILL * 0.5, x, y)));
    let ring = |r_out: f32, thick: f32, x: f32, y: f32| {
        let r = (x * x + y * y).sqrt();
        (r <= r_out && r >= r_out - thick) as u8 as f32
    };
    put(sheet, "ar_border", 128.0, 128.0, d, &|x, y| white(ring(64.0, BORDER, x, y)));
    // after a hit the border turns into an accent gradient fading from 50% to 0% down the circle
    put(sheet, "ar_border_fade", 128.0, 128.0, d, &|x, y| gray(1.0, ring(64.0, BORDER, x, y) * 0.5 * (1.0 - ((y + 64.0) / 128.0).clamp(0.0, 1.0))));
    // hit lighting: a glow of lazer's edge-effect style around a 64-unit disc
    put(sheet, "ar_flash", 220.0, 220.0, d * 0.5, &|x, y| {
        let r = (x * x + y * y).sqrt();
        let a = if r < 32.0 { 1.0 } else { (1.0 - ((r - 32.0) / (RADIUS * 1.2)).clamp(0.0, 1.0)).powi(2) };
        gray(1.0, a)
    });
    add_approach_ring(sheet, "ar_approach", d);

    // slider ball: gradient fill, white inner border, chevron icon
    put(sheet, "ar_ball_fill", OUTER_GRADIENT, OUTER_GRADIENT, d, &|x, y| gray(grad(OUTER_GRADIENT, y, 1.0, 1.0 / 1.5), inside(OUTER_GRADIENT * 0.5, x, y)));
    put(sheet, "ar_ball_border", OUTER_GRADIENT, OUTER_GRADIENT, d, &|x, y| white(ring(OUTER_GRADIENT * 0.5, GRADIENT, x, y)));
    put(sheet, "ar_ball_icon", 30.0, 40.0, d, &|x, y| white(chevron(x, y, 5.5, 13.0, 5.5, 1, 0.0) as u8 as f32));
    // follow circle: accent ring with a translucent gradient fill, drawn additively
    put(sheet, "ar_follow", 128.0, 128.0, d, &|x, y| {
        let r = (x * x + y * y).sqrt();
        let b = grad(128.0, y, 1.0, 1.0 / 1.5);
        let a = if r > 64.0 { 0.0 } else if r >= 60.0 { 1.0 } else { 0.3 };
        gray(b, a)
    });
    // slider tick: hollow ring
    put(sheet, "ar_tick", 12.0, 12.0, d * 2.0, &|x, y| white(ring(6.0, 3.0, x, y)));
    // reverse arrow: white pill with a dark double chevron (tinted to the combo color darkened by 4)
    put(sheet, "ar_rev_pill", 40.0, 20.0, d, &|x, y| {
        let (hw, hh) = (20.0, 10.0);
        let cx = (x.abs() - (hw - hh)).max(0.0);
        white(((cx * cx + y * y).sqrt() <= hh) as u8 as f32)
    });
    put(sheet, "ar_rev_icon", 16.0, 16.0, d * 2.0, &|x, y| white(chevron(x, y, 2.5, 4.5, 2.2, 2, 4.2) as u8 as f32));
    // cursor: pink gradient ring over a darker pink fill, a white inner edge and a glowing dot
    put(sheet, "ar_cursor", 80.0, 80.0, ss * 2.0, &|x, y| {
        let r = (x * x + y * y).sqrt();
        let t = ((y + 14.0) / 28.0).clamp(0.0, 1.0);
        let top = [0.988, 0.38, 0.561];
        let bot = [0.733, 0.102, 0.255];
        let ring = [top[0] + (bot[0] - top[0]) * t, top[1] + (bot[1] - top[1]) * t, top[2] + (bot[2] - top[2]) * t];
        // glow around the center dot, radius 20 beyond a 5.6-unit dot
        let glow = if r > 2.8 { (1.0 - ((r - 2.8) / 20.0).clamp(0.0, 1.0)).powi(2) * 100.0 / 255.0 } else { 0.0 };
        let mut c = [0.67, 1.0, 1.0, glow];
        if r <= 14.0 {
            let fill = [top[0] * 0.4, top[1] * 0.4, top[2] * 0.4];
            c = if r >= 8.0 { [ring[0], ring[1], ring[2], 1.0] } else { [fill[0], fill[1], fill[2], 0.4] };
            if r >= 12.0 {
                let w = 0.8;
                c = [c[0] * (1.0 - w) + w, c[1] * (1.0 - w) + w, c[2] * (1.0 - w) + w, 1.0];
            }
        }
        if r <= 2.8 {
            c = [1.0, 1.0, 1.0, 1.0];
        }
        c
    });


    // ---- the rest of the Argon set: judgement rings, cursor trail, repeat edge, spinner ----
    put(sheet, "ar_jring_s", 9.0, 9.0, d * 2.0, &|x, y| white(ring(4.5, 4.0f32.min(4.5), x, y)));
    put(sheet, "ar_jring_l", 14.0, 14.0, d * 2.0, &|x, y| white(ring(7.0, 4.0, x, y)));
    // lazer's own texture: the edge piece of a repeat arrow (shown 110 units across)
    // lazer's `Cursor/cursortrail`: a 10 px soft white dot. Textures from its resource store are shown at half size
    // (ScaleAdjust 2), so it is 5 units across before the trail's own 0.4 scale. Drawn here as the same soft round falloff.
    put(sheet, "ar_trail", 5.0, 5.0, ss * 4.0, &|x, y| {
        let r = (x * x + y * y).sqrt() * 2.0; // in texture pixels
        let a = if r < 4.6 { 0.65 * (1.0 - (r / 4.6) * (r / 4.6)).powi(2) } else { 0.0 };
        white(a)
    });
    // the edge piece of a repeat arrow: a left-facing arc with a soft glow, drawn from scratch to match lazer's look
    // (a 200-unit design: radius 100, arc 19 thick from 90 to 173 degrees either side of the left, glow fading to the right)
    let es = OUTER_GRADIENT / 200.0;
    put(sheet, "ar_edge", OUTER_GRADIENT, OUTER_GRADIENT, d, &|x, y| {
        let (px, py) = (x / es, y / es);
        let r = (px * px + py * py).sqrt();
        if r > 100.0 {
            return [0.0; 4];
        }
        let ang = py.atan2(px).abs().to_degrees(); // 180 = straight left
        let (a0, a1, mid, half) = (90.0f32, 173.3f32, 90.4f32, 9.6f32);
        let cap = |deg: f32| {
            let t = deg.to_radians();
            let (cx, cy) = (mid * t.cos(), mid * t.sin() * py.signum());
            ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() <= half
        };
        let body = r >= 80.8 && ang >= a0 && ang <= a1;
        if body || cap(a0) || cap(a1) {
            return white(1.0);
        }
        white((0.498 * (1.0 - (px + 100.0) / 128.0)).clamp(0.0, 0.498))
    });
    let pink = [0.988f32, 0.38, 0.561];
    // spinner: soft pink fill (lazer draws it as an edge-effect shadow of radius 40 around a 368-unit disc)
    put(sheet, "ar_sp_fill", 460.0, 460.0, ss * 1.5, &|x, y| {
        let r = (x * x + y * y).sqrt();
        let a = if r <= 184.0 { 1.0 } else { (1.0 - ((r - 184.0) / 40.0).clamp(0.0, 1.0)).powi(2) };
        [pink[0], pink[1], pink[2], a]
    });
    // 25 ticks: hollow pills of 30x5 with a 2-unit white border
    put(sheet, "ar_sp_tick", 30.0, 5.0, ss * 3.0, &|x, y| {
        let pill = |hw: f32, hh: f32| {
            let cx = (x.abs() - (hw - hh)).max(0.0);
            (cx * cx + y * y).sqrt() <= hh
        };
        white((pill(15.0, 2.5) && !pill(13.0, 0.5)) as u8 as f32)
    });
    // ring arcs, top half only (the bottom one is the same sprite turned around): 0.31 of a circle, or half when complete
    for (key, half, thick) in [("ar_sp_ring0", 0.31 * std::f32::consts::PI, 368.0 * 0.02), ("ar_sp_ring1", 0.5 * std::f32::consts::PI, 368.0 * 0.02 * 2.2)] {
        put(sheet, key, 380.0, 196.0, ss * 1.5, &move |x, y| {
            let (px, py) = (x, y - 92.0); // sprite center is 92 units above the disc center
            let r = (px * px + py * py).sqrt();
            let ang = px.atan2(-py).abs(); // from straight up
            let body = r <= 184.0 && r >= 184.0 - thick && ang <= half;
            // round caps
            let cap = |s: f32| {
                let a = s * half;
                let (cx, cy) = (a.sin() * (184.0 - thick * 0.5), -a.cos() * (184.0 - thick * 0.5));
                ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() <= thick * 0.5
            };
            white((body || cap(1.0) || cap(-1.0)) as u8 as f32)
        });
    }
    // side bars: arcs of 0.15 of a circle on the right (the left one is the same sprite turned around). One sprite for the
    // dim background and 20 for the fill as it grows.
    let bar = |frac: f32, px: f32, py: f32| -> bool {
        let (px, py) = (px + 158.0, py); // sprite center is 158 units right of the disc center
        let r = (px * px + py * py).sqrt();
        let half = 0.15 * std::f32::consts::PI * frac;
        let ang = py.atan2(px).abs();
        let thick = 384.0 * 0.12;
        let mid = 192.0 - thick * 0.5;
        let body = r <= 192.0 && r >= 192.0 - thick && ang <= half;
        let cap = |s: f32| ((px - mid * s.cos()).powi(2) + (py - mid * s.sin()).powi(2)).sqrt() <= thick * 0.5;
        body || cap(half) || cap(-half)
    };
    put(sheet, "ar_sp_bar_bg", 80.0, 200.0, ss * 1.5, &|x, y| white(bar(1.0, x, y) as u8 as f32 * 0.25));
    for i in 0..20 {
        let f = (i + 1) as f32 / 20.0;
        put(sheet, &format!("ar_sp_bar_{i}"), 80.0, 200.0, ss * 1.5, &move |x, y| white(bar(f, x, y) as u8 as f32));
    }
    // the default follow point: two chevrons, the first dark, under a pink gradient, drawn additively
    put(sheet, "ar_followpoint", 16.0, 10.0, ss * 6.0, &|x, y| {
        let t = ((y + 5.0) / 10.0).clamp(0.0, 1.0);
        let top = [0.988f32, 0.38, 0.561];
        let bot = [0.733f32, 0.102, 0.255];
        let g = [top[0] + (bot[0] - top[0]) * t, top[1] + (bot[1] - top[1]) * t, top[2] + (bot[2] - top[2]) * t];
        if chevron(x - 2.0, y, 2.2, 3.4, 2.2, 1, 0.0) {
            return [g[0], g[1], g[2], 1.0];
        }
        if chevron(x + 2.0, y, 2.2, 3.4, 2.2, 1, 0.0) {
            return [g[0] * 0.2, g[1] * 0.2, g[2] * 0.2, 1.0];
        }
        [0.0; 4]
    });
    put(sheet, "ar_ring_thin", 80.0, 80.0, ss * 3.0, &|x, y| white(ring(40.0, 3.0, x, y)));
    put(sheet, "ar_ring_thick", 64.0, 64.0, ss * 3.0, &|x, y| white(ring(32.0, 10.0, x, y)));

    // numbers: one cell per digit, sized like lazer's 52-unit bold Torus digits (measured against the clip: a digit is about 0.23 of a circle tall)
    if let Ok(font) = fontdue::Font::from_bytes(digit_font_bytes(), fontdue::FontSettings::default()) {
        let probe = 100.0;
        let h0 = font.rasterize('0', probe).0.height.max(1) as f32;
        let size = 52.0 * d * (0.475 * probe / h0);
        let lm = font.horizontal_line_metrics(size);
        for dgt in 0..10u32 {
            let ch = char::from_digit(dgt, 10).unwrap();
            let (m, bmp) = font.rasterize(ch, size);
            let (ascent, line) = lm.map(|l| (l.ascent, l.ascent - l.descent)).unwrap_or((size * 0.8, size * 1.2));
            let adv = font.metrics(ch, size).advance_width;
            let (cw, ch_) = (adv.ceil() as usize, line.ceil() as usize);
            let mut px = vec![0u8; cw * ch_ * 4];
            let gx = ((cw as f32 - m.width as f32) * 0.5).round() as i32;
            let gy = (ascent - m.height as f32 - m.ymin as f32).round() as i32;
            for y in 0..m.height {
                for x in 0..m.width {
                    let (tx, ty) = (x as i32 + gx, y as i32 + gy);
                    if tx < 0 || ty < 0 || tx >= cw as i32 || ty >= ch_ as i32 {
                        continue;
                    }
                    let a = bmp[y * m.width + x];
                    let o = (ty as usize * cw + tx as usize) * 4;
                    px[o..o + 4].copy_from_slice(&[a, a, a, a]);
                }
            }
            sheet.add_image(&format!("num{dgt}"), cw as u32, ch_ as u32, &px, d);
        }
    }
    if let Some(bytes) = font_bytes() {
        if let Ok(font) = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()) {
            // spinner labels
            for (key, text, px_size) in [("ar_sp_label", "SPINS PER MINUTE", 16.0f32), ("ar_sp_max", "MAX", 28.0), ("ar_great", "GREAT", 20.0)] {
                let size = px_size * d;
                let spacing = if key == "ar_great" { 5.0 * d } else { 1.0 * d };
                let lm = font.horizontal_line_metrics(size);
                let (ascent, line) = lm.map(|l| (l.ascent, l.ascent - l.descent)).unwrap_or((size * 0.8, size * 1.2));
                let glyphs: Vec<(fontdue::Metrics, Vec<u8>)> = text.chars().map(|c| font.rasterize(c, size)).collect();
                let total: f32 = glyphs.iter().map(|(m, _)| m.advance_width + spacing).sum();
                let (cw, ch_) = (total.ceil() as usize, line.ceil() as usize);
                let mut px = vec![0u8; cw * ch_ * 4];
                let mut pen = 0.0f32;
                for (m, bmp) in &glyphs {
                    let gx = (pen + m.xmin as f32).round() as i32;
                    let gy = (ascent - m.height as f32 - m.ymin as f32).round() as i32;
                    for y in 0..m.height {
                        for x in 0..m.width {
                            let (tx, ty) = (x as i32 + gx, y as i32 + gy);
                            if tx < 0 || ty < 0 || tx >= cw as i32 || ty >= ch_ as i32 {
                                continue;
                            }
                            let a = bmp[y * m.width + x];
                            let o = (ty as usize * cw + tx as usize) * 4;
                            let v = px[o + 3].max(a);
                            px[o..o + 4].copy_from_slice(&[v, v, v, v]);
                        }
                    }
                    pen += m.advance_width + spacing;
                }
                sheet.add_image(key, cw as u32, ch_ as u32, &px, d);
            }
        }
    }
}
