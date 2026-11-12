//! Konfigurácia (TOML podmnožina) a farebná téma (vrátane matugen CSS súboru).
use crate::toml::{self, Table, TableExt, Value};
use std::collections::HashMap;
use std::path::PathBuf;

pub type Color = [f32; 4];

#[derive(Clone, Debug, Default)]
pub struct Btn {
    pub click: String,
    pub right: String,
    /// stredné tlačidlo (`on_click_middle`); predvolene bez akcie
    pub middle: String,
    pub tooltip: String,
}

#[derive(Clone, Debug)]
pub struct CustomCfg {
    pub name: String,
    pub exec: String,
    /// statický text/ikona bez procesu (alternatíva k `exec`)
    pub text: String,
    pub interval: f64, // 0 = trvalý proces, riadok = aktualizácia
    pub json: bool,
    /// sledovaný súbor: modul sa zobrazí ako odznak (ako CAPS), len kým súbor existuje; prázdne = bežný modul
    pub track_file: String,
    pub btn: Btn,
}

/// Pravidlo notifikácie: podreťazce sa hľadajú v súhrne / aplikácii / tele.
#[derive(Clone, Debug, Default)]
pub struct NotifRule {
    pub summary: String,
    pub app: String,
    pub body: String,
    pub on_click: String,
    pub timeout_ms: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKind {
    Icons,
    Text,
    /// Typ podľa položky: `icon` → ikona, `label` → text, `separator` → čiara.
    Hybrid,
}

/// Poloha čiary oddeľovača v jeho riadku (`separator = "top" | "center" | "bottom"`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SepAlign {
    Top,
    #[default]
    Center,
    Bottom,
}

#[derive(Clone, Debug, Default)]
pub struct MenuItemCfg {
    pub icon: String,
    pub label: String,
    pub command: String,
    /// Príkaz (alebo skript), ktorého prvý riadok výstupu sa zobrazí v položke (pozri `data::menu_item_text`).
    pub exec: String,
    /// Obnovovanie `exec` počas otvoreného menu v sekundách; 0 = spustí sa len raz pri otvorení menu.
    pub interval: f64,
    /// Vlastný riadok s oddelovačom; `icon`/`label`/`command` sa ignorujú.
    pub separator: bool,
    /// Poloha čiary oddeľovača v riadku (len pri `separator`).
    pub sep_align: SepAlign,
    /// Riadok textu cez celú šírku menu (`line = "text {}"`): text je v `label`, `exec`/`interval` fungujú ako pri
    /// bežnej textovej položke; prázdny text = prázdny riadok. `command` je voliteľný (klik na celý riadok).
    pub line: bool,
    /// Skupina sledovaného procesu (napr. `"radio"`): ostrovček si príkaz spustí sám, pamätá si ho (jeden proces na
    /// skupinu – nový štart predošlý ukončí) a modul s názvom skupiny ukáže ikonu, kým beží. Prázdne = bežný príkaz.
    pub track: String,
}

/// Vlastné menu (panel ako kalendár). Spúšťa sa cez `on_click = "meno_menu"`.
#[derive(Clone, Debug)]
pub struct MenuCfg {
    pub name: String,
    pub kind: MenuKind,
    pub title: String,
    /// Spoločný fallback pre `icon_columns` / `text_columns` (staré configy).
    pub columns: usize,
    /// Počet stĺpcov pre ikony (hybrid / icons).
    pub icon_columns: usize,
    /// Počet stĺpcov pre textové položky (hybrid / text).
    pub text_columns: usize,
    pub rows: Option<usize>,
    pub icon_size: f32,
    pub text_size: f32,
    pub cell_width: f32,
    pub gap: f32,
    pub padding: f32,
    /// Hrúbka čiary separátora (logické px).
    pub separator_thickness: f32,
    /// Dĺžka separátora ako podiel šírky obsahu (0.0–1.0).
    pub separator_length: f32,
    /// Menu sa zobrazí až po načítaní všetkých položiek s `exec` (alebo po `wait_ms`); inak hneď s prázdnymi hodnotami.
    pub wait_exec: bool,
    /// Najdlhšie čakanie pri `wait_exec` (ms); potom sa menu zobrazí aj bez dokončených výstupov.
    pub wait_ms: u64,
    pub items: Vec<MenuItemCfg>,
}

#[derive(Clone, Debug)]
pub struct AutoCfg {
    pub enabled: bool,
    pub bar_height: f64,
    pub edge: f64,
    pub edge_width: f64,
    pub margin: f64,
    pub poll_ms: u64,
    pub ignore: Vec<String>,
}

/// Modul sledovaného procesu: ľubovoľný názov v `modules` so sekciou `[názov]` (okrem vstavaných a `custom`).
/// Zobrazí sa, kým beží proces skupiny `group` (položka menu s `track`).
#[derive(Clone, Debug)]
pub struct TrackerCfg {
    pub name: String,
    pub group: String,
    pub icon: String,
    pub btn: Btn,
}

pub const BUILTIN_MODULES: [&str; 11] = ["network", "vpn", "clock", "keyboard", "audio", "capslock", "inhibitor", "recorder", "radio", "media", "disks"];

/// Spúšťač aplikácií (`dynamic-island --drun`).
#[derive(Clone, Debug, PartialEq)]
pub struct DrunCfg {
    pub columns: usize,
    pub rows: usize,
    /// "icon" | "name" | "icon+name-right" | "icon+name-below"
    pub show: String,
    pub icon_size: f32,
    pub name_size: f32,
    /// šírka / výška jednej bunky (0 = podľa režimu a veľkostí)
    pub cell_w: f32,
    pub cell_h: f32,
    pub padding: f32,
    pub radius: f32,
    pub cell_radius: f32,
    /// zoraďovanie (`order = "vertical"`): ďalšia položka je pod predošlou (po stĺpcoch), inak vedľa nej (po riadkoch)
    pub vertical: bool,
    /// zvýraznenie bunky pod ukazovateľom
    pub hover: bool,
    /// riadok hľadania vždy viditeľný (písaním sa filtruje aj bez neho)
    pub search: bool,
    pub terminal: String,
    pub icon_theme: String,
    /// názov prostredia pre OnlyShowIn / NotShowIn (prázdne = nekontrolovať)
    pub desktop: String,
}

