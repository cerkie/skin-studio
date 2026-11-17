//! Argon-skin drawing for the gameplay scene, following lazer's ArgonMainCirclePiece / ArgonSliderBall /
//! ArgonFollowCircle / ArgonReverseArrow / ArgonSliderScorePoint transforms.
use crate::argon::{self, BORDER};
use crate::beatmap::{HitObject, SliderData};
use crate::path::P;
use crate::scene::{Frame, Scene};

fn ease_out_quad(t: f32) -> f32 {
    1.0 - (1.0 - t) * (1.0 - t)
}
fn ease_out_quint(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(5)
}
fn elastic_half(t: f32) -> f32 {
    if t <= 0.0 || t >= 1.0 {
        return t.clamp(0.0, 1.0);
    }
    2f32.powf(-10.0 * t) * ((0.5 * t - 0.075) * std::f32::consts::TAU / 0.3).sin() + 1.0
}
fn frac(t: f32, a: f32, b: f32) -> f32 {
    ((t - a) / (b - a).max(1e-6)).clamp(0.0, 1.0)
}

impl Scene {
    pub(crate) fn accent(&self, o: &HitObject) -> [f32; 3] {
        self.colors[o.color_index % self.colors.len()]
    }

    /// One Argon hit circle (a full circle, or a slider head without the outer fill).
    pub(crate) fn argon_circle(&self, out: &mut Frame, o: &HitObject, pos: P, t: f32, appear: f32, hit_at: f32, with_fill: bool) {
        if t < appear {
            return;
        }
        let ac = self.accent(o);
        let dark4 = argon::darken(ac, 4.0);
        let a_in = frac(t, appear, appear + self.fade_in);
        let (mut whole, mut num_a, mut fill_a) = (1.0, 1.0, 1.0);
        let (mut border_k, mut grad_k, mut flash_a, mut grad_a) = (1.0, 1.0, 0.0, 1.0);
        let mut grad_tint = ac;
        let mut border_fade = 0.0;
        let dt = t - hit_at;
        if dt >= 0.0 {
            // lazer fades the whole piece over 800 ms with hit lighting, 640 ms without
            whole = 1.0 - ease_out_quad(frac(dt, 0.0, if self.hit_lighting { 800.0 } else { 640.0 }));
            num_a = 1.0 - frac(dt, 0.0, 75.0);
            fill_a = 1.0 - ease_out_quint(frac(dt, 0.0, 150.0));
            let shrink_border = (128.0 * 0.8 + BORDER) / 128.0;
            border_k = 1.0 - (1.0 - shrink_border) * elastic_half(frac(dt, 0.0, 400.0));
            border_fade = frac(dt, 0.0, 800.0);
            let d2 = dt - 12.5;
            if d2 >= 0.0 {
                grad_k = 1.0 - 0.2 * elastic_half(frac(d2, 0.0, 400.0));
                let w = frac(d2, 0.0, 80.0);
                grad_tint = [ac[0] + (1.0 - ac[0]) * w, ac[1] + (1.0 - ac[1]) * w, ac[2] + (1.0 - ac[2]) * w];
                grad_a = 1.0 - frac(d2, 80.0, 230.0);
            }
            // with hit lighting the flash stays and fades with the piece; without it flashes in and straight out again
            flash_a = if self.hit_lighting { ease_out_quint(frac(dt, 0.0, 150.0)) } else { 0.6 * ease_out_quint(frac(dt, 0.0, 150.0)) * (1.0 - ease_out_quint(frac(dt, 150.0, 300.0))) };
        }
        let k = self.scale;
        let a = a_in * whole;
        if a <= 0.0 {
            return;
        }
        if with_fill {
            self.sprite(out, "ar_outerfill", 0, pos, k, 0.0, a * fill_a, Some(dark4));
        }
        self.sprite(out, "ar_outergrad", 0, pos, k * grad_k, 0.0, a * grad_a, Some(grad_tint));
        self.sprite(out, "ar_innergrad", 0, pos, k, 0.0, a * fill_a, Some(ac));
        self.sprite(out, "ar_innerfill", 0, pos, k, 0.0, a * fill_a, Some(dark4));
        self.number(out, o.number, [pos[0], pos[1] - 2.0 * k], k, a * num_a);
        if flash_a > 0.0 {
            self.sprite_add(out, "ar_flash", pos, k, 0.0, a * flash_a, Some(ac));
        }
        if border_fade < 1.0 {
            self.sprite(out, "ar_border", 0, pos, k * border_k, 0.0, a * (1.0 - border_fade), None);
        }
        if border_fade > 0.0 {
            self.sprite(out, "ar_border_fade", 0, pos, k * border_k, 0.0, a * border_fade, Some(ac));
        }
    }

    pub(crate) fn argon_approach(&self, out: &mut Frame, o: &HitObject, pos: P, t: f32, appear: f32) {
        if t >= o.time + 50.0 || t < appear {
            return;
        }
        let mut a = 0.9 * frac(t, appear, appear + (self.fade_in * 2.0).min(self.preempt));
        if t > o.time {
            a *= 1.0 - frac(t, o.time, o.time + 50.0);
        }
        let k = 1.0 + 3.0 * (1.0 - frac(t, appear, o.time));
        self.sprite(out, "ar_approach", 0, pos, self.scale * k, 0.0, a, Some(self.accent(o)));
    }

