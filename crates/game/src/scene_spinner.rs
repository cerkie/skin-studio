//! Spinner drawing: legacy old style (background/circle/metre), legacy new style (top/bottom/middle) and Argon,
//! after lazer's LegacyOldStyleSpinner, LegacyNewStyleSpinner, LegacySpinner and ArgonSpinner(Disc).
use crate::beatmap::HitObject;
use crate::path::P;
use crate::scene::Frame;
use crate::scene::Scene;
use crate::spinner::SpinnerInfo;

const K: f32 = 0.625; // legacy SPRITE_SCALE
const TOP: f32 = 45.0 - 16.0; // SPINNER_TOP_OFFSET

fn ease_out_quad(t: f32) -> f32 {
    1.0 - (1.0 - t) * (1.0 - t)
}
fn ease_out_quint(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(5)
}
fn frac(t: f32, a: f32, b: f32) -> f32 {
    ((t - a) / (b - a).max(1e-6)).clamp(0.0, 1.0)
}

/// Cheap deterministic noise for the old-style metre blink, steady within ~16 ms.
fn blink(t: f32, p: f32) -> bool {
    let mut x = ((t / 16.0) as u64).wrapping_mul(0x9e3779b97f4a7c15);
    x ^= x >> 29;
    x = x.wrapping_mul(0xbf58476d1ce4e5b9);
    ((x >> 40) as f32 / (1u64 << 24) as f32) < p
}

impl Scene {
    /// Window space (640x480, as stable lays spinners out) to playfield coordinates.
    fn win(&self, wx: f32, wy: f32) -> P {
        [wx - 64.0, wy - 56.0]
    }

    pub(crate) fn spinner_info(&self, o: &HitObject) -> SpinnerInfo {
        SpinnerInfo::new(self.map.od, o.time, o.end_time)
    }

    /// Score-font text (legacy `score-N` digits): `align` 0 center, 1 right.
    fn score_text(&self, out: &mut Frame, text: &str, pos: P, scale: f32, alpha: f32, align: u8) {
        let overlap = self.skin.ini.get("fonts", "scoreoverlap").and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
        let width = |c: char| self.sheet.get(&format!("sc{c}")).map(|e| e.frames[0].w as f32 / e.ratio).unwrap_or(0.0);
        let total: f32 = text.chars().map(|c| width(c) - overlap).sum::<f32>() + overlap;
        let mut x = if align == 1 { -total * scale } else { -total * 0.5 * scale };
        for c in text.chars() {
            let w = width(c);
            if w > 0.0 {
                self.sprite(out, &format!("sc{c}"), 0, [pos[0] + x + w * 0.5 * scale, pos[1]], scale, 0.0, alpha, None);
            }
            x += (w - overlap) * scale;
        }
    }

