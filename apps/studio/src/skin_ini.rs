//! Full osu! `skin.ini` round-trip model and editor UI.
//! Supports all official `[General]`, `[Colours]`, `[Fonts]`, `[CatchTheBeat]`, and `[Mania]` settings
//! (preserving the exact osu! `[Colours]` header, `CursorCentre` spelling, comments, and `[Mania]` blocks).
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use egui::{Pos2, Rect, Ui, Vec2};

use crate::theme::{self as th, Kind};

/// Versions recognized by osu! for `[General] Version:`.
pub const VERSIONS: &[&str] = &["1.0", "2.0", "2.1", "2.2", "2.3", "2.4", "2.5", "2.6", "2.7", "latest"];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkinIniDoc {
    pub loaded: bool,
    pub dirty: bool,
    /// `[General]`
    pub name: String,
    pub author: String,
    pub version: String,
    pub animation_framerate: String,
    pub allow_slider_ball_tint: bool,
    pub combo_burst_random: bool,
    pub cursor_centre: bool,
    pub cursor_expand: bool,
    pub cursor_rotate: bool,
    pub cursor_trail_rotate: bool,
    pub custom_combo_burst_sounds: String,
    pub hit_circle_overlay_above_number: bool,
    pub layered_hit_sounds: bool,
    pub slider_ball_flip: bool,
    pub spinner_fade_playfield: bool,
    pub spinner_frequency_modulate: bool,
    pub spinner_no_blink: bool,
    /// `[Colours]` (always written with osu!'s required `[Colours]` header)
    pub combos: Vec<[u8; 3]>,
    pub slider_ball: Option<[u8; 3]>,
    pub slider_border: Option<[u8; 3]>,
    pub slider_track_override: Option<[u8; 3]>,
    pub song_select_active_text: Option<[u8; 3]>,
    pub song_select_inactive_text: Option<[u8; 3]>,
    pub input_overlay_text: Option<[u8; 3]>,
    pub menu_glow: Option<[u8; 3]>,
    pub spinner_background: Option<[u8; 3]>,
    pub star_break_additive: Option<[u8; 3]>,
    /// `[Fonts]`
    pub hit_circle_prefix: String,
    pub hit_circle_overlap: String,
    pub score_prefix: String,
    pub score_overlap: String,
    pub combo_prefix: String,
    pub combo_overlap: String,
    /// `[CatchTheBeat]`
    pub hyper_dash: Option<[u8; 3]>,
    pub hyper_dash_fruit: Option<[u8; 3]>,
    pub hyper_dash_after_image: Option<[u8; 3]>,
    /// Extra unknown keys per section (preserved verbatim on save).
    pub extra: BTreeMap<String, Vec<(String, String)>>,
    /// Verbatim `[Mania]` sections (each keycount is its own `[Mania]` block in osu!).
    pub mania_raw: String,
    /// Persistent text buffers for hex color fields so intermediate keystrokes don't reset each frame.
    pub bufs: HashMap<String, String>,
}

fn parse_rgb(s: &str) -> Option<[u8; 3]> {
    let parts: Vec<u8> = s.split(',').filter_map(|p| p.trim().parse::<u8>().ok()).collect();
    (parts.len() >= 3).then(|| [parts[0], parts[1], parts[2]])
}

fn fmt_rgb(c: [u8; 3]) -> String {
    format!("{},{},{}", c[0], c[1], c[2])
}

fn parse_bool(s: &str, default: bool) -> bool {
    match s.trim() {
        "1" | "true" | "True" => true,
        "0" | "false" | "False" => false,
        _ => default,
    }
}

