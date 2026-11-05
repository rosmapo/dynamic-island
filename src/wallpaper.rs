//! Modul wallpaper: mriežka náhľadov obrázkov; kliknutie spustí externý skript `command -v CESTA`.
//! Náhľady (PNG so zaoblenými rohmi) vyrába ImageMagick do vlastnej cache.
use crate::config::expand;
use crate::data::Msg;
use crate::sys::Tx;
use crate::toml::{Table, TableExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

const EXTENSIONS: [&str; 6] = ["jpg", "jpeg", "png", "bmp", "gif", "webp"];

/// Polomer zaoblenia rohov náhľadu (px náhľadu).
pub const THUMB_RADIUS: f32 = 6.0;

/// Nastavenia sekcie `[wallpaper]`.
#[derive(Clone, Debug)]
pub struct WallCfg {
    pub dir: String,
    pub columns: usize,
    /// počet viditeľných riadkov mriežky (viac → posuvník)
    pub rows: usize,
    /// šírka náhľadu v logických px (výška = 9/16); náhľad sa vyrobí presne v tejto veľkosti
    pub thumb_w: f32,
    /// skript, ktorý nastaví pozadie: `command -v CESTA`
    pub command: String,
    /// stav (`current`) a náhľady (`thumbs/`)
    pub cache_dir: String,
}

impl WallCfg {
    pub fn from_table(t: &Table) -> WallCfg {
        WallCfg {
            dir: t.str_or("directory", "~/.config/wayfire/media/wallpaper"),
            columns: t.f64_or("columns", 4.0).clamp(1.0, 12.0) as usize,
            rows: t.f64_or("rows", 4.0).clamp(1.0, 20.0) as usize,
            thumb_w: t.f64_or("thumb_width", 160.0).clamp(60.0, 600.0) as f32,
            command: t.str_or("command", "~/.config/wayfire/scripts/wallpaper"),
            cache_dir: t.str_or("cache_dir", "~/.cache/wallpaper-chooser"),
        }
    }

    /// Rozmery náhľadu v logických px.
    pub fn thumb_size(&self) -> (f32, f32) {
        (self.thumb_w, (self.thumb_w * 9.0 / 16.0).round())
    }
}

/// Jeden obrázok v mriežke.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct WallEntry {
    pub path: String,
    pub thumb: String,
    pub ready: bool,
}

/// Obrázky z adresára (zoradené podľa mena), len podporované prípony.
pub fn list_images(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .and_then(|e| e.to_str())
                        .map(|e| EXTENSIONS.contains(&e.to_lowercase().as_str()))
                        .unwrap_or(false)
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    v.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default());
    v
}

/// FNV-1a 64-bit.
fn fnv(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Názov súboru náhľadu: závisí od cesty, veľkosti, času zmeny a rozmeru náhľadu.
fn thumb_path(cache: &Path, src: &Path, w: u32, h: u32) -> PathBuf {
    let (size, mtime) = std::fs::metadata(src)
        .map(|m| (m.len(), m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)))
        .unwrap_or((0, 0));
    let key = format!("{}|{}|{}", src.to_string_lossy(), size, mtime);
    cache.join(format!("{:016x}-{}x{}r{}.png", fnv(key.as_bytes()), w, h, THUMB_RADIUS as u32))
}

/// Vyrobí náhľad (výrez na stred do pomeru strán, zaoblené rohy cez alfa kanál).
/// Skúša `magick`, potom `convert`.
fn make_thumb(src: &Path, dst: &Path, w: u32, h: u32) -> bool {
    let tmp = dst.with_extension("tmp");
    let geom = format!("{w}x{h}");
    let src_arg = format!("{}[0]", src.to_string_lossy());
    let out_arg = format!("png:{}", tmp.to_string_lossy());
    let r = THUMB_RADIUS as u32;
    let mask = format!("roundrectangle 0,0 {},{} {r},{r}", w - 1, h - 1);
    for bin in ["magick", "convert"] {
        let st = Command::new(bin)
            .args([&src_arg, "-auto-orient", "-thumbnail", &format!("{geom}^"), "-gravity", "center", "-extent", &geom])
            .args(["(", "-size", &geom, "xc:black", "-fill", "white", "-draw", &mask, ")"])
            .args(["-alpha", "off", "-compose", "CopyOpacity", "-composite", &out_arg])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        match st {
            Ok(s) if s.success() => return std::fs::rename(&tmp, dst).is_ok(),
            Ok(_) => {
                let _ = std::fs::remove_file(&tmp);
                return false;
            }
            Err(_) => continue, // binárka neexistuje – skús ďalšiu
        }
    }
    false
}

