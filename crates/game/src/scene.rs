//! The gameplay scene: what an autoplayed osu!standard map looks like at map time `t`, with lazer's timings for
//! fade-in, hit animations, slider body snaking, ticks, ball, follow circle, repeat arrows.
//! Output is a flat draw list (sprites and slider bodies, in z order) that any renderer can consume.
use std::ops::Range;
use std::sync::Arc;

use engine::lazer;

use crate::beatmap::{Beatmap, HitObject, Kind, SliderData};
use crate::path::P;
use crate::skin::{Rect, Sheet, Skin};

/// View coordinates: the 512x384 playfield sits inside a 640x420 view (see `lazer::FIELD_OFF`).
#[derive(Clone, Copy, Debug, Default)]
pub struct SpriteDraw {
    pub pos: [f32; 2],
    /// Half the displayed size in view units.
    pub half: [f32; 2],
    pub rot: f32,
    pub alpha: f32,
    pub rect: [f32; 4], // x0, y0, x1, y1 in sheet pixels
    pub tint: Option<[f32; 3]>,
    pub additive: bool,
}

#[derive(Clone, Debug)]
pub struct SliderDraw {
    pub geom: usize,
    /// Segment range of the body to draw (snaking).
    pub from: usize,
    pub to: usize,
    /// The body's growing end: the last whole path point and the exact point the body reaches (a partial segment and a round
    /// cap are drawn there, so snaking is smooth however long the segments are).
    pub head: Option<(P, P, P)>,
    /// The same for the shrinking end behind the ball: the first whole path point and the exact point.
    pub tail: Option<(P, P, P)>,
    pub alpha: f32,
    pub border: [f32; 3],
    pub outer: [f32; 3],
    pub inner: [f32; 3],
    pub body_alpha: f32,
    pub border_portion: f32,
    pub shadow_portion: f32,
    /// Depth slot: smaller is in front.
    pub z: f32,
    pub z_range: f32,
}

#[derive(Clone, Debug)]
pub enum Op {
    Sprites(Range<usize>, bool),
    Slider(usize),
}

#[derive(Default)]
pub struct Frame {
    pub sprites: Vec<SpriteDraw>,
    pub sliders: Vec<SliderDraw>,
    pub ops: Vec<Op>,
}

impl Frame {
    fn clear(&mut self) {
        self.sprites.clear();
        self.sliders.clear();
        self.ops.clear();
    }
    pub(crate) fn flush(&mut self, start: usize, _additive: bool) {
        // one run per stretch of equal blending
        let mut i = start;
        while i < self.sprites.len() {
            let add = self.sprites[i].additive;
            let mut j = i;
            while j < self.sprites.len() && self.sprites[j].additive == add {
                j += 1;
            }
            self.ops.push(Op::Sprites(i..j, add));
            i = j;
        }
    }
}

/// Static mesh of one slider's body: a triangle strip per path segment plus round caps, as a flat vertex list of
/// `(x, y, d)` where d is 0 on the path and 1 at the border, in view units.
pub struct SliderGeom {
    pub verts: Vec<[f32; 3]>,
    /// Per segment: vertex range of the quad.
    pub quads: Vec<Range<u32>>,
    /// Per path point: vertex range of its round cap (empty for the extra points on straight stretches).
    pub caps: Vec<Range<u32>>,
    /// Per path point: how far along the path it is (0 to 1, by length), so trimming follows the ball exactly.
    pub cum: Vec<f32>,
    /// The path points, as placed in the view.
    pub pts: Vec<P>,
    /// Per segment: its unit normal. Every segment of a straight stretch shares one, so the strips meet edge to edge with
    /// identical vertices (no cracks between them).
    pub norms: Vec<P>,
    pub radius: f32,
}

const CAP: usize = 48;

impl SliderGeom {
    /// The strip between two points: a double-sided quad whose third coordinate is the distance from the centre line.
    pub fn quad_verts(a: P, b: P, n: P, radius: f32, out: &mut Vec<[f32; 3]>) {
        let (nx, ny) = (n[0] * radius, n[1] * radius);
        let v = |p: P, off: f32, d: f32| [p[0] + nx * off, p[1] + ny * off, d];
        let tri = [(a, 1.0, 1.0), (a, 0.0, 0.0), (b, 1.0, 1.0), (a, 0.0, 0.0), (b, 0.0, 0.0), (b, 1.0, 1.0), (a, 0.0, 0.0), (a, -1.0, 1.0), (b, 0.0, 0.0), (a, -1.0, 1.0), (b, -1.0, 1.0), (b, 0.0, 0.0)];
        for (p, off, d) in tri {
            out.push(v(p, off, d));
        }
    }

    /// A round cap: a circumscribed polygon, so its flat sides sit exactly on the circle and meet the strips without notches.
    pub fn cap_verts(c: P, radius: f32, out: &mut Vec<[f32; 3]>) {
        let rim = radius / (std::f32::consts::PI / CAP as f32).cos();
        for k in 0..CAP {
            let (a0, a1) = (k as f32 / CAP as f32 * std::f32::consts::TAU, (k + 1) as f32 / CAP as f32 * std::f32::consts::TAU);
            out.push([c[0], c[1], 0.0]);
            // the third coordinate is the true distance from the path over the radius, so it reads 1 on the circle itself and
            // matches the strips exactly where they overlap (no depth ties, no seams)
            out.push([c[0] + rim * (a0 + std::f32::consts::PI / CAP as f32).cos(), c[1] + rim * (a0 + std::f32::consts::PI / CAP as f32).sin(), rim / radius]);
            out.push([c[0] + rim * (a1 + std::f32::consts::PI / CAP as f32).cos(), c[1] + rim * (a1 + std::f32::consts::PI / CAP as f32).sin(), rim / radius]);
        }
    }

    /// The last path point at or before `f` (0 to 1 along the path) and the exact point at `f`.
    pub fn point_at(&self, f: f32) -> (usize, P) {
        let n = self.pts.len();
        let i = self.cum.partition_point(|&c| c <= f).saturating_sub(1).min(n.saturating_sub(1));
        if i + 1 >= n {
            return (i, self.pts[n - 1]);
        }
        let (c0, c1) = (self.cum[i], self.cum[i + 1]);
        let u = if c1 > c0 { ((f - c0) / (c1 - c0)).clamp(0.0, 1.0) } else { 0.0 };
        (i, [self.pts[i][0] + (self.pts[i + 1][0] - self.pts[i][0]) * u, self.pts[i][1] + (self.pts[i + 1][1] - self.pts[i][1]) * u])
    }
}