    pub(crate) fn spinner(&self, out: &mut Frame, o: &HitObject, t: f32) {
        let sp = self.spinner_info(o);
        if t < o.time - self.preempt || t > o.end_time + 400.0 {
            return;
        }
        if self.argon {
            self.argon_spinner(out, &sp, t);
            return;
        }
        let has = |n: &str| self.sheet.get(n).is_some();
        // whole spinner fades in over its fade-in time, out for 160 ms after it ends
        let mut a = frac(t, o.time - self.fade_in, o.time);
        if t > o.end_time {
            a *= 1.0 - frac(t, o.end_time, o.end_time + 160.0);
        }
        if a <= 0.0 {
            return;
        }
        let rot = sp.rotation(t).to_radians();
        let progress = sp.progress(t);
        let center = self.win(320.0, TOP + 219.0);
        let approach_k = K * (1.86 - (1.86 - 0.1) * frac(t, o.time, o.end_time));
        if has("spinner-background") {
            // old style
            let bg = self.skin.ini.color("spinnerbackground").map(|c| [c[0] / 255.0, c[1] / 255.0, c[2] / 255.0]).unwrap_or([100.0 / 255.0; 3]);
            self.sprite(out, "spinner-background", 0, center, K, 0.0, a, Some(bg));
            self.sprite(out, "spinner-circle", 0, center, K, rot, a, None);
            if let Some(e) = self.sheet.get("spinner-metre") {
                // bars light up from the bottom, with a flicker on the next bar like the original
                let p = (progress * 100.0).min(99.0);
                let mut bars = (p as i32 / 10) as f32;
                if blink(t, (p % 10.0) / 10.0) {
                    bars += 1.0;
                }
                let f = (bars / 10.0).clamp(0.0, 1.0);
                if f > 0.0 {
                    let r = e.frames[0];
                    let (full_w, full_h) = (r.w as f32 / e.ratio * K, r.h as f32 / e.ratio * K);
                    let vis_h = full_h * f;
                    let top_left = self.win(0.0, TOP);
                    let rect = [r.x as f32, r.y as f32 + r.h as f32 * (1.0 - f), (r.x + r.w) as f32, (r.y + r.h) as f32];
                    out.sprites.push(crate::scene::SpriteDraw {
                        pos: [top_left[0] + full_w * 0.5 + crate::scene::OFF_X, top_left[1] + full_h - vis_h * 0.5 + crate::scene::OFF_Y],
                        half: [full_w * 0.5, vis_h * 0.5],
                        rot: 0.0,
                        alpha: a,
                        rect,
                        tint: None,
                        additive: false,
                    });
                }
            }
            if has("spinner-approachcircle") && t >= o.time - self.fade_in {
                self.sprite(out, "spinner-approachcircle", 0, center, approach_k, 0.0, a, None);
            }
        } else if has("spinner-top") {
            // new style
            let kk = K * (0.8 + ease_out_quad(progress) * 0.2);
            let glow_a = progress * if t > o.end_time { 1.0 - frac(t, o.end_time, o.end_time + 300.0) } else { 1.0 };
            if has("spinner-glow") {
                self.sprite_add(out, "spinner-glow", center, kk, 0.0, glow_a * a, Some([3.0 / 255.0, 151.0 / 255.0, 1.0]));
            }
            let (top_rot, mid_rot) = if has("spinner-middle2") { (rot * 0.5, rot) } else { (rot, rot) };
            self.sprite(out, "spinner-bottom", 0, center, kk, top_rot / 3.0, a, None);
            self.sprite(out, "spinner-top", 0, center, kk, top_rot, a, None);
            if has("spinner-middle2") {
                self.sprite(out, "spinner-middle2", 0, center, kk, mid_rot, a, None);
            }
            // the fixed middle goes from white to red over the spinner's duration
            let red = frac(t, o.time, o.end_time);
            self.sprite(out, "spinner-middle", 0, center, kk, 0.0, a, Some([1.0, 1.0 - red, 1.0 - red]));
            if has("spinner-approachcircle") && t >= o.time - self.fade_in {
                self.sprite(out, "spinner-approachcircle", 0, center, approach_k, 0.0, a, None);
            }
        } else {
            return;
        }

        // shared overlay: spin prompt, clear, rpm meter, bonus counter
        let completed = sp.completed_at();
        let tick0 = sp.tick_time(0);
        if has("spinner-spin") {
            let mut sa = frac(t, o.time - self.fade_in * 0.5, o.time);
            let fade = 400.0f32.min(sp.duration());
            sa *= 1.0 - frac(t, o.end_time - fade, o.end_time);
            sa *= 1.0 - frac(t, tick0, tick0 + 300.0);
            self.sprite(out, "spinner-spin", 0, self.win(320.0, TOP + 335.0), K, 0.0, sa * a, None);
        }
        if has("spinner-clear") && t >= completed {
            let start = completed;
            let ca = ease_out_quad(frac(t, start, start + 400.0)) * (1.0 - frac(t, o.end_time - 50.0, o.end_time));
            let dt = t - start;
            let s = if dt < 240.0 { 2.0 - 1.2 * ease_out_quad(dt / 240.0) } else { 0.8 + 0.2 * frac(dt, 240.0, 400.0) };
            self.sprite(out, "spinner-clear", 0, self.win(320.0, TOP + 115.0), K * s, 0.0, ca, None);
        }
        if let Some(e) = self.sheet.get("spinner-rpm") {
            let slide = 50.0 * (1.0 - ease_out_quad(frac(t, o.time - self.fade_in, o.time)));
            let r = e.frames[0];
            let (w, h) = (r.w as f32 / e.ratio * K, r.h as f32 / e.ratio * K);
            let tl = self.win(320.0 - 87.0, 445.0 + slide);
            self.sprite(out, "spinner-rpm", 0, [tl[0] + w * 0.5, tl[1] + h * 0.5], K, 0.0, a, None);
            let rpm = format!("{}", sp.rpm(t) as i32);
            let rp = self.win(320.0 + 80.0, 448.0 + slide + 10.0);
            self.score_text(out, &rpm, rp, K * 0.9, a, 1);
        }
        // bonus counter
        let (n, score, max) = sp.bonus_at(t);
        if n > 0 {
            let tb = (0..sp.total_ticks()).filter(|&i| sp.is_bonus(i) && sp.tick_time(i) <= t).map(|i| sp.tick_time(i)).fold(f32::MIN, f32::max);
            let dt = t - tb;
            let (s, al) = if max { (K * (1.4 + 0.4 * ease_out_quad(frac(dt, 0.0, 1000.0))), 1.0 - ease_out_quad(frac(dt, 0.0, 500.0))) } else { (K * (2.0 - 0.72 * ease_out_quad(frac(dt, 0.0, 800.0))), 1.0 - ease_out_quad(frac(dt, 0.0, 800.0))) };
            if al > 0.0 {
                self.score_text(out, &score.to_string(), self.win(320.0, TOP + 299.0), s, al, 0);
            }
        }
    }
}
