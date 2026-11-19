//! A small read-only reader for osu!lazer's `client.realm`.
//!
//! lazer keeps its beatmap sets in a Realm database and the files themselves in a content-addressed `files` folder
//! (`files/a/ab/abcdef...` named by SHA-256). The database is what says which hashes belong to which set and what each
//! is called. This reads just enough of the Realm file format (core 13, file format 24) to list those: the arrays, the
//! cluster trees of tables, string, link and link-list columns of `BeatmapSet`, `RealmNamedFileUsage` and `File`.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

struct Hdr {
    inner: bool,
    has_refs: bool,
    ctx: bool,
    wtype: u8,
    width: usize,
    size: usize,
}

struct Realm {
    d: Vec<u8>,
}

type R<T> = Result<T, String>;

impl Realm {
    fn hdr(&self, r: usize) -> R<Hdr> {
        let h = self.d.get(r..r + 8).ok_or("bad ref")?;
        if &h[..4] != b"AAAA" {
            return Err(format!("not an array at {r}"));
        }
        let b = h[4];
        let wndx = (b & 7) as usize;
        Ok(Hdr { inner: b >> 7 & 1 == 1, has_refs: b >> 6 & 1 == 1, ctx: b >> 5 & 1 == 1, wtype: b >> 3 & 3, width: if wndx == 0 { 0 } else { 1 << (wndx - 1) }, size: (h[5] as usize) << 16 | (h[6] as usize) << 8 | h[7] as usize })
    }

    fn at(&self, r: usize, h: &Hdr, i: usize, signed: bool) -> R<i64> {
        let base = r + 8;
        let w = h.width;
        if w == 0 {
            return Ok(0);
        }
        if w < 8 {
            let per = 8 / w;
            let byte = *self.d.get(base + i / per).ok_or("short array")?;
            return Ok(((byte >> ((i % per) * w)) & ((1u16 << w) - 1) as u8) as i64);
        }
        let n = w / 8;
        let s = self.d.get(base + i * n..base + (i + 1) * n).ok_or("short array")?;
        let mut v = 0u64;
        for (k, b) in s.iter().enumerate() {
            v |= (*b as u64) << (8 * k);
        }
        Ok(if signed && n < 8 && v >> (n * 8 - 1) & 1 == 1 { (v | (u64::MAX << (n * 8))) as i64 } else { v as i64 })
    }

    fn arr(&self, r: usize) -> R<Vec<i64>> {
        let h = self.hdr(r)?;
        if h.wtype != 0 {
            return Err("unexpected array type".into());
        }
        (0..h.size).map(|i| self.at(r, &h, i, true)).collect()
    }

    fn uarr(&self, r: usize) -> R<Vec<i64>> {
        let h = self.hdr(r)?;
        (0..h.size).map(|i| self.at(r, &h, i, false)).collect()
    }

    fn blob(&self, r: usize) -> R<&[u8]> {
        let h = self.hdr(r)?;
        self.d.get(r + 8..r + 8 + h.size).ok_or_else(|| "short blob".to_string())
    }

    fn utf8(b: &[u8]) -> String {
        String::from_utf8_lossy(b).into_owned()
    }

    /// The strings of one column leaf: short strings, medium (offsets + blob) or big (one blob each).
    fn strings(&self, r: usize) -> R<Vec<String>> {
        let h = self.hdr(r)?;
        if !h.has_refs {
            if h.width == 0 {
                return Ok(vec![String::new(); h.size]);
            }
            return (0..h.size)
                .map(|i| {
                    let raw = self.d.get(r + 8 + i * h.width..r + 8 + (i + 1) * h.width).ok_or("short strings")?;
                    let pad = *raw.last().unwrap() as usize;
                    Ok(Self::utf8(&raw[..h.width.saturating_sub(1 + pad)]))
                })
                .collect();
        }
        if h.ctx {
            return self.arr(r)?.into_iter().map(|b| if b == 0 { Ok(String::new()) } else { Ok(Self::utf8(self.blob(b as usize)?).trim_end_matches('\0').to_string()) }).collect();
        }
        let a = self.arr(r)?;
        let offs = self.arr(a[0] as usize)?;
        let bl = self.blob(a[1] as usize)?;
        let mut prev = 0usize;
        let mut out = Vec::new();
        for o in offs {
            let o = (o as usize).min(bl.len());
            out.push(Self::utf8(&bl[prev.min(o)..o]));
            prev = o;
        }
        Ok(out)
    }

