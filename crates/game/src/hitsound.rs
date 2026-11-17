//! Hit sounds for an autoplayed map, resolved the way lazer does: hitSample sets/index/volume/filename on each
//! object (and per slider edge), falling back to the timing point's sample set, index and volume, then to the
//! beatmap default set. Sliders also loop `sliderslide` / `sliderwhistle`; spinners loop `spinnerspin` and play
//! `spinnerbonus`.
use crate::beatmap::{Beatmap, Kind, SampleSpec};
use crate::spinner::SpinnerInfo;

#[derive(Clone, Debug)]
pub enum Sound {
    /// A normal hit (circle, slider edge, spinner end): hit sound flags decide the additions.
    Hit { bits: u8 },
    SliderTick,
    SpinnerBonus,
}

#[derive(Clone, Debug)]
pub struct HitEvent {
    pub time: f32,
    pub sound: Sound,
    /// 1 normal, 2 soft, 3 drum.
    pub normal: u8,
    pub addition: u8,
    pub index: u8,
    /// 0..1
    pub volume: f32,
    /// Custom sample file from the hitSample field (replaces the normal hit sound).
    pub file: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LoopKind {
    Slide,
    Spin,
}

#[derive(Clone, Debug)]
pub struct LoopEvent {
    pub kind: LoopKind,
    pub start: f32,
    pub end: f32,
    pub names: Vec<String>,
    pub volume: f32,
    /// For spinners: the spinner, so the pitch can follow its progress.
    pub spinner: Option<SpinnerInfo>,
}

pub fn set_name(set: u8) -> &'static str {
    match set {
        2 => "soft",
        3 => "drum",
        _ => "normal",
    }
}

fn suffix(index: u8) -> String {
    if index > 1 { index.to_string() } else { String::new() }
}

struct Resolved {
    normal: u8,
    addition: u8,
    index: u8,
    volume: f32,
}

/// Apply the hitSample inheritance rules at time `t`.
fn resolve(map: &Beatmap, t: f32, spec: &SampleSpec, edge: Option<(u8, u8)>) -> Resolved {
    let tp = map.timing_at(t).2;
    let tp_set = tp.map(|p| p.sample_set).filter(|s| *s != 0).unwrap_or(map.sample_set.max(1));
    let tp_index = tp.map(|p| p.sample_index).unwrap_or(0);
    let tp_vol = tp.map(|p| p.volume).unwrap_or(100.0);
    let (en, ea) = edge.unwrap_or((0, 0));
    let normal = [en, spec.normal].into_iter().find(|s| *s != 0).unwrap_or(tp_set);
    let addition = [ea, spec.addition].into_iter().find(|s| *s != 0).unwrap_or(normal);
    Resolved { normal, addition, index: if spec.index != 0 { spec.index } else { tp_index }, volume: if spec.volume > 0.0 { spec.volume } else { tp_vol } / 100.0 }
}

pub fn events(map: &Beatmap) -> Vec<HitEvent> {
    let mut ev = Vec::new();
    for o in &map.objects {
        match &o.kind {
            Kind::Circle => {
                let r = resolve(map, o.time, &o.sample, None);
                ev.push(HitEvent { time: o.time, sound: Sound::Hit { bits: o.hit_sound }, normal: r.normal, addition: r.addition, index: r.index, volume: r.volume, file: (!o.sample.file.is_empty()).then(|| o.sample.file.clone()) });
            }
            Kind::Slider(s) => {
                for i in 0..=s.slides {
                    let t = o.time + i as f32 * s.span_duration;
                    // the head uses the object's own sample point, later edges the timing point at that moment
                    let r = resolve(map, if i == 0 { o.time } else { t }, &o.sample, s.edge_sets.get(i as usize).copied());
                    let bits = s.edge_sounds.get(i as usize).copied().unwrap_or(o.hit_sound);
                    ev.push(HitEvent { time: t, sound: Sound::Hit { bits }, normal: r.normal, addition: r.addition, index: r.index, volume: r.volume, file: None });
                }
                let r = resolve(map, o.time, &o.sample, None);
                for span in 0..s.slides {
                    for &tick in &s.ticks {
                        let t = o.time + (span as f32 + tick) * s.span_duration;
                        ev.push(HitEvent { time: t, sound: Sound::SliderTick, normal: r.normal, addition: r.addition, index: r.index, volume: r.volume, file: None });
                    }
                }
            }
            Kind::Spinner => {
                let sp = SpinnerInfo::new(map.od, o.time, o.end_time);
                for i in 0..sp.total_ticks() {
                    if sp.is_bonus(i) {
                        ev.push(HitEvent { time: sp.tick_time(i), sound: Sound::SpinnerBonus, normal: 1, addition: 1, index: 0, volume: 1.0, file: None });
                    }
                }
                let r = resolve(map, o.end_time, &o.sample, None);
                ev.push(HitEvent { time: o.end_time, sound: Sound::Hit { bits: o.hit_sound }, normal: r.normal, addition: r.addition, index: r.index, volume: r.volume, file: (!o.sample.file.is_empty()).then(|| o.sample.file.clone()) });
            }
        }
    }
    ev.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap());
    ev
}

