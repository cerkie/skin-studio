//! .osu parser for osu!standard: difficulty, timing, combo colors, hit objects (circles, sliders, spinners),
//! combo numbering and stacking.
use std::path::{Path as FsPath, PathBuf};

use crate::path::{CurveKind, Path, P};

#[derive(Clone, Debug)]
pub struct Timing {
    pub sample_index: u8,
    pub time: f32,
    pub beat_len: f32,
    pub uninherited: bool,
    pub sample_set: u8,
    pub volume: f32,
}

#[derive(Clone, Debug)]
pub struct SliderData {
    pub path: Path,
    pub slides: u32,
    pub length: f32,
    /// Pixels per millisecond.
    pub velocity: f32,
    pub span_duration: f32,
    /// Tick positions as a fraction of one span (0..1), before mirroring on reverse spans.
    pub ticks: Vec<f32>,
    pub edge_sounds: Vec<u8>,
    /// Per edge: (normal set, addition set), 0 meaning inherit.
    pub edge_sets: Vec<(u8, u8)>,
}

/// The `hitSample` field of a hit object: normalSet:additionSet:index:volume:filename.
#[derive(Clone, Debug, Default)]
pub struct SampleSpec {
    pub normal: u8,
    pub addition: u8,
    pub index: u8,
    pub volume: f32,
    pub file: String,
}

impl SampleSpec {
    pub fn parse(s: &str) -> SampleSpec {
        let p: Vec<&str> = s.split(':').collect();
        let n = |i: usize| p.get(i).and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(0.0);
        SampleSpec { normal: n(0) as u8, addition: n(1) as u8, index: n(2) as u8, volume: n(3), file: p.get(4).map(|s| s.trim().to_string()).unwrap_or_default() }
    }
}

#[derive(Clone, Debug)]
pub enum Kind {
    Circle,
    Slider(Box<SliderData>),
    Spinner,
}

#[derive(Clone, Debug)]
pub struct HitObject {
    pub pos: P,
    pub time: f32,
    pub end_time: f32,
    pub kind: Kind,
    pub new_combo: bool,
    /// osu!lazer's `ComboIndex`: the number of combos so far, counting the first (so the first combo is 1). The skin's colour
    /// for the combo is `colors[combo_index % count]`.
    pub combo_index: usize,
    /// `ComboIndexWithOffsets`: the same with the combo offsets of the map added, which is what the colours of the map itself use.
    pub combo_index_offsets: usize,
    /// Number shown on the circle (1-based).
    pub number: u32,
    pub stack: i32,
    pub hit_sound: u8,
    pub sample_set: u8,
    pub volume: f32,
    pub sample: SampleSpec,
}