impl SkinIniDoc {
    pub fn defaults() -> Self {
        SkinIniDoc {
            loaded: true,
            dirty: false,
            name: "Unknown".into(),
            author: String::new(),
            version: "2.7".into(),
            animation_framerate: "-1".into(),
            allow_slider_ball_tint: false,
            combo_burst_random: false,
            cursor_centre: true,
            cursor_expand: true,
            cursor_rotate: true,
            cursor_trail_rotate: true,
            custom_combo_burst_sounds: String::new(),
            hit_circle_overlay_above_number: true,
            layered_hit_sounds: true,
            slider_ball_flip: true,
            spinner_fade_playfield: false,
            spinner_frequency_modulate: true,
            spinner_no_blink: false,
            combos: vec![[255, 192, 0], [0, 202, 0], [18, 124, 255], [242, 24, 57]],
            slider_ball: None,
            slider_border: None,
            slider_track_override: None,
            song_select_active_text: None,
            song_select_inactive_text: None,
            input_overlay_text: None,
            menu_glow: None,
            spinner_background: None,
            star_break_additive: None,
            hit_circle_prefix: "default".into(),
            hit_circle_overlap: "-2".into(),
            score_prefix: "score".into(),
            score_overlap: "0".into(),
            combo_prefix: "score".into(),
            combo_overlap: "0".into(),
            hyper_dash: None,
            hyper_dash_fruit: None,
            hyper_dash_after_image: None,
            extra: BTreeMap::new(),
            mania_raw: String::new(),
            bufs: HashMap::new(),
        }
    }

    pub fn load_from_dir(dir: &Path) -> Self {
        let p = dir.join("skin.ini");
        match std::fs::read_to_string(&p) {
            Ok(text) => Self::parse(&text),
            Err(_) => Self::defaults(),
        }
    }

