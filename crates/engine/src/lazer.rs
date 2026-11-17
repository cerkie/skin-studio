//! osu!lazer follow-point behavior for legacy-format skins. A port of the Python reference implementation
//! (`fp_sprites`, `_frame_at`), which in turn follows `FollowPointConnection` and the legacy skin animation code.

pub const SPACING: f32 = 32.0;
pub const FADE: f32 = 400.0;
pub const BASE_PREEMPT: f32 = 800.0;
/// The 512x384 playfield sits at this offset inside the 640x420 preview view.
pub const FIELD_OFF: [f32; 2] = [64.0, 48.0];
pub const VIEW_W: f32 = 640.0;
pub const VIEW_H: f32 = 480.0;

pub fn circle_scale(cs: f32) -> f32 {
    (1.0 - 0.7 * (cs - 5.0) / 5.0) / 2.0 * 1.00041
}

pub fn time_preempt(ar: f32) -> f32 {
    if ar < 5.0 {
        1800.0 - 600.0 * ar / 5.0
    } else if ar > 5.0 {
        1200.0 - 750.0 * (ar - 5.0) / 5.0
    } else {
        1200.0
    }
}

pub fn preempt_for(ar: f32) -> f32 {
    BASE_PREEMPT * (time_preempt(ar) / 450.0).min(1.0)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Circle,
    Slider,
    Spinner,
}

#[derive(Clone, Debug)]
pub struct Obj {
    pub x: f32,
    pub y: f32,
    pub time: f32,
    pub end_time: f32,
    pub end_x: f32,
    pub end_y: f32,
    pub new_combo: bool,
    pub kind: Kind,
}

impl Obj {
    pub fn circle(x: f32, y: f32, time: f32, new_combo: bool) -> Self {
        Obj { x, y, time, end_time: time, end_x: x, end_y: y, new_combo, kind: Kind::Circle }
    }
}

/// The "Zigzag" demo map from the Python tool.
pub fn demo_zigzag(bpm: f32) -> Vec<Obj> {
    let beat = 60000.0 / bpm;
    (0..24)
        .map(|i| {
            let (a, b) = (i % 2, i / 2);
            Obj::circle(120.0 + a as f32 * 280.0 + (b % 3) as f32 * 20.0, 90.0 + (b % 6) as f32 * 40.0,
                        1500.0 + i as f32 * beat / 2.0, i % 8 == 0)
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
pub struct PointSprite {
    pub pos: [f32; 2],
    /// Final scale including circle size (1.5x -> 1x squeeze while fading in).
    pub sc: f32,
    pub alpha: f32,
    /// Age in ms since this point started fading in; selects the animation frame.
    pub tt: f32,
    pub rot_deg: f32,
}

/// Follow points between two objects at time `t`, appended to `out`.
#[allow(clippy::too_many_arguments)]
pub fn follow_points(a: [f32; 2], b: [f32; 2], t_start: f32, t_end: f32, t: f32, ar: f32, cs: f32, game_fx: bool,
                     out: &mut Vec<PointSprite>) {
    let v = [b[0] - a[0], b[1] - a[1]];
    let dist = (v[0] * v[0] + v[1] * v[1]).sqrt();
    let di = dist as i32;
    if (di as f32) < SPACING * 2.5 {
        return;
    }
    let rot = v[1].atan2(v[0]).to_degrees();
    let preempt = preempt_for(ar);
    let dur = t_end - t_start;
    let so = circle_scale(cs);
    let mut dd = (SPACING * 1.5) as i32;
    while dd < di - SPACING as i32 {
        let frac = dd as f32 / di as f32;
        dd += SPACING as i32;
        let fade_out = t_start + frac * dur;
        let tt = t - (fade_out - preempt);
        if tt < 0.0 || t > fade_out + FADE {
            continue;
        }
        let p = (tt / FADE).min(1.0);
        let mut eo = 1.0 - (1.0 - p) * (1.0 - p); // Easing.Out (quad)
        let mut alpha = p;
        if tt > preempt {
            alpha *= 1.0 - ((tt - preempt) / FADE).min(1.0);
        }
        if !game_fx {
            eo = 1.0;
            alpha = 1.0;
        }
        if alpha <= 0.0 {
            continue;
        }
        let f = frac - 0.1 + 0.1 * eo;
        out.push(PointSprite { pos: [a[0] + f * v[0], a[1] + f * v[1]], sc: (1.5 - 0.5 * eo) * so, alpha, tt, rot_deg: rot });
    }
}

/// Which frame of an `n`-frame animation is showing `tt` ms after the point started (Loop = true, as in lazer).
pub fn frame_index(n: usize, fps: f32, ini_set: bool, tt: f32) -> usize {
    let flen = if ini_set && fps > 0.0 { 1000.0 / fps } else { 1000.0 / n as f32 };
    let dur = n as f32 * flen;
    let pb = (tt % dur).min(dur);
    ((pb / flen) as usize).min(n - 1)
}

impl Obj {
    /// One line of text: `x y time end_time end_x end_y new_combo kind` (kind 0 circle, 1 slider, 2 spinner).
    pub fn to_line(&self) -> String {
        let k = match self.kind {
            Kind::Circle => 0,
            Kind::Slider => 1,
            Kind::Spinner => 2,
        };
        format!("{} {} {} {} {} {} {} {}", self.x, self.y, self.time, self.end_time, self.end_x, self.end_y, self.new_combo as u8, k)
    }

    pub fn from_line(line: &str) -> Option<Obj> {
        let v: Vec<&str> = line.split_whitespace().collect();
        if v.len() != 8 {
            return None;
        }
        let f = |i: usize| v[i].parse::<f32>().ok();
        Some(Obj {
            x: f(0)?,
            y: f(1)?,
            time: f(2)?,
            end_time: f(3)?,
            end_x: f(4)?,
            end_y: f(5)?,
            new_combo: v[6] == "1",
            kind: match v[7] {
                "1" => Kind::Slider,
                "2" => Kind::Spinner,
                _ => Kind::Circle,
            },
        })
    }
}

/// The stretch of map time worth looping: from 1.2 s before the first object to 1.5 s after the last.
pub fn default_section(objs: &[Obj]) -> (f32, f32) {
    let first = objs.first().map(|o| o.time).unwrap_or(0.0);
    let last = objs.last().map(|o| o.end_time).unwrap_or(first + 1000.0);
    let start = (first - 1200.0).max(0.0);
    (start, last + 1500.0 - start)
}
