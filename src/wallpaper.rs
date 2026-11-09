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
    /// stav (`current`, `applied-theme`) a náhľady (`thumbs/`)
    pub cache_dir: String,
    /// adresár s témami (`*.json` pre `command -t`)
    pub themes_dir: String,
    /// šírka rozbaľovacej ponuky tém (px)
    pub theme_width: f32,
    /// tlačidlá v hlavičke (`buttonN_label` / `buttonN_command`; bez labelu sa tlačidlo nezobrazí)
    pub buttons: Vec<WallButton>,
    /// najväčšia šírka tlačidla (px); bez nej si tlačidlá rozdelia celý priestor
    pub button_max_width: Option<f32>,
    /// rámik okolo náhľadu pod kurzorom
    pub hover_ring: bool,
    /// rámik okolo náhľadu aktuálneho pozadia
    pub current_ring: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WallButton {
    pub label: String,
    pub command: String,
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
            themes_dir: t.str_or("themes_directory", "~/.config/matugen/themes"),
            theme_width: t.f64_or("theme_width", 170.0).clamp(80.0, 600.0) as f32,
            buttons: (1..=16)
                .filter_map(|i| {
                    let label = t.str_or(&format!("button{i}_label"), "");
                    (!label.trim().is_empty()).then(|| WallButton { label, command: t.str_or(&format!("button{i}_command"), "") })
                })
                .collect(),
            hover_ring: t.bool_or("hover_ring", true),
            current_ring: t.bool_or("current_ring", true),
            button_max_width: Some(t.f64_or("button_max_width", 0.0)).filter(|w| *w > 0.0).map(|w| w.clamp(40.0, 1000.0) as f32),
        }
    }

    /// Rozmery náhľadu v logických px.
    pub fn thumb_size(&self) -> (f32, f32) {
        (self.thumb_w, (self.thumb_w * 9.0 / 16.0).round())
    }
}

/// Rozloženie hlavičky v rozsahu `x0..x0+w`: (x rollout ponuky, šírka rollout, [(x, šírka)] tlačidiel).
/// Pri párnom počte tlačidiel (aj 0) je rollout v strede a tlačidlá sú rozdelené na polovice vľavo a vpravo,
/// pri nepárnom je rollout vpravo a všetky tlačidlá vľavo. Tlačidlá sú rovnako široké; bez `max_btn` vyplnia
/// celú svoju oblasť, s ním sa ľavá skupina radí od ľavého okraja a pravá je zarovnaná k pravému okraju.
pub fn header_layout(x0: f32, w: f32, rollout_w: f32, n: usize, max_btn: Option<f32>, gap: f32) -> (f32, f32, Vec<(f32, f32)>) {
    let rw = rollout_w.min(w);
    let centered = n % 2 == 0;
    let rx = if centered { x0 + (w - rw) * 0.5 } else { x0 + w - rw };
    let (left_cnt, right_cnt) = if centered { (n / 2, n / 2) } else { (n, 0) };
    let mut v = Vec::new();
    let mut place = |start: f32, end: f32, cnt: usize, to_right: bool| {
        if cnt == 0 {
            return;
        }
        let avail = ((end - start).max(0.0) - (cnt as f32 - 1.0) * gap).max(0.0);
        let mut bw = avail / cnt as f32;
        if let Some(m) = max_btn {
            bw = bw.min(m);
        }
        let total = cnt as f32 * bw + (cnt as f32 - 1.0) * gap;
        let first = if to_right { end - total } else { start };
        for k in 0..cnt {
            v.push((first + k as f32 * (bw + gap), bw));
        }
    };
    place(x0, rx - gap, left_cnt, false);
    place(rx + rw + gap, x0 + w, right_cnt, true);
    (rx, rw, v)
}

/// Témy z adresára: (názov bez `.json`, cesta), zoradené podľa názvu.
pub fn list_themes(dir: &Path) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("json")).unwrap_or(false))
            .filter_map(|p| Some((p.file_stem()?.to_string_lossy().into_owned(), p.to_string_lossy().into_owned())))
            .collect(),
        Err(_) => Vec::new(),
    };
    v.sort_by_key(|(n, _)| n.to_lowercase());
    v
}