    pub fn parse(text: &str) -> Self {
        let mut doc = Self::defaults();
        doc.combos.clear();
        let mut combos_map: BTreeMap<usize, [u8; 3]> = BTreeMap::new();
        let mut sec = String::new();
        let mut in_mania = false;
        let mut mania_lines: Vec<String> = Vec::new();
        for raw_line in text.lines() {
            let line = raw_line.trim_start_matches('\u{feff}').trim();
            if line.starts_with('[') && line.ends_with(']') {
                let name = &line[1..line.len() - 1];
                if name.eq_ignore_ascii_case("Mania") {
                    in_mania = true;
                    if !mania_lines.is_empty() {
                        mania_lines.push(String::new());
                    }
                    mania_lines.push("[Mania]".into());
                } else {
                    in_mania = false;
                    sec = name.to_lowercase();
                }
                continue;
            }
            if in_mania {
                mania_lines.push(raw_line.to_string());
                continue;
            }
            if line.is_empty() || line.starts_with("//") {
                continue;
            }
            let Some((k_raw, v_raw)) = line.split_once(':') else { continue };
            let k = k_raw.trim();
            let v = v_raw.split("//").next().unwrap_or("").trim();
            match sec.as_str() {
                "general" => match k.to_lowercase().as_str() {
                    "name" => doc.name = v.into(),
                    "author" => doc.author = v.into(),
                    "version" => doc.version = v.into(),
                    "animationframerate" => doc.animation_framerate = v.into(),
                    "allowsliderballtint" => doc.allow_slider_ball_tint = parse_bool(v, false),
                    "comboburstrandom" => doc.combo_burst_random = parse_bool(v, false),
                    "cursorcentre" => doc.cursor_centre = parse_bool(v, true),
                    "cursorexpand" => doc.cursor_expand = parse_bool(v, true),
                    "cursorrotate" => doc.cursor_rotate = parse_bool(v, true),
                    "cursortrailrotate" => doc.cursor_trail_rotate = parse_bool(v, true),
                    "customcomboburstsounds" => doc.custom_combo_burst_sounds = v.into(),
                    "hitcircleoverlayabovenumber" | "hitcircleoverlayabovenumer" => doc.hit_circle_overlay_above_number = parse_bool(v, true),
                    "layeredhitsounds" => doc.layered_hit_sounds = parse_bool(v, true),
                    "sliderballflip" => doc.slider_ball_flip = parse_bool(v, true),
                    "spinnerfadeplayfield" => doc.spinner_fade_playfield = parse_bool(v, false),
                    "spinnerfrequencymodulate" => doc.spinner_frequency_modulate = parse_bool(v, true),
                    "spinnernoblink" => doc.spinner_no_blink = parse_bool(v, false),
                    _ => doc.extra.entry("General".into()).or_default().push((k.into(), v.into())),
                },
                "colours" => {
                    let kl = k.to_lowercase();
                    if let Some(num) = kl.strip_prefix("combo").and_then(|n| n.parse::<usize>().ok()) {
                        if (1..=8).contains(&num) {
                            if let Some(rgb) = parse_rgb(v) {
                                combos_map.insert(num, rgb);
                            }
                        }
                    } else {
                        match kl.as_str() {
                            "sliderball" => doc.slider_ball = parse_rgb(v),
                            "sliderborder" => doc.slider_border = parse_rgb(v),
                            "slidertrackoverride" => doc.slider_track_override = parse_rgb(v),
                            "songselectactivetext" => doc.song_select_active_text = parse_rgb(v),
                            "songselectinactivetext" => doc.song_select_inactive_text = parse_rgb(v),
                            "inputoverlaytext" => doc.input_overlay_text = parse_rgb(v),
                            "menuglow" => doc.menu_glow = parse_rgb(v),
                            "spinnerbackground" => doc.spinner_background = parse_rgb(v),
                            "starbreakadditive" => doc.star_break_additive = parse_rgb(v),
                            _ => doc.extra.entry("Colours".into()).or_default().push((k.into(), v.into())),
                        }
                    }
                }
                "fonts" => match k.to_lowercase().as_str() {
                    "hitcircleprefix" => doc.hit_circle_prefix = v.into(),
                    "hitcircleoverlap" => doc.hit_circle_overlap = v.into(),
                    "scoreprefix" => doc.score_prefix = v.into(),
                    "scoreoverlap" => doc.score_overlap = v.into(),
                    "comboprefix" => doc.combo_prefix = v.into(),
                    "combooverlap" => doc.combo_overlap = v.into(),
                    _ => doc.extra.entry("Fonts".into()).or_default().push((k.into(), v.into())),
                },
                "catchthebeat" => match k.to_lowercase().as_str() {
                    "hyperdash" => doc.hyper_dash = parse_rgb(v),
                    "hyperdashfruit" => doc.hyper_dash_fruit = parse_rgb(v),
                    "hyperdashafterimage" => doc.hyper_dash_after_image = parse_rgb(v),
                    _ => doc.extra.entry("CatchTheBeat".into()).or_default().push((k.into(), v.into())),
                },
                _ if !sec.is_empty() => {
                    doc.extra.entry(sec.clone()).or_default().push((k.into(), v.into()));
                }
                _ => {}
            }
        }
        doc.combos = combos_map.into_values().collect();
        doc.mania_raw = mania_lines.join("\n").trim().to_string();
        doc.loaded = true;
        doc.dirty = false;
        doc
    }

