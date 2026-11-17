//! Finding maps in an osu! Songs folder.
use std::path::{Path, PathBuf};

use crate::beatmap::Beatmap;

fn xorshift(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

/// A random osu!standard difficulty with an existing audio file, a few hundred objects and a decent length.
pub fn random_map(songs: &Path, seed: u64) -> Option<(PathBuf, Beatmap)> {
    let dirs: Vec<PathBuf> = std::fs::read_dir(songs).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    if dirs.is_empty() {
        return None;
    }
    let mut s = seed | 1;
    for _ in 0..400 {
        let d = &dirs[(xorshift(&mut s) % dirs.len() as u64) as usize];
        let mut files: Vec<PathBuf> = std::fs::read_dir(d).ok()?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("osu"))).collect();
        if files.is_empty() {
            continue;
        }
        files.sort();
        let f = &files[(xorshift(&mut s) % files.len() as u64) as usize];
        let Ok(bm) = Beatmap::parse_file(f) else { continue };
        let ok = bm.mode == 0 && bm.objects.len() >= 150 && bm.objects.iter().any(|o| matches!(o.kind, crate::beatmap::Kind::Slider(_))) && bm.audio_path().is_file();
        if ok {
            return Some((f.clone(), bm));
        }
    }
    None
}

/// Unpack a beatmap archive (.osz from stable, .olz exported by lazer) into the temp folder and pick one osu!standard
/// difficulty from it: the one with the most objects.
pub fn open_archive(path: &Path) -> Result<PathBuf, String> {
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "map".into());
    let dir = std::env::temp_dir().join("skinstudio").join("maps").join(stem);
    let stamp = dir.join(".extracted");
    let src_time = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let fresh = std::fs::metadata(&stamp).and_then(|m| m.modified()).ok().zip(src_time).is_some_and(|(a, b)| a >= b);
    if !fresh {
        let _ = std::fs::remove_dir_all(&dir);
        let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut zip = zip::ZipArchive::new(f).map_err(|e| format!("{}: {e}", path.display()))?;
        zip.extract(&dir).map_err(|e| e.to_string())?;
        let _ = std::fs::write(&stamp, b"");
    }
    let mut best: Option<(usize, PathBuf)> = None;
    let mut stack = vec![dir.clone()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("osu")) {
                if let Ok(bm) = Beatmap::parse_file(&p) {
                    if bm.mode == 0 && !bm.objects.is_empty() && best.as_ref().is_none_or(|(n, _)| bm.objects.len() > *n) {
                        best = Some((bm.objects.len(), p));
                    }
                }
            }
        }
    }
    best.map(|(_, p)| p).ok_or_else(|| format!("{} has no osu!standard difficulty", path.display()))
}