impl HitObject {
    pub fn end_pos(&self) -> P {
        match &self.kind {
            Kind::Slider(s) => {
                let end = if s.slides % 2 == 1 { s.path.length() } else { 0.0 };
                let p = s.path.at(end).0;
                [self.pos[0] + p[0] - s.path.pts[0][0], self.pos[1] + p[1] - s.path.pts[0][1]]
            }
            _ => self.pos,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Beatmap {
    pub title: String,
    pub version: String,
    pub audio: String,
    pub audio_lead_in: f32,
    pub background: Option<String>,
    pub mode: u8,
    pub cs: f32,
    pub ar: f32,
    pub od: f32,
    pub slider_multiplier: f32,
    pub slider_tick_rate: f32,
    pub stack_leniency: f32,
    pub sample_set: u8,
    pub timing: Vec<Timing>,
    pub combo_colors: Vec<[f32; 3]>,
    /// Breaks of the map as (start, end) in ms; the first object after one starts a new combo.
    pub breaks: Vec<(f32, f32)>,
    pub slider_border: Option<[f32; 3]>,
    pub slider_track: Option<[f32; 3]>,
    pub objects: Vec<HitObject>,
    pub dir: PathBuf,
}

fn nums(s: &str) -> Vec<f32> {
    s.split(',').filter_map(|v| v.trim().parse().ok()).collect()
}

fn color(s: &str) -> Option<[f32; 3]> {
    let v = nums(s);
    (v.len() >= 3).then(|| [v[0], v[1], v[2]])
}

impl Beatmap {
    /// (beat length, slider velocity multiplier, timing point) in effect at `t`.
    pub fn timing_at(&self, t: f32) -> (f32, f32, Option<&Timing>) {
        let mut beat = 500.0;
        let mut sv = 1.0;
        let mut last = None;
        // first uninherited point applies before its own time too
        if let Some(first) = self.timing.iter().find(|p| p.uninherited) {
            beat = first.beat_len;
        }
        for p in &self.timing {
            if p.time > t + 0.5 {
                break;
            }
            if p.uninherited {
                beat = p.beat_len;
                sv = 1.0;
            } else {
                sv = (-100.0 / p.beat_len).clamp(0.1, 10.0);
            }
            last = Some(p);
        }
        (beat, sv, last)
    }

    pub fn parse_file(path: &FsPath) -> Result<Beatmap, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let text = String::from_utf8_lossy(&bytes);
        let mut bm = Beatmap::parse(&text);
        bm.dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        Ok(bm)
    }

    pub fn parse(text: &str) -> Beatmap {
        let mut bm = Beatmap { cs: 5.0, ar: -1.0, od: 5.0, slider_multiplier: 1.4, slider_tick_rate: 1.0, stack_leniency: 0.7, sample_set: 1, ..Default::default() };
        let mut section = String::new();
        let mut raw_objects: Vec<String> = Vec::new();
        for line in text.lines() {
            let line = line.trim_start_matches('\u{feff}').trim();
            if line.is_empty() || line.starts_with("//") {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                section = line[1..line.len() - 1].to_string();
                continue;
            }
            match section.as_str() {
                "General" | "Metadata" | "Difficulty" | "Colours" => {
                    let Some((k, v)) = line.split_once(':') else { continue };
                    let (k, v) = (k.trim(), v.trim());
                    match k {
                        "AudioFilename" => bm.audio = v.to_string(),
                        "AudioLeadIn" => bm.audio_lead_in = v.parse().unwrap_or(0.0),
                        "Mode" => bm.mode = v.parse().unwrap_or(0),
                        "StackLeniency" => bm.stack_leniency = v.parse().unwrap_or(0.7),
                        "SampleSet" => bm.sample_set = match v { "Soft" => 2, "Drum" => 3, _ => 1 },
                        "Title" => bm.title = v.to_string(),
                        "Version" => bm.version = v.to_string(),
                        "CircleSize" => bm.cs = v.parse().unwrap_or(5.0),
                        "ApproachRate" => bm.ar = v.parse().unwrap_or(5.0),
                        "OverallDifficulty" => bm.od = v.parse().unwrap_or(5.0),
                        "SliderMultiplier" => bm.slider_multiplier = v.parse().unwrap_or(1.4),
                        "SliderTickRate" => bm.slider_tick_rate = v.parse().unwrap_or(1.0),
                        "SliderBorder" => bm.slider_border = color(v),
                        "SliderTrackOverride" => bm.slider_track = color(v),
                        _ if k.starts_with("Combo") => {
                            if let Some(c) = color(v) {
                                bm.combo_colors.push(c);
                            }
                        }
                        _ => {}
                    }
                }
                "Events" => {
                    let parts: Vec<&str> = line.split(',').collect();
                    if parts.len() >= 3 && (parts[0] == "2" || parts[0] == "Break") {
                        if let (Ok(s), Ok(e)) = (parts[1].trim().parse::<f32>(), parts[2].trim().parse::<f32>()) {
                            bm.breaks.push((s, e));
                        }
                    }
                    if parts.len() >= 3 && parts[0] == "0" && parts[1] == "0" && bm.background.is_none() {
                        bm.background = Some(parts[2].trim_matches('"').to_string());
                    }
                }
                "TimingPoints" => {
                    let v: Vec<&str> = line.split(',').collect();
                    if v.len() >= 2 {
                        let f = |i: usize, d: f32| v.get(i).and_then(|s| s.trim().parse::<f32>().ok()).unwrap_or(d);
                        bm.timing.push(Timing {
                            time: f(0, 0.0),
                            beat_len: f(1, 500.0),
                            uninherited: v.get(6).map(|s| s.trim() != "0").unwrap_or(true),
                            sample_set: f(3, 0.0) as u8,
                            sample_index: f(4, 0.0) as u8,
                            volume: f(5, 100.0),
                        });
                    }
                }
                "HitObjects" => raw_objects.push(line.to_string()),
                _ => {}
            }
        }
        if bm.ar < 0.0 {
            bm.ar = bm.od;
        }
        bm.timing.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap());
        bm.build_objects(&raw_objects);
        bm.apply_stacking();
        bm
    }

    fn build_objects(&mut self, raw: &[String]) {
        // osu!lazer: Beatmap decoding and OsuBeatmapProcessor force a new combo on the first object, on the first object after a
        // spinner and on the first object after a break; IHasComboInformation.UpdateComboInformation then counts the combos
        let (mut combo_index, mut combo_index_offsets, mut in_combo) = (0usize, 0usize, 0u32);
        let mut last: Option<bool> = None; // whether the previous object was a spinner
        let mut break_i = 0usize;
        let mut out = Vec::with_capacity(raw.len());
        for line in raw {
            let v: Vec<&str> = line.split(',').collect();
            if v.len() < 5 {
                continue;
            }
            let f = |i: usize| v.get(i).and_then(|s| s.trim().parse::<f32>().ok()).unwrap_or(0.0);
            let (x, y, time, ty, hs) = (f(0), f(1), f(2), f(3) as u32, f(4) as u8);
            let is_spinner = ty & 8 != 0;
            let mut nc = ty & 4 != 0;
            if !is_spinner && matches!(last, None | Some(true)) {
                nc = true;
            }
            let mut after_break = false;
            while break_i < self.breaks.len() && self.breaks[break_i].1 < time {
                after_break = true;
                break_i += 1;
            }
            nc |= after_break;
            if nc || last.is_none() {
                in_combo = 0;
                combo_index += 1;
                combo_index_offsets += ((ty >> 4) & 7) as usize + 1;
            } else {
                in_combo += 1;
            }
            last = Some(is_spinner);
            let shown_number = in_combo + 1;
            let (beat, sv, tp) = self.timing_at(time);
            let (set, volume) = tp.map(|t| (t.sample_set, t.volume)).unwrap_or((0, 100.0));
            let sample_set = if set == 0 { self.sample_set } else { set };
            let mut obj = HitObject { pos: [x, y], time, end_time: time, kind: Kind::Circle, new_combo: nc, combo_index, combo_index_offsets, number: shown_number, stack: 0, hit_sound: hs, sample_set, volume, sample: SampleSpec::default() };
            if is_spinner {
                obj.kind = Kind::Spinner;
                obj.end_time = f(5);
                obj.sample = SampleSpec::parse(v.get(6).copied().unwrap_or(""));
            } else if ty & 2 == 0 {
                obj.sample = SampleSpec::parse(v.get(5).copied().unwrap_or(""));
            } else if ty & 2 != 0 && v.len() >= 8 {
                let mut parts = v[5].split('|');
                let kind = match parts.next().unwrap_or("B") {
                    "L" => CurveKind::Linear,
                    "P" => CurveKind::Perfect,
                    "C" => CurveKind::Catmull,
                    _ => CurveKind::Bezier,
                };
                let mut ctrl: Vec<P> = vec![[0.0, 0.0]];
                for p in parts {
                    if let Some((px, py)) = p.split_once(':') {
                        if let (Ok(px), Ok(py)) = (px.parse::<f32>(), py.parse::<f32>()) {
                            ctrl.push([px - x, py - y]);
                        }
                    }
                }
                let slides = (f(6) as u32).max(1);
                let length = f(7).max(0.0);
                let path = Path::build(kind, &ctrl, length);
                let velocity = self.slider_multiplier * 100.0 * sv / beat;
                let span_duration = length / velocity.max(1e-6);
                let tick_dist = self.slider_multiplier * 100.0 * sv / self.slider_tick_rate;
                let min_from_end = velocity * 10.0;
                let mut ticks = Vec::new();
                let mut d = tick_dist;
                while d < length - min_from_end && tick_dist > 1.0 {
                    ticks.push(d / length);
                    d += tick_dist;
                }
                let edge_sounds = v.get(8).map(|s| s.split('|').filter_map(|e| e.parse().ok()).collect()).unwrap_or_default();
                let edge_sets = v.get(9).map(|s| s.split('|').map(|e| { let (a, b) = e.split_once(':').unwrap_or(("0", "0")); (a.parse().unwrap_or(0), b.parse().unwrap_or(0)) }).collect()).unwrap_or_default();
                obj.sample = SampleSpec::parse(v.get(10).copied().unwrap_or(""));
                obj.end_time = time + span_duration * slides as f32;
                obj.kind = Kind::Slider(Box::new(SliderData { path, slides, length, velocity, span_duration, ticks, edge_sounds, edge_sets }));
            }
            out.push(obj);
        }
        self.objects = out;
    }

    /// Override Circle Size, Approach Rate, and/or Hard Rock (1.3x CS, 1.4x AR capped at 10, vertical flip) for live preview testing.
    pub fn override_difficulty(&mut self, cs: Option<f32>, ar: Option<f32>, hard_rock: bool) {
        if cs.is_none() && ar.is_none() && !hard_rock {
            return;
        }
        if hard_rock {
            self.cs = (self.cs * 1.3).min(10.0);
            self.ar = (self.ar * 1.4).min(10.0);
            for o in &mut self.objects {
                o.pos[1] = 384.0 - o.pos[1];
                if let Kind::Slider(s) = &mut o.kind {
                    for pt in &mut s.path.pts {
                        pt[1] = -pt[1];
                    }
                }
            }
        }
        if let Some(v) = cs {
            self.cs = v.clamp(0.0, 10.0);
        }
        if let Some(v) = ar {
            self.ar = v.clamp(0.0, 11.0);
        }
        for o in &mut self.objects {
            o.stack = 0;
        }
        self.apply_stacking();
    }

    fn apply_stacking(&mut self) {
        let n = self.objects.len();
        if n < 2 {
            return;
        }
        const STACK_DIST: f32 = 3.0;
        let preempt = engine::lazer::time_preempt(self.ar);
        let threshold = preempt * self.stack_leniency;
        let dist = |a: P, b: P| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
        let ends: Vec<P> = self.objects.iter().map(|o| o.end_pos()).collect();
        for i in (1..n).rev() {
            if self.objects[i].stack != 0 || matches!(self.objects[i].kind, Kind::Spinner) {
                continue;
            }
            let mut cur = i;
            let is_circle = matches!(self.objects[i].kind, Kind::Circle);
            let mut k = i;
            while k > 0 {
                k -= 1;
                if matches!(self.objects[k].kind, Kind::Spinner) {
                    continue;
                }
                if is_circle {
                    if self.objects[cur].time - self.objects[k].end_time > threshold {
                        break;
                    }
                    if matches!(self.objects[k].kind, Kind::Slider(_)) && dist(ends[k], self.objects[cur].pos) < STACK_DIST {
                        let offset = self.objects[cur].stack - self.objects[k].stack + 1;
                        for j in k + 1..=i {
                            if dist(ends[k], self.objects[j].pos) < STACK_DIST {
                                self.objects[j].stack -= offset;
                            }
                        }
                        break;
                    }
                    if dist(self.objects[k].pos, self.objects[cur].pos) < STACK_DIST {
                        self.objects[k].stack = self.objects[cur].stack + 1;
                        cur = k;
                    }
                } else {
                    if self.objects[cur].time - self.objects[k].time > threshold {
                        break;
                    }
                    if dist(ends[k], self.objects[cur].pos) < STACK_DIST {
                        self.objects[k].stack = self.objects[cur].stack + 1;
                        cur = k;
                    }
                }
            }
        }
    }

    /// Position of an object including its stack offset.
    pub fn stacked(&self, o: &HitObject) -> P {
        let off = o.stack as f32 * engine::lazer::circle_scale(self.cs) * -6.4;
        [o.pos[0] + off, o.pos[1] + off]
    }

    pub fn audio_path(&self) -> PathBuf {
        self.dir.join(&self.audio)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAP: &str = "osu file format v14\n[General]\nAudioFilename: a.mp3\n[Difficulty]\nCircleSize:4\nApproachRate:9\nSliderMultiplier:1.4\nSliderTickRate:1\n[TimingPoints]\n0,500,4,1,0,100,1,0\n[Colours]\nCombo1 : 255,0,0\nCombo2 : 0,255,0\n[HitObjects]\n100,100,1000,5,0,0:0:0:0:\n200,100,1500,1,0,0:0:0:0:\n100,200,2000,6,0,L|300:200,1,140,0|0,0:0|0:0,0:0:0:0:\n";

    #[test]
    fn parses_objects_combos_and_sliders() {
        let b = Beatmap::parse(MAP);
        assert_eq!(b.objects.len(), 3);
        assert_eq!(b.objects[0].number, 1);
        assert_eq!(b.objects[1].number, 2);
        assert_eq!(b.objects[2].number, 1);
        assert_eq!(b.objects[2].combo_index, 2);
        let Kind::Slider(s) = &b.objects[2].kind else { panic!() };
        // 140 px at 1.4 * 100 / 500 = 0.28 px/ms
        assert!((s.span_duration - 500.0).abs() < 1.0);
        assert!((b.objects[2].end_time - 2500.0).abs() < 1.0);
        assert!((b.objects[2].end_pos()[0] - 240.0).abs() < 0.5);
    }
}

#[cfg(test)]
mod combo_tests {
    use super::*;

    fn map(objects: &str, events: &str) -> Beatmap {
        Beatmap::parse(&format!("osu file format v14\n[General]\nAudioFilename: a.mp3\n[Difficulty]\nCircleSize:4\nApproachRate:9\nSliderMultiplier:1.4\nSliderTickRate:1\n[Events]\n{events}\n[HitObjects]\n{objects}\n"))
    }

    #[test]
    fn combo_indices_follow_lazer() {
        // the first object starts combo 1; a new combo with offset 2 (bits 4-6 of the type) skips two colours
        let b = map("100,100,1000,1,0\n100,100,1500,1,0\n100,100,2000,5,0\n100,100,2500,37,0", "");
        assert_eq!(b.objects.iter().map(|o| o.combo_index).collect::<Vec<_>>(), vec![1, 1, 2, 3]);
        assert_eq!(b.objects.iter().map(|o| o.combo_index_offsets).collect::<Vec<_>>(), vec![1, 1, 2, 5]);
        assert_eq!(b.objects.iter().map(|o| o.number).collect::<Vec<_>>(), vec![1, 2, 1, 1]);
    }

    #[test]
    fn a_break_or_a_spinner_starts_a_new_combo() {
        let b = map("100,100,1000,1,0\n100,100,5000,1,0\n256,192,6000,8,0,7000\n100,100,8000,1,0", "2,2000,4000");
        // the object after the break (5000) and the one after the spinner (8000) start combos
        assert_eq!(b.objects.iter().map(|o| o.number).collect::<Vec<_>>(), vec![1, 1, 2, 1]);
        assert_eq!(b.objects.iter().map(|o| o.combo_index).collect::<Vec<_>>(), vec![1, 2, 2, 3]);
    }
}
