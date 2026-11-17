//! Effect presets and default settings, as JSON values in the same shape the Python tool saves, so a settings file
//! is the editor's single source of truth.
use std::sync::OnceLock;

use serde_json::{json, Value};

fn data() -> &'static Value {
    static D: OnceLock<Value> = OnceLock::new();
    D.get_or_init(|| serde_json::from_str(include_str!("effects.json")).expect("effects.json"))
}

pub fn effect_names() -> Vec<String> {
    data()["order"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
}

pub fn easings() -> Vec<String> {
    data()["easings"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
}

/// The settings a new session starts from: the baked-in default animation.
pub fn default_settings() -> Value {
    data()["startup"].clone()
}

/// Neutral settings (a static line): what missing keys and effect presets are built on.
pub fn base_settings() -> Value {
    data()["default"].clone()
}

/// Sources the editor offers, in order: drawn presets, the seamless line, a custom image.
pub fn source_names() -> Vec<String> {
    let mut v: Vec<String> = crate::sources::OFFERED.iter().map(|s| s.to_string()).collect();
    v.push(crate::settings::SEAMLESS.to_string());
    v.push("Custom image...".to_string());
    v
}

/// Apply an effect preset on top of the property defaults (the Python `apply_effect`).
pub fn apply_effect(s: &mut Value, name: &str) {
    let Some(e) = data()["effects"].get(name) else { return };
    let d = base_settings();
    let obj = s.as_object_mut().expect("settings object");
    // every preset brings its own timing, so nothing is kept from the one before
    for k in ["speed", "shift", "lead", "tail", "disappear", "duration", "copies", "no_loop"] {
        if let Some(v) = d.get(k) {
            obj.insert(k.into(), v.clone());
        }
    }
    let mut props = d["props"].clone();
    if let Some(p) = e["props"].as_object() {
        for (k, v) in p {
            props[k] = v.clone();
        }
    }
    obj.insert("props".into(), props);
    obj.insert("fade_in".into(), e.get("fade_in").cloned().unwrap_or(json!(0)));
    obj.insert("fade_out".into(), e.get("fade_out").cloned().unwrap_or(json!(0)));
    obj.insert("pingpong".into(), e.get("pingpong").cloned().unwrap_or(json!(false)));
    obj.insert("window_mode".into(), json!("shift"));
    if let Some(set) = e.get("set").and_then(|v| v.as_object()) {
        for (k, v) in set {
            obj.insert(k.clone(), v.clone());
        }
    }
    obj.remove("color_keys");
    if let Some(ck) = e.get("color_keys") {
        obj.insert("color_on".into(), json!(true));
        obj.insert("color_keys".into(), ck.clone());
    } else if let Some(c) = e.get("color").and_then(|c| c.as_array()) {
        obj.insert("color_on".into(), json!(true));
        obj.insert("color_start".into(), c[0].clone());
        obj.insert("color_end".into(), c[1].clone());
    } else {
        obj.insert("color_on".into(), json!(false));
    }
}

/// Make a property a list of `[time, value, easing]` keys whatever form it was stored in.
pub fn keys_of(v: &Value) -> Vec<Value> {
    match v.as_array() {
        Some(a) if a.first().is_some_and(|k| k.is_array()) => a.clone(),
        Some(a) if a.len() >= 2 => vec![json!([0, a[0], a.get(2).cloned().unwrap_or(json!("linear"))]), json!(["end", a[1], "linear"])],
        _ => vec![json!([0, 0, "linear"])],
    }
}

/// Color keys of the settings, from `color_keys` or the older start/end pair.
pub fn color_keys_of(s: &Value) -> Vec<Value> {
    if let Some(a) = s.get("color_keys").and_then(|v| v.as_array()).filter(|a| !a.is_empty()) {
        return a.clone();
    }
    vec![
        json!([0, s["color_start"].clone(), s.get("color_ease").cloned().unwrap_or(json!("linear"))]),
        json!(["end", s["color_end"].clone(), "linear"]),
    ]
}

/// A one-line description of settings, used as the default name of a history entry.
pub fn summary(s: &Value) -> String {
    let src = s["preset"].as_str().unwrap_or("?");
    let src = if src == crate::settings::SEAMLESS { "Seamless line" } else { src };
    let mut bits = vec![src.to_string()];
    let f = |k: &str| s[k].as_f64().unwrap_or(0.0);
    if (s["speed"].as_f64().unwrap_or(1.0) - 1.0).abs() > 1e-9 {
        bits.push(format!("speed x{}", f("speed")));
    }
    if f("lead") != 0.0 || f("disappear") != 0.0 {
        let d = if f("disappear") != 0.0 { format!("{}", f("disappear")) } else { "end".into() };
        bits.push(format!("{}-{d}ms", f("lead")));
    }
    bits.push(match s["export"].as_str().unwrap_or("1x") {
        "2x" => "@2x",
        "both" => "1x+@2x",
        _ => "1x",
    }
    .to_string());
    bits.join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effects_load_and_apply() {
        assert!(effect_names().len() >= 15);
        let mut s = default_settings();
        apply_effect(&mut s, "Sonar ring (pings at the hit moment)");
        assert_eq!(s["preset"], "Ring");
        assert_eq!(s["window_mode"], "clamp");
        let parsed = crate::settings::Settings::from_json(&s);
        assert!(parsed.real_length() > 1000.0);
        assert!(!summary(&s).is_empty());
    }
}