impl DrunCfg {
    pub fn from_table(d: &Table) -> DrunCfg {
        let show = d.str_or("show", "icon");
        let show = match show.as_str() {
            "icon" | "name" | "icon+name-right" | "icon+name-below" => show,
            _ => "icon".to_string(),
        };
        DrunCfg {
            columns: d.f64_or("columns", 5.0).clamp(1.0, 20.0) as usize,
            rows: d.f64_or("rows", 4.0).clamp(1.0, 20.0) as usize,
            show,
            icon_size: d.f64_or("icon_size", 64.0).clamp(16.0, 256.0) as f32,
            name_size: d.f64_or("name_size", 14.0).clamp(8.0, 48.0) as f32,
            cell_w: d.f64_or("cell_width", 0.0).clamp(0.0, 600.0) as f32,
            cell_h: d.f64_or("cell_height", 0.0).clamp(0.0, 600.0) as f32,
            padding: d.f64_or("padding", 15.0).clamp(0.0, 100.0) as f32,
            radius: d.f64_or("radius", 30.0).clamp(0.0, 100.0) as f32,
            cell_radius: d.f64_or("cell_radius", 15.0).clamp(0.0, 100.0) as f32,
            vertical: matches!(d.str_or("order", "horizontal").trim().to_lowercase().as_str(), "vertical" | "vertikalne" | "columns"),
            hover: d.bool_or("hover", true),
            search: d.bool_or("search", false),
            terminal: d.str_or("terminal", "foot"),
            icon_theme: d.str_or("icon_theme", ""),
            desktop: d.str_or("desktop", ""),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub path: PathBuf,
    pub font_family: Vec<String>,
    pub icon_family: Vec<String>,
    pub font_size: f32,
    pub row_height: f32,
    pub top_margin: f32,
    pub radius: f32,
    pub radius_expanded: f32,
    pub opacity: f32,
    pub shadow: bool,
    /// animácia otvorenia okna v strede (spúšťač, panely zvonka); ostatné animácie sa nemenia
    pub window_animation: bool,
    pub surface_w: u32,
    pub surface_h: u32,
    pub hover_ms: u64,
    pub collapse_ms: u64,
    pub poll_ms: u64,
    pub colors_file: String,
    pub color_spec: HashMap<String, String>,
    pub modules: Vec<String>,

    pub clock_format: String,
    pub clock_locale: String,
    pub clock: Btn,

    pub net_iface: String,
    pub net_icons: Vec<String>,
    pub net_icon_eth: String,
    pub net_icon_off: String,
    pub net: Btn,

    pub vpn_ifaces: Vec<String>,
    pub vpn_text: String,
    pub vpn: Btn,

    pub audio_icon_muted: String,
    pub audio_icons: Vec<String>,
    pub audio_hud: bool,
    pub hud_ms: u64,
    pub audio: Btn,
    pub bright_icons: Vec<String>,
    pub vol_step: u32,
    pub vol_limit: f64,
    pub bright_step: u32,
    pub bright_get: String,
    pub bright_set: String,

    pub kb_names: HashMap<String, String>,
    pub kb: Btn,

    pub net_tip_wifi: String,
    pub net_tip_eth: String,
    pub net_tip_off: String,
    pub caps: Btn,
    pub caps_text: String,
    pub inh_icon: String,
    pub inh_file: String,
    pub inh: Btn,
    /// modul `radio`: ikona sledovaného procesu zo skupiny `radio` (pozri `MenuItemCfg::track`)
    pub radio_icon: String,
    pub radio: Btn,
    /// modul `media` („Práve hrá“, MPRIS): ikona pri prehrávaní / pozastavení, tlačidlá, ignorované prehrávače
    /// (podreťazec mena na zbernici), šírka panela, veľkosť obalu a písmo (logické px)
    pub media_icon: String,
    pub media_icon_paused: String,
    pub media: Btn,
    pub media_ignore: Vec<String>,
    pub media_w: f32,
    pub media_art: f32,
    pub media_font: f32,
    pub rec_file: String,
    pub rec: Btn,
    pub disks: Btn,
    pub disks_icon: String,
    pub disks_format: String,
    pub disks_format_many: String,
    pub disks_automount: bool,
    pub disks_notify: bool,
    pub disks_timeout: i32,

    pub custom: Vec<CustomCfg>,
    pub trackers: Vec<TrackerCfg>,
    pub menus: Vec<MenuCfg>,
    /// panel histórie schránky (cliphist) – `[clipboard]`
    pub clip_font: f32,
    pub clip_rows: usize,
    pub clip_width: f32,
    pub clip_max: usize,
    pub clip_list: String,
    pub clip_copy: String,
    /// modul wallpaper – `[wallpaper]`
    pub wall: crate::wallpaper::WallCfg,
    /// modul Wi-Fi – `[wifi]`
    pub wifi: crate::wifi::WifiCfg,
    /// panel audio – kľúče `panel_*` v `[audio]`
    pub audio_panel: crate::audio::AudioCfg,
    /// panel monitor – `[monitor]`
    pub monitor: crate::monitor::MonitorCfg,
    /// panel capture (snímka / nahrávanie obrazovky) – `[capture]`
    pub capture: crate::capture::CaptureCfg,
    /// panel bluetooth – `[bluetooth]`
    pub bluetooth: crate::bluetooth::BtCfg,
    /// kontajner so záložkami audio / bluetooth / wifi / monitor – `[quickset]`
    pub hw: crate::quickset::HwCfg,
    /// kôš (zoznam, obnova, vyprázdnenie) – `[trash]`
    pub trash: crate::trash::TrashCfg,
    pub auto: AutoCfg,

    pub notif_enabled: bool,
    pub notif_width: f32,
    pub notif_max: usize,
    pub notif_lines: usize,
    /// `[system]`: desatinný oddeľovač a rozhranie pre natívne zdroje `exec = "builtin:…"`
    pub sys_decimal: String,
    pub sys_net_iface: String,
    /// `[drun]`: spúšťač aplikácií (`--drun`)
    pub drun: DrunCfg,
    /// zobrazovať ikony notifikácií (vo vyskakovacom okne aj v histórii)
    pub notif_icons: bool,
    /// panel histórie notifikácií: šírka, výška (px), najviac riadkov tela na kartu
    pub hist_w: f32,
    pub hist_h: f32,
    pub hist_lines: usize,
    pub notif_font: f32,
    pub notif_timeout: i32,
    pub notif_urgent_timeout: i32,
    pub notif_border: f32,
    pub notif_center: bool,
    pub notif_click: String,
    pub notif_right: String,
    pub notif_rules: Vec<NotifRule>,
    pub history_file: PathBuf,
    pub history_max: usize,
}

pub fn expand(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Ok(h) = std::env::var("HOME") {
            return format!("{h}/{rest}");
        }
    }
    p.to_string()
}

pub fn default_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}/.config", std::env::var("HOME").unwrap_or_default()));
    PathBuf::from(base).join("dynamic-island/config.toml")
}