    pub fn serialize(&self) -> String {
        let b = |v: bool| if v { "1" } else { "0" };
        let mut out = String::new();
        out.push_str("[General]\n");
        out.push_str(&format!("Name: {}\n", self.name));
        out.push_str(&format!("Author: {}\n", self.author));
        out.push_str(&format!("Version: {}\n", if self.version.is_empty() { "2.7" } else { &self.version }));
        if !self.animation_framerate.trim().is_empty() {
            out.push_str(&format!("AnimationFramerate: {}\n", self.animation_framerate.trim()));
        }
        out.push_str(&format!("AllowSliderBallTint: {}\n", b(self.allow_slider_ball_tint)));
        out.push_str(&format!("ComboBurstRandom: {}\n", b(self.combo_burst_random)));
        out.push_str(&format!("CursorCentre: {}\n", b(self.cursor_centre)));
        out.push_str(&format!("CursorExpand: {}\n", b(self.cursor_expand)));
        out.push_str(&format!("CursorRotate: {}\n", b(self.cursor_rotate)));
        out.push_str(&format!("CursorTrailRotate: {}\n", b(self.cursor_trail_rotate)));
        if !self.custom_combo_burst_sounds.trim().is_empty() {
            out.push_str(&format!("CustomComboBurstSounds: {}\n", self.custom_combo_burst_sounds.trim()));
        }
        out.push_str(&format!("HitCircleOverlayAboveNumber: {}\n", b(self.hit_circle_overlay_above_number)));
        out.push_str(&format!("LayeredHitSounds: {}\n", b(self.layered_hit_sounds)));
        out.push_str(&format!("SliderBallFlip: {}\n", b(self.slider_ball_flip)));
        out.push_str(&format!("SpinnerFadePlayfield: {}\n", b(self.spinner_fade_playfield)));
        out.push_str(&format!("SpinnerFrequencyModulate: {}\n", b(self.spinner_frequency_modulate)));
        out.push_str(&format!("SpinnerNoBlink: {}\n", b(self.spinner_no_blink)));
        if let Some(xs) = self.extra.get("General") {
            for (k, v) in xs {
                out.push_str(&format!("{k}: {v}\n"));
            }
        }

        out.push_str("\n[Colours]\n");
        for (i, c) in self.combos.iter().take(8).enumerate() {
            out.push_str(&format!("Combo{}: {}\n", i + 1, fmt_rgb(*c)));
        }
        for (k, v) in [
            ("SliderBall", self.slider_ball),
            ("SliderBorder", self.slider_border),
            ("SliderTrackOverride", self.slider_track_override),
            ("SongSelectActiveText", self.song_select_active_text),
            ("SongSelectInactiveText", self.song_select_inactive_text),
            ("InputOverlayText", self.input_overlay_text),
            ("MenuGlow", self.menu_glow),
            ("SpinnerBackground", self.spinner_background),
            ("StarBreakAdditive", self.star_break_additive),
        ] {
            if let Some(c) = v {
                out.push_str(&format!("{k}: {}\n", fmt_rgb(c)));
            }
        }
        if let Some(xs) = self.extra.get("Colours") {
            for (k, v) in xs {
                out.push_str(&format!("{k}: {v}\n"));
            }
        }

        out.push_str("\n[Fonts]\n");
        out.push_str(&format!("HitCirclePrefix: {}\n", self.hit_circle_prefix));
        out.push_str(&format!("HitCircleOverlap: {}\n", self.hit_circle_overlap));
        out.push_str(&format!("ScorePrefix: {}\n", self.score_prefix));
        out.push_str(&format!("ScoreOverlap: {}\n", self.score_overlap));
        out.push_str(&format!("ComboPrefix: {}\n", self.combo_prefix));
        out.push_str(&format!("ComboOverlap: {}\n", self.combo_overlap));
        if let Some(xs) = self.extra.get("Fonts") {
            for (k, v) in xs {
                out.push_str(&format!("{k}: {v}\n"));
            }
        }

        let has_ctb = self.hyper_dash.is_some() || self.hyper_dash_fruit.is_some() || self.hyper_dash_after_image.is_some() || self.extra.contains_key("CatchTheBeat");
        if has_ctb {
            out.push_str("\n[CatchTheBeat]\n");
            for (k, v) in [
                ("HyperDash", self.hyper_dash),
                ("HyperDashFruit", self.hyper_dash_fruit),
                ("HyperDashAfterImage", self.hyper_dash_after_image),
            ] {
                if let Some(c) = v {
                    out.push_str(&format!("{k}: {}\n", fmt_rgb(c)));
                }
            }
            if let Some(xs) = self.extra.get("CatchTheBeat") {
                for (k, v) in xs {
                    out.push_str(&format!("{k}: {v}\n"));
                }
            }
        }

        if !self.mania_raw.trim().is_empty() {
            out.push('\n');
            out.push_str(self.mania_raw.trim());
            out.push('\n');
        }
        out
    }