fn build_geom(pts: &[P], origin: P, radius: f32) -> SliderGeom {
    // thin out nearly collinear dense points so long sliders stay cheap
    let mut orig: Vec<P> = vec![pts[0]];
    for p in &pts[1..pts.len() - 1] {
        let l = orig.last().unwrap();
        if (p[0] - l[0]).hypot(p[1] - l[1]) >= 1.5 {
            orig.push(*p);
        }
    }
    orig.push(*pts.last().unwrap());
    let mut keep: Vec<(P, bool)> = vec![(orig[0], true)];
    let mut norms: Vec<P> = Vec::new();
    for w2 in orig.windows(2) {
        let (dx, dy) = (w2[1][0] - w2[0][0], w2[1][1] - w2[0][1]);
        let l = dx.hypot(dy);
        let normal = if l > 1e-6 { [-dy / l, dx / l] } else { [0.0, 1.0] };
        // One strip per straight stretch, as in the game: the exact ends of a growing or shrinking body are drawn separately,
        // so no extra points (and no seams between many small strips) are needed.
        let n = 1usize;
        for k in 1..=n {
            let u = k as f32 / n as f32;
            keep.push(([w2[0][0] + dx * u, w2[0][1] + dy * u], k == n));
            norms.push(normal);
        }
    }
    let mut cum = vec![0.0f32];
    for w2 in keep.windows(2) {
        cum.push(cum.last().unwrap() + (w2[1].0[0] - w2[0].0[0]).hypot(w2[1].0[1] - w2[0].0[1]));
    }
    let total = cum.last().copied().unwrap_or(1.0).max(1e-6);
    for c in cum.iter_mut() {
        *c /= total;
    }
    let w = |p: P| [p[0] + origin[0], p[1] + origin[1]];
    let placed: Vec<P> = keep.iter().map(|(p, _)| w(*p)).collect();
    let mut g = SliderGeom { verts: vec![], quads: vec![], caps: vec![], cum, pts: placed.clone(), norms: norms.clone(), radius };
    for (i, (_, is_orig)) in keep.iter().enumerate() {
        let s = g.verts.len() as u32;
        if *is_orig {
            SliderGeom::cap_verts(placed[i], radius, &mut g.verts);
        }
        g.caps.push(s..g.verts.len() as u32);
        if i + 1 < keep.len() {
            let s = g.verts.len() as u32;
            SliderGeom::quad_verts(placed[i], placed[i + 1], norms[i], radius, &mut g.verts);
            g.quads.push(s..g.verts.len() as u32);
        }
    }
    g
}

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

/// What the follow-point layer needs from the animation frames.
pub struct FollowFrames {
    pub rects: Vec<Option<Rect>>,
    pub fps: f32,
    pub ratio: f32,
    pub ini_set: bool,
    pub additive: bool,
}

pub struct Scene {
    pub argon: bool,
    /// Size of the cursor and its trail, 1 = the skin's own size.
    pub cursor_scale: f32,
    /// The glow that flashes around a hit circle (off unless asked for).
    pub hit_lighting: bool,
    /// lazer's "snaking in" and "snaking out" slider settings (both on by default).
    pub snake_in: bool,
    pub snake_out: bool,
    /// Hidden mod preview: hides approach circles (after object 0) and fades hit circles out early.
    pub hidden: bool,
    pub map: Arc<Beatmap>,
    pub skin: Arc<Skin>,
    pub sheet: Arc<Sheet>,
    pub geoms: Vec<Option<SliderGeom>>,
    pub colors: Vec<[f32; 3]>,
    /// How the cursor grows while the autoplay holds a key (see `press_timeline`), as (start, from, to) of a 100 ms ease-out.
    pub presses: Vec<(f32, f32, f32)>,
    /// Whether the colours are the map's own (picked with the combo offsets) rather than the skin's.
    pub map_colours: bool,
    pub follow: Option<FollowFrames>,
    pub(crate) scale: f32,
    pub(crate) preempt: f32,
    pub(crate) fade_in: f32,
    pub(crate) overlap: f32,
    pub(crate) overlay_above: bool,
    pub(crate) ball_tint: bool,
    /// (time, position, linear_hold) keyframes of osu!lazer's `OsuAutoGenerator` cursor movement between objects.
    cursor_keys: Vec<(f32, P, bool)>,
}

const OFF: P = lazer::FIELD_OFF;
pub(crate) const OFF_X: f32 = lazer::FIELD_OFF[0];
pub(crate) const OFF_Y: f32 = lazer::FIELD_OFF[1];

fn dark(c: [f32; 3], a: f32) -> [f32; 3] {
    [c[0] / (1.0 + a), c[1] / (1.0 + a), c[2] / (1.0 + a)]
}
fn lighten(c: [f32; 3], a: f32) -> [f32; 3] {
    let a = a * 0.5;
    [(c[0] * (1.0 + 0.5 * a) + a).min(1.0), (c[1] * (1.0 + 0.5 * a) + a).min(1.0), (c[2] * (1.0 + 0.5 * a) + a).min(1.0)]
}

impl Scene {
    pub fn new(map: Arc<Beatmap>, skin: Arc<Skin>, sheet: Arc<Sheet>, follow: Option<FollowFrames>) -> Scene {
        let scale = lazer::circle_scale(map.cs);
        let preempt = lazer::time_preempt(map.ar);
        let argon = matches!(skin.style, crate::skin::Style::Argon { .. });
        let mut colors: Vec<[f32; 3]> = if !map.combo_colors.is_empty() { map.combo_colors.clone() } else { skin.combo_colors() };
        if colors.is_empty() {
            colors = if argon { crate::argon::COMBO.to_vec() } else { vec![[255.0, 192.0, 0.0], [0.0, 202.0, 0.0], [18.0, 124.0, 255.0], [242.0, 24.0, 57.0]] };
        }
        let map_colours = !map.combo_colors.is_empty();
        let presses = press_timeline(&map);
        let colors = colors.iter().map(|c| [c[0] / 255.0, c[1] / 255.0, c[2] / 255.0]).collect();
        let radius = if argon { crate::argon::OUTER_GRADIENT * 0.5 * scale } else { 64.0 * scale };
        let geoms = map
            .objects
            .iter()
            .map(|o| match &o.kind {
                Kind::Slider(s) => {
                    let p = map.stacked(o);
                    Some(build_geom(&s.path.pts, [p[0] + OFF[0], p[1] + OFF[1]], radius))
                }
                _ => None,
            })
            .collect();
        let overlap = crate::skin::hitcircle_overlap(&skin);
        let overlay_above = skin.flag("hitcircleoverlayabovenumber", "hitcircleoverlayabovenumer", true);
        let ball_tint = skin.flag("allowsliderballtint", "allowsliderballtint", false);
        let mut s = Scene {
            argon,
            cursor_scale: 1.0,
            hit_lighting: false,
            snake_in: true,
            snake_out: true,
            hidden: false,
            map,
            skin,
            sheet,
            geoms,
            colors,
            presses,
            map_colours,
            follow,
            scale,
            preempt,
            fade_in: 400.0 * (preempt / 450.0).min(1.0),
            overlap,
            overlay_above,
            ball_tint,
            cursor_keys: vec![],
        };
        s.cursor_keys = s.build_cursor_keys();
        s
    }