/// Zoznam písem: pole `["A", "B"]` alebo reťazec `"A, B"`; poradie = poradie náhrad.
fn font_list(t: &Table, key: &str, default: &[&str]) -> Vec<String> {
    if let Some(v) = t.strs(key) {
        return v;
    }
    if let Some(s) = t.get(key).and_then(|v| v.as_str()) {
        return s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
    }
    default.iter().map(|s| s.to_string()).collect()
}

fn btn(t: &Table, click: &str, right: &str, tip: &str) -> Btn {
    Btn {
        click: t.str_or("on_click", click),
        right: t.str_or("on_click_right", right),
        middle: t.str_or("on_click_middle", ""),
        tooltip: t.str_or("tooltip", tip),
    }
}

impl Config {
    pub fn load(path: Option<PathBuf>) -> Result<Config, String> {
        let path = path.unwrap_or_else(default_path);
        let t = match std::fs::read_to_string(&path) {
            Ok(s) => toml::parse(&s).map_err(|e| format!("{}: {e}", path.display()))?,
            Err(_) => Table::new(), // bez súboru: vstavané predvolené hodnoty
        };
        Ok(Config::from_table(&t, path))
    }

    pub fn from_table(t: &Table, path: PathBuf) -> Config {
        let mut c = Self::from_table_raw(t, path);
        // `[island] font_scale`: jednotný násobiteľ všetkých veľkostí písma (kapsula aj panely)
        let fs = t.sub("island").f64_or("font_scale", 1.0).clamp(0.5, 3.0) as f32;
        if (fs - 1.0).abs() > 1e-3 {
            c.font_size *= fs;
            c.media_font *= fs;
            c.notif_font *= fs;
            c.clip_font *= fs;
            c.wifi.font_size *= fs;
            c.monitor.font_size *= fs;
            c.capture.font_size *= fs;
            c.bluetooth.font_size *= fs;
            c.trash.font *= fs;
            c.audio_panel.font_size *= fs;
        }
        c
    }