    fn table(&self, name: &str) -> R<Table> {
        let flags = *self.d.get(23).ok_or("short file")?;
        let top = u64::from_le_bytes(self.d[(flags & 1) as usize * 8..(flags & 1) as usize * 8 + 8].try_into().unwrap()) as usize;
        let t = self.arr(top)?;
        let names = self.strings(t[0] as usize)?;
        let tabs = self.arr(t[1] as usize)?;
        let want = format!("class_{name}");
        let i = names.iter().position(|n| *n == want).ok_or_else(|| format!("no table {name}"))?;
        let tt = self.arr(tabs[i] as usize)?;
        let spec = self.arr(tt[0] as usize)?;
        let cols = self.strings(spec[1] as usize)?;
        let idx = self.arr(spec[5] as usize)?.into_iter().map(|k| (k & 0xFFFF) as usize).collect();
        Ok(Table { root: tt[2] as usize, cols, idx })
    }

    /// The values of a list column: one array, or, for a long list, the leaves of a B+tree of arrays (an inner node holds a
    /// tagged size or an offsets array first, the children, and a tagged total last).
    fn flat_list(&self, r: usize) -> R<Vec<i64>> {
        let h = self.hdr(r)?;
        let a = self.arr(r)?;
        if !h.inner {
            return Ok(a);
        }
        let mut out = Vec::new();
        for c in a.iter().skip(1).take(a.len().saturating_sub(2)) {
            out.extend(self.flat_list(*c as usize)?);
        }
        Ok(out)
    }

    /// Every leaf of a cluster tree with the first object key it holds.
    fn leaves(&self, r: usize, base: i64, out: &mut Vec<(usize, i64)>) -> R<()> {
        let h = self.hdr(r)?;
        if h.inner {
            let a = self.arr(r)?;
            let children = &a[3..];
            let keys = if a[0] != 0 { self.uarr(a[0] as usize)? } else { (0..children.len() as i64).map(|i| i * 256).collect() };
            for (k, c) in keys.iter().zip(children) {
                self.leaves(*c as usize, base + k, out)?;
            }
        } else {
            out.push((r, base));
        }
        Ok(())
    }

    /// The object keys of a leaf.
    fn keys(&self, leaf: &[i64], base: i64) -> R<Vec<i64>> {
        let k = leaf[0];
        if k & 1 == 1 {
            Ok((0..k >> 1).map(|i| base + i).collect())
        } else {
            Ok(self.uarr(k as usize)?.into_iter().map(|x| base + x).collect())
        }
    }
}

struct Table {
    root: usize,
    cols: Vec<String>,
    idx: Vec<usize>,
}

impl Table {
    fn col(&self, leaf: &[i64], name: &str) -> R<usize> {
        let i = self.cols.iter().position(|c| c == name).ok_or_else(|| format!("no column {name}"))?;
        leaf.get(1 + self.idx[i]).map(|v| *v as usize).ok_or_else(|| "short cluster".to_string())
    }
}

/// One beatmap set of the lazer database: its files as (name, SHA-256).
pub struct LazerSet {
    pub files: Vec<(String, String)>,
}

/// Where lazer keeps its data (the folder holding `client.realm` and `files`).
pub fn lazer_dir() -> Option<PathBuf> {
    let mut c: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("OSU_LAZER_DIR") {
        c.push(PathBuf::from(p));
    }
    if let Some(a) = std::env::var_os("APPDATA") {
        c.push(PathBuf::from(a).join("osu"));
    }
    if let Some(h) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let h = PathBuf::from(h);
        c.push(h.join(".local/share/osu"));
        c.push(h.join("Library/Application Support/osu"));
        c.push(h.join(".var/app/sh.ppy.osu/data/osu"));
    }
    c.into_iter().find(|p| p.join("client.realm").is_file() && p.join("files").is_dir())
}

