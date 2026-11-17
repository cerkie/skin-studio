//! Slider paths: control points -> polyline of exactly the slider's pixel length, with position lookup by distance.
pub type P = [f32; 2];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CurveKind {
    Bezier,
    Catmull,
    Linear,
    Perfect,
}

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1]]
}
fn len(a: P) -> f32 {
    (a[0] * a[0] + a[1] * a[1]).sqrt()
}
fn lerp(a: P, b: P, t: f32) -> P {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

fn bezier(ctrl: &[P]) -> Vec<P> {
    if ctrl.len() < 2 {
        return ctrl.to_vec();
    }
    let mut approx = 0.0;
    for w in ctrl.windows(2) {
        approx += len(sub(w[1], w[0]));
    }
    let n = ((approx / 3.0).ceil() as usize).clamp(8, 400);
    let mut out = Vec::with_capacity(n + 1);
    let mut tmp = vec![[0.0; 2]; ctrl.len()];
    for i in 0..=n {
        let t = i as f32 / n as f32;
        tmp.copy_from_slice(ctrl);
        for k in (1..ctrl.len()).rev() {
            for j in 0..k {
                tmp[j] = lerp(tmp[j], tmp[j + 1], t);
            }
        }
        out.push(tmp[0]);
    }
    out
}

fn perfect(c: &[P]) -> Option<Vec<P>> {
    let (a, b, cc) = (c[0], c[1], c[2]);
    let d = 2.0 * (a[0] * (b[1] - cc[1]) + b[0] * (cc[1] - a[1]) + cc[0] * (a[1] - b[1]));
    if d.abs() < 1e-6 {
        return None;
    }
    let (a2, b2, c2) = (a[0] * a[0] + a[1] * a[1], b[0] * b[0] + b[1] * b[1], cc[0] * cc[0] + cc[1] * cc[1]);
    let ux = (a2 * (b[1] - cc[1]) + b2 * (cc[1] - a[1]) + c2 * (a[1] - b[1])) / d;
    let uy = (a2 * (cc[0] - b[0]) + b2 * (a[0] - cc[0]) + c2 * (b[0] - a[0])) / d;
    let center = [ux, uy];
    let r = len(sub(a, center));
    let ang = |p: P| (p[1] - center[1]).atan2(p[0] - center[0]);
    let (ta, mut tc) = (ang(a), ang(cc));
    let tb = ang(b);
    let tau = std::f32::consts::TAU;
    // direction: the arc must pass through b
    let mut dir = 1.0;
    let mut tb2 = tb;
    while tb2 < ta {
        tb2 += tau;
    }
    while tc < ta {
        tc += tau;
    }
    if tb2 > tc {
        dir = -1.0;
        tc -= tau;
    }
    let span = (tc - ta).abs();
    let n = ((span * r / 4.0).ceil() as usize).clamp(8, 400);
    Some((0..=n).map(|i| {
        let t = ta + dir * span * i as f32 / n as f32;
        [center[0] + r * t.cos(), center[1] + r * t.sin()]
    }).collect())
}

fn catmull(c: &[P]) -> Vec<P> {
    let mut out = Vec::new();
    for i in 0..c.len() - 1 {
        let p0 = if i > 0 { c[i - 1] } else { c[i] };
        let (p1, p2) = (c[i], c[i + 1]);
        let p3 = if i + 2 < c.len() { c[i + 2] } else { c[i + 1] };
        for s in 0..=40 {
            let t = s as f32 / 40.0;
            let (t2, t3) = (t * t, t * t * t);
            let f = |k: usize| 0.5 * ((2.0 * p1[k]) + (-p0[k] + p2[k]) * t + (2.0 * p0[k] - 5.0 * p1[k] + 4.0 * p2[k] - p3[k]) * t2 + (-p0[k] + 3.0 * p1[k] - 3.0 * p2[k] + p3[k]) * t3);
            out.push([f(0), f(1)]);
        }
    }
    out
}

/// A slider path trimmed or extended to `length` pixels.
#[derive(Clone, Debug)]
pub struct Path {
    pub pts: Vec<P>,
    /// Cumulative distance of each point.
    pub dist: Vec<f32>,
}

impl Path {
    pub fn build(kind: CurveKind, ctrl: &[P], length: f32) -> Path {
        let mut raw: Vec<P> = Vec::new();
        match kind {
            CurveKind::Linear => raw = ctrl.to_vec(),
            CurveKind::Perfect if ctrl.len() == 3 => match perfect(ctrl) {
                Some(p) => raw = p,
                None => raw = bezier(ctrl),
            },
            CurveKind::Catmull => raw = catmull(ctrl),
            _ => {
                // Bezier, split into pieces wherever a point is repeated (a red anchor)
                let mut start = 0;
                for i in 1..ctrl.len() {
                    if ctrl[i] == ctrl[i - 1] {
                        let seg = bezier(&ctrl[start..i]);
                        raw.extend(seg);
                        start = i;
                    }
                }
                raw.extend(bezier(&ctrl[start..]));
            }
        }
        raw.dedup_by(|a, b| len(sub(*a, *b)) < 1e-4);
        if raw.is_empty() {
            raw.push(ctrl.first().copied().unwrap_or([0.0; 2]));
        }
        let mut pts = vec![raw[0]];
        let mut dist = vec![0.0f32];
        for p in &raw[1..] {
            let d = dist.last().unwrap() + len(sub(*p, *pts.last().unwrap()));
            pts.push(*p);
            dist.push(d);
        }
        let total = *dist.last().unwrap();
        if length > 0.0 && total > 0.0 {
            if total > length {
                // cut
                while dist.len() > 1 && dist[dist.len() - 2] >= length {
                    dist.pop();
                    pts.pop();
                }
                let n = pts.len();
                if n >= 2 {
                    let t = (length - dist[n - 2]) / (dist[n - 1] - dist[n - 2]).max(1e-6);
                    pts[n - 1] = lerp(pts[n - 2], pts[n - 1], t);
                    dist[n - 1] = length;
                }
            } else if total < length && pts.len() >= 2 {
                // extend along the final segment
                let n = pts.len();
                let d = sub(pts[n - 1], pts[n - 2]);
                let l = len(d).max(1e-6);
                let e = [pts[n - 1][0] + d[0] / l * (length - total), pts[n - 1][1] + d[1] / l * (length - total)];
                pts.push(e);
                dist.push(length);
            }
        }
        Path { pts, dist }
    }

    pub fn length(&self) -> f32 {
        *self.dist.last().unwrap_or(&0.0)
    }

    /// Point and direction at distance `d` along the path.
    pub fn at(&self, d: f32) -> (P, P) {
        let n = self.pts.len();
        if n < 2 {
            return (self.pts[0], [1.0, 0.0]);
        }
        let d = d.clamp(0.0, self.length());
        let i = match self.dist.binary_search_by(|v| v.partial_cmp(&d).unwrap()) {
            Ok(i) => i.min(n - 2),
            Err(i) => i.clamp(1, n - 1) - 1,
        };
        let seg = (self.dist[i + 1] - self.dist[i]).max(1e-6);
        let t = (d - self.dist[i]) / seg;
        let dir = sub(self.pts[i + 1], self.pts[i]);
        let l = len(dir).max(1e-6);
        (lerp(self.pts[i], self.pts[i + 1], t), [dir[0] / l, dir[1] / l])
    }

    /// The polyline between two distances, for drawing a partly snaked body.
    pub fn slice(&self, from: f32, to: f32) -> Vec<P> {
        if to <= from {
            return vec![];
        }
        let mut out = vec![self.at(from).0];
        for (p, d) in self.pts.iter().zip(&self.dist) {
            if *d > from && *d < to {
                out.push(*p);
            }
        }
        out.push(self.at(to).0);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_is_trimmed_and_extended() {
        let p = Path::build(CurveKind::Linear, &[[0.0, 0.0], [100.0, 0.0]], 60.0);
        assert!((p.length() - 60.0).abs() < 1e-3);
        let p = Path::build(CurveKind::Linear, &[[0.0, 0.0], [100.0, 0.0]], 150.0);
        assert!((p.at(150.0).0[0] - 150.0).abs() < 1e-3);
    }

    #[test]
    fn perfect_arc_passes_through_middle_point() {
        let p = Path::build(CurveKind::Perfect, &[[0.0, 0.0], [50.0, 50.0], [100.0, 0.0]], 157.0);
        let near = p.pts.iter().any(|q| len(sub(*q, [50.0, 50.0])) < 2.0);
        assert!(near);
        assert!((p.length() - 157.0).abs() < 1.0);
    }
}