    pub fn save_to_dir(&mut self, dir: &Path) -> Result<(), String> {
        let p = dir.join("skin.ini");
        if p.is_file() {
            let backup = dir.join(".skin-studio-backup");
            let kept = backup.join("skin.ini");
            if !kept.exists() {
                std::fs::create_dir_all(&backup).map_err(|e| e.to_string())?;
                std::fs::copy(&p, &kept).map_err(|e| e.to_string())?;
            }
        }
        std::fs::write(&p, self.serialize()).map_err(|e| e.to_string())?;
        self.dirty = false;
        Ok(())
    }
}

fn opt_color_row(ui: &mut Ui, bufs: &mut HashMap<String, String>, id: &str, label: &str, val: &mut Option<[u8; 3]>, default: [u8; 3]) -> bool {
    let mut ch = false;
    let mut on = val.is_some();
    if th::switch_row(ui, &format!("ini_on_{id}"), label, &mut on) {
        *val = if on { Some(default) } else { None };
        bufs.remove(id);
        ch = true;
    }
    if let Some(rgb) = val.as_mut() {
        let cur_hex = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
        let buf = bufs.entry(id.to_string()).or_insert_with(|| cur_hex.clone());
        let (resp, row_rect) = th::text_row_pad(ui, &format!("ini_c_{id}"), &format!("{label} (#rrggbb)"), buf, 34.0);
        let text = buf.clone();
        if resp.changed() {
            if let Some(c) = crate::state::hex_to_rgb(&text) {
                *rgb = c;
                ch = true;
            }
        } else if !resp.has_focus() && text != cur_hex {
            bufs.insert(id.to_string(), cur_hex);
        }
        let sw = Rect::from_center_size(Pos2::new(row_rect.right() - 22.0, row_rect.center().y), Vec2::new(24.0, 24.0));
        if let Some(c) = crate::ui::swatch_picker(ui, &format!("ini_sw_{id}"), sw, *rgb) {
            *rgb = c;
            bufs.insert(id.to_string(), format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]));
            ch = true;
        }
    }
    ch
}

