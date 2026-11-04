//! Konfigurácia (TOML podmnožina) a farebná téma (vrátane matugen CSS súboru).
use crate::toml::{self, Table, TableExt, Value};
use std::collections::HashMap;
use std::path::PathBuf;

pub type Color = [f32; 4];

#[derive(Clone, Debug, Default)]
pub struct Btn {
    pub click: String,
    pub right: String,
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

/// Náhrada swayidle: po `timeout` sekundách nečinnosti spusti `on_idle`, po návrate `on_resume`.
#[derive(Clone, Debug, Default)]
pub struct IdleCfg {
    pub timeout: u32,
    pub on_idle: String,
    pub on_resume: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKind {
    Icons,
    Text,
}

#[derive(Clone, Debug, Default)]
pub struct MenuItemCfg {
    pub icon: String,
    pub label: String,
    pub command: String,
}

/// Vlastné menu (panel ako kalendár). Spúšťa sa cez `on_click = "meno_menu"`.
#[derive(Clone, Debug)]
pub struct MenuCfg {
    pub name: String,
    pub kind: MenuKind,
    pub title: String,
    pub columns: usize,
    pub rows: Option<usize>,
    pub icon_size: f32,
    pub text_size: f32,
    pub cell_width: f32,
    pub gap: f32,
    pub padding: f32,
    /// true = menu sa zavrie aj odchodom myši (ako kalendár); false = len klikom na položku alebo mimo
    pub autoclose: bool,
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
    pub menus: Vec<MenuCfg>,
    pub auto: AutoCfg,

    pub notif_enabled: bool,
    pub notif_width: f32,
    pub notif_max: usize,
    pub notif_lines: usize,
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
    pub idle: Vec<IdleCfg>,
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
    Btn { click: t.str_or("on_click", click), right: t.str_or("on_click_right", right), tooltip: t.str_or("tooltip", tip) }
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

        let clock = t.sub("clock");
        let net = t.sub("network");
        let vpn = t.sub("vpn");
        let audio = t.sub("audio");
        let kb = t.sub("keyboard");
        let caps = t.sub("capslock");
        let inh = t.sub("inhibitor");
        let rec = t.sub("recorder");
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
                    let kind = match m.str_or("type", "icons").as_str() {
                        "text" => MenuKind::Text,
                        _ => MenuKind::Icons,
                    };
                    let mut items = Vec::new();
                    if let Some(ia) = m.get("item").and_then(|v| v.as_arr()) {
                        for iv in ia {
                            if let Value::Table(it) = iv {
                                let icon = it.str_or("icon", "");
                                let label = it.str_or("label", "");
                                let command = it.str_or("command", "");
                                if command.is_empty() && icon.is_empty() && label.is_empty() {
                                    continue;
                                }
                                items.push(MenuItemCfg { icon, label, command });
                            }
                        }
                    }
                    let columns = m.f64_or("columns", 3.0).max(1.0) as usize;
                    let rows = m.get("rows").and_then(|v| v.as_f64()).map(|v| v.max(1.0) as usize);
                    menus.push(MenuCfg {
                        name,
                        kind,
                        title: m.str_or("title", ""),
                        columns,
                        rows,
                        icon_size: m.f64_or("icon_size", 28.0).max(8.0) as f32,
                        text_size: m.f64_or("text_size", 15.0).max(8.0) as f32,
                        cell_width: m.f64_or("cell_width", 120.0).max(20.0) as f32,
                        gap: m.f64_or("gap", 8.0).max(0.0) as f32,
                        padding: m.f64_or("padding", 12.0).max(0.0) as f32,
                        // `autoclose = true` aj `autoclose = "true"` (predvolene false)
                        autoclose: match m.get("autoclose") {
                            Some(Value::Bool(b)) => *b,
                            Some(Value::Str(s)) => {
                                matches!(s.trim().to_ascii_lowercase().as_str(), "true" | "yes" | "on" | "1")
                            }
                            _ => false,
                        },
                        items,
                    });
                }
            }
        }

        let mut modules = t.strs("modules").unwrap_or_else(|| {
            ["network", "vpn", "clock", "keyboard", "audio", "capslock", "inhibitor", "recorder", "disks"]
                .map(String::from)
                .to_vec()
        });
        for c in &custom {
            if !modules.contains(&c.name) {
                modules.push(c.name.clone());
            }
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
            rec_file: expand(&rec.str_or("file", "/tmp/RECORDING")),
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
            idle: t
                .get("idle")
                .and_then(|v| v.as_arr())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_table())
                        .filter(|r| r.f64_or("timeout", 0.0) > 0.0)
                        .map(|r| IdleCfg {
                            timeout: r.f64_or("timeout", 0.0) as u32,
                            on_idle: r.str_or("on_idle", ""),
                            on_resume: r.str_or("on_resume", ""),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            custom,
            menus,
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
        Theme {
            bg: get("background", "@surface", surface),
            fg: get("foreground", "@primary", primary),
            accent: get("accent", "@primary", primary),
            on_accent: get("on_accent", "@surface", surface),
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
    fn menu_autoclose() {
        let t = toml::parse(
            "[[menu]]\nname=\"a\"\n[[menu]]\nname=\"b\"\nautoclose=true\n[[menu]]\nname=\"c\"\nautoclose=\"true\"\n[[menu]]\nname=\"d\"\nautoclose=\"false\"\n",
        )
        .unwrap();
        let c = Config::from_table(&t, PathBuf::new());
        let v: Vec<bool> = c.menus.iter().map(|m| m.autoclose).collect();
        assert_eq!(v, vec![false, true, true, false]);
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
