//! Pure-Rust extractor for osu!'s classic 2014 Legacy default skin from the user's local osu!lazer installation
//! (`osu.Game.Resources.dll`, .NET ECMA-335 `ManifestResource` table #40).
//!
//! No osu! textures are shipped with Skin Studio: if osu!lazer is installed locally, its `Skins.Legacy.*`
//! resources are extracted once into a temp cache folder and attached as a fallback skin for partial legacy skins.
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const PREFIX: &str = "osu.Game.Resources.Skins.Legacy.";
const MARKER: &str = ".extracted-v1";

static CACHED_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Locate and extract osu!lazer's bundled classic 2014 Legacy default skin once per process (cached on disk).
pub fn legacy_default_skin_dir() -> Option<PathBuf> {
    CACHED_DIR
        .get_or_init(|| {
            let cache = std::env::temp_dir().join("skinstudio").join("legacy-default-skin");
            if cache.join(MARKER).is_file() && cache.join("hitcircle@2x.png").is_file() {
                return Some(cache);
            }
            let dll = find_resources_dll()?;
            match extract_legacy_skin(&dll, &cache) {
                Ok(n) if n > 0 => {
                    let _ = std::fs::write(cache.join(MARKER), format!("{n}\n"));
                    Some(cache)
                }
                _ => None,
            }
        })
        .clone()
}

fn find_resources_dll() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("OSU_LAZER_APP_DIR") {
        let p = PathBuf::from(d).join("osu.Game.Resources.dll");
        if p.is_file() {
            return Some(p);
        }
    }
    #[cfg(windows)]
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let base = PathBuf::from(local).join("osulazer");
        let cur = base.join("current").join("osu.Game.Resources.dll");
        if cur.is_file() {
            return Some(cur);
        }
        // Fallback: newest app-* directory
        if let Ok(rd) = std::fs::read_dir(&base) {
            let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
            for e in rd.flatten() {
                let p = e.path().join("osu.Game.Resources.dll");
                if p.is_file() {
                    let t = p.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
                    if best.as_ref().is_none_or(|(bt, _)| t > *bt) {
                        best = Some((t, p));
                    }
                }
            }
            if let Some((_, p)) = best {
                return Some(p);
            }
        }
    }
    #[cfg(not(windows))]
    {
        for p in [
            "/usr/lib/osu-lazer/osu.Game.Resources.dll",
            "/opt/osu-lazer/osu.Game.Resources.dll",
            "/Applications/osu!.app/Contents/MacOS/osu.Game.Resources.dll",
        ] {
            let pb = PathBuf::from(p);
            if pb.is_file() {
                return Some(pb);
            }
        }
    }
    None
}

// ------------------------------------------------------------------------------------------------ PE + ECMA-335 reader

