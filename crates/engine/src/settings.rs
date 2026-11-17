//! Animation settings, read from the JSON the Python tool writes (`get_settings()` / "Save settings").
use serde_json::Value;

use crate::keys::{self, ColKeys, NumKeys};

pub const PROPS: [&str; 7] = ["x", "y", "sx", "sy", "rot", "alpha", "blur"];

#[derive(Clone, Debug)]
pub struct Settings {
    pub preset: String,
    pub image: String,
    pub fps: f32,
    pub speed: f32,
    pub duration: f32,
    pub lead: f32,
    /// Moves the keyframe timeline: positive starts the animation that far in, negative holds its first pose first.
    pub shift: f32,
    pub tail: f32,
    pub disappear: f32,
    pub fade_in: f32,
    pub fade_out: f32,
    pub width: usize,
    pub height: usize,
    pub copies: usize,
    pub pingpong: bool,
    pub color_on: bool,
    pub color_keys: ColKeys,
    pub color_strength: f32,
    pub no_loop: bool,
    pub clamp_mode: bool,
    pub sl_cs: f32,
    pub sl_ar: f32,
    pub sl_soft: f32,
    pub sl_tight: f32,
    pub sl_thick: f32,
    pub sl_alpha: f32,
    pub sl_overlap: f32,
    pub outline_on: bool,
    pub outline_w: f32,
    pub outline_color: [f32; 3],
    pub outline_opacity: f32,
    pub shadow_on: bool,
    pub shadow_dx: f32,
    pub shadow_dy: f32,
    pub shadow_blur: f32,
    pub shadow_color: [f32; 3],
    pub shadow_opacity: f32,
    pub deband_on: bool,
    pub deband_radius: f32,
    pub deband_thr_rgb: f32,
    pub deband_thr_a: f32,
    pub deband_iter: usize,
    pub deband_grain: f32,
    /// Properties in the order of `PROPS`.
    pub props: Vec<NumKeys>,
}

/// No animation needs more than the 1000 fps the preview targets; higher values only cost memory.
pub const MAX_FPS: f32 = 1000.0;

pub const SEAMLESS: &str = "Seamless line";
/// The name older settings files use for the seamless line.
const SEAMLESS_OLD: &str = "Seamless line (CS-matched)";

fn f(v: &Value, k: &str, d: f32) -> f32 {
    v.get(k).and_then(|x| x.as_f64()).map(|x| x as f32).unwrap_or(d)
}
fn b(v: &Value, k: &str, d: bool) -> bool {
    v.get(k).and_then(|x| x.as_bool()).unwrap_or(d)
}
fn s(v: &Value, k: &str, d: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or(d).to_string()
}

impl Settings {
    pub fn from_json_str(text: &str) -> Result<Settings, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        Ok(Settings::from_json(&v))
    }

    pub fn from_json(v: &Value) -> Settings {
        let props_v = v.get("props").cloned().unwrap_or(Value::Null);
        let default_prop = |name: &str| -> f32 {
            match name {
                "x" => 0.5,
                "sx" | "sy" | "alpha" => 1.0,
                _ => 0.0,
            }
        };
        let props = PROPS
            .iter()
            .map(|n| match props_v.get(*n) {
                Some(p) => keys::parse_num_keys(p),
                None => {
                    let d = default_prop(n);
                    vec![
                        keys::Key { t: keys::KeyTime::Ms(0.0), v: d, ease: "linear".into() },
                        keys::Key { t: keys::KeyTime::End, v: d, ease: "linear".into() },
                    ]
                }
            })
            .collect();
        let mut color_keys = v.get("color_keys").map(keys::parse_col_keys).unwrap_or_default();
        if color_keys.is_empty() {
            let c0 = keys::hex_rgb(&s(v, "color_start", "#00ff26"));
            let c1 = keys::hex_rgb(&s(v, "color_end", "#ffffff"));
            color_keys = vec![
                keys::Key { t: keys::KeyTime::Ms(0.0), v: c0, ease: s(v, "color_ease", "linear") },
                keys::Key { t: keys::KeyTime::End, v: c1, ease: "linear".into() },
            ];
        }
        Settings {
            preset: { let p = s(v, "preset", "Chevron (bold)"); if p == SEAMLESS_OLD { SEAMLESS.to_string() } else { p } },
            image: s(v, "image", ""),
            fps: f(v, "fps", 60.0).clamp(1.0, MAX_FPS),
            speed: f(v, "speed", 1.0),
            duration: f(v, "duration", 500.0),
            lead: f(v, "lead", 0.0),
            shift: f(v, "shift", 0.0),
            tail: f(v, "tail", 0.0),
            disappear: f(v, "disappear", 0.0),
            fade_in: f(v, "fade_in", 0.0),
            fade_out: f(v, "fade_out", 0.0),
            width: (f(v, "width", 100.0) as usize).clamp(1, 2048),
            height: (f(v, "height", 50.0) as usize).clamp(1, 2048),
            copies: (f(v, "copies", 1.0) as usize).max(1),
            pingpong: b(v, "pingpong", false),
            color_on: b(v, "color_on", false),
            color_keys,
            color_strength: f(v, "color_strength", 1.0),
            no_loop: b(v, "no_loop", true),
            clamp_mode: s(v, "window_mode", "shift") == "clamp",
            sl_cs: f(v, "sl_cs", 4.0),
            sl_ar: f(v, "sl_ar", 9.0),
            sl_soft: f(v, "sl_soft", 160.0),
            sl_tight: f(v, "sl_tight", 0.0),
            sl_thick: f(v, "sl_thick", 2.0),
            sl_alpha: f(v, "sl_alpha", 0.9),
            sl_overlap: f(v, "sl_overlap", 1.0),
            outline_on: b(v, "outline_on", false),
            outline_w: f(v, "outline_w", 2.0),
            outline_color: keys::hex_rgb(&s(v, "outline_color", "#000000")),
            outline_opacity: f(v, "outline_opacity", 1.0),
            shadow_on: b(v, "shadow_on", false),
            shadow_dx: f(v, "shadow_dx", 2.0),
            shadow_dy: f(v, "shadow_dy", 2.0),
            shadow_blur: f(v, "shadow_blur", 3.0),
            shadow_color: keys::hex_rgb(&s(v, "shadow_color", "#000000")),
            shadow_opacity: f(v, "shadow_opacity", 0.6),
            deband_on: b(v, "deband_on", false),
            deband_radius: f(v, "deband_radius", 8.0),
            deband_thr_rgb: f(v, "deband_thr_rgb", 3.0),
            deband_thr_a: f(v, "deband_thr_a", 3.0),
            deband_iter: f(v, "deband_iter", 2.0) as usize,
            deband_grain: f(v, "deband_grain", 0.0),
            props,
        }
    }

    pub fn prop(&self, name: &str) -> &NumKeys {
        &self.props[PROPS.iter().position(|p| *p == name).expect("known property")]
    }

    pub fn is_seamless(&self) -> bool {
        self.preset == SEAMLESS
    }

    /// Length of the animation as designed, in ms: the last keyframe time, at least 500 ms.
    pub fn design_length(&self) -> f32 {
        let m = self.props.iter().map(|p| keys::max_time(p)).fold(keys::max_time(&self.color_keys), f32::max);
        m.max(500.0)
    }

    /// How long the animation really runs: design length divided by speed.
    pub fn real_length(&self) -> f32 {
        self.design_length() / self.speed.max(0.05)
    }
}