    /// Port of osu!lazer's `OsuAutoGenerator` cursor path:
    /// starts at (256, 500), waits until `start_time - max(0, preempt - 100)` (100 ms reaction time),
    /// then moves to the next object with `Easing.Out` (fast snap-out, settling on the target).
    fn build_cursor_keys(&self) -> Vec<(f32, P, bool)> {
        let mut k: Vec<(f32, P, bool)> = Vec::new();
        let Some(first) = self.map.objects.first() else { return k };
        let reaction = 100.0f32;
        let move_window = (self.preempt - reaction).max(0.0);
        let mut last_t = first.time - 1500.0;
        let mut last_p: P = [256.0, 500.0];
        k.push((last_t, last_p, true));

        for o in &self.map.objects {
            let target_p = if matches!(o.kind, Kind::Spinner) {
                let a = self.spinner_info(o).cursor_angle(o.time);
                [256.0 + 50.0 * a.cos(), 192.0 + 50.0 * a.sin()]
            } else {
                self.map.stacked(o)
            };
            let wait_t = o.time - move_window;
            if wait_t > last_t {
                k.push((wait_t, last_p, true));
                last_t = wait_t;
            }
            if o.time >= last_t {
                k.push((o.time, target_p, false));
                last_t = o.time;
                last_p = target_p;
            }
            match &o.kind {
                Kind::Slider(_) => {
                    let e = o.end_pos();
                    let p = self.map.stacked(o);
                    let off = [p[0] - o.pos[0], p[1] - o.pos[1]];
                    last_p = [e[0] + off[0], e[1] + off[1]];
                    last_t = o.end_time;
                    k.push((last_t, last_p, true));
                }
                Kind::Spinner => {
                    let a = self.spinner_info(o).cursor_angle(o.end_time);
                    last_p = [256.0 + 50.0 * a.cos(), 192.0 + 50.0 * a.sin()];
                    last_t = o.end_time;
                    k.push((last_t, last_p, true));
                }
                Kind::Circle => {}
            }
        }
        k
    }