    fn from_table_raw(t: &Table, path: PathBuf) -> Config {
        let isl = t.sub("island");
        // presne to, čo je v konfigurácii (bez skrytých náhradných písem); chýbajúci kľúč = predvolené
        let font_family = font_list(&isl, "font_family", &["Adwaita Sans", "DejaVu Sans"]);
        let icon_family = font_list(&isl, "icon_family", &["Hack Nerd Font Propo"]);

        let mut color_spec = HashMap::new();
        for (k, v) in t.sub("colors") {
            if let Some(s) = v.as_str() {
                color_spec.insert(k, s.to_string());
            }
        }

        let clip = t.sub("clipboard");
        let clock = t.sub("clock");
        let net = t.sub("network");
        let vpn = t.sub("vpn");
        let audio = t.sub("audio");
        let kb = t.sub("keyboard");
        let caps = t.sub("capslock");
        let inh = t.sub("inhibitor");
        let rec = t.sub("recorder");
        let radio = t.sub("radio");
        let media = t.sub("media");
        let system = t.sub("system");
        let dk = t.sub("disks");
        let auto = t.sub("autohide");
        let nt = t.sub("notifications");

        let mut kb_names: HashMap<String, String> = HashMap::new();
        kb_names.insert("Slovak".into(), "SK".into());
        kb_names.insert("English (US)".into(), "US".into());
        for (k, v) in kb.sub("names") {
            if let Some(s) = v.as_str() {
                kb_names.insert(k, s.to_string());
            }
        }

        let mut custom = Vec::new();
        if let Some(arr) = t.get("custom").and_then(|v| v.as_arr()) {
            for v in arr {
                if let Value::Table(c) = v {
                    let name = c.str_or("name", "");
                    let exec = c.str_or("exec", "");
                    let text = c.str_or("text", "");
                    if name.is_empty() || (exec.is_empty() && text.is_empty()) {
                        continue;
                    }
                    custom.push(CustomCfg {
                        name,
                        exec,
                        text,
                        interval: c.f64_or("interval", 0.0),
                        json: c.bool_or("json", true),
                        track_file: {
                            let f = c.str_or("track_file", "");
                            if f.trim().is_empty() { String::new() } else { expand(f.trim()) }
                        },
                        btn: btn(c, "", "", ""),
                    });
                }
            }
        }

        let mut menus = Vec::new();
        if let Some(arr) = t.get("menu").and_then(|v| v.as_arr()) {
            for v in arr {
                if let Value::Table(m) = v {
                    let name = m.str_or("name", "");
                    if name.is_empty() {
                        continue;
                    }
                    // Explicitný type, inak sa odvodí z položiek (čisté ikony → Icons, …).
                    let type_str = m.str_or("type", "");
                    let mut items = Vec::new();
                    if let Some(ia) = m.get("item").and_then(|v| v.as_arr()) {
                        for iv in ia {
                            if let Value::Table(it) = iv {
                                // `separator = "top" | "center" | "bottom"` (staré `true` = center; `false` / "" = nie je oddeľovač)
                                let sep_align = match it.get("separator") {
                                    Some(Value::Bool(true)) => Some(SepAlign::Center),
                                    Some(Value::Str(s)) => match s.trim().to_lowercase().as_str() {
                                        "" | "false" | "none" => None,
                                        "top" => Some(SepAlign::Top),
                                        "bottom" => Some(SepAlign::Bottom),
                                        _ => Some(SepAlign::Center),
                                    },
                                    _ => None,
                                };
                                if let Some(sep_align) = sep_align {
                                    items.push(MenuItemCfg {
                                        separator: true,
                                        sep_align,
                                        ..Default::default()
                                    });
                                    continue;
                                }
                                // `line = "text"`: riadok na celú šírku (aj prázdny = vynechaný riadok)
                                if let Some(text) = it.get("line").and_then(|v| v.as_str()) {
                                    items.push(MenuItemCfg {
                                        label: text.to_string(),
                                        command: it.str_or("command", ""),
                                        exec: it.str_or("exec", ""),
                                        interval: it.f64_or("interval", 0.0).max(0.0),
                                        line: true,
                                        ..Default::default()
                                    });
                                    continue;
                                }
                                let icon = it.str_or("icon", "");
                                let label = it.str_or("label", "");
                                let command = it.str_or("command", "");
                                let exec = it.str_or("exec", "");
                                if command.is_empty() && icon.is_empty() && label.is_empty() && exec.is_empty() {
                                    continue;
                                }
                                // Buď icon, alebo label — nie oboje; položka s `exec` je vždy textová (label = šablóna).
                                let (icon, label) = if !icon.is_empty() && exec.is_empty() {
                                    (icon, String::new())
                                } else {
                                    (String::new(), label)
                                };
                                items.push(MenuItemCfg {
                                    icon,
                                    label,
                                    command,
                                    exec,
                                    interval: it.f64_or("interval", 0.0).max(0.0),
                                    separator: false,
                                    sep_align: SepAlign::Center,
                                    line: false,
                                    track: it.str_or("track", "").trim().to_string(),
                                });
                            }
                        }
                    }
                    let kind = match type_str.as_str() {
                        "text" => MenuKind::Text,
                        "icons" => MenuKind::Icons,
                        "hybrid" => MenuKind::Hybrid,
                        _ => {
                            // bez type: odvodiť podľa obsahu
                            let has_sep = items.iter().any(|it| it.separator);
                            let has_icon = items.iter().any(|it| !it.icon.is_empty());
                            let has_label = items.iter().any(|it| !it.label.is_empty() || !it.exec.is_empty());
                            if has_sep || (has_icon && has_label) {
                                MenuKind::Hybrid
                            } else if has_label {
                                MenuKind::Text
                            } else {
                                MenuKind::Icons
                            }
                        }
                    };
                    // `columns` = spoločný fallback (staré configy).
                    // Ak chýba úplne a ide o čistý text, default 1 stĺpec (nie 3).
                    let columns_explicit = m.get("columns").and_then(|v| v.as_f64()).map(|v| v.max(1.0) as usize);
                    let columns = columns_explicit.unwrap_or(3);
                    let icon_columns = m
                        .get("icon_columns")
                        .and_then(|v| v.as_f64())
                        .map(|v| v.max(1.0) as usize)
                        .unwrap_or(columns);
                    let text_columns = m
                        .get("text_columns")
                        .and_then(|v| v.as_f64())
                        .map(|v| v.max(1.0) as usize)
                        .unwrap_or_else(|| {
                            if let Some(c) = columns_explicit {
                                c
                            } else if kind == MenuKind::Text {
                                1
                            } else {
                                columns
                            }
                        });
                    let rows = m.get("rows").and_then(|v| v.as_f64()).map(|v| v.max(1.0) as usize);
                    menus.push(MenuCfg {
                        name,
                        kind,
                        title: m.str_or("title", ""),
                        columns,
                        icon_columns,
                        text_columns,
                        rows,
                        icon_size: m.f64_or("icon_size", 28.0).max(8.0) as f32,
                        text_size: m.f64_or("text_size", 15.0).max(8.0) as f32,
                        cell_width: m.f64_or("cell_width", 120.0).max(20.0) as f32,
                        gap: m.f64_or("gap", 8.0).max(0.0) as f32,
                        padding: m.f64_or("padding", 12.0).max(0.0) as f32,
                        separator_thickness: m.f64_or("separator_thickness", 1.5).max(0.5) as f32,
                        separator_length: m.f64_or("separator_length", 0.6).clamp(0.1, 1.0) as f32,
                        wait_exec: m.bool_or("wait_exec", false),
                        wait_ms: m.f64_or("wait_timeout_ms", 1500.0).clamp(100.0, 30_000.0) as u64,
                        items,
                    });
                }
            }
        }

        let mut modules = t.strs("modules").unwrap_or_else(|| {
BUILTIN_MODULES.map(String::from).to_vec()
        });
        for c in &custom {
            if !modules.contains(&c.name) {
                modules.push(c.name.clone());
            }
        }

        let mut trackers = Vec::new();
        for name in &modules {
            if BUILTIN_MODULES.contains(&name.as_str()) || custom.iter().any(|c| &c.name == name) {
                continue;
            }
            let Some(sec) = t.get(name.as_str()).and_then(|v| v.as_table()) else { continue };
            let group = sec.str_or("track", name);
            let click = format!("track-stop:{group}");
            trackers.push(TrackerCfg { name: name.clone(), icon: sec.str_or("icon", "\u{f0439}"), btn: btn(sec, &click, "", "{name}\nKlik = vypnúť"), group });
        }

        Config {
            path: path.clone(),
            font_family,
            icon_family,
            font_size: isl.f64_or("font_size", 17.0) as f32,
            row_height: isl.f64_or("height", 34.0) as f32,
            top_margin: isl.f64_or("margin_top", 6.0) as f32,
            radius: isl.f64_or("radius", 17.0) as f32,
            radius_expanded: isl.f64_or("radius_expanded", 26.0) as f32,
            opacity: isl.f64_or("opacity", 0.75) as f32,
            shadow: isl.bool_or("shadow", true),
            window_animation: isl.bool_or("window_animation", true),
            surface_w: isl.f64_or("surface_width", 760.0) as u32,
            surface_h: isl.f64_or("surface_height", 560.0) as u32,
            hover_ms: isl.f64_or("tooltip_delay_ms", 450.0) as u64,
            collapse_ms: isl.f64_or("collapse_delay_ms", 700.0) as u64,
            poll_ms: isl.f64_or("poll_ms", 300.0).max(50.0) as u64,
            colors_file: isl.str_or("colors_file", "~/.config/waybar/waybar-colors.css"),
            color_spec,
            modules,

            clock_format: clock.str_or("format", "%A  %d. %B,  %H:%M"),
            clock_locale: clock.str_or("locale", "sk_SK.UTF-8"),
            clock: btn(&clock, "calendar", "", ""),

            net_iface: net.str_or("interface", "wlan0"),
            net_icons: net
                .strs("icons")
                .unwrap_or_else(|| ["\u{f091f}", "\u{f0922}", "\u{f0925}", "\u{f0928}"].map(String::from).to_vec()),
            net_icon_eth: net.str_or("icon_ethernet", "\u{f0200}"),
            net_icon_off: net.str_or("icon_disconnected", "\u{f092d}"),
            net: btn(&net, "", "", ""),

            vpn_ifaces: vpn.strs("interfaces").unwrap_or_else(|| vec!["tun*".into()]),
            vpn_text: vpn.str_or("text", "VPN"),
            vpn: btn(&vpn, "", "", "{ifname}: {ip}"),

            audio_icon_muted: audio.str_or("icon_muted", "\u{eee8}"),
            audio_icons: audio
                .strs("icons")
                .unwrap_or_else(|| ["\u{f026}", "\u{f027}", "\u{f028}"].map(String::from).to_vec()),
            audio_hud: audio.bool_or("hud", true),
            hud_ms: audio.f64_or("hud_ms", 1600.0) as u64,
            audio: btn(&audio, "audio-mute", "", ""),
            vol_step: audio.f64_or("step", 5.0).max(1.0) as u32,
            vol_limit: audio.f64_or("limit", 1.0).max(0.1),
            bright_step: t.sub("brightness").f64_or("step", 10.0).max(1.0) as u32,
            bright_get: t.sub("brightness").str_or("get", "lumix get"),
            bright_set: t.sub("brightness").str_or("set", "lumix set \"{value}\""),
            bright_icons: t
                .sub("brightness")
                .strs("icons")
                .unwrap_or_else(|| ["\u{f00de}", "\u{f00df}", "\u{f00e0}"].map(String::from).to_vec()),

            kb_names,
            kb: btn(&kb, "layout-toggle", "", "Klávesnica SK/US"),

            net_tip_wifi: net.str_or(
                "tooltip_wifi",
                "SSID: {ssid}\nRozhranie: {ifname}\nIP adresa: {ip}\nSila signálu: {signal}% ({dbm} dBm)\nFrekvencia: {freq} GHz",
            ),
            net_tip_eth: net.str_or("tooltip_ethernet", "Rozhranie: {ifname}\nIP adresa: {ip}\nBrána: {gateway}"),
            net_tip_off: net.str_or("tooltip_disconnected", ""),
            caps: btn(&caps, "", "", ""),
            caps_text: caps.str_or("text", "CAPS"),
            inh_icon: inh.str_or("icon", "\u{f0f4}"),
            inh_file: expand(&inh.str_or("file", "~/.idle_inhibit")),
            inh: btn(&inh, "inhibit-toggle", "", "Idle Inhibitor"),
            radio_icon: radio.str_or("icon", "\u{f0439}"),
            radio: btn(&radio, "track-stop:radio", "", "{name}\nKlik = vypnúť"),
            media_icon: media.str_or("icon", "\u{f001}"),
            media_icon_paused: media.str_or("icon_paused", "\u{f04c}"),
            media: {
                let mut b = btn(&media, "media", "media-play-pause", "{artist} – {title}");
                if media.get("on_click_middle").is_none() {
                    b.middle = "media-next".into();
                }
                b
            },
            media_ignore: media.strs("ignore").unwrap_or_default(),
            media_w: media.f64_or("panel_width", 460.0).clamp(300.0, 900.0) as f32,
            media_art: media.f64_or("art_size", 128.0).clamp(80.0, 256.0) as f32,
            media_font: media.f64_or("font_size", 16.0).clamp(10.0, 32.0) as f32,
            rec_file: crate::capture::rec_path(),
            rec: btn(&rec, "", "", ""),
            disks: btn(&dk, "menu", "eject-all", "{list}"),
            disks_icon: dk.str_or("icon", "\u{f287}"),
            disks_format: dk.str_or("format", "{icon}"),
            disks_format_many: dk.str_or("format_many", "{icon} {count}"),
            disks_automount: dk.bool_or("automount", true),
            disks_notify: dk.bool_or("notify", true),
            disks_timeout: dk.f64_or("notify_timeout_ms", 5000.0) as i32,

            history_file: {
                let f = nt.str_or("history_file", "");
                if f.is_empty() {
                    path.parent().map(|p| p.join("notifications.log")).unwrap_or_else(|| PathBuf::from("notifications.log"))
                } else {
                    PathBuf::from(expand(&f))
                }
            },
            history_max: if nt.bool_or("history", true) { nt.f64_or("history_max", 1000.0).max(0.0) as usize } else { 0 },
            notif_enabled: nt.bool_or("enabled", true),
            notif_width: nt.f64_or("width", 450.0) as f32,
            notif_max: nt.f64_or("max_visible", 3.0).max(1.0) as usize,
            notif_lines: nt.f64_or("max_lines", 3.0).max(1.0) as usize,
            sys_decimal: system.str_or("decimal", ","),
            sys_net_iface: system.str_or("net_interface", ""),
            drun: DrunCfg::from_table(&t.sub("drun")),
            notif_icons: nt.bool_or("icons", true),
            hist_w: nt.f64_or("panel_width", 480.0).clamp(240.0, 1400.0) as f32,
            hist_h: nt.f64_or("panel_height", 520.0).clamp(200.0, 1600.0) as f32,
            hist_lines: nt.f64_or("panel_lines", 3.0).max(1.0) as usize,
            notif_font: nt.f64_or("font_size", 15.0) as f32,
            notif_timeout: nt.f64_or("timeout_ms", 10000.0) as i32,
            notif_urgent_timeout: nt.sub("urgent").f64_or("timeout_ms", 0.0) as i32,
            notif_border: nt.sub("urgent").f64_or("border_size", 2.0) as f32,
            notif_center: nt.str_or("align", "center") == "center",
            notif_click: nt.str_or("on_click", "default-action"),
            notif_right: nt.str_or("on_click_right", "dismiss"),
            notif_rules: nt
                .get("rule")
                .and_then(|v| v.as_arr())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_table())
                        .map(|r| NotifRule {
                            summary: r.str_or("summary", ""),
                            app: r.str_or("app", ""),
                            body: r.str_or("body", ""),
                            on_click: r.str_or("on_click", ""),
                            timeout_ms: r.get("timeout_ms").and_then(|v| v.as_f64()).map(|v| v as i32),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            custom,
            trackers,
            menus,
            clip_font: clip.f64_or("font_size", 15.0).clamp(8.0, 64.0) as f32,
            clip_rows: clip.f64_or("rows", 8.0).clamp(1.0, 50.0) as usize,
            clip_width: clip.f64_or("width", 420.0).clamp(120.0, 1400.0) as f32,
            clip_max: clip.f64_or("max_items", 200.0).clamp(1.0, 5000.0) as usize,
            clip_list: clip.str_or("list_command", "cliphist list"),
            clip_copy: clip.str_or("copy_command", "cliphist decode | wl-copy"),
            wall: crate::wallpaper::WallCfg::from_table(&t.sub("wallpaper")),
            wifi: crate::wifi::WifiCfg::from_table(&t.sub("wifi")),
            audio_panel: crate::audio::AudioCfg::from_table(&audio),
            monitor: crate::monitor::MonitorCfg::from_table(&t.sub("monitor")),
            capture: crate::capture::CaptureCfg::from_table(&t.sub("capture"), &crate::capture::rec_path()),
            bluetooth: crate::bluetooth::BtCfg::from_table(&t.sub("bluetooth")),
            hw: crate::quickset::HwCfg::from_table(&{
                // `[quickset]`; starý názov `[quickset]` sa ešte číta, ak `[quickset]` chýba
                let q = t.sub("quickset");
                if q.is_empty() { t.sub("hardware") } else { q }
            }),
            trash: crate::trash::TrashCfg::from_table(&t.sub("trash")),
            auto: AutoCfg {
                enabled: auto.bool_or("enabled", true),
                bar_height: auto.f64_or("bar_height", 40.0),
                edge: auto.f64_or("edge", 2.0),
                edge_width: auto.f64_or("edge_width", 30.0).clamp(0.0, 100.0),
                margin: auto.f64_or("margin", 0.0),
                poll_ms: auto.f64_or("poll_ms", 30.0).max(10.0) as u64,
                ignore: auto.strs("ignore").unwrap_or_default(),
            },
        }
    }
}

// ------------------------------------------------------------------ farby

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub bg: Color,
    pub fg: Color,
    pub accent: Color,
    pub on_accent: Color,
    /// pozadie a text vybranej (hover) položky textového menu; predvolene accent / on_accent
    pub menu_selected: Color,
    pub menu_selected_text: Color,
    pub alert: Color,
    pub on_alert: Color,
    pub urgent_border: Color,
    pub urgent_text: Color,
}

