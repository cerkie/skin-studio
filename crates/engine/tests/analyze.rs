use std::path::PathBuf;

#[test]
fn recovers_settings_from_frames() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/oracle");
    for (case, preset) in [("arrow_slide", "Arrow"), ("chevron_spin", "Chevron (bold)"), ("seamless_t0", engine::settings::SEAMLESS)] {
        let tmp = std::env::temp_dir().join(format!("analyze_{case}.png"));
        let a = engine::analyze::analyze(&root.join(case), Some(120.0), &tmp).unwrap();
        println!("{case}: preset {} match {:?} {}", a.settings["preset"], a.matched, a.settings["props"]);
        for n in &a.notes {
            println!("   {n}");
        }
        assert_eq!(a.settings["preset"], preset, "{case}");
        assert!(a.matched.unwrap_or(0.0) > 0.8, "{case}: match {:?}", a.matched);
    }
}