    /// lazer's CursorTrail: dots laid along the path the cursor took, one every `interval` units of distance, each fading
    /// for `life` ms as (1 - age / life)^exponent. With `avoid_near` nothing is drawn within one interval of the cursor.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn trail(&self, out: &mut Frame, t: f32, name: &str, additive: bool, tint: Option<[f32; 3]>, scale: f32, alpha: f32, exponent: f32, life: f32, interval: f32, avoid_near: bool, legacy: bool) {
        if self.sheet.get(name).is_none() {
            return;
        }
        let interval = interval.max(0.2);
        let rot_now = if legacy { self.cursor_rotation(t) } else { 0.0 };
        let mut prev = self.cursor_at(t);
        // walk back along the path the cursor took, laying a dot every `interval` units of distance (also between samples)
        let (mut acc, mut from_cursor) = (0.0f32, 0.0f32);
        let mut emitted = 0;
        let mut age = 1.0f32;
        while age <= life && emitted < 6000 {
            let p = self.cursor_at(t - age);
            let step = ((p[0] - prev[0]).powi(2) + (p[1] - prev[1]).powi(2)).sqrt();
            let mut need = interval - acc;
            let mut walked = 0.0;
            let mut placed = false;
            while step - walked >= need && emitted < 6000 {
                placed = true;
                walked += need;
                need = interval;
                let u = if step > 0.0 { walked / step } else { 1.0 };
                let q = [prev[0] + (p[0] - prev[0]) * u, prev[1] + (p[1] - prev[1]) * u];
                if !avoid_near || from_cursor + walked >= interval {
                    emitted += 1;
                    let a = alpha * (1.0 - (age - 1.0 + u) / life).max(0.0).powf(exponent);
                    // a part is made at the size the cursor has then (NewPartScale), and all of them turn with the cursor now (PartRotation)
                    let (sc, rot) = if legacy { (scale * self.cursor_expand(t - age), rot_now) } else { (scale, 0.0) };
                    if additive {
                        self.sprite_add(out, name, q, sc, rot, a, tint);
                    } else {
                        self.sprite(out, name, 0, q, sc, rot, a, tint);
                    }
                }
            }
            // the distance since the last part: what is left of this step after the last one, or, when none fell in it, all of it so far
            acc = if placed { step - walked } else { acc + step };
            from_cursor += step;
            prev = p;
            age += 1.0;
        }
    }

    fn cursor_at(&self, t: f32) -> P {
        // objects are sorted by time: only the last few starting before `t` can still be active
        let objs = &self.map.objects;
        let hi = objs.partition_point(|o| o.time <= t);
        for o in &objs[hi.saturating_sub(4)..hi] {
            if t > o.end_time {
                continue;
            }
            match &o.kind {
                // spinning: a circle of radius 50 around the playfield center
                Kind::Spinner => {
                    let a = self.spinner_info(o).cursor_angle(t);
                    return [256.0 + 50.0 * a.cos(), 192.0 + 50.0 * a.sin()];
                }
                // while a slider is being followed the cursor rides the ball
                Kind::Slider(s) => return self.slider_ball(o, s, t).0,
                Kind::Circle => {}
            }
        }
        let k = &self.cursor_keys;
        if k.is_empty() {
            return [256.0, 192.0];
        }
        let i = k.partition_point(|(kt, _, _)| *kt <= t);
        if i == 0 {
            return k[0].1;
        }
        if i >= k.len() {
            return k[k.len() - 1].1;
        }
        let ((t0, a, _), (t1, b, hold)) = (k[i - 1], k[i]);
        let u = frac(t, t0, t1);
        // osu!lazer `Easing.Out` (OutQuad): fast initial snap towards the next hit object
        let u = if hold { u } else { ease_out_quad(u) };
        [a[0] + (b[0] - a[0]) * u, a[1] + (b[1] - a[1]) * u]
    }

    /// Ball position (playfield coordinates) and heading at time `t`.
    fn slider_ball(&self, o: &HitObject, s: &SliderData, t: f32) -> (P, P, u32) {
        let progress = ((t - o.time) / (s.span_duration * s.slides as f32)).clamp(0.0, 1.0) * s.slides as f32;
        let span = (progress.floor() as u32).min(s.slides - 1);
        let mut f = progress - span as f32;
        if span % 2 == 1 {
            f = 1.0 - f;
        }
        let (p, dir) = s.path.at(f * s.length);
        let dir = if span % 2 == 1 { [-dir[0], -dir[1]] } else { dir };
        let base = self.map.stacked(o);
        let p0 = s.path.pts[0];
        ([base[0] + p[0] - p0[0], base[1] + p[1] - p0[1]], dir, span)
    }

    pub(crate) fn slider_ball_pub(&self, o: &HitObject, s: &SliderData, t: f32) -> (P, P, u32) {
        self.slider_ball(o, s, t)
    }

    pub(crate) fn sprite_add(&self, out: &mut Frame, name: &str, pos: P, scale: f32, rot: f32, alpha: f32, tint: Option<[f32; 3]>) {
        let n = out.sprites.len();
        self.sprite(out, name, 0, pos, scale, rot, alpha, tint);
        if out.sprites.len() > n {
            out.sprites[n].additive = true;
        }
    }

    pub(crate) fn sprite(&self, out: &mut Frame, name: &str, frame: usize, pos: P, scale: f32, rot: f32, alpha: f32, tint: Option<[f32; 3]>) {
        let Some(e) = self.sheet.get(name) else { return };
        let r = e.frames[frame.min(e.frames.len() - 1)];
        let (hw, hh) = (r.w as f32 / e.ratio * scale * 0.5, r.h as f32 / e.ratio * scale * 0.5);
        out.sprites.push(SpriteDraw { pos: [pos[0] + OFF[0], pos[1] + OFF[1]], half: [hw, hh], rot, alpha, rect: [r.x as f32, r.y as f32, (r.x + r.w) as f32, (r.y + r.h) as f32], tint, additive: false });
    }

    /// The colour of an object's combo, as osu!lazer picks it: the colours of the map itself go by `ComboIndexWithOffsets` (the map's
    /// beatmap skin), those of the skin by `ComboIndex`, which counts the first combo as 1 (so the first combo is the skin's second colour).
    pub(crate) fn combo_colour(&self, o: &HitObject) -> [f32; 3] {
        let i = if self.map_colours { o.combo_index_offsets } else { o.combo_index };
        self.colors[i % self.colors.len()]
    }

    /// The expansion of the legacy cursor at `t`.
    pub(crate) fn cursor_expand(&self, t: f32) -> f32 {
        expand_at(&self.presses, t)
    }

    /// The rotation of the legacy cursor at `t`, in radians: it turns once every 10 s (LegacyCursor.REVOLUTION_DURATION) unless
    /// the skin says CursorRotate: 0.
    pub(crate) fn cursor_rotation(&self, t: f32) -> f32 {
        if self.skin.flag("cursorrotate", "cursorrotate", true) {
            (t / 10000.0).rem_euclid(1.0) * std::f32::consts::TAU
        } else {
            0.0
        }
    }

    pub(crate) fn has(&self, name: &str) -> bool {
        self.sheet.get(name).is_some()
    }

    pub(crate) fn number(&self, out: &mut Frame, n: u32, pos: P, scale: f32, alpha: f32) {
        let digits: Vec<u32> = n.to_string().chars().filter_map(|c| c.to_digit(10)).collect();
        let size = |d: u32| self.sheet.get(&format!("num{d}")).map(|e| (e.frames[0].w as f32 / e.ratio, e.frames[0].h as f32 / e.ratio));
        let sc = if self.argon { scale } else { scale * 0.8 };
        let mut total = 0.0;
        for d in &digits {
            if let Some((w, _)) = size(*d) {
                total += w - self.overlap;
            }
        }
        total += self.overlap;
        let mut x = -total * 0.5 * sc;
        for d in &digits {
            let Some((w, _)) = size(*d) else { continue };
            let cx = x + w * 0.5 * sc;
            self.sprite(out, &format!("num{d}"), 0, [pos[0] + cx, pos[1]], sc, 0.0, alpha, None);
            x += (w - self.overlap) * sc;
        }
    }

    /// A circle (head of a circle or slider): body, number, overlay.
    #[allow(clippy::too_many_arguments)]
    fn circle(&self, out: &mut Frame, o: &HitObject, base: &str, pos: P, t: f32, appear: f32, hit_at: Option<f32>, show_number: bool) {
        let col = self.combo_colour(o);
        let name = if self.has(base) { base } else { "hitcircle" };
        let overlay = format!("{name}overlay");
        let overlay = if self.has(&overlay) { overlay } else if name == base { "hitcircleoverlay".to_string() } else { overlay };
        let mut alpha = if self.hidden {
            let fi = self.preempt * 0.4;
            let fo = self.preempt * 0.3;
            frac(t, appear, appear + fi) * (1.0 - frac(t, appear + fi, appear + fi + fo))
        } else {
            frac(t, appear, appear + self.fade_in)
        };
        let mut sc = self.scale;
        let mut num_alpha = 1.0;
        let mut num_scale = 1.0;
        if let Some(h) = hit_at {
            if t >= h {
                if self.hidden {
                    return;
                }
                let u = frac(t, h, h + 240.0);
                alpha = 1.0 - u;
                sc *= 1.0 + 0.4 * ease_out_quad(u);
                if self.skin.version() > 1.0 {
                    num_alpha = 1.0 - frac(t, h, h + 60.0);
                } else {
                    num_alpha = 1.0 - u;
                    num_scale = 1.0 + 0.4 * ease_out_quad(u);
                }
            }
        }
        if alpha <= 0.0 {
            return;
        }
        self.sprite(out, name, 0, pos, sc, 0.0, alpha, Some(col));
        if !self.overlay_above {
            self.sprite(out, &overlay, 0, pos, sc, 0.0, alpha, None);
        }
        if show_number {
            self.number(out, o.number, pos, self.scale * num_scale, alpha.min(num_alpha));
        }
        if self.overlay_above {
            self.sprite(out, &overlay, 0, pos, sc, 0.0, alpha, None);
        }
    }

    /// How long ago the last slider tick or repeat was hit, if that was within `within` ms (they drive the follow circle's pulse).
    pub(crate) fn last_pulse(&self, o: &HitObject, s: &SliderData, t: f32, within: f32) -> Option<f32> {
        let mut best: Option<f32> = None;
        let mut consider = |tt: f32| {
            let dt = t - tt;
            if dt >= 0.0 && dt < within && best.is_none_or(|b| dt < b) {
                best = Some(dt);
            }
        };
        for span in 0..s.slides {
            let span_start = o.time + span as f32 * s.span_duration;
            for &tick in &s.ticks {
                consider(span_start + tick * s.span_duration);
            }
            if span >= 1 {
                consider(span_start);
            }
        }
        best
    }

    fn approach(&self, out: &mut Frame, o: &HitObject, pos: P, t: f32, appear: f32) {
        if t >= o.time + 50.0 || t < appear || !self.has("approachcircle") {
            return;
        }
        let col = self.combo_colour(o);
        let mut a = 0.9 * frac(t, appear, appear + (self.fade_in * 2.0).min(self.preempt));
        if t > o.time {
            a *= 1.0 - frac(t, o.time, o.time + 50.0);
        }
        let k = 1.0 + 3.0 * (1.0 - frac(t, appear, o.time));
        self.sprite(out, "approachcircle", 0, pos, self.scale * k, 0.0, a, Some(col));
    }


    fn anim_fps(&self) -> f32 {
        self.skin.ini.get("general", "animationframerate").and_then(|s| s.parse().ok()).unwrap_or(60.0)
    }

    pub fn build(&self, t: f32, out: &mut Frame) {
        out.clear();
        let map = &*self.map;
        let first_visible = map.objects.partition_point(|o| o.end_time + 1000.0 < t);
        let visible: Vec<usize> = (first_visible..map.objects.len()).take_while(|&i| map.objects[i].time - self.preempt <= t).collect();

        // follow points under everything
        if let Some(f) = &self.follow {
            let start = out.sprites.len();
            let mut scratch = Vec::new();
            for i in first_visible.saturating_sub(1)..map.objects.len().saturating_sub(1) {
                let (a, b) = (&map.objects[i], &map.objects[i + 1]);
                if b.time - 2000.0 > t {
                    break;
                }
                if b.new_combo || matches!(a.kind, Kind::Spinner) || matches!(b.kind, Kind::Spinner) || t > b.time + lazer::FADE + 50.0 {
                    continue;
                }
                let (pa, pb) = (a.end_pos(), map.stacked(b));
                let sa = map.stacked(a);
                let ea = [pa[0] + sa[0] - a.pos[0], pa[1] + sa[1] - a.pos[1]];
                scratch.clear();
                lazer::follow_points([ea[0] + OFF[0], ea[1] + OFF[1]], [pb[0] + OFF[0], pb[1] + OFF[1]], a.end_time, b.time, t, map.ar, map.cs, true, &mut scratch);
                for q in &scratch {
                    let n = f.rects.len();
                    if n == 0 {
                        continue;
                    }
                    let fi = lazer::frame_index(n, f.fps, f.ini_set, q.tt);
                    let Some(r) = f.rects[fi] else { continue };
                    let k = q.sc / f.ratio;
                    out.sprites.push(SpriteDraw {
                        pos: q.pos,
                        half: [r.w as f32 * k * 0.5, r.h as f32 * k * 0.5],
                        rot: q.rot_deg.to_radians(),
                        alpha: q.alpha,
                        rect: [r.x as f32, r.y as f32, (r.x + r.w) as f32, (r.y + r.h) as f32],
                        tint: None,
                        additive: f.additive,
                    });
                }
            }
            out.flush(start, false);
        }

        let slot_step = 1.0 / (visible.len() as f32 + 2.0);
        let mut approaches: Vec<usize> = Vec::new();
        // earlier objects end up on top: draw later ones first
        for (rank, &i) in visible.iter().enumerate().rev() {
            let o = &map.objects[i];
            let appear = o.time - self.preempt;
            let p = map.stacked(o);
            match &o.kind {
                Kind::Circle => {
                    if t > o.time + 900.0 {
                        continue;
                    }
                    let start = out.sprites.len();
                    if self.argon {
                        self.argon_circle(out, o, p, t, appear, o.time, true);
                    } else {
                        self.circle(out, o, "hitcircle", p, t, appear, Some(o.time), true);
                    }
                    out.flush(start, false);
                    approaches.push(i);
                }
                Kind::Slider(s) => {
                    let end = o.end_time;
                    if t > end + 900.0 {
                        continue;
                    }
                    let mut a = if self.hidden {
                        let fi = self.preempt * 0.4;
                        let fade_in = frac(t, appear, appear + fi);
                        let fade_out = 1.0 - frac(t, appear + fi, end.max(appear + fi + 1.0));
                        fade_in * fade_out
                    } else {
                        let mut v = frac(t, appear, appear + self.fade_in);
                        if t > end {
                            v *= 1.0 - frac(t, end, end + 240.0);
                        }
                        v
                    };
                    // body, snaking in then out on the final span
                    if let Some(g) = &self.geoms[i] {
                        let nseg = g.quads.len();
                        let snake_in = if self.snake_in { frac(t, appear, appear + self.preempt / 3.0) } else { 1.0 };
                        let mut from = 0usize;
                        let mut to = nseg;
                        let mut head = None;
                        let mut tail = None;
                        if snake_in < 1.0 {
                            let (i, exact) = g.point_at(snake_in);
                            to = i;
                            head = Some((g.pts[i], exact, g.norms[i.min(g.norms.len() - 1)]));
                        }
                        if t >= o.time {
                            to = nseg;
                            let total = s.span_duration * s.slides as f32;
                            let prog = ((t - o.time) / total).clamp(0.0, 1.0) * s.slides as f32;
                            if self.snake_out && prog >= (s.slides - 1) as f32 {
                                // the last span: the body shrinks away behind the ball (from the start when it runs forwards,
                                // from the far end when it runs back)
                                let along = (prog - (s.slides - 1) as f32).clamp(0.0, 1.0);
                                if s.slides % 2 == 1 {
                                    let (j, exact) = g.point_at(along);
                                    from = (j + 1).min(nseg);
                                    tail = Some((g.pts[from], exact, g.norms[j.min(g.norms.len() - 1)]));
                                } else {
                                    let (j, exact) = g.point_at(1.0 - along);
                                    to = j;
                                    head = Some((g.pts[j], exact, g.norms[j.min(g.norms.len() - 1)]));
                                }
                            }
                        }
                        // After the end: a body that has been snaking out fades over 40 ms (lazer: Body.FadeOut(40), measured linear)
                        // on top of the slider's own 240 ms fade; otherwise the full body fades over 240 ms.
                        if !self.hidden && t > end {
                            let whole = frac(t, appear, appear + self.fade_in) * (1.0 - frac(t, end, end + 240.0));
                            a = if self.snake_out { whole * (1.0 - frac(t, end, end + 40.0)) } else { whole };
                        }
                        let track = map.slider_track.or_else(|| self.skin.ini.color("slidertrackoverride")).map(|c| [c[0] / 255.0, c[1] / 255.0, c[2] / 255.0]);
                        let col = track.unwrap_or(self.combo_colour(o));
                        let border = map.slider_border.or_else(|| self.skin.ini.color("sliderborder")).map(|c| [c[0] / 255.0, c[1] / 255.0, c[2] / 255.0]).unwrap_or([1.0; 3]);
                        // Argon: accent border, accent/5 body at 0.98 (0.92 for Pro) alpha, no shadow, border 0.2 of the radius
                        let (border, outer, inner, body_alpha, border_portion, shadow_portion) = if self.argon {
                            let ac = self.combo_colour(o);
                            let pro = matches!(self.skin.style, crate::skin::Style::Argon { pro: true });
                            let body = crate::argon::darken(ac, 4.0);
                            (ac, body, body, if pro { 0.92 } else { 0.98 }, crate::argon::GRADIENT / (crate::argon::OUTER_GRADIENT * 0.5), 0.0)
                        } else {
                            (border, dark(col, 0.1), lighten(col, 0.5), 0.7, 0.1875, 0.078125)
                        };
                        out.sliders.push(SliderDraw {
                            geom: i,
                            from,
                            to: to.max(from),
                            head,
                            tail,
                            alpha: a,
                            border,
                            outer,
                            inner,
                            body_alpha,
                            border_portion,
                            shadow_portion,
                            z: (rank as f32 + 1.0) * slot_step,
                            z_range: slot_step * 0.5,
                        });
                        out.ops.push(Op::Slider(out.sliders.len() - 1));
                    }
                    let start = out.sprites.len();
                    let base = map.stacked(o);
                    let to_view = |q: P| [base[0] + q[0] - s.path.pts[0][0], base[1] + q[1] - s.path.pts[0][1]];
                    let alpha_whole = if t > end { 1.0 - frac(t, end, end + 240.0) } else { 1.0 };
                    if self.argon {
                        self.argon_slider_parts(out, o, s, p, t, appear, alpha_whole);
                        out.flush(start, false);
                        approaches.push(i);
                        continue;
                    }
                    // tail circle (only skins that have one) and repeat arrows on a circle each
                    let end_pos = to_view(s.path.at(s.length).0);
                    let start_pos = to_view(s.path.at(0.0).0);
                    let ccol = self.combo_colour(o);
                    // DrawableOsuHitObject.ApplyRepeatFadeIn: the first end circle fades in with the slider (after a third of the pre-empt
                    // when the body snakes in); later ones appear two spans before they are due and fade in over at most a span.
                    let fade = |t: f32, from: f32, dur: f32| if dur <= 0.0 { (t >= from) as u8 as f32 } else { frac(t, from, from + dur) };
                    let delay = if self.snake_in { self.preempt / 3.0 } else { 0.0 };
                    let circle_name = if self.has("sliderendcircle") { ("sliderendcircle", "sliderendcircleoverlay") } else { ("hitcircle", "hitcircleoverlay") };
                    if self.has("sliderendcircle") {
                        // the tail (SliderTailCircle, repeat index slides - 1); hit tails stay until the slider is gone
                        let r = s.slides - 1;
                        let (from, dur) = if r == 0 { (appear + delay, self.fade_in) } else { (o.time + s.slides as f32 * s.span_duration - 2.0 * s.span_duration, 0.0) };
                        let ta = fade(t, from, dur) * alpha_whole;
                        self.sprite(out, "sliderendcircle", 0, end_pos, self.scale, 0.0, ta, Some(ccol));
                        self.sprite(out, "sliderendcircleoverlay", 0, end_pos, self.scale, 0.0, ta, None);
                    }
                    for k in 1..s.slides {
                        let pass = o.time + k as f32 * s.span_duration;
                        let r = k - 1;
                        let (from, cdur, adur) = if r == 0 { (appear + delay, self.fade_in, 150.0) } else { (pass - 2.0 * s.span_duration, 0.0, s.span_duration.min(150.0)) };
                        let anim = s.span_duration.min(300.0);
                        let hit_u = frac(t, pass, pass + anim);
                        // the whole repeat fades out (Easing.Out) once it is hit
                        let whole = alpha_whole * (1.0 - ease_out_quad(hit_u));
                        if whole <= 0.0 {
                            continue;
                        }
                        let (pos, toward) = if k % 2 == 1 { (end_pos, s.path.at((s.length - 8.0).max(0.0)).0) } else { (start_pos, s.path.at(8.0f32.min(s.length)).0) };
                        let toward = to_view(toward);
                        let ca = fade(t, from, cdur) * whole;
                        self.sprite(out, circle_name.0, 0, pos, self.scale, 0.0, ca, Some(ccol));
                        self.sprite(out, circle_name.1, 0, pos, self.scale, 0.0, ca, None);
                        let aim = (toward[1] - pos[1]).atan2(toward[0] - pos[0]);
                        // LegacyReverseArrow: a 300 ms loop from the repeat's animation start, or a 1 -> 1.4 pop once hit
                        let (rot, sc) = if t >= pass {
                            (aim, 1.0 + 0.4 * ease_out_quad(hit_u))
                        } else {
                            let u = (t - from).rem_euclid(300.0) / 300.0;
                            if self.skin.version() <= 1.0 {
                                (aim + (5.625 - 11.25 * u).to_radians(), 1.3 - 0.3 * u)
                            } else {
                                (aim, 1.3 - 0.3 * ease_out_quad(u))
                            }
                        };
                        self.sprite(out, "reversearrow", 0, pos, self.scale * sc, rot, fade(t, from, adur) * whole, None);
                    }
                    // ticks
                    if self.has("sliderscorepoint") {
                        for span in 0..s.slides {
                            for &tick in &s.ticks {
                                let f = if span % 2 == 1 { 1.0 - tick } else { tick };
                                let span_start = o.time + span as f32 * s.span_duration;
                                let tt = span_start + tick * s.span_duration;
                                // SliderTick: pre-empt = half the time since the span began + 200 ms (66% of the slider's pre-empt in the first span)
                                let ap = (span_start + (tt - span_start) * 0.5 - if span > 0 { 200.0 } else { self.preempt * 0.66 }).max(appear);
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
                                self.sprite(out, "sliderscorepoint", 0, pos, self.scale * sc, 0.0, al * alpha_whole, None);
                            }
                        }
                    }
                    // head circle
                    self.circle(out, o, "sliderstartcircle", p, t, appear, Some(o.time), true);
                    // ball and follow circle while sliding
                    if t >= o.time && t <= end + 200.0 {
                        let tb = t.min(end);
                        let (bp, dir, _) = self.slider_ball(o, s, tb);
                        let col = self.combo_colour(o);
                        let sliding = t <= end;
                        if self.has("sliderfollowcircle") {
                            let (sc, al) = if sliding {
                                // LegacyFollowCircle: grows 1 -> 2 over 180 ms (Easing.Out) while fading in over 60 ms, and every tick or
                                // repeat that is hit pops it to 2.2 and back to 2 over 200 ms
                                let mut sc = 1.0 + ease_out_quad(frac(t, o.time, o.time + 180.0));
                                if t >= o.time + 180.0 {
                                    if let Some(dt) = self.last_pulse(o, s, t, 200.0) {
                                        sc = 2.2 - 0.2 * dt / 200.0;
                                    }
                                }
                                (sc, frac(t, o.time, o.time + 60.0))
                            } else {
                                let u = frac(t, end, end + 200.0);
                                (2.0 - 0.4 * ease_out_quad(u), 1.0 - u * u)
                            };
                            let frames = self.sheet.get("sliderfollowcircle").map(|e| e.frames.len()).unwrap_or(1);
                            let fi = ((t / (1000.0 / self.anim_fps())) as usize) % frames;
                            self.sprite(out, "sliderfollowcircle", fi, bp, self.scale * sc * 0.5, 0.0, al * alpha_whole, None);
                        }
                        if sliding {
                            // LegacySliderBall: the combo colour when the skin allows it, else the skin's SliderBall colour (white by default);
                            // the "-nd" layer is always near black, and the "-spec" glint is added on top, neither of them turning with the ball
                            let tint = if self.ball_tint { Some(col) } else { self.skin.ini.color("sliderball").map(|c| [c[0] / 255.0, c[1] / 255.0, c[2] / 255.0]) };
                            if self.has("sliderb-nd") {
                                self.sprite(out, "sliderb-nd", 0, bp, self.scale, 0.0, 1.0, Some([5.0 / 255.0; 3]));
                            }
                            if let Some(e) = self.sheet.get("sliderb") {
                                let delay = (0.15 / s.velocity * 16.667).max(16.667);
                                let fi = ((t - o.time) / delay) as usize % e.frames.len();
                                self.sprite(out, "sliderb", fi, bp, self.scale, dir[1].atan2(dir[0]), 1.0, tint);
                            }
                            if self.has("sliderb-spec") {
                                self.sprite_add(out, "sliderb-spec", bp, self.scale, 0.0, 1.0, None);
                            }
                        }
                    }
                    out.flush(start, false);
                    approaches.push(i);
                }
                Kind::Spinner => {
                    if t > o.end_time + 400.0 {
                        continue;
                    }
                    let start = out.sprites.len();
                    self.spinner(out, o, t);
                    out.flush(start, false);
                }
            }
        }
        // approach circles above all objects
        let start = out.sprites.len();
        for &i in approaches.iter().rev() {
            if self.hidden && i > 0 {
                continue;
            }
            let o = &map.objects[i];
            if self.argon {
                self.argon_approach(out, o, map.stacked(o), t, o.time - self.preempt);
            } else {
                self.approach(out, o, map.stacked(o), t, o.time - self.preempt);
            }
        }
        // the hit popups (300, GREAT with its rings) are left out of the preview, which is about the follow points
        // cursor trail then cursor
        let cs = self.cursor_scale;
        if self.argon {
            // ArgonCursorTrail, as in lazer's source: the soft dot texture is 5 units across (10 px, ScaleAdjust 2) and the trail
            // drawable is scaled by 0.8 / ScaleAdjust = 0.4; additive, alpha 0.8, parts fade as (1 - age / 300 ms) ^ 4, and a part is
            // laid every `texture width x cursor scale / 2.5 x 0.4` units (in the trail's own, unscaled space).
            let w = self.sheet.get("ar_trail").map(|e| e.frames[0].w as f32 / e.ratio).unwrap_or(5.0);
            self.trail(out, t, "ar_trail", true, None, 0.4 * cs, 0.8, 4.0, 300.0, w * cs / 2.5 * 0.4 * 0.4, false, false);
        } else if let Some(e) = self.sheet.get("cursortrail") {
            // LegacyCursorTrail: a skin with a cursormiddle gets a long continuous trail that stops short of the
            // cursor; without one the trail is short and disjoint
            let disjoint = !self.has("cursormiddle");
            // LegacyCursorTrail multiplies the texture's ScaleAdjust by STABLE_MAGIC_SCALE_FACTOR (1.6): the parts are that much smaller
            const MAGIC: f32 = 1.6;
            let w = e.frames[0].w as f32 / e.ratio / MAGIC;
            if disjoint {
                // lazer's LegacyCursorTrail without a cursormiddle: a part at the cursor every 1/60 s, nothing laid between them,
                // plain blending, fading linearly over 150 ms
                let mut age = 1000.0 / 60.0;
                let rot_now = self.cursor_rotation(t);
                while age < 150.0 {
                    let a = 1.0 - age / 150.0;
                    self.sprite(out, "cursortrail", 0, self.cursor_at(t - age), cs / MAGIC * self.cursor_expand(t - age), rot_now, a, None);
                    age += 1000.0 / 60.0;
                }
            } else {
                // with a cursormiddle: additive, interpolated along the path, linear fade over 500 ms, nothing drawn near the cursor
                self.trail(out, t, "cursortrail", true, None, cs / MAGIC, 1.0, 1.0, 500.0, w * cs / 2.5 / cs.max(1.0), true, true);
            }
        }
        let cp = self.cursor_at(t);
        if self.argon {
            self.sprite(out, "ar_cursor", 0, cp, cs, 0.0, 1.0, None);
        } else {
            // only the cursor image grows and turns; the cursormiddle stays as it is (LegacyCursor's ExpandTarget)
            self.sprite(out, "cursor", 0, cp, cs * self.cursor_expand(t), self.cursor_rotation(t), 1.0, None);
            self.sprite(out, "cursormiddle", 0, cp, cs, 0.0, 1.0, None);
        }
        out.flush(start, false);
    }
}

