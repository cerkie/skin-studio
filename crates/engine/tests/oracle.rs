//! Compares the Rust frame generator with reference frames written by the Python tool (`tools/gen_oracle.py`).
//! Run with `cargo test -p engine --release -- --nocapture` to see the table.
use std::path::PathBuf;

use engine::gen::{build_frames, Options};
use engine::img::Sampler;
use engine::settings::Settings;
use engine::sources::load_png;

struct Report {
    name: String,
    frames: (usize, usize),
    blank_mismatch: usize,
    size_mismatch: usize,
    mean: f64,
    p999: f64,
    max: f64,
}

fn compare(dir: &PathBuf, sampler: Sampler) -> Report {
    let name = dir.file_name().unwrap().to_string_lossy().to_string();
    let s = Settings::from_json_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap()).unwrap();
    let ours = build_frames(&s, 1, &Options { sampler });
    let mut expected = Vec::new();
    while let Ok(sp) = load_png(&dir.join(format!("followpoint-{}.png", expected.len()))) {
        expected.push(sp);
    }
    let mut r = Report { name, frames: (ours.len(), expected.len()), blank_mismatch: 0, size_mismatch: 0, mean: 0.0, p999: 0.0, max: 0.0 };
    let (mut sum, mut count, mut diffs) = (0.0f64, 0usize, Vec::new());
    for (o, e) in ours.iter().zip(&expected) {
        let e_blank = e.w <= 1 && e.h <= 1;
        match (o, e_blank) {
            (None, true) => continue,
            (Some(_), true) | (None, false) => {
                r.blank_mismatch += 1;
                continue;
            }
            (Some(img), false) => {
                if (img.w, img.h) != (e.w, e.h) {
                    r.size_mismatch += 1;
                    continue;
                }
                for (a, b) in img.px.iter().zip(&e.px) {
                    // compare premultiplied, in 0..255
                    for c in 0..4 {
                        let eb = if c < 3 { b[c] * b[3] } else { b[3] };
                        let d = ((a[c] - eb) * 255.0).abs() as f64;
                        sum += d;
                        count += 1;
                        diffs.push(d as f32);
                    }
                }
            }
        }
    }
    if count > 0 {
        r.mean = sum / count as f64;
        diffs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        r.p999 = diffs[((diffs.len() - 1) as f64 * 0.999) as usize] as f64;
        r.max = *diffs.last().unwrap() as f64;
    }
    r
}

#[test]
fn matches_python_reference() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/oracle");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root).expect("run tools/gen_oracle.py first").filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    for sampler in [Sampler::CatmullRom, Sampler::Hermite] {
        println!("\nsampler {sampler:?}: premultiplied difference to the Python frames (0..255 scale)");
        println!("{:26} {:>9} {:>6} {:>6} {:>8} {:>8} {:>8}", "case", "frames", "blank", "size", "mean", "p99.9", "max");
        for d in &dirs {
            let r = compare(d, sampler);
            println!("{:26} {:>4}/{:<4} {:>6} {:>6} {:>8.3} {:>8.2} {:>8.1}", r.name, r.frames.0, r.frames.1, r.blank_mismatch, r.size_mismatch, r.mean, r.p999, r.max);
            if sampler == Sampler::CatmullRom {
                assert_eq!(r.frames.0, r.frames.1, "{}: frame count", r.name);
                assert_eq!(r.blank_mismatch, 0, "{}: blank/drawn frames differ", r.name);
                assert_eq!(r.size_mismatch, 0, "{}: frame sizes differ", r.name);
            }
        }
    }
}

#[test]
fn generation_speed_at_1000fps() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/oracle");
    for case in ["arrow_slide", "seamless_t0", "chevron_outline_shadow"] {
        let text = std::fs::read_to_string(root.join(case).join("settings.json")).unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v["fps"] = serde_json::json!(1000); v["deband_on"] = serde_json::json!(true);
        let s = Settings::from_json(&v);
        let t = std::time::Instant::now();
        let f = build_frames(&s, 1, &Options { sampler: Sampler::Hermite });
        println!("{case}: {} frames in {:.0} ms", f.len(), t.elapsed().as_secs_f64() * 1000.0);
    }
}
