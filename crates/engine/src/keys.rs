//! Keyframes and easings, ported from the Python reference (`EASINGS`, `to_keys`, `eval_keys`, `eval_color`).
use serde_json::Value;

pub fn ease(name: &str, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match name {
        "ease-in" => t * t,
        "ease-out" => 1.0 - (1.0 - t) * (1.0 - t),
        "ease-in-out" => 3.0 * t * t - 2.0 * t * t * t,
        "ease-in cubic" => t * t * t,
        "ease-out cubic" => 1.0 - (1.0 - t).powi(3),
        "smoother" => t * t * t * (t * (6.0 * t - 15.0) + 10.0),
        "sine in-out" => 0.5 - 0.5 * (std::f32::consts::PI * t).cos(),
        "ease-out back" => 1.0 + 2.70158 * (t - 1.0).powi(3) + 1.70158 * (t - 1.0).powi(2),
        "bounce" => {
            if t < 1.0 / 2.75 {
                7.5625 * t * t
            } else if t < 2.0 / 2.75 {
                7.5625 * (t - 1.5 / 2.75).powi(2) + 0.75
            } else if t < 2.5 / 2.75 {
                7.5625 * (t - 2.25 / 2.75).powi(2) + 0.9375
            } else {
                7.5625 * (t - 2.625 / 2.75).powi(2) + 0.984375
            }
        }
        _ => t, // "linear" and anything unknown
    }
}

/// A keyframe time: milliseconds, or the end of the animation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KeyTime {
    Ms(f32),
    End,
}

impl KeyTime {
    pub fn at(&self, total: f32) -> f32 {
        match self {
            KeyTime::Ms(v) => *v,
            KeyTime::End => total,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Key<V> {
    pub t: KeyTime,
    pub v: V,
    /// Easing of the step TO the next key.
    pub ease: String,
}

pub type NumKeys = Vec<Key<f32>>;
pub type ColKeys = Vec<Key<[f32; 3]>>;

fn key_time(v: &Value) -> KeyTime {
    match v {
        Value::String(_) => KeyTime::End,
        other => KeyTime::Ms(other.as_f64().unwrap_or(0.0) as f32),
    }
}

/// Parse a property from the Python tool's JSON: either the old `[start, end, easing]` triple or a list of
/// `[time, value, easing]` keys.
pub fn parse_num_keys(v: &Value) -> NumKeys {
    let arr = v.as_array().cloned().unwrap_or_default();
    if let Some(first) = arr.first() {
        if first.is_array() {
            return arr
                .iter()
                .filter_map(|k| {
                    let k = k.as_array()?;
                    Some(Key {
                        t: key_time(k.first()?),
                        v: k.get(1)?.as_f64()? as f32,
                        ease: k.get(2).and_then(|e| e.as_str()).unwrap_or("linear").to_string(),
                    })
                })
                .collect();
        }
        if arr.len() >= 2 {
            let ease = arr.get(2).and_then(|e| e.as_str()).unwrap_or("linear").to_string();
            return vec![
                Key { t: KeyTime::Ms(0.0), v: arr[0].as_f64().unwrap_or(0.0) as f32, ease },
                Key { t: KeyTime::End, v: arr[1].as_f64().unwrap_or(0.0) as f32, ease: "linear".into() },
            ];
        }
    }
    vec![Key { t: KeyTime::Ms(0.0), v: 0.0, ease: "linear".into() }]
}

pub fn hex_rgb(h: &str) -> [f32; 3] {
    let h = h.trim_start_matches('#');
    let c = |i: usize| u8::from_str_radix(h.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0) as f32;
    [c(0), c(2), c(4)]
}

pub fn parse_col_keys(v: &Value) -> ColKeys {
    v.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|k| {
                    let k = k.as_array()?;
                    Some(Key {
                        t: key_time(k.first()?),
                        v: hex_rgb(k.get(1)?.as_str()?),
                        ease: k.get(2).and_then(|e| e.as_str()).unwrap_or("linear").to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn sorted<V: Copy>(keys: &[Key<V>], total: f32) -> Vec<(f32, V, &str)> {
    let mut p: Vec<(f32, V, &str)> = keys.iter().map(|k| (k.t.at(total), k.v, k.ease.as_str())).collect();
    p.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    p
}

pub fn eval(keys: &[Key<f32>], tau: f32, total: f32) -> f32 {
    let pts = sorted(keys, total);
    if pts.is_empty() {
        return 0.0;
    }
    if tau <= pts[0].0 {
        return pts[0].1;
    }
    for w in pts.windows(2) {
        let ((t0, v0, e0), (t1, v1, _)) = (w[0], w[1]);
        if tau < t1 {
            let u = if t1 > t0 { (tau - t0) / (t1 - t0) } else { 1.0 };
            return v0 + (v1 - v0) * ease(e0, u);
        }
    }
    pts[pts.len() - 1].1
}

pub fn eval_color(keys: &[Key<[f32; 3]>], tau: f32, total: f32) -> [f32; 3] {
    let pts = sorted(keys, total);
    if pts.is_empty() {
        return [255.0; 3];
    }
    if tau <= pts[0].0 {
        return pts[0].1;
    }
    for w in pts.windows(2) {
        let ((t0, c0, e0), (t1, c1, _)) = (w[0], w[1]);
        if tau < t1 {
            let u = ease(e0, if t1 > t0 { (tau - t0) / (t1 - t0) } else { 1.0 });
            return [c0[0] + (c1[0] - c0[0]) * u, c0[1] + (c1[1] - c0[1]) * u, c0[2] + (c1[2] - c0[2]) * u];
        }
    }
    pts[pts.len() - 1].1
}

/// Largest finite keyframe time, as `design_length` needs.
pub fn max_time<V>(keys: &[Key<V>]) -> f32 {
    keys.iter().filter_map(|k| if let KeyTime::Ms(v) = k.t { Some(v) } else { None }).fold(0.0, f32::max)
}