/// When the autoplay holds a key down, as osu!lazer's OsuAutoGenerator plays it: a key goes down at the start of every object and up
/// KEY_UP_DELAY (50 ms) after its end. The legacy cursor grows to 1.3 over 100 ms (out-quad) from 1 on every press and eases back to 1
/// over 100 ms when the last key is released (OsuCursorContainer + LegacyCursor). Returned as segments (start, from, to).
fn press_timeline(map: &Beatmap) -> Vec<(f32, f32, f32)> {
    const KEY_UP_DELAY: f32 = 50.0;
    // (time, is_press); a press at the same time as a release comes first
    let mut ev: Vec<(f32, bool)> = Vec::with_capacity(map.objects.len() * 2);
    for o in &map.objects {
        ev.push((o.time, true));
        ev.push((o.end_time + KEY_UP_DELAY + if matches!(o.kind, Kind::Spinner) { 1.0 } else { 0.0 }, false));
    }
    ev.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(b.1.cmp(&a.1)));
    let mut segs: Vec<(f32, f32, f32)> = Vec::new();
    let mut down = 0i32;
    for (time, press) in ev {
        if press {
            down += 1;
            segs.push((time, 1.0, 1.3));
        } else {
            down = (down - 1).max(0);
            if down == 0 {
                let from = expand_at(&segs, time);
                segs.push((time, from, 1.0));
            }
        }
    }
    segs
}

