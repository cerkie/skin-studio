//! More of the Argon skin: the spinner (ArgonSpinner / ArgonSpinnerDisc), judgements with ring explosions
//! (ArgonJudgementPiece; Argon Pro hides Great) and the cursor trail (ArgonCursorTrail).
use crate::path::P;
use crate::scene::Frame;
use crate::scene::Scene;
use crate::spinner::SpinnerInfo;

const PINK: [f32; 3] = [0.988, 0.38, 0.561];

fn ease_out_quad(t: f32) -> f32 {
    1.0 - (1.0 - t) * (1.0 - t)
}
fn ease_out_quint(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(5)
}
fn frac(t: f32, a: f32, b: f32) -> f32 {
    ((t - a) / (b - a).max(1e-6)).clamp(0.0, 1.0)
}

impl Scene {
    /// Digits of lazer's bold text at `size` units tall, centerd on `pos`.
    fn argon_text(&self, out: &mut Frame, text: &str, pos: P, size: f32, alpha: f32, tint: Option<[f32; 3]>) {
        let k = size / 52.0;
        let width = |c: char| self.sheet.get(&format!("num{c}")).map(|e| e.frames[0].w as f32 / e.ratio).unwrap_or(0.0);
        let total: f32 = text.chars().map(width).sum();
        let mut x = -total * 0.5 * k;
        for c in text.chars() {
            let w = width(c);
            if w > 0.0 {
                self.sprite(out, &format!("num{c}"), 0, [pos[0] + x + w * 0.5 * k, pos[1]], k, 0.0, alpha, tint);
            }
            x += w * k;
        }
    }

    pub(crate) fn argon_spinner(&self, out: &mut Frame, sp: &SpinnerInfo, t: f32) {
        let p = self.preempt;
        let tp = sp.start - p;
        let mut a = frac(t, tp, tp + self.fade_in * 0.5);
        if t > sp.end {
            a *= 1.0 - frac(t, sp.end, sp.end + 160.0);
        }
        if a <= 0.0 {
            return;
        }
        let c: P = [256.0, 192.0];
        // the disc and its center grow in two stages, ending half a pre-empt after the start
        let mut disc = if t < tp + 0.5 * p {
            0.0
        } else if t < sp.start {
            0.2 * ease_out_quint(frac(t, tp + 0.5 * p, tp + 0.75 * p))
        } else {
            0.2 + 0.8 * ease_out_quint(frac(t, sp.start, sp.start + 0.5 * p))
        };
        let mut center = if t < tp + 0.5 * p { 0.0 } else if t < sp.start { 0.3 * ease_out_quint(frac(t, tp + 0.5 * p, tp + 0.75 * p)) } else { 0.3 + 0.5 * ease_out_quint(frac(t, sp.start, sp.start + 0.5 * p)) };
        let hit_t = t - sp.end;
        let mut ring_turn = 0.0;
        if hit_t > 0.0 {
            disc *= 1.0 + 0.2 * ease_out_quad(frac(hit_t, 0.0, 320.0));
            center = center.max(0.0);
            ring_turn = 180.0 * frac(hit_t, 0.0, 320.0);
        }
        let progress = sp.progress(t);
        let completed = t >= sp.completed_at();
        let tracking = if t < sp.start { 0.0 } else { 1.0 - 0.985f32.powf(t - sp.start) };
        let fill_a = if completed { 0.4 } else { 0.2 + tracking * 0.2 };
        let fill_k = 0.1 + 0.88 * progress;

        // fill, ticks
        self.sprite(out, "ar_sp_fill", 0, c, disc * fill_k, 0.0, a * fill_a, Some(PINK));
        let spin = sp.rotation(t) + 25.0 * sp.duration() / 2000.0 * frac(t, tp + 0.5 * p, sp.end) + ring_turn;
        let rr = spin.to_radians();
        let n = 25;
        for i in 0..n {
            let th = i as f32 / n as f32 * std::f32::consts::TAU;
            let (x, y) = (th.sin() * 138.0 * disc, th.cos() * 138.0 * disc);
            let (px, py) = (x * rr.cos() - y * rr.sin(), x * rr.sin() + y * rr.cos());
            let rot = (-(i as f32) / n as f32 * 360.0 - 120.0).to_radians() + rr;
            self.sprite(out, "ar_sp_tick", 0, [c[0] + px, c[1] + py], disc, rot, a, None);
        }
        // ring arcs (top and bottom)
        let ring = if completed { "ar_sp_ring1" } else { "ar_sp_ring0" };
        self.sprite(out, ring, 0, [c[0], c[1] - 92.0 * disc], disc, 0.0, a, None);
        self.sprite(out, ring, 0, [c[0], c[1] + 92.0 * disc], disc, std::f32::consts::PI, a, None);
        // side bars
        if progress < 1.0 {
            for (dx, rot) in [(158.0, 0.0), (-158.0, std::f32::consts::PI)] {
                self.sprite(out, "ar_sp_bar_bg", 0, [c[0] + dx * disc, c[1]], disc, rot, a, None);
                if progress > 0.0 {
                    let level = ((progress * 20.0).ceil() as usize).clamp(1, 20) - 1;
                    self.sprite(out, &format!("ar_sp_bar_{level}"), 0, [c[0] + dx * disc, c[1]], disc, rot, a, None);
                }
            }
        }
        // center: two rings that shrink while the spinner is being tracked
        let size = 80.0 + (40.0 - 80.0) * tracking;
        self.sprite(out, "ar_ring_thin", 0, c, size / 80.0 * center, 0.0, a, None);
        self.sprite(out, "ar_ring_thick", 0, c, 0.8 * size / 64.0 * center * (80.0 / 80.0), 0.0, a, None);

        // spins per minute
        let spm_a = frac(t, sp.start, sp.start + self.fade_in) * a;
        if spm_a > 0.0 && t <= sp.end {
            self.argon_text(out, &format!("{}", sp.rpm(t) as i32), [c[0], c[1] + 60.0 + 14.0], 28.0, spm_a, None);
            self.sprite(out, "ar_sp_label", 0, [c[0], c[1] + 60.0 + 30.0 + 8.0], 1.0, 0.0, spm_a, None);
        }
        // bonus
        let (nb, score, max) = sp.bonus_at(t);
        if nb > 0 {
            let tb = (0..sp.total_ticks()).filter(|&i| sp.is_bonus(i) && sp.tick_time(i) <= t).map(|i| sp.tick_time(i)).fold(f32::MIN, f32::max);
            let dt = t - tb;
            let pos = [c[0], c[1] - 100.0];
            if max {
                let flash = 1.0 - frac(dt, 0.0, 400.0);
                let tint = [1.0 - (1.0 - PINK[0]) * flash * 0.0, 1.0 - (1.0 - PINK[1]) * flash, 1.0 - (1.0 - PINK[2]) * flash];
                self.sprite(out, "ar_sp_max", 0, pos, 1.5 + 1.3 * ease_out_quint(frac(dt, 0.0, 1000.0)), 0.0, (1.0 - frac(dt, 0.0, 500.0)) * a, Some(tint));
            } else {
                let k = 1.5 - 0.5 * ease_out_quint(frac(dt, 0.0, 1000.0));
                self.argon_text(out, &score.to_string(), pos, 28.0 * k, (1.0 - frac(dt, 0.0, 1500.0)) * a, None);
            }
        }
    }

