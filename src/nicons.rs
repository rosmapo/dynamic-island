//! Ikony notifikácií: zo zadania aplikácie (`image-data`, `image-path`, `app_icon`) vyrobí cestu k PNG súboru,
//! ktorú vie vykresliť `IconCache`. Beží vo vlákne D-Bus servera, nie v hlavnej slučke.
//!
//! Poradie podľa špecifikácie: `image-data` > `image-path` > `app_icon`. Podporuje sa iba PNG (SVG sa nekreslí;
//! v tom prípade sa zobrazí písmeno aplikácie). Názov ikony sa hľadá v téme ikon (hicolor a ostatné nainštalované
//! témy, rozmery 64 → 24 px) a v `/usr/share/pixmaps`.
use crate::dbus::V;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Adresár pre ikony vyrobené z `image-data` (`~/.cache/dynamic-island/notif-icons`).
pub fn cache_dir() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var("HOME").ok().filter(|s| !s.is_empty()).map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("dynamic-island").join("notif-icons")
}

/// Zmaže súbory ikon staršie ako `days` dní (volá sa raz pri štarte).
pub fn cleanup(days: u64) {
    cleanup_dir(&cache_dir(), days);
}

/// Zmaže súbory v adresári `dir` staršie ako `days` dní.
pub fn cleanup_dir(dir: &Path, days: u64) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let limit = std::time::Duration::from_secs(days * 86400);
    for e in rd.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|a| a > limit).unwrap_or(false);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Vyrieši ikonu notifikácie na cestu k PNG súboru (prázdny reťazec = žiadna použiteľná ikona).
pub fn resolve(app_icon: &str, image_path: &str, image_data: Option<&V>) -> String {
    if let Some(p) = image_data.and_then(from_image_data) {
        return p;
    }
    for s in [image_path, app_icon] {
        if let Some(p) = from_name_or_path(s) {
            return p.to_string_lossy().into_owned();
        }
    }
    String::new()
}

fn from_name_or_path(s: &str) -> Option<PathBuf> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let s = s.strip_prefix("file://").map(|r| r.replace("%20", " ")).unwrap_or_else(|| s.to_string());
    if s.starts_with('/') {
        let p = PathBuf::from(&s);
        let png = p.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("png")).unwrap_or(false);
        return if png && p.is_file() { Some(p) } else { None };
    }
    find_theme_icon(&s)
}

// ------------------------------------------------------------ image-data

/// `image-data`: štruktúra `(iiibiiay)` = šírka, výška, rowstride, má alfu, bity na zložku, počet zložiek, dáta.
fn from_image_data(v: &V) -> Option<String> {
    let V::Struct(f) = v else { return None };
    if f.len() < 7 {
        return None;
    }
    let (w, h, stride) = (f[0].as_i32()?, f[1].as_i32()?, f[2].as_i32()?);
    let bits = f[4].as_i32()?;
    let ch = f[5].as_i32()?;
    let V::Arr(arr) = &f[6] else { return None };
    if bits != 8 || !(1..=1024).contains(&w) || !(1..=1024).contains(&h) || stride < 0 {
        return None;
    }
    let data: Vec<u8> = arr.iter().filter_map(|b| if let V::Byte(x) = b { Some(*x) } else { None }).collect();
    let rgba = to_rgba(w as usize, h as usize, stride as usize, ch as usize, &data)?;
    let dir = cache_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let name = format!("{:016x}.png", fnv(&rgba, w as u32, h as u32));
    let path = dir.join(name);
    if !path.is_file() {
        write_png(&path, w as u32, h as u32, &rgba)?;
    }
    Some(path.to_string_lossy().into_owned())
}

/// Zloží tesné RGBA (8 bit) zo zdroja s riadkovým posunom `stride` a 3 alebo 4 zložkami.
pub fn to_rgba(w: usize, h: usize, stride: usize, channels: usize, data: &[u8]) -> Option<Vec<u8>> {
    if w == 0 || h == 0 || !(channels == 3 || channels == 4) || stride < w * channels || data.len() < (h - 1) * stride + w * channels {
        return None;
    }
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = &data[y * stride..y * stride + w * channels];
        for px in row.chunks_exact(channels) {
            out.extend_from_slice(&[px[0], px[1], px[2], if channels == 4 { px[3] } else { 255 }]);
        }
    }
    Some(out)
}