fn expand_at(segs: &[(f32, f32, f32)], t: f32) -> f32 {
    let i = segs.partition_point(|s| s.0 <= t);
    if i == 0 {
        return 1.0;
    }
    let (start, from, to) = segs[i - 1];
    let x = ((t - start) / 100.0).clamp(0.0, 1.0);
    from + (to - from) * (1.0 - (1.0 - x) * (1.0 - x))
}

#[cfg(test)]
mod cursor_tests {
    use super::expand_at;

    #[test]
    fn the_cursor_grows_on_a_press_and_eases_back() {
        // a press at 1000 ms, released at 1200: 1 -> 1.3 over 100 ms (out-quad), then back to 1 over 100 ms
        let segs = vec![(1000.0, 1.0, 1.3), (1200.0, 1.3, 1.0)];
        assert_eq!(expand_at(&segs, 900.0), 1.0);
        assert!((expand_at(&segs, 1000.0) - 1.0).abs() < 1e-6);
        assert!((expand_at(&segs, 1050.0) - 1.225).abs() < 1e-3); // out-quad at half: 0.75 of the way
        assert!((expand_at(&segs, 1150.0) - 1.3).abs() < 1e-6);
        assert!((expand_at(&segs, 1250.0) - 1.075).abs() < 1e-3);
        assert!((expand_at(&segs, 1400.0) - 1.0).abs() < 1e-6);
    }
}