/// Every set in the database that is not waiting to be deleted.
pub fn read_sets(dir: &Path) -> R<Vec<LazerSet>> {
    let path = dir.join("client.realm");
    let d = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if d.get(16..20) != Some(b"T-DB") {
        return Err("client.realm is not a Realm file".into());
    }
    let rl = Realm { d };
    let leaf_rows = |t: &Table| -> R<Vec<(Vec<i64>, Vec<i64>)>> {
        let mut ls = Vec::new();
        rl.leaves(t.root, 0, &mut ls)?;
        ls.into_iter().map(|(l, base)| {
            let a = rl.arr(l)?;
            let keys = rl.keys(&a, base)?;
            Ok((a, keys))
        }).collect()
    };
    let file_t = rl.table("File")?;
    let mut hashes: HashMap<i64, String> = HashMap::new();
    for (a, keys) in leaf_rows(&file_t)? {
        let h = rl.strings(file_t.col(&a, "Hash")?)?;
        for (k, s) in keys.into_iter().zip(h) {
            hashes.insert(k, s);
        }
    }
    let nfu_t = rl.table("RealmNamedFileUsage")?;
    let mut usage: HashMap<i64, (String, i64)> = HashMap::new();
    for (a, keys) in leaf_rows(&nfu_t)? {
        let names = rl.strings(nfu_t.col(&a, "Filename")?)?;
        let links = rl.arr(nfu_t.col(&a, "File")?)?;
        for ((k, n), l) in keys.into_iter().zip(names).zip(links) {
            usage.insert(k, (n, l - 1));
        }
    }
    let set_t = rl.table("BeatmapSet")?;
    let mut sets = Vec::new();
    for (a, keys) in leaf_rows(&set_t)? {
        let lists = rl.arr(set_t.col(&a, "Files")?)?;
        let pending = rl.arr(set_t.col(&a, "DeletePending")?)?;
        for i in 0..keys.len() {
            if pending.get(i).copied().unwrap_or(0) != 0 || lists[i] == 0 {
                continue;
            }
            let files = rl.flat_list(lists[i] as usize)?.into_iter().filter_map(|k| {
                let (name, f) = usage.get(&k)?;
                Some((name.clone(), hashes.get(f)?.clone()))
            }).collect();
            sets.push(LazerSet { files });
        }
    }
    Ok(sets)
}

/// One skin of the lazer database: its name and files as (name, SHA-256).
pub struct LazerSkin {
    pub name: String,
    pub files: Vec<(String, String)>,
}

/// Every skin of the lazer database that has files of its own (the skins that come with the game have none).
pub fn read_skins(dir: &Path) -> R<Vec<LazerSkin>> {
    let path = dir.join("client.realm");
    let d = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if d.get(16..20) != Some(b"T-DB") {
        return Err("client.realm is not a Realm file".into());
    }
    let rl = Realm { d };
    let leaf_rows = |t: &Table| -> R<Vec<(Vec<i64>, Vec<i64>)>> {
        let mut ls = Vec::new();
        rl.leaves(t.root, 0, &mut ls)?;
        ls.into_iter().map(|(l, base)| {
            let a = rl.arr(l)?;
            let keys = rl.keys(&a, base)?;
            Ok((a, keys))
        }).collect()
    };
    let file_t = rl.table("File")?;
    let mut hashes: HashMap<i64, String> = HashMap::new();
    for (a, keys) in leaf_rows(&file_t)? {
        let h = rl.strings(file_t.col(&a, "Hash")?)?;
        for (k, s) in keys.into_iter().zip(h) {
            hashes.insert(k, s);
        }
    }
    let nfu_t = rl.table("RealmNamedFileUsage")?;
    let mut usage: HashMap<i64, (String, i64)> = HashMap::new();
    for (a, keys) in leaf_rows(&nfu_t)? {
        let names = rl.strings(nfu_t.col(&a, "Filename")?)?;
        let links = rl.arr(nfu_t.col(&a, "File")?)?;
        for ((k, n), l) in keys.into_iter().zip(names).zip(links) {
            usage.insert(k, (n, l - 1));
        }
    }
    let skin_t = rl.table("Skin")?;
    let mut skins = Vec::new();
    for (a, keys) in leaf_rows(&skin_t)? {
        let names = rl.strings(skin_t.col(&a, "Name")?)?;
        let lists = rl.arr(skin_t.col(&a, "Files")?)?;
        let pending = rl.arr(skin_t.col(&a, "DeletePending")?)?;
        for i in 0..keys.len() {
            if pending.get(i).copied().unwrap_or(0) != 0 || lists[i] == 0 {
                continue;
            }
            let files: Vec<(String, String)> = rl.flat_list(lists[i] as usize)?.into_iter().filter_map(|k| {
                let (name, f) = usage.get(&k)?;
                Some((name.trim_end_matches(' ').to_string(), hashes.get(f)?.trim_end_matches(' ').to_string()))
            }).collect();
            if std::env::var_os("SKIN_STUDIO_REALM_DEBUG").is_some() {
                let raw = rl.flat_list(lists[i] as usize)?;
                eprintln!("{}: list {} resolved {} raw {:?}", names.get(i).cloned().unwrap_or_default(), raw.len(), files.len(), &raw[..raw.len().min(8)]);
            }
            if !files.is_empty() {
                skins.push(LazerSkin { name: names.get(i).cloned().unwrap_or_default(), files });
            }
        }
    }
    Ok(skins)
}