pub fn fnv(data: &[u8], w: u32, h: u32) -> u64 {
    let mut x: u64 = 0xcbf2_9ce4_8422_2325;
    for b in w.to_le_bytes().iter().chain(h.to_le_bytes().iter()).chain(data.iter()) {
        x ^= *b as u64;
        x = x.wrapping_mul(0x0000_0100_0000_01b3);
    }
    x
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Option<()> {
    let tmp = path.with_extension("tmp");
    let file = std::fs::File::create(&tmp).ok()?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().ok()?;
    wr.write_image_data(rgba).ok()?;
    drop(wr);
    std::fs::rename(&tmp, path).ok()
}

// ------------------------------------------------------------ téma ikon

fn cache() -> &'static Mutex<HashMap<String, Option<PathBuf>>> {
    static C: OnceLock<Mutex<HashMap<String, Option<PathBuf>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Nájde PNG ikonu podľa názvu (bez prípony); výsledok (aj neúspech) sa pamätá.
pub fn find_theme_icon(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains('/') || name.contains("..") {
        return None;
    }
    if let Some(hit) = cache().lock().ok().and_then(|c| c.get(name).cloned()) {
        return hit;
    }
    let found = search_theme_icon(name);
    if let Ok(mut c) = cache().lock() {
        c.insert(name.to_string(), found.clone());
    }
    found
}

const SIZES: [&str; 8] = ["64x64", "48x48", "96x96", "128x128", "256x256", "32x32", "512x512", "24x24"];
const CONTEXTS: [&str; 9] = ["apps", "status", "actions", "devices", "categories", "mimetypes", "places", "emblems", "notifications"];

fn search_theme_icon(name: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let bases = [
        PathBuf::from(&home).join(".local/share/icons"),
        PathBuf::from(&home).join(".icons"),
        PathBuf::from("/usr/local/share/icons"),
        PathBuf::from("/usr/share/icons"),
    ];
    let file = format!("{name}.png");
    for base in &bases {
        let Ok(rd) = std::fs::read_dir(base) else { continue };
        let mut themes: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        // hicolor je štandardná záloha s PNG ikonami aplikácií – ide prvá
        themes.sort_by_key(|p| (p.file_name().map(|n| n != "hicolor").unwrap_or(true), p.clone()));
        for theme in themes {
            for size in SIZES {
                let short = size.split('x').next().unwrap_or(size);
                for ctx in CONTEXTS {
                    for cand in [theme.join(size).join(ctx).join(&file), theme.join(ctx).join(short).join(&file)] {
                        if cand.is_file() {
                            return Some(cand);
                        }
                    }
                }
            }
        }
    }
    let px = Path::new("/usr/share/pixmaps").join(&file);
    if px.is_file() {
        return Some(px);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_with_stride_to_rgba() {
        // 2×2, 3 zložky, rowstride 8 (2 bajty výplne na riadok)
        let data = [1, 2, 3, 4, 5, 6, 0, 0, 7, 8, 9, 10, 11, 12, 0, 0];
        let out = to_rgba(2, 2, 8, 3, &data).unwrap();
        assert_eq!(out, vec![1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]);
    }

    #[test]
    fn rgba_passthrough_and_invalid() {
        let data = [9, 8, 7, 6];
        assert_eq!(to_rgba(1, 1, 4, 4, &data).unwrap(), vec![9, 8, 7, 6]);
        assert!(to_rgba(1, 1, 4, 2, &data).is_none());
        assert!(to_rgba(2, 1, 4, 4, &data).is_none());
    }

    #[test]
    fn path_forms() {
        assert!(from_name_or_path("").is_none());
        assert!(from_name_or_path("/nonexistent/icon.png").is_none());
        assert!(from_name_or_path("/etc/hostname").is_none());
        assert!(find_theme_icon("../x").is_none());
    }

    #[test]
    fn hash_differs_by_size() {
        assert_ne!(fnv(&[1, 2, 3, 4], 1, 1), fnv(&[1, 2, 3, 4], 2, 1));
    }
}