/// Cesta k aktuálnemu pozadiu (zapamätaná skriptom `wallpaper` aj ostrovčekom).
pub fn read_current(cfg: &WallCfg) -> String {
    std::fs::read_to_string(Path::new(&expand(&cfg.cache_dir)).join("current")).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// Vlákno pri otvorení panela: pošle zoznam (hneď) a potom dorába chýbajúce náhľady.
pub fn load(cfg: WallCfg, tx: Tx<Msg>) {
    std::thread::spawn(move || {
        let imgs = list_images(Path::new(&expand(&cfg.dir)));
        let cache = PathBuf::from(expand(&cfg.cache_dir)).join("thumbs");
        let _ = std::fs::create_dir_all(&cache);
        let (tw, th) = cfg.thumb_size();
        let (w, h) = (tw as u32, th as u32);
        let entries: Vec<WallEntry> = imgs
            .iter()
            .map(|p| {
                let t = thumb_path(&cache, p, w, h);
                WallEntry { path: p.to_string_lossy().into_owned(), ready: t.is_file(), thumb: t.to_string_lossy().into_owned() }
            })
            .collect();
        let todo: Vec<usize> = (0..entries.len()).filter(|&i| !entries[i].ready).collect();
        let paths: Vec<(String, String)> = entries.iter().map(|e| (e.path.clone(), e.thumb.clone())).collect();
        tx.send(Msg::Wall(entries, read_current(&cfg)));
        for i in todo {
            let (src, dst) = &paths[i];
            if make_thumb(Path::new(src), Path::new(dst), w, h) {
                tx.send(Msg::WallThumb(i, dst.clone()));
            }
        }
    });
}

static APPLY_LOCK: Mutex<()> = Mutex::new(());

fn notify_error(msg: &str) {
    eprintln!("dynamic-island: wallpaper: {msg}");
}

fn write_current(cfg: &WallCfg, path: &str) {
    let dir = PathBuf::from(expand(&cfg.cache_dir));
    let _ = std::fs::create_dir_all(&dir);
    let tmp = dir.join("current.tmp");
    if std::fs::write(&tmp, path).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join("current"));
    }
}

/// Spustí `command -v CESTA` (notifikácie o chybách posiela skript); po úspechu zapíše `current`
/// a pošle `Msg::WallCurrent`. Beží vo vlastnom vlákne, zmeny sa serializujú.
pub fn apply(cfg: WallCfg, path: String, tx: Tx<Msg>) {
    std::thread::spawn(move || {
        let _g = APPLY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let st = Command::new(expand(&cfg.command)).arg("-v").arg(&path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
        match st {
            Ok(s) if s.success() => {
                write_current(&cfg, &path);
                tx.send(Msg::WallCurrent(path));
            }
            Ok(s) => notify_error(&format!("{} skončil s kódom {:?}", cfg.command, s.code())),
            Err(e) => notify_error(&format!("{} sa nepodarilo spustiť: {e}", cfg.command)),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toml;

    #[test]
    fn config_defaults() {
        let c = WallCfg::from_table(&Table::new());
        assert_eq!((c.columns, c.rows), (4, 4));
        assert_eq!(c.cache_dir, "~/.cache/wallpaper-chooser");
        assert_eq!(c.thumb_size(), (160.0, 90.0));
        let t = toml::parse("columns=6\nthumb_width=200\ncommand=\"x\"\n").unwrap();
        let c = WallCfg::from_table(&t);
        assert_eq!((c.columns, c.thumb_size().1, c.command.as_str()), (6, 113.0, "x"));
    }

    #[test]
    fn hash_differs() {
        assert_ne!(fnv(b"a"), fnv(b"b"));
    }

    #[test]
    fn list_filters_and_sorts() {
        let dir = std::env::temp_dir().join(format!("di-wall-list-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for n in ["b.JPG", "a.png", "c.txt", "d.webp"] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        let v: Vec<String> = list_images(&dir).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(v, ["a.png", "b.JPG", "d.webp"]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