pub fn parse_hex(s: &str) -> Option<Color> {
    let h = s.trim().strip_prefix('#')?;
    let v = |i: usize, n: usize| u32::from_str_radix(h.get(i..i + n)?, 16).ok();
    match h.len() {
        3 => Some([v(0, 1)? as f32 * 17.0 / 255.0, v(1, 1)? as f32 * 17.0 / 255.0, v(2, 1)? as f32 * 17.0 / 255.0, 1.0]),
        6 => Some([v(0, 2)? as f32 / 255.0, v(2, 2)? as f32 / 255.0, v(4, 2)? as f32 / 255.0, 1.0]),
        8 => Some([v(0, 2)? as f32 / 255.0, v(2, 2)? as f32 / 255.0, v(4, 2)? as f32 / 255.0, v(6, 2)? as f32 / 255.0]),
        _ => None,
    }
}

/// `@define-color name #hex;` (matugen / GTK CSS) -> mapa farieb.
pub fn parse_css_palette(src: &str) -> HashMap<String, Color> {
    let mut m = HashMap::new();
    // odstráň /* komentáre */ (matugen ich pridáva na začiatok súboru)
    let mut clean = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(a) = rest.find("/*") {
        clean.push_str(&rest[..a]);
        match rest[a + 2..].find("*/") {
            Some(b) => rest = &rest[a + 2 + b + 2..],
            None => {
                rest = "";
                break;
            }
        }
    }
    clean.push_str(rest);
    for stmt in clean.split(';') {
        let s = stmt.trim();
        if let Some(rest) = s.strip_prefix("@define-color") {
            let mut it = rest.trim().splitn(2, char::is_whitespace);
            if let (Some(name), Some(val)) = (it.next(), it.next()) {
                if let Some(c) = parse_hex(val) {
                    m.insert(name.trim().to_string(), c);
                }
            }
        }
    }
    m
}