fn u16_le(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn u32_le(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn u64_le(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

struct Section {
    va: u32,
    vsize: u32,
    raw_ptr: u32,
    raw_size: u32,
}

fn rva_to_off(secs: &[Section], rva: u32) -> Option<usize> {
    for s in secs {
        let len = s.vsize.max(s.raw_size);
        if rva >= s.va && rva < s.va.saturating_add(len) {
            return Some((s.raw_ptr + (rva - s.va)) as usize);
        }
    }
    None
}

/// Extract all `osu.Game.Resources.Skins.Legacy.*` manifest resources from a .NET assembly DLL into `dest_dir`.
pub fn extract_legacy_skin(dll: &Path, dest_dir: &Path) -> Result<usize, String> {
    let data = std::fs::read(dll).map_err(|e| e.to_string())?;
    if data.get(0..2) != Some(b"MZ") {
        return Err("Not a PE file".into());
    }
    let pe_off = u32_le(&data, 0x3c).ok_or("bad pe_off")? as usize;
    if data.get(pe_off..pe_off + 4) != Some(b"PE\0\0") {
        return Err("Missing PE signature".into());
    }
    let n_secs = u16_le(&data, pe_off + 6).ok_or("bad n_secs")? as usize;
    let opt_size = u16_le(&data, pe_off + 20).ok_or("bad opt_size")? as usize;
    let opt_off = pe_off + 24;
    let magic = u16_le(&data, opt_off).ok_or("bad magic")?;
    let dd_off = match magic {
        0x10b => opt_off + 96,  // PE32
        0x20b => opt_off + 112, // PE32+
        _ => return Err("Unknown PE magic".into()),
    };
    // Data directory #14 = CLI Header (COR20)
    let cli_rva = u32_le(&data, dd_off + 14 * 8).ok_or("missing CLI dir")?;
    if cli_rva == 0 {
        return Err("No .NET CLI header".into());
    }
    let sec_off = opt_off + opt_size;
    let mut secs = Vec::with_capacity(n_secs);
    for i in 0..n_secs {
        let o = sec_off + i * 40;
        secs.push(Section {
            vsize: u32_le(&data, o + 8).ok_or("sec")?,
            va: u32_le(&data, o + 12).ok_or("sec")?,
            raw_size: u32_le(&data, o + 16).ok_or("sec")?,
            raw_ptr: u32_le(&data, o + 20).ok_or("sec")?,
        });
    }
    let cli_off = rva_to_off(&secs, cli_rva).ok_or("bad CLI rva")?;
    let md_rva = u32_le(&data, cli_off + 8).ok_or("bad md rva")?;
    let res_rva = u32_le(&data, cli_off + 24).ok_or("bad resources rva")?;
    let res_size = u32_le(&data, cli_off + 28).ok_or("bad resources size")? as usize;
    if md_rva == 0 || res_rva == 0 {
        return Err("Missing metadata or resources".into());
    }
    let md_off = rva_to_off(&secs, md_rva).ok_or("bad md off")?;
    let res_off = rva_to_off(&secs, res_rva).ok_or("bad res off")?;
    let res_blob = data.get(res_off..res_off + res_size).ok_or("resources out of bounds")?;

    // Metadata Root (ECMA-335 II.24.2.1): magic 0x424A5342 ("BSJB")
    if data.get(md_off..md_off + 4) != Some(b"BSJB") {
        return Err("Missing BSJB metadata root".into());
    }
    let ver_len = u32_le(&data, md_off + 12).ok_or("ver_len")? as usize;
    let mut cur = md_off + 16 + ((ver_len + 3) & !3);
    let n_streams = u16_le(&data, cur + 2).ok_or("n_streams")? as usize;
    cur += 4;
    let mut tilde: Option<(usize, usize)> = None;
    let mut strings: Option<(usize, usize)> = None;
    for _ in 0..n_streams {
        let s_off = u32_le(&data, cur).ok_or("s_off")? as usize;
        let s_sz = u32_le(&data, cur + 4).ok_or("s_sz")? as usize;
        cur += 8;
        let name_start = cur;
        while cur < data.len() && data[cur] != 0 {
            cur += 1;
        }
        let name = std::str::from_utf8(&data[name_start..cur]).unwrap_or("");
        cur = (cur + 1 + 3) & !3; // NUL-terminated, padded to 4-byte boundary
        if name == "#~" || name == "#-" {
            tilde = Some((md_off + s_off, s_sz));
        } else if name == "#Strings" {
            strings = Some((md_off + s_off, s_sz));
        }
    }
    let (t_off, t_sz) = tilde.ok_or("missing #~ stream")?;
    let (str_off, str_sz) = strings.ok_or("missing #Strings stream")?;
    let str_heap = data.get(str_off..str_off + str_sz).ok_or("bad #Strings")?;
    let t_data = data.get(t_off..t_off + t_sz).ok_or("bad #~")?;

    // #~ header (ECMA-335 II.24.2.6)
    let heap_sizes = *t_data.get(6).ok_or("heap_sizes")?;
    let str_ix: usize = if (heap_sizes & 1) != 0 { 4 } else { 2 };
    let guid_ix: usize = if (heap_sizes & 2) != 0 { 4 } else { 2 };
    let blob_ix: usize = if (heap_sizes & 4) != 0 { 4 } else { 2 };
    let valid = u64_le(t_data, 8).ok_or("valid")?;
    let mut pos = 24usize;
    let mut counts = [0u32; 64];
    for t in 0..64 {
        if (valid >> t) & 1 != 0 {
            counts[t] = u32_le(t_data, pos).ok_or("row count")?;
            pos += 4;
        }
    }
    // Helper for simple table index size and coded index size (ECMA-335 II.24.2.6)
    let tidx = |t: usize| -> usize { if counts[t] >= 65536 { 4 } else { 2 } };
    let coded = |bits: u32, tables: &[usize]| -> usize {
        let max_rows = tables.iter().map(|&t| counts[t]).max().unwrap_or(0);
        if max_rows < (1u32 << (16 - bits)) { 2 } else { 4 }
    };
    let type_def_or_ref = coded(2, &[0x02, 0x01, 0x1B]);
    let has_constant = coded(2, &[0x04, 0x08, 0x17]);
    let has_custom_attr = coded(5, &[0x06, 0x04, 0x01, 0x02, 0x08, 0x09, 0x0A, 0x00, 0x0E, 0x17, 0x14, 0x11, 0x1A, 0x1B, 0x20, 0x23, 0x26, 0x27, 0x28, 0x2A, 0x2C, 0x2B]);
    let has_field_marshal = coded(1, &[0x04, 0x08]);
    let has_decl_sec = coded(2, &[0x02, 0x06, 0x20]);
    let member_ref_parent = coded(3, &[0x02, 0x01, 0x1A, 0x06, 0x1B]);
    let has_semantics = coded(1, &[0x14, 0x17]);
    let method_def_or_ref = coded(1, &[0x06, 0x0A]);
    let member_forwarded = coded(1, &[0x04, 0x06]);
    let implementation = coded(2, &[0x26, 0x23, 0x27]);
    let custom_attr_type = coded(3, &[0x06, 0x0A]);
    let resolution_scope = coded(2, &[0x00, 0x1A, 0x23, 0x01]);
    let type_or_method_def = coded(1, &[0x02, 0x06]);

    // Walk tables 0x00..0x27 to reach table 0x28 (ManifestResource)
    let row_size = |t: usize| -> Result<usize, String> {
        Ok(match t {
            0x00 => 2 + str_ix + guid_ix * 3,                                           // Module
            0x01 => resolution_scope + str_ix * 2,                                      // TypeRef
            0x02 => 4 + str_ix * 2 + type_def_or_ref + tidx(0x04) + tidx(0x06),         // TypeDef
            0x04 => 2 + str_ix + blob_ix,                                               // Field
            0x06 => 4 + 2 + 2 + str_ix + blob_ix + tidx(0x08),                          // MethodDef
            0x08 => 2 + 2 + str_ix,                                                     // Param
            0x09 => tidx(0x02) + type_def_or_ref,                                       // InterfaceImpl
            0x0A => member_ref_parent + str_ix + blob_ix,                               // MemberRef
            0x0B => 2 + has_constant + blob_ix,                                         // Constant
            0x0C => has_custom_attr + custom_attr_type + blob_ix,                       // CustomAttribute
            0x0D => has_field_marshal + blob_ix,                                        // FieldMarshal
            0x0E => 2 + has_decl_sec + blob_ix,                                         // DeclSecurity
            0x0F => 2 + 4 + tidx(0x02),                                                 // ClassLayout
            0x10 => 4 + tidx(0x04),                                                     // FieldLayout
            0x11 => blob_ix,                                                            // StandAloneSig
            0x12 => tidx(0x02) + tidx(0x14),                                            // EventMap
            0x14 => 2 + str_ix + type_def_or_ref,                                       // Event
            0x15 => tidx(0x02) + tidx(0x17),                                            // PropertyMap
            0x17 => 2 + str_ix + blob_ix,                                               // Property
            0x18 => 2 + tidx(0x06) + has_semantics,                                     // MethodSemantics
            0x19 => tidx(0x02) + method_def_or_ref * 2,                                 // MethodImpl
            0x1A => str_ix,                                                             // ModuleRef
            0x1B => blob_ix,                                                            // TypeSpec
            0x1C => 2 + member_forwarded + str_ix + tidx(0x1A),                         // ImplMap
            0x1D => 4 + tidx(0x04),                                                     // FieldRVA
            0x20 => 4 + 2 * 4 + 4 + blob_ix + str_ix * 2,                               // Assembly
            0x21 => 4,                                                                  // AssemblyProcessor
            0x22 => 4 * 3,                                                              // AssemblyOS
            0x23 => 2 * 4 + 4 + blob_ix + str_ix * 2 + blob_ix,                         // AssemblyRef
            0x24 => 4 + tidx(0x23),                                                     // AssemblyRefProcessor
            0x25 => 4 * 3 + tidx(0x23),                                                 // AssemblyRefOS
            0x26 => 4 + str_ix + blob_ix,                                               // File
            0x27 => 4 + 4 + str_ix * 2 + implementation,                                // ExportedType
            0x28 => 4 + 4 + str_ix + implementation,                                    // ManifestResource
            _ => return Err(format!("Unsupported metadata table 0x{t:02x} before ManifestResource")),
        })
    };

    for t in 0..0x28usize {
        if counts[t] > 0 {
            pos += counts[t] as usize * row_size(t)?;
        }
    }
    let mr_count = counts[0x28] as usize;
    let mr_row = row_size(0x28)?;
    let _ = type_or_method_def;

    std::fs::create_dir_all(dest_dir).map_err(|e| e.to_string())?;
    let mut extracted = 0usize;
    for i in 0..mr_count {
        let ro = pos + i * mr_row;
        let offset = u32_le(t_data, ro).ok_or("mr offset")? as usize;
        let name_ix = if str_ix == 4 { u32_le(t_data, ro + 8).ok_or("mr name")? as usize } else { u16_le(t_data, ro + 8).ok_or("mr name")? as usize };
        let impl_val = if implementation == 4 { u32_le(t_data, ro + 8 + str_ix).ok_or("mr impl")? } else { u16_le(t_data, ro + 8 + str_ix).ok_or("mr impl")? as u32 };
        if impl_val != 0 {
            continue; // external resource, not embedded in this DLL
        }
        let Some(tail) = str_heap.get(name_ix..) else { continue };
        let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
        let Ok(res_name) = std::str::from_utf8(&tail[..end]) else { continue };
        let Some(fname) = res_name.strip_prefix(PREFIX) else { continue };
        if fname.is_empty() || fname.contains('/') || fname.contains('\\') || fname.contains("..") {
            continue;
        }
        let Some(len) = u32_le(res_blob, offset) else { continue };
        let Some(bytes) = res_blob.get(offset + 4..offset + 4 + len as usize) else { continue };
        if std::fs::write(dest_dir.join(fname), bytes).is_ok() {
            extracted += 1;
        }
    }
    Ok(extracted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_lazer_legacy_default_skin_when_installed() {
        let Some(dir) = legacy_default_skin_dir() else {
            return; // osu!lazer not installed on this runner (e.g. CI)
        };
        assert!(dir.join("hitcircle@2x.png").is_file(), "hitcircle@2x.png missing in {}", dir.display());
        assert!(dir.join("hitcircleoverlay@2x.png").is_file());
        assert!(dir.join("default-0@2x.png").is_file());
        assert!(dir.join("sliderb0@2x.png").is_file());
        assert!(dir.join("cursor@2x.png").is_file());
    }
}