/// Draw the full `skin.ini` editor page. Returns `true` if the user clicked Save.
pub fn draw_page(ui: &mut Ui, doc: &mut SkinIniDoc, has_dir: bool) -> bool {
    th::section_header(ui, "skin.ini");
    if !has_dir {
        th::note(ui, "Open a skin folder or .osk on the Pictures page to edit its skin.ini.");
        return false;
    }
    let mut save_now = false;
    if th::button_wide(ui, if doc.dirty { "Save skin.ini *" } else { "Save skin.ini" }, Kind::Accent, true).clicked() {
        save_now = true;
    }
    let mut ch = false;

    th::subsection_header(ui, "General");
    ch |= th::text_row(ui, "ini_name", "Skin name", &mut doc.name).changed();
    ch |= th::text_row(ui, "ini_author", "Author", &mut doc.author).changed();
    let vers: Vec<String> = VERSIONS.iter().map(|s| s.to_string()).collect();
    if let Some(k) = th::dropdown_row(ui, "ini_ver", "Skin version", &doc.version, &vers) {
        doc.version = vers[k].clone();
        ch = true;
    }
    ch |= th::text_row(ui, "ini_fps", "Animation framerate (-1 = 1 loop/s)", &mut doc.animation_framerate).changed();
    ch |= th::switch_row(ui, "ini_cc", "Center cursor (CursorCentre)", &mut doc.cursor_centre);
    ch |= th::switch_row(ui, "ini_ce", "Expand cursor on click", &mut doc.cursor_expand);
    ch |= th::switch_row(ui, "ini_cr", "Rotate cursor", &mut doc.cursor_rotate);
    ch |= th::switch_row(ui, "ini_ctr", "Rotate cursor trail", &mut doc.cursor_trail_rotate);
    ch |= th::switch_row(ui, "ini_hco", "Hit circle overlay above number", &mut doc.hit_circle_overlay_above_number);
    ch |= th::switch_row(ui, "ini_sbt", "Allow slider ball tint", &mut doc.allow_slider_ball_tint);
    ch |= th::switch_row(ui, "ini_sbf", "Flip slider ball on reverse", &mut doc.slider_ball_flip);
    ch |= th::switch_row(ui, "ini_lhs", "Layered hit sounds", &mut doc.layered_hit_sounds);
    ch |= th::switch_row(ui, "ini_cbr", "Random combo burst order", &mut doc.combo_burst_random);
    ch |= th::text_row(ui, "ini_cbs", "Custom combo burst sounds (e.g. 50, 100)", &mut doc.custom_combo_burst_sounds).changed();
    ch |= th::switch_row(ui, "ini_sfp", "Spinner fade playfield", &mut doc.spinner_fade_playfield);
    ch |= th::switch_row(ui, "ini_sfm", "Spinner frequency modulate", &mut doc.spinner_frequency_modulate);
    ch |= th::switch_row(ui, "ini_snb", "Spinner no blink", &mut doc.spinner_no_blink);

    th::subsection_header(ui, "Colors ([Colours])");
    let mut rm_combo = None;
    for i in 0..doc.combos.len() {
        let key = format!("cb_{i}");
        let cur_hex = format!("#{:02x}{:02x}{:02x}", doc.combos[i][0], doc.combos[i][1], doc.combos[i][2]);
        let buf = doc.bufs.entry(key.clone()).or_insert_with(|| cur_hex.clone());
        let (resp, row_rect) = th::text_row_pad(ui, &format!("ini_cb_{i}"), &format!("Combo{} (#rrggbb)", i + 1), buf, 62.0);
        let text = buf.clone();
        if resp.changed() {
            if let Some(c) = crate::state::hex_to_rgb(&text) {
                doc.combos[i] = c;
                ch = true;
            }
        } else if !resp.has_focus() && text != cur_hex {
            doc.bufs.insert(key.clone(), cur_hex);
        }
        let sw = Rect::from_center_size(Pos2::new(row_rect.right() - 48.0, row_rect.center().y), Vec2::new(24.0, 24.0));
        if let Some(c) = crate::ui::swatch_picker(ui, &format!("ini_cbsw_{i}"), sw, doc.combos[i]) {
            doc.combos[i] = c;
            doc.bufs.insert(key, format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]));
            ch = true;
        }
        let xr = Rect::from_center_size(Pos2::new(row_rect.right() - 18.0, row_rect.center().y), Vec2::new(24.0, 24.0));
        let xresp = ui.interact(xr, ui.scope_id().with(("ini_cb_rm", i)), egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Remove combo color");
        let xh = th::ease(ui, xresp.id.with("h"), xresp.hovered());
        let xc = xr.center();
        let xcol = th::lerp_c(th::pal::LIGHT3, th::pal::CONTENT1, xh);
        ui.painter().line_segment([xc + Vec2::new(-4.5, -4.5), xc + Vec2::new(4.5, 4.5)], egui::Stroke::new(1.8, xcol));
        ui.painter().line_segment([xc + Vec2::new(-4.5, 4.5), xc + Vec2::new(4.5, -4.5)], egui::Stroke::new(1.8, xcol));
        if xresp.clicked() {
            rm_combo = Some(i);
        }
    }
    if let Some(i) = rm_combo {
        doc.combos.remove(i);
        doc.bufs.retain(|k, _| !k.starts_with("cb_"));
        ch = true;
    }
    if doc.combos.len() < 8 && th::button_wide(ui, "Add combo color", Kind::Dark, true).clicked() {
        doc.combos.push([255, 255, 255]);
        ch = true;
    }
    ch |= opt_color_row(ui, &mut doc.bufs, "sb", "SliderBorder", &mut doc.slider_border, [255, 255, 255]);
    ch |= opt_color_row(ui, &mut doc.bufs, "sto", "SliderTrackOverride", &mut doc.slider_track_override, [0, 0, 0]);
    ch |= opt_color_row(ui, &mut doc.bufs, "sball", "SliderBall", &mut doc.slider_ball, [2, 170, 255]);
    ch |= opt_color_row(ui, &mut doc.bufs, "ssat", "SongSelectActiveText", &mut doc.song_select_active_text, [0, 0, 0]);
    ch |= opt_color_row(ui, &mut doc.bufs, "ssit", "SongSelectInactiveText", &mut doc.song_select_inactive_text, [255, 255, 255]);
    ch |= opt_color_row(ui, &mut doc.bufs, "iot", "InputOverlayText", &mut doc.input_overlay_text, [0, 0, 0]);
    ch |= opt_color_row(ui, &mut doc.bufs, "mg", "MenuGlow", &mut doc.menu_glow, [0, 78, 155]);
    ch |= opt_color_row(ui, &mut doc.bufs, "spbg", "SpinnerBackground", &mut doc.spinner_background, [100, 100, 100]);
    ch |= opt_color_row(ui, &mut doc.bufs, "sba", "StarBreakAdditive", &mut doc.star_break_additive, [255, 182, 193]);

    th::subsection_header(ui, "Fonts");
    ch |= th::text_row(ui, "ini_hcp", "HitCirclePrefix", &mut doc.hit_circle_prefix).changed();
    ch |= th::text_row(ui, "ini_hco2", "HitCircleOverlap", &mut doc.hit_circle_overlap).changed();
    ch |= th::text_row(ui, "ini_sp", "ScorePrefix", &mut doc.score_prefix).changed();
    ch |= th::text_row(ui, "ini_so", "ScoreOverlap", &mut doc.score_overlap).changed();
    ch |= th::text_row(ui, "ini_cp", "ComboPrefix", &mut doc.combo_prefix).changed();
    ch |= th::text_row(ui, "ini_co", "ComboOverlap", &mut doc.combo_overlap).changed();

    th::subsection_header(ui, "CatchTheBeat");
    ch |= opt_color_row(ui, &mut doc.bufs, "hd", "HyperDash", &mut doc.hyper_dash, [255, 0, 0]);
    ch |= opt_color_row(ui, &mut doc.bufs, "hdf", "HyperDashFruit", &mut doc.hyper_dash_fruit, [255, 0, 0]);
    ch |= opt_color_row(ui, &mut doc.bufs, "hda", "HyperDashAfterImage", &mut doc.hyper_dash_after_image, [255, 0, 0]);

    if !doc.mania_raw.is_empty() {
        th::subsection_header(ui, "Mania (preserved verbatim)");
        th::note(ui, "Existing [Mania] keycount blocks are preserved untouched when saving skin.ini.");
    }
    if ch {
        doc.dirty = true;
    }
    save_now
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_skin_ini_preserving_colours_and_mania() {
        let src = "[General]\nName: Test Skin\nAuthor: Someone\nVersion: 2.7\nCursorCentre: 0\n\n[Colours]\nCombo1: 10,20,30\nCombo2: 40,50,60\nSliderBorder: 255,255,255\n\n[Fonts]\nHitCirclePrefix: default\nHitCircleOverlap: 4\n\n[Mania]\nKeys: 4\nColumnWidth: 30,30,30,30\n";
        let doc = SkinIniDoc::parse(src);
        assert_eq!(doc.name, "Test Skin");
        assert!(!doc.cursor_centre);
        assert_eq!(doc.combos, vec![[10, 20, 30], [40, 50, 60]]);
        assert_eq!(doc.slider_border, Some([255, 255, 255]));
        assert!(doc.mania_raw.contains("Keys: 4"));
        let out = doc.serialize();
        assert!(out.contains("[Colours]"));
        assert!(out.contains("CursorCentre: 0"));
        assert!(out.contains("Combo1: 10,20,30"));
        assert!(out.contains("[Mania]\nKeys: 4"));
    }
}
