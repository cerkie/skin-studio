//! Editor state: the settings as a JSON value (same shape as the Python tool's files), edit operations on it,
//! history on disk, and the models the UI shows.
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use engine::presets;

pub const PROPS: [(&str, &str); 7] = [
    ("x", "Position (0-1)"),
    ("y", "Y offset (px)"),
    ("sx", "Scale X (width)"),
    ("sy", "Scale Y (height)"),
    ("rot", "Rotation (deg)"),
    ("alpha", "Opacity"),
    ("blur", "Blur (px)"),
];

/// Files saved by older versions of the Python tool name sources that no longer exist.
pub fn migrate(mut s: Value) -> Value {
    let mut merged = presets::base_settings();
    if let (Some(m), Some(o)) = (merged.as_object_mut(), s.as_object_mut()) {
        for (k, v) in o.iter() {
            m.insert(k.clone(), v.clone());
        }
    }
    s = merged;
    if let Some(p) = s["preset"].as_str().map(String::from) {
        let newp = match p.as_str() {
            "Thin line" | "Thick line" | "Soft glow line" | "Dashed line" | "Dotted line" | "Double line" | "Tapered line" | "Fade-edge line" | "Comet" => Some(engine::settings::SEAMLESS),
            "Arrow chain" => Some("Chevron (bold)"),
            "Seamless line (CS-matched)" => Some(engine::settings::SEAMLESS),
            "Dot chain" => Some("Dot"),
            _ => None,
        };
        if let Some(n) = newp {
            s["preset"] = json!(n);
        }
    }
    s
}

pub fn fmt_num(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => {
            let f = n.as_f64().unwrap_or(0.0);
            if (f - f.round()).abs() < 1e-9 {
                format!("{}", f.round() as i64)
            } else {
                let s = format!("{:.4}", f);
                s.trim_end_matches('0').trim_end_matches('.').to_string()
            }
        }
        Value::Bool(b) => (if *b { "1" } else { "0" }).to_string(),
        _ => String::new(),
    }
}

/// Set one top-level setting from the text a field produced, keeping the type of the default.
pub fn set_field(s: &mut Value, id: &str, text: &str) -> bool {
    let def = presets::base_settings();
    let Some(d) = def.get(id) else { return false };
    let new = match d {
        Value::Bool(_) => json!(text == "1"),
        Value::String(_) => json!(text),
        Value::Number(n) => {
            let Ok(f) = text.trim().parse::<f64>() else { return false };
            if n.is_i64() {
                json!(f.round() as i64)
            } else {
                json!(f)
            }
        }
        _ => return false,
    };
    s[id] = new;
    true
}

fn parse_time(t: &str) -> Option<Value> {
    let t = t.trim();
    if t.eq_ignore_ascii_case("end") {
        return Some(json!("end"));
    }
    t.parse::<f64>().ok().map(|v| json!(v))
}

fn time_key(v: &Value) -> f64 {
    v.as_f64().unwrap_or(f64::MAX)
}

/// Keys of a motion property, stored back in the list form.
pub fn prop_keys(s: &mut Value, id: &str) -> Vec<Value> {
    let keys = presets::keys_of(&s["props"][id]);
    s["props"][id] = Value::Array(keys.clone());
    keys
}

fn sort_keys(keys: &mut [Value]) {
    keys.sort_by(|a, b| time_key(&a[0]).partial_cmp(&time_key(&b[0])).unwrap_or(std::cmp::Ordering::Equal));
}

pub fn edit_key(s: &mut Value, id: &str, idx: usize, field: i32, text: &str) {
    let mut keys = prop_keys(s, id);
    let Some(k) = keys.get_mut(idx) else { return };
    match field {
        0 => {
            if let Some(t) = parse_time(text) {
                k[0] = t;
            }
        }
        1 => {
            if let Ok(v) = text.trim().parse::<f64>() {
                k[1] = json!(v);
            }
        }
        _ => k[2] = json!(text),
    }
    s["props"][id] = Value::Array(keys);
}

fn color_list(s: &Value) -> Vec<Value> {
    presets::color_keys_of(s)
}

pub fn edit_color(s: &mut Value, idx: usize, field: i32, text: &str) {
    let mut keys = color_list(s);
    let Some(k) = keys.get_mut(idx) else { return };
    match field {
        0 => {
            if let Some(t) = parse_time(text) {
                k[0] = t;
            }
        }
        1 => {
            let t = text.trim();
            let t = if t.starts_with('#') { t.to_string() } else { format!("#{t}") };
            k[1] = json!(t);
        }
        _ => k[2] = json!(text),
    }
    s["color_keys"] = Value::Array(keys);
}

pub fn add_color(s: &mut Value) {
    let mut keys = color_list(s);
    let last_t = keys.iter().filter_map(|k| k[0].as_f64()).fold(0.0, f64::max);
    let last_c = keys.last().map(|k| k[1].clone()).unwrap_or(json!("#ffffff"));
    keys.push(json!([last_t + 100.0, last_c, "linear"]));
    sort_keys(&mut keys);
    s["color_keys"] = Value::Array(keys);
}

pub fn remove_color(s: &mut Value, idx: usize) {
    let mut keys = color_list(s);
    if keys.len() > 1 && idx < keys.len() {
        keys.remove(idx);
    }
    s["color_keys"] = Value::Array(keys);
}

pub fn hex_to_rgb(h: &str) -> Option<[u8; 3]> {
    let h = h.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    Some([u8::from_str_radix(&h[0..2], 16).ok()?, u8::from_str_radix(&h[2..4], 16).ok()?, u8::from_str_radix(&h[4..6], 16).ok()?])
}

/// Switching the source also switches to its natural canvas.
pub fn set_source(s: &mut Value, name: &str) {
    s["preset"] = json!(name);
    if name == engine::settings::SEAMLESS {
        s["width"] = json!(128);
        s["height"] = json!(32);
    } else if name != "Custom image..." {
        let (w, h) = engine::sources::default_canvas(name);
        s["width"] = json!(w);
        s["height"] = json!(h);
    }
}

// ---------------------------------------------------------------------------------------------------- history
#[derive(Clone, Debug)]
pub struct HistoryEntry {
    pub id: String,
    pub name: String,
    pub when: String,
    pub settings: Value,
}

pub fn history_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("history")
}

pub fn history_list(config_dir: &Path) -> Vec<HistoryEntry> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(history_dir(config_dir)) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
        let settings = v["settings"].clone();
        out.push(HistoryEntry {
            id: p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
            name: v["name"].as_str().unwrap_or("settings").to_string(),
            when: v["saved"].as_str().unwrap_or("").to_string(),
            settings,
        });
    }
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out
}

pub fn history_add(config_dir: &Path, settings: &Value, name: Option<String>) {
    let dir = history_dir(config_dir);
    let _ = std::fs::create_dir_all(&dir);
    let now = chrono::Local::now();
    let id = now.format("%Y%m%d-%H%M%S-%3f").to_string();
    let entry = json!({
        "name": name.unwrap_or_else(|| presets::summary(settings)),
        "saved": now.format("%Y-%m-%d %H:%M").to_string(),
        "settings": settings,
    });
    let _ = std::fs::write(dir.join(format!("{id}.json")), serde_json::to_string_pretty(&entry).unwrap_or_default());
    // keep the newest 200
    let all = history_list(config_dir);
    for e in all.iter().skip(200) {
        let _ = std::fs::remove_file(dir.join(format!("{}.json", e.id)));
    }
}

pub fn history_delete(config_dir: &Path, id: &str) {
    let _ = std::fs::remove_file(history_dir(config_dir).join(format!("{id}.json")));
}