/// A development check against osu!lazer: what the scene draws for the slider that starts at `SS_SLIDER` ms around its end, to be set
/// beside the alphas the lazer reference reports (dev/lazer-reference, the "slider" command).
/// SS_MAP and SS_SKIN name the map and the skin; run with `cargo test -p game dump_slider_end -- --ignored --nocapture`.
#[cfg(test)]
mod slider_end_probe {
    use super::*;

    #[test]
    #[ignore]
    fn dump_slider_end() {
        let map = Arc::new(Beatmap::parse_file(std::path::Path::new(&std::env::var("SS_MAP").unwrap())).unwrap());
        let skin = Arc::new(Skin::open(std::path::Path::new(&std::env::var("SS_SKIN").unwrap_or_else(|_| "argon".into()))).unwrap());
        let start: f32 = std::env::var("SS_SLIDER").unwrap().parse().unwrap();
        let sheet = Arc::new(crate::skin::base_sheet(&skin, map.cs, 2.0));
        let scene = Scene::new(map.clone(), skin, sheet.clone(), None);
        let o = map.objects.iter().find(|o| (o.time - start).abs() < 1.0).unwrap();
        println!("slider {}..{}", o.time, o.end_time);
        for dt in [-30.0f32, 0.0, 22.0, 47.0, 72.0, 97.0, 122.0, 172.0, 222.0] {
            let t = o.end_time + dt;
            let mut f = Frame::default();
            scene.build(t, &mut f);
            let name_of = |r: [f32; 4]| sheet.elems.iter().find(|(_, e)| e.frames.iter().any(|fr| (fr.x as f32, fr.y as f32) == (r[0], r[1]))).map(|(n, _)| n.clone()).unwrap_or_default();
            let head = map.stacked(o);
            let head = [head[0] + OFF[0], head[1] + OFF[1]];
            let (tail_pos, _, _) = match &o.kind { Kind::Slider(s) => scene.slider_ball(o, s, o.end_time), _ => unreachable!() };
            let tail = [tail_pos[0] + OFF[0], tail_pos[1] + OFF[1]];
            print!("end{dt:+}:");
            for s in &f.sprites {
                let n = name_of(s.rect);
                let d = |p: [f32; 2]| ((s.pos[0] - p[0]).powi(2) + (s.pos[1] - p[1]).powi(2)).sqrt();
                let where_ = if d(head) < 2.0 { "head" } else if d(tail) < 2.0 { "tail" } else { continue };
                print!(" {where_}:{n}={:.2}@{:.1}", s.alpha, s.half[0]);
            }
            for s in f.sliders.iter().filter(|s| s.geom == map.objects.iter().position(|q| (q.time - o.time).abs() < 1.0).unwrap()) {
                print!(" body={:.2}", s.alpha);
            }
            println!();
        }
    }
}