/// Copy the files of a skin into `dest` under their own names.
pub fn materialize_skin(dir: &Path, skin: &LazerSkin, dest: &Path) -> R<()> {
    for (name, hash) in &skin.files {
        if name.contains("..") || Path::new(name).is_absolute() || hash.len() < 2 {
            continue;
        }
        let to = dest.join(name);
        let src = file_path(dir, hash);
        if to.is_file() && std::fs::metadata(&to).ok().map(|m| m.len()) == std::fs::metadata(&src).ok().map(|m| m.len()) {
            continue;
        }
        if let Some(p) = to.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        std::fs::copy(&src, &to).map_err(|e| format!("{}: {e}", src.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod skin_tests {
    #[test]
    #[ignore]
    fn lists_lazer_skins() {
        let dir = super::lazer_dir().expect("lazer dir");
        for s in super::read_skins(&dir).expect("skins") {
            println!("{} ({} files) {}", s.name, s.files.len(), if s.name.to_lowercase().contains("cerkie") { s.files.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>().join(", ") } else { String::new() });
        }
    }
}

/// Where lazer stores a file.
pub fn file_path(dir: &Path, hash: &str) -> PathBuf {
    dir.join("files").join(&hash[..1]).join(&hash[..2]).join(hash)
}

/// Copy the parts of a set that the preview needs (difficulties, music, hit sounds) under their own names, and return the
/// folder.
fn materialize(dir: &Path, set: &LazerSet, all: bool) -> R<PathBuf> {
    let key = set.files.iter().map(|(_, h)| h.as_str()).min().unwrap_or("none");
    let dest = std::env::temp_dir().join("skinstudio").join("lazer").join(&key[..key.len().min(16)]);
    for (name, hash) in &set.files {
        let ext = Path::new(name).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if !(ext == "osu" || (all && matches!(ext.as_str(), "mp3" | "ogg" | "wav"))) || name.contains("..") || hash.len() < 2 {
            continue;
        }
        let to = dest.join(name);
        let src = file_path(dir, hash);
        if to.is_file() && std::fs::metadata(&to).ok().map(|m| m.len()) == std::fs::metadata(&src).ok().map(|m| m.len()) {
            continue;
        }
        if let Some(p) = to.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        std::fs::copy(&src, &to).map_err(|e| format!("{}: {e}", src.display()))?;
    }
    Ok(dest)
}

fn xorshift(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

/// A random osu!standard difficulty from the lazer library, copied to a temporary folder.
pub fn random_map(dir: &Path, seed: u64) -> R<PathBuf> {
    let sets: Vec<LazerSet> = read_sets(dir)?.into_iter().filter(|s| s.files.iter().any(|(n, _)| n.to_lowercase().ends_with(".osu"))).collect();
    if sets.is_empty() {
        return Err("The lazer library has no beatmaps.".into());
    }
    let mut s = seed | 1;
    for _ in 0..300 {
        let set = &sets[(xorshift(&mut s) % sets.len() as u64) as usize];
        let osus: Vec<&(String, String)> = set.files.iter().filter(|(n, _)| n.to_lowercase().ends_with(".osu")).collect();
        let (name, _) = osus[(xorshift(&mut s) % osus.len() as u64) as usize];
        // the difficulty alone first, so rejected sets cost almost nothing
        let Ok(folder) = materialize(dir, set, false) else { continue };
        let f = folder.join(name);
        let Ok(bm) = crate::beatmap::Beatmap::parse_file(&f) else { continue };
        if bm.mode == 0 && bm.objects.len() >= 150 && bm.objects.iter().any(|o| matches!(o.kind, crate::beatmap::Kind::Slider(_))) {
            if materialize(dir, set, true).is_ok() && bm.audio_path().is_file() {
                return Ok(f);
            }
        }
    }
    Err("No suitable map found in the lazer library.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads the real database when lazer is installed on the machine running the tests.
    #[test]
    fn reads_the_local_library() {
        let Some(dir) = lazer_dir() else { return };
        let t = std::time::Instant::now();
        let sets = read_sets(&dir).unwrap();
        eprintln!("{} sets in {:?}", sets.len(), t.elapsed());
        assert!(!sets.is_empty());
        let (_, h) = &sets[sets.len() - 1].files[0];
        assert!(file_path(&dir, h).is_file());
        let m = random_map(&dir, 7).unwrap();
        eprintln!("random map {}", m.display());
        assert!(m.is_file());
    }
}