    /// "GREAT" with a burst of rings, as ArgonJudgementPiece. Argon Pro shows nothing for a Great.
    pub(crate) fn argon_judgement(&self, out: &mut Frame, pos: P, t: f32, at: f32, seed: u32) {
        if matches!(self.skin.style, crate::skin::Style::Argon { pro: true }) {
            return;
        }
        let dt = t - at;
        if !(0.0..=1000.0).contains(&dt) {
            return;
        }
        let blue = [0.6, 0.933, 1.0]; // BlueLight, the color of a Great
        let k = self.scale;
        let fade = 1.0 - frac(dt, 0.0, 800.0);
        let text_a = ease_out_quint(frac(dt, 0.0, 300.0));
        let text_k = 1.0 + 0.2 * ease_out_quint(frac(dt, 0.0, 1800.0));
        self.sprite_add(out, "ar_great", pos, k * text_k, 0.0, text_a * fade, Some(blue));
        // eight rings fly outwards: four small, four large
        let ring_fade = 1.0 - ease_out_quint(frac(dt, 0.0, 1000.0));
        for i in 0..8u32 {
            let mut h = (seed.wrapping_mul(2654435761)).wrapping_add(i.wrapping_mul(40503)).wrapping_mul(2246822519);
            h ^= h >> 15;
            let dir = (h & 0xffff) as f32 / 65535.0 * std::f32::consts::TAU;
            h = h.wrapping_mul(3266489917);
            let dist = 26.0 + (h & 0xffff) as f32 / 65535.0 * 26.0;
            let travel = ease_out_quint(frac(dt, 0.0, 600.0));
            let r = dist * (0.3 + 0.7 * travel);
            let name = if i < 4 { "ar_jring_s" } else { "ar_jring_l" };
            self.sprite_add(out, name, [pos[0] + dir.cos() * r * k, pos[1] + dir.sin() * r * k], k, 0.0, ring_fade, Some(blue));
        }
    }
}