/// Názov aktuálne nastavenej témy zo súboru `applied-theme` (prázdny = režim „wallpaper“).
pub fn read_applied_theme(cfg: &WallCfg) -> String {
    std::fs::read_to_string(Path::new(&expand(&cfg.cache_dir)).join("applied-theme")).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// Návrat do režimu „wallpaper“: zmaže zapamätanú tému (farby sa zmenia až pri ďalšom nastavení pozadia).
pub fn clear_theme(cfg: &WallCfg) {
    let _ = std::fs::remove_file(Path::new(&expand(&cfg.cache_dir)).join("applied-theme"));
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
        tx.send(Msg::WallThemes(list_themes(Path::new(&expand(&cfg.themes_dir))), read_applied_theme(&cfg)));
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

/// Nastaví pozadie: `command -v CESTA` (pozadie + farby z obrázka) alebo `command -w CESTA` (`background_only`,
/// pri vybranej téme). Notifikácie o chybách posiela skript; po úspechu sa zapíše `current` a príde `Msg::WallCurrent`.
/// Beží vo vlastnom vlákne, zmeny sa serializujú.
pub fn apply(cfg: WallCfg, path: String, background_only: bool, tx: Tx<Msg>) {
    std::thread::spawn(move || {
        let _g = APPLY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let st = Command::new(expand(&cfg.command)).arg(if background_only { "-w" } else { "-v" }).arg(&path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
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

/// Nastaví tému: `command -t CESTA` (skript zapíše `applied-theme`); po úspechu príde `Msg::WallTheme(názov)`.
pub fn set_theme(cfg: WallCfg, name: String, path: String, tx: Tx<Msg>) {
    std::thread::spawn(move || {
        let _g = APPLY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let st = Command::new(expand(&cfg.command)).arg("-t").arg(&path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
        match st {
            Ok(s) if s.success() => tx.send(Msg::WallTheme(name)),
            Ok(s) => notify_error(&format!("{} -t skončil s kódom {:?}", cfg.command, s.code())),
            Err(e) => notify_error(&format!("{} sa nepodarilo spustiť: {e}", cfg.command)),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toml;

    #[test]
    fn header_layouts() {
        // 0 tlačidiel: rollout v strede
        let (rx, rw, b) = header_layout(0.0, 600.0, 200.0, 0, None, 10.0);
        assert_eq!((rx, rw, b.len()), (200.0, 200.0, 0));
        // 4 tlačidlá: 2 vľavo, rollout v strede, 2 vpravo; rovnaká šírka
        let (rx, _, b) = header_layout(0.0, 600.0, 200.0, 4, None, 10.0);
        assert_eq!(rx, 200.0);
        assert_eq!(b.len(), 4);
        assert!(b.iter().all(|(_, w)| (*w - 90.0).abs() < 0.01));
        assert!((b[0].0 - 0.0).abs() < 0.01 && (b[1].0 - 100.0).abs() < 0.01 && (b[2].0 - 410.0).abs() < 0.01);
        // 3 tlačidlá: rollout vpravo, všetky vľavo
        let (rx, _, b) = header_layout(0.0, 600.0, 200.0, 3, None, 10.0);
        assert_eq!(rx, 400.0);
        assert_eq!(b.len(), 3);
        assert!((b[0].1 - (390.0 - 20.0) / 3.0).abs() < 0.01);
        // max šírka: tlačidlá sa radia zľava
        let (_, _, b) = header_layout(0.0, 600.0, 200.0, 3, Some(60.0), 10.0);
        assert!(b.iter().all(|(_, w)| *w == 60.0));
        assert_eq!(b[1].0, 70.0);
        // párny počet s max šírkou: ľavá skupina od ľavého okraja, pravá zarovnaná k pravému okraju (nie k rollout)
        let (rx, rw, b) = header_layout(0.0, 600.0, 200.0, 4, Some(60.0), 10.0);
        assert_eq!((rx, rw), (200.0, 200.0));
        assert!(b.iter().all(|(_, w)| *w == 60.0));
        assert_eq!((b[0].0, b[1].0), (0.0, 70.0));
        assert_eq!((b[2].0, b[3].0), (470.0, 540.0));
        assert_eq!(b[3].0 + b[3].1, 600.0);
    }

    #[test]
    fn buttons_and_themes_config() {
        let t = toml::parse("button1_label=\"GTK\"\nbutton1_command=\"nwg-look\"\nbutton3_label=\"Qt6\"\nbutton3_command=\"qt6ct\"\nbutton_max_width=120\n").unwrap();
        let c = WallCfg::from_table(&t);
        assert_eq!(c.buttons.len(), 2);
        assert_eq!((c.buttons[1].label.as_str(), c.buttons[1].command.as_str()), ("Qt6", "qt6ct"));
        assert_eq!(c.button_max_width, Some(120.0));
        let d = WallCfg::from_table(&Table::new());
        assert!(d.buttons.is_empty() && d.button_max_width.is_none());
        assert_eq!(d.themes_dir, "~/.config/matugen/themes");
        assert!(d.hover_ring && d.current_ring);
        let r = WallCfg::from_table(&toml::parse("hover_ring=false\ncurrent_ring=false\n").unwrap());
        assert!(!r.hover_ring && !r.current_ring);
    }

    #[test]
    fn themes_listing() {
        let dir = std::env::temp_dir().join(format!("di-wall-themes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for n in ["Zima.json", "amber.json", "x.txt"] {
            std::fs::write(dir.join(n), b"{}").unwrap();
        }
        let v = list_themes(&dir);
        assert_eq!(v.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["amber", "Zima"]);
        std::fs::remove_dir_all(&dir).ok();
    }

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