fn resolve(spec: &str, pal: &HashMap<String, Color>) -> Option<Color> {
    let s = spec.trim();
    if let Some(n) = s.strip_prefix('@') {
        pal.get(n).copied()
    } else {
        parse_hex(s)
    }
}

impl Theme {
    pub fn load(cfg: &Config) -> Theme {
        let pal = std::fs::read_to_string(expand(&cfg.colors_file))
            .map(|s| parse_css_palette(&s))
            .unwrap_or_default();
        let get = |key: &str, default_spec: &str, fallback: Color| -> Color {
            let spec = cfg.color_spec.get(key).map(String::as_str).unwrap_or(default_spec);
            resolve(spec, &pal).or_else(|| resolve(default_spec, &pal)).unwrap_or(fallback)
        };
        let surface = [0.094, 0.067, 0.082, 1.0]; // #181115
        let primary = [0.988, 0.690, 0.839, 1.0]; // #fcb0d6
        // voliteľná farba bez vlastného predvoleného zápisu: neplatná / chýbajúca = `fallback`
        let opt = |key: &str, fallback: Color| -> Color {
            cfg.color_spec.get(key).and_then(|spec| resolve(spec, &pal)).unwrap_or(fallback)
        };
        let accent = get("accent", "@primary", primary);
        let on_accent = get("on_accent", "@surface", surface);
        Theme {
            bg: get("background", "@surface", surface),
            fg: get("foreground", "@primary", primary),
            accent,
            on_accent,
            menu_selected: opt("menu_selected", accent),
            menu_selected_text: opt("menu_selected_text", on_accent),
            alert: get("alert", "#dc2f2f", [0.863, 0.184, 0.184, 1.0]),
            on_alert: get("on_alert", "#ffffff", [1.0; 4]),
            urgent_border: get("urgent_border", "#fa3939", [0.98, 0.22, 0.22, 1.0]),
            urgent_text: get("urgent_text", "@primary", primary),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn palette_and_theme() {
        let pal = parse_css_palette(
            "    @define-color surface #181115;\n\n @define-color primary #fcb0d6;\n/* c */ @define-color x #12345678;",
        );
        assert!((pal["primary"][0] - 0.988).abs() < 0.01);
        assert!((pal["x"][3] - 0x78 as f32 / 255.0).abs() < 0.01);
        assert_eq!(resolve("@surface", &pal), pal.get("surface").copied());
        assert!(parse_hex("#fff").is_some() && parse_hex("zz").is_none());
    }
    #[test]
    fn menu_selected_colors() {
        // bez nastavenia = accent / on_accent; `[colors]` ich prepíše ("#rrggbb"), neplatná hodnota sa ignoruje
        let d = Theme::load(&Config::from_table(&Table::new(), PathBuf::from("x.toml")));
        assert_eq!(d.menu_selected, d.accent);
        assert_eq!(d.menu_selected_text, d.on_accent);
        let t = toml::parse("[colors]\nmenu_selected=\"#ff0000\"\nmenu_selected_text=\"nonsense\"\n").unwrap();
        let c = Theme::load(&Config::from_table(&t, PathBuf::from("x.toml")));
        assert_eq!(c.menu_selected, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(c.menu_selected_text, c.on_accent);
    }

    #[test]
    fn middle_click_binding() {
        let t = toml::parse("[clock]\non_click_middle=\"clipboard\"\n[[custom]]\nname=\"x\"\ntext=\"t\"\non_click_middle=\"foot\"\n").unwrap();
        let c = Config::from_table(&t, PathBuf::new());
        assert_eq!(c.clock.middle, "clipboard");
        assert_eq!(c.custom[0].btn.middle, "foot");
        assert_eq!(c.net.middle, "");
    }

    #[test]
    fn clipboard_config() {
        let d = Config::from_table(&Table::new(), PathBuf::new());
        assert_eq!((d.clip_rows, d.clip_max), (8, 200));
        assert_eq!(d.clip_list, "cliphist list");
        assert_eq!(d.clip_copy, "cliphist decode | wl-copy");
        let t = toml::parse("[clipboard]\nfont_size=18\nrows=5\nwidth=500\n").unwrap();
        let c = Config::from_table(&t, PathBuf::new());
        assert_eq!((c.clip_font, c.clip_rows, c.clip_width), (18.0, 5, 500.0));
    }

    #[test]
    fn menu_separator_positions() {
        let t = toml::parse("[[menu]]\nname=\"m\"\n[[menu.item]]\nseparator=true\n[[menu.item]]\nseparator=\"top\"\n[[menu.item]]\nseparator=\"center\"\n[[menu.item]]\nseparator=\"bottom\"\n[[menu.item]]\nseparator=false\nlabel=\"A\"\n[[menu.item]]\nseparator=\"\"\nlabel=\"B\"\n").unwrap();
        let c = Config::from_table(&t, PathBuf::new());
        let it = &c.menus[0].items;
        assert_eq!(it.len(), 6);
        let al: Vec<_> = it.iter().map(|i| (i.separator, i.sep_align)).collect();
        assert_eq!(al[0], (true, SepAlign::Center));
        assert_eq!(al[1], (true, SepAlign::Top));
        assert_eq!(al[2], (true, SepAlign::Center));
        assert_eq!(al[3], (true, SepAlign::Bottom));
        assert!(!it[4].separator && it[4].label == "A");
        assert!(!it[5].separator && it[5].label == "B");
    }

    #[test]
    fn menu_line_items() {
        let t = toml::parse("[[menu]]\nname=\"m\"\n[[menu.item]]\nlabel=\"A\"\ncommand=\"x\"\n[[menu.item]]\nline=\"\"\n[[menu.item]]\nline=\"Hotovo\"\n[[menu.item]]\nline=\"CPU: {}\"\nexec=\"echo 1\"\ninterval=2\n[[menu.item]]\nseparator=true\n").unwrap();
        let c = Config::from_table(&t, PathBuf::new());
        let it = &c.menus[0].items;
        assert_eq!(it.len(), 5);
        assert!(!it[0].line && it[1].line && it[2].line && it[3].line && !it[4].line);
        assert_eq!(it[1].label, "");
        assert_eq!(it[2].label, "Hotovo");
        assert_eq!((it[3].label.as_str(), it[3].exec.as_str(), it[3].interval), ("CPU: {}", "echo 1", 2.0));
        assert!(it[4].separator);
    }

    #[test]
    fn custom_track_file() {
        let t = toml::parse("[[custom]]\nname=\"rec\"\ntext=\"REC\"\ntrack_file=\"/tmp/RECORDING\"\n[[custom]]\nname=\"plain\"\ntext=\"x\"\n[[custom]]\nname=\"empty\"\ntext=\"y\"\ntrack_file=\"\"\n").unwrap();
        let c = Config::from_table(&t, PathBuf::new());
        assert_eq!(c.custom[0].track_file, "/tmp/RECORDING");
        assert_eq!(c.custom[1].track_file, "");
        assert_eq!(c.custom[2].track_file, "");
    }

    #[test]
    fn tracker_module_from_unknown_section() {
        let t = toml::parse("modules=[\"clock\",\"tv\",\"nothing\"]\n[tv]\nicon=\"TV\"\n[clock]\nformat=\"%H\"\n").unwrap();
        let c = Config::from_table(&t, PathBuf::new());
        assert_eq!(c.trackers.len(), 1);
        assert_eq!((c.trackers[0].name.as_str(), c.trackers[0].group.as_str(), c.trackers[0].icon.as_str()), ("tv", "tv", "TV"));
        assert_eq!(c.trackers[0].btn.click, "track-stop:tv");
        let t = toml::parse("modules=[\"tv\"]\n[tv]\ntrack=\"video\"\n").unwrap();
        let c = Config::from_table(&t, PathBuf::new());
        assert_eq!((c.trackers[0].group.as_str(), c.trackers[0].btn.click.as_str()), ("video", "track-stop:video"));
    }

    #[test]
    fn wifi_config() {
        let t = toml::parse("[wifi]\nrows=5\nwidth=400\n").unwrap();
        let c = Config::from_table(&t, PathBuf::new());
        assert_eq!((c.wifi.rows, c.wifi.width), (5, 400.0));
    }

    #[test]
    fn wallpaper_config() {
        let d = Config::from_table(&Table::new(), PathBuf::new());
        assert_eq!((d.wall.columns, d.wall.rows), (4, 4));
        let t = toml::parse("[wallpaper]\ncolumns=5\nrows=2\ndirectory=\"/x\"\n").unwrap();
        let c = Config::from_table(&t, PathBuf::new());
        assert_eq!((c.wall.columns, c.wall.rows, c.wall.dir.as_str()), (5, 2, "/x"));
    }

    #[test]
    fn config_defaults_and_custom() {
        let t = toml::parse(
            "modules=[\"clock\"]\n[clock]\nformat=\"%H\"\n[[custom]]\nname=\"eject\"\nexec=\"x watch\"\n[keyboard.names]\nCzech=\"CZ\"",
        )
        .unwrap();
        let c = Config::from_table(&t, PathBuf::new());
        assert_eq!(c.clock_format, "%H");
        assert_eq!(c.modules, vec!["clock", "eject"]);
        assert_eq!(c.kb_names["Czech"], "CZ");
        assert_eq!(c.kb_names["Slovak"], "SK");
        assert_eq!(c.clock.click, "calendar");
        assert!(c.auto.enabled);
    }

    #[test]
    fn font_lists() {
        let t = toml::parse("[island]\nfont_family = \"A, B\"\nicon_family = []\n").unwrap();
        let cfg = Config::from_table(&t, PathBuf::from("x.toml"));
        assert_eq!(cfg.font_family, vec!["A", "B"]);
        assert!(cfg.icon_family.is_empty());
        let d = Config::from_table(&Table::new(), PathBuf::from("x.toml"));
        assert_eq!(d.icon_family, vec!["Hack Nerd Font Propo"]);
    }
}

/// Adresár pre HUD správy od externých skriptov (`dynamic-island --hud ...`).
pub fn hud_dir() -> PathBuf {
    let base = std::env::var("XDG_RUNTIME_DIR").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/tmp".into());
    PathBuf::from(base).join("dynamic-island")
}

/// Súbor s PID bežiaceho ostrovčeka (v `hud_dir`), podľa ktorého `--drun` / `--action` zistia, komu poslať povel.
pub fn island_pid_file() -> PathBuf {
    hud_dir().join("island.pid")
}

/// Pošle bežiacemu ostrovčeku povel: otvor panel `name` v strede obrazovky (alebo ho zatvor, ak je nejaký otvorený).
/// Povel je súbor `action.<čas>` v `hud_dir` (zapíše sa a premenuje naraz), ktorý ostrovček sleduje cez inotify.
pub fn send_action(name: &str) -> Result<(), String> {
    let pid = std::fs::read_to_string(island_pid_file()).ok().and_then(|s| s.trim().parse::<i32>().ok()).ok_or("dynamic-island nebeží")?;
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).map_err(|_| "dynamic-island nebeží".to_string())?;
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let name_of = |p: &std::path::Path| p.file_name().map(|n| n.to_string_lossy().trim_end_matches(" (deleted)").to_string());
    if name_of(&exe) != name_of(&me) {
        return Err("dynamic-island nebeží".into());
    }
    let dir = hud_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let tmp = dir.join(format!("action.{stamp}.tmp"));
    std::fs::write(&tmp, name).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dir.join(format!("action.{stamp}"))).map_err(|e| e.to_string())
}