    /// Everything of a slider except its body: repeat arrows, ticks, head, follow circle and ball.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn argon_slider_parts(&self, out: &mut Frame, o: &HitObject, s: &SliderData, p: P, t: f32, appear: f32, alpha_whole: f32) {
        let map = &*self.map;
        let ac = self.accent(o);
        let end = o.end_time;
        let base = map.stacked(o);
        let p0 = s.path.pts[0];
        let to_view = |q: P| [base[0] + q[0] - p0[0], base[1] + q[1] - p0[1]];
        let ba = frac(t, appear, appear + self.fade_in) * alpha_whole;
        let k = self.scale;
        let (end_pos, start_pos) = (to_view(s.path.at(s.length).0), to_view(s.path.at(0.0).0));

        // repeat arrows
        for r in 1..s.slides {
            let pass = o.time + r as f32 * s.span_duration;
            let anim = s.span_duration.min(300.0);
            let ka = ba * (1.0 - frac(t, pass, pass + anim));
            if ka <= 0.0 {
                continue;
            }
            let (pos, toward) = if r % 2 == 1 { (end_pos, s.path.at((s.length - 8.0).max(0.0)).0) } else { (start_pos, s.path.at(8.0f32.min(s.length)).0) };
            let toward = to_view(toward);
            let rot = (toward[1] - pos[1]).atan2(toward[0] - pos[0]);
            let grow = if t >= pass { 1.0 + 0.5 * ease_out_quad(frac(t, pass, pass + anim)) } else { 1.0 };
            let lt = (t - appear).rem_euclid(300.0);
            let pulse = if t >= pass { 1.0 } else if lt < 35.0 { 1.0 + 0.3 * ease_out_quad(lt / 35.0) } else { 1.3 - 0.3 * ease_out_quad((lt - 35.0) / 250.0) };
            self.sprite(out, "ar_rev_pill", 0, pos, k * grow * pulse, rot, ka, None);
            self.sprite(out, "ar_rev_icon", 0, pos, k * grow * pulse, rot, ka, Some(argon::darken(ac, 4.0)));
            let slide = if t >= pass { 0.0 } else if lt < 35.0 { -12.0 * ease_out_quad(lt / 35.0) } else { -12.0 * (1.0 - ease_out_quad((lt - 35.0) / 250.0)) };
            let off = slide * k * grow;
            self.sprite(out, "ar_edge", 0, [pos[0] + rot.cos() * off, pos[1] + rot.sin() * off], k * grow, rot, ka, None);
        }
        // ticks
        for span in 0..s.slides {
            for &tick in &s.ticks {
                let f = if span % 2 == 1 { 1.0 - tick } else { tick };
                let span_start = o.time + span as f32 * s.span_duration;
                let tt = span_start + tick * s.span_duration;
                let ap = (span_start + (tt - span_start) * 0.5 - self.preempt).max(appear);
                if t < ap || t > tt + 150.0 {
                    continue;
                }
                let pos = to_view(s.path.at(f * s.length).0);
                let (mut al, mut sc) = (frac(t, ap, ap + 150.0), 0.5 + 0.5 * elastic_half(frac(t, ap, ap + 600.0)));
                if t >= tt {
                    let u = frac(t, tt, tt + 150.0);
                    al = 1.0 - ease_out_quint(u);
                    sc = 1.0 + 0.5 * ease_out_quad(u);
                }
                self.sprite(out, "ar_tick", 0, pos, k * sc, 0.0, al * alpha_whole, Some(ac));
            }
        }
        // head
        self.argon_circle(out, o, p, t, appear, o.time, false);
        // follow circle and ball
        if t >= o.time && t <= end + 300.0 {
            let tb = t.min(end);
            let (bp, dir, _) = self.slider_ball_pub(o, s, tb);
            let (fs, fa) = if t <= end {
                let u = ease_out_quint(frac(t, o.time, o.time + 300.0));
                let mut fs = 1.0 + (argon::FOLLOW_AREA - 1.0) * u;
                // ArgonFollowCircle.OnSliderTick: once at full size, a hit tick or repeat grows it by 8% over 40 ms and back over 200 ms
                if fs >= argon::FOLLOW_AREA * 0.98 {
                    if let Some(dt) = self.last_pulse(o, s, t, 240.0) {
                        let bump = if dt < 40.0 { ease_out_quint(dt / 40.0) } else { 1.0 - ease_out_quint((dt - 40.0) / 200.0) };
                        fs = argon::FOLLOW_AREA * (1.0 + 0.08 * bump);
                    }
                }
                (fs, u)
            } else {
                (1.0 + (argon::FOLLOW_AREA - 1.0) * (1.0 - ease_out_quint(frac(t, end, end + 300.0))), 1.0 - ease_out_quint(frac(t, end, end + 150.0)))
            };
            if fa > 0.0 {
                self.sprite_add(out, "ar_follow", bp, k * fs, 0.0, fa, Some(ac));
            }
            let mut ball_a = ease_out_quint(frac(t, o.time, o.time + 200.0));
            let mut icon_k = elastic_half(frac(t, o.time, o.time + 200.0)) * 1.0;
            if t > end {
                ball_a *= 1.0 - ease_out_quint(frac(t, end, end + 50.0));
                icon_k *= 1.0 - 0.1 * ease_out_quint(frac(t, end, end + 200.0));
            }
            if ball_a > 0.0 {
                self.sprite(out, "ar_ball_fill", 0, bp, k, 0.0, ball_a, Some(ac));
                self.sprite(out, "ar_ball_border", 0, bp, k, 0.0, ball_a, None);
                self.sprite(out, "ar_ball_icon", 0, bp, k * icon_k, dir[1].atan2(dir[0]), ball_a, None);
            }
        }
    }
}