pub fn loops(map: &Beatmap) -> Vec<LoopEvent> {
    let mut v = Vec::new();
    for o in &map.objects {
        match &o.kind {
            Kind::Slider(_) => {
                let r = resolve(map, o.time, &o.sample, None);
                let mut names = vec![format!("{}-sliderslide{}", set_name(r.normal), suffix(r.index))];
                if o.hit_sound & 2 != 0 {
                    names.push(format!("{}-sliderwhistle{}", set_name(r.addition), suffix(r.index)));
                }
                v.push(LoopEvent { kind: LoopKind::Slide, start: o.time, end: o.end_time, names, volume: r.volume, spinner: None });
            }
            Kind::Spinner => {
                let sp = SpinnerInfo::new(map.od, o.time, o.end_time);
                v.push(LoopEvent { kind: LoopKind::Spin, start: o.time, end: o.end_time, names: vec!["spinnerspin".into()], volume: 1.0, spinner: Some(sp) });
            }
            Kind::Circle => {}
        }
    }
    v.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());
    v
}

/// Sample names to try for an event, most specific first. Each entry is a list of fallbacks for one layer.
pub fn sample_names(e: &HitEvent) -> Vec<Vec<String>> {
    let with_index = |set: u8, kind: &str| {
        let mut v = Vec::new();
        if e.index > 1 {
            v.push(format!("{}-{kind}{}", set_name(set), e.index));
        }
        v.push(format!("{}-{kind}", set_name(set)));
        v
    };
    match &e.sound {
        Sound::SliderTick => vec![with_index(e.normal, "slidertick")],
        Sound::SpinnerBonus => vec![vec!["spinnerbonus".to_string()]],
        Sound::Hit { bits } => {
            let mut layers = Vec::new();
            match &e.file {
                Some(f) => {
                    let stem = f.rsplit_once('.').map(|(a, _)| a).unwrap_or(f);
                    layers.push(vec![stem.to_string()]);
                }
                None => layers.push(with_index(e.normal, "hitnormal")),
            }
            for (bit, n) in [(2, "hitwhistle"), (4, "hitfinish"), (8, "hitclap")] {
                if bits & bit != 0 {
                    layers.push(with_index(e.addition, n));
                }
            }
            layers
        }
    }
}

/// Every sample name any event or loop might need, for preloading.
pub fn all_names(events: &[HitEvent], loops: &[LoopEvent]) -> Vec<String> {
    let mut names: Vec<String> = events.iter().flat_map(|e| sample_names(e).into_iter().flatten()).collect();
    for l in loops {
        names.extend(l.names.iter().cloned());
        for n in &l.names {
            // a missing indexed sample falls back to the plain one
            let plain: String = n.trim_end_matches(|c: char| c.is_ascii_digit()).to_string();
            names.push(plain);
        }
    }
    names.sort();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inherits_from_timing_and_overrides_from_sample() {
        let map = Beatmap::parse("[General]\nSampleSet: Normal\n[Difficulty]\nCircleSize:4\nApproachRate:9\nSliderMultiplier:1.4\n[TimingPoints]\n0,500,4,2,3,60,1,0\n[HitObjects]\n100,100,1000,1,2,0:0:0:0:\n200,100,1500,1,4,3:2:0:80:custom.wav\n");
        let ev = events(&map);
        assert_eq!(ev[0].normal, 2); // soft from the timing point
        assert_eq!(ev[0].index, 3);
        assert!((ev[0].volume - 0.6).abs() < 1e-3);
        assert_eq!(ev[1].normal, 3);
        assert_eq!(ev[1].addition, 2);
        assert!((ev[1].volume - 0.8).abs() < 1e-3);
        let names = sample_names(&ev[0]);
        assert_eq!(names[0], vec!["soft-hitnormal3".to_string(), "soft-hitnormal".to_string()]);
        assert_eq!(names[1], vec!["soft-hitwhistle3".to_string(), "soft-hitwhistle".to_string()]);
        assert_eq!(sample_names(&ev[1])[0], vec!["custom".to_string()]);
    }
}
