//! Modul „quickset“: kontajner so záložkami pre panely audio, bluetooth, wifi a monitor.
//! Tu je len konfigurácia `[quickset]` a čistá geometria (obdĺžniky ako štvorice `(x, y, w, h)`);
//! stav a kreslenie sú v `ui.rs`, kde kontajner obalí ten istý panel, ktorý sa dá otvoriť aj samostatne.
use crate::toml::{Table, TableExt, Value};

/// Záložky predvolene (bez `tab1` … v configu).
pub const MODULES: [&str; 4] = ["audio", "bluetooth", "wifi", "monitor"];
/// Všetky panely, ktoré môžu byť záložkou (okrem `menu:NAZOV`). Nie `media` (lepkavý panel) a kalendár.
pub const ALLOWED: [&str; 8] = ["audio", "bluetooth", "wifi", "monitor", "clipboard", "trash", "notifications", "wallpaper"];
/// Najviac záložiek (`tab1` … `tab8`).
const MAX_TABS: usize = 8;

/// Obdĺžnik `(x, y, w, h)`.
pub type R = (f32, f32, f32, f32);

/// Kde je lišta so záložkami.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TabPos {
    Left,
    Right,
    Top,
    Bottom,
}

impl TabPos {
    pub fn parse(s: &str) -> TabPos {
        match s.trim().to_lowercase().as_str() {
            "right" => TabPos::Right,
            "top" => TabPos::Top,
            "bottom" => TabPos::Bottom,
            _ => TabPos::Left,
        }
    }

    /// Opačná strana: vľavo ↔ vpravo, hore ↔ dole.
    pub fn opposite(self) -> TabPos {
        match self {
            TabPos::Left => TabPos::Right,
            TabPos::Right => TabPos::Left,
            TabPos::Top => TabPos::Bottom,
            TabPos::Bottom => TabPos::Top,
        }
    }

    /// Lišta je zvislá (vľavo / vpravo), záložky sú pod sebou.
    pub fn vertical(self) -> bool {
        matches!(self, TabPos::Left | TabPos::Right)
    }
}

/// Jedna záložka: názov modulu (`audio` | `bluetooth` | `wifi` | `monitor`) a ikona (Nerd Font glyf).
#[derive(Clone, Debug, PartialEq)]
pub struct HwTab {
    pub module: String,
    pub icon: String,
}

/// Tlačidlo druhej (opačnej) lišty `inversetabN`: nezobrazuje panel v kontajneri, ale spustí príkaz alebo otvorí
/// panel samostatne (ako `on_click` modulu). `action` je príkaz, názov panela / menu alebo `module:NAZOV`.
#[derive(Clone, Debug, PartialEq)]
pub struct HwBtn {
    pub action: String,
    pub icon: String,
}

/// Nastavenia sekcie `[quickset]`.
#[derive(Clone, Debug)]
pub struct HwCfg {
    pub tabs: Vec<HwTab>,
    pub pos: TabPos,
    /// strana štvorcového tlačidla záložky (px)
    pub tab_size: f32,
    /// veľkosť ikony (px)
    pub icon_size: f32,
    /// medzera medzi tlačidlami záložiek (px)
    pub tab_gap: f32,
    /// celková šírka okna vrátane lišty; `None` = podľa obsahu aktívnej záložky
    pub width: Option<f32>,
    /// celková výška okna vrátane lišty; `None` = podľa najvyššej záložky
    pub height: Option<f32>,
    /// tlačidlá druhej lišty na opačnej strane (prázdne, ak `inversetab` nie je zapnuté)
    pub inverse: Vec<HwBtn>,
    /// medzera medzi tlačidlami druhej lišty (px)
    pub inverse_gap: f32,
    /// pri otvorení kontajnera načítať údaje monitora hneď (aj keď je aktívna iná záložka), aby bol pripravený
    pub preload: bool,
}

/// Predvolená ikona modulu (kód z Nerd Fontu).
pub fn default_icon(module: &str) -> &'static str {
    match module {
        "audio" => "\u{f057e}",
        "bluetooth" => "\u{f00af}",
        "wifi" => "\u{f0928}",
        "clipboard" => "\u{f0ea}",
        "trash" => "\u{f1f8}",
        "notifications" => "\u{f0f3}",
        "wallpaper" => "\u{f03e}",
        m if m.starts_with(MENU_PREFIX) => "\u{f0493}",
        _ => "\u{f0379}",
    }
}

/// Názov záložky `menu:NAZOV` = vlastné `[[menu]]` s `name = "NAZOV"` (veľkosť písmen sa zachová).
pub const MENU_PREFIX: &str = "menu:";

/// Normalizuje názov záložky: vstavané moduly bez ohľadu na veľkosť písmen, `menu:NAZOV` s názvom menu tak, ako je
/// napísaný. Neplatný názov (neznámy modul, prázdny názov menu) vráti `None`.
pub fn parse_tab_name(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.len() > MENU_PREFIX.len() && raw.is_char_boundary(MENU_PREFIX.len()) && raw[..MENU_PREFIX.len()].eq_ignore_ascii_case(MENU_PREFIX) {
        let name = raw[MENU_PREFIX.len()..].trim();
        return (!name.is_empty()).then(|| format!("{MENU_PREFIX}{name}"));
    }
    let lower = raw.to_lowercase();
    ALLOWED.contains(&lower.as_str()).then_some(lower)
}

/// Normalizuje akciu tlačidla druhej lišty: odstráni `module:` aj `menu:` (názov menu otvorí menu samostatne);
/// prázdna akcia = `None`.
pub fn parse_button_action(raw: &str) -> Option<String> {
    let mut a = raw.trim();
    for p in ["module:", MENU_PREFIX] {
        if a.len() > p.len() && a.is_char_boundary(p.len()) && a[..p.len()].eq_ignore_ascii_case(p) {
            a = a[p.len()..].trim();
        }
    }
    (!a.is_empty()).then(|| a.to_string())
}

/// Predvolená ikona tlačidla: pre známy panel ikona záložky, inak blesk.
fn button_default_icon(action: &str) -> &'static str {
    if ALLOWED.contains(&action.to_lowercase().as_str()) {
        default_icon(&action.to_lowercase())
    } else {
        "\u{f0e7}"
    }
}

impl HwCfg {
    pub fn from_table(t: &Table) -> HwCfg {
        let mut tabs = Vec::new();
        let mut configured = false;
        for i in 1..=MAX_TABS {
            let raw = t.str_or(&format!("tab{i}"), "");
            if raw.trim().is_empty() {
                continue;
            }
            configured = true;
            let Some(name) = parse_tab_name(&raw) else { continue };
            let icon = t.str_or(&format!("tab{i}_icon"), "");
            let icon = if icon.trim().is_empty() { default_icon(&name).to_string() } else { icon };
            tabs.push(HwTab { module: name, icon });
        }
        if !configured {
            tabs = MODULES.iter().map(|m| HwTab { module: m.to_string(), icon: default_icon(m).to_string() }).collect();
        }
        let tab_gap = t.f64_or("tab_gap", 6.0).clamp(0.0, 64.0) as f32;
        let inverse_on = match t.get("inversetab") {
            Some(Value::Bool(b)) => *b,
            Some(Value::Str(s)) => s.trim().eq_ignore_ascii_case("true"),
            _ => false,
        };
        let mut inverse = Vec::new();
        if inverse_on {
            for i in 1..=MAX_TABS {
                let Some(action) = parse_button_action(&t.str_or(&format!("inversetab{i}"), "")) else { continue };
                let icon = t.str_or(&format!("inversetab{i}_icon"), "");
                let icon = if icon.trim().is_empty() { button_default_icon(&action).to_string() } else { icon };
                inverse.push(HwBtn { action, icon });
            }
        }
        let size = |k: &str| Some(t.f64_or(k, 0.0)).filter(|v| *v > 0.0).map(|v| v.clamp(120.0, 3000.0) as f32);
        HwCfg {
            tabs,
            pos: TabPos::parse(&t.str_or("tabs_position", "left")),
            tab_size: t.f64_or("tab_size", 44.0).clamp(24.0, 120.0) as f32,
            icon_size: t.f64_or("icon_size", 22.0).clamp(8.0, 64.0) as f32,
            tab_gap,
            inverse,
            inverse_gap: t.f64_or("inversetab_gap", tab_gap as f64).clamp(0.0, 64.0) as f32,
            width: size("width"),
            height: size("height"),
            preload: t.bool_or("preload", true),
        }
    }
}

/// Hrúbka lišty (šírka zvislej, výška vodorovnej): tlačidlo + okraj z oboch strán.
pub fn bar_thickness(tab: f32, pad: f32) -> f32 {
    tab + 2.0 * pad
}

/// Dĺžka lišty (pozdĺž jej osi), aby sa do nej zmestilo `n` záložiek.
pub fn bar_length(n: usize, tab: f32, gap: f32, pad: f32) -> f32 {
    n as f32 * tab + n.saturating_sub(1) as f32 * gap + 2.0 * pad
}

/// Celková veľkosť okna z veľkosti obsahu `inner` a lišty. `fixed_w` / `fixed_h` (vrátane lišty) nastavenú
/// hodnotu prebijú; lišta sa do okna musí vždy zmestiť.
pub fn window_size(pos: TabPos, inner: (f32, f32), bar_t: f32, bar_len: f32, fixed_w: Option<f32>, fixed_h: Option<f32>) -> (f32, f32) {
    let vertical = pos.vertical();
    let (mut w, mut h) = if vertical { (inner.0 + bar_t, inner.1.max(bar_len)) } else { (inner.0.max(bar_len), inner.1 + bar_t) };
    if let Some(fw) = fixed_w {
        w = fw.max(if vertical { bar_t } else { bar_len });
    }
    if let Some(fh) = fixed_h {
        h = fh.max(if vertical { bar_len } else { bar_t });
    }
    (w, h)
}

/// Rozdelí okno na lištu a obsah: `(lišta, obsah)`.
pub fn split(pos: TabPos, win: R, bar_t: f32) -> (R, R) {
    let (x, y, w, h) = win;
    let bt = bar_t.clamp(0.0, if pos.vertical() { w } else { h });
    match pos {
        TabPos::Left => ((x, y, bt, h), (x + bt, y, w - bt, h)),
        TabPos::Right => ((x + w - bt, y, bt, h), (x, y, w - bt, h)),
        TabPos::Top => ((x, y, w, bt), (x, y + bt, w, h - bt)),
        TabPos::Bottom => ((x, y + h - bt, w, bt), (x, y, w, h - bt)),
    }
}

/// Obdĺžniky `n` záložiek v lište `bar`: od začiatku lišty (zhora / zľava) s okrajom `pad` a medzerou `gap`,
/// naprieč lištou vycentrované.
pub fn tab_rects(pos: TabPos, bar: R, n: usize, tab: f32, gap: f32, pad: f32) -> Vec<R> {
    (0..n)
        .map(|k| {
            let off = pad + k as f32 * (tab + gap);
            if pos.vertical() {
                (bar.0 + (bar.2 - tab) * 0.5, bar.1 + off, tab, tab)
            } else {
                (bar.0 + off, bar.1 + (bar.3 - tab) * 0.5, tab, tab)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toml;

    #[test]
    fn defaults_have_all_modules() {
        let c = HwCfg::from_table(&Table::new());
        let names: Vec<&str> = c.tabs.iter().map(|t| t.module.as_str()).collect();
        assert_eq!(names, ["audio", "bluetooth", "wifi", "monitor"]);
        assert!(c.tabs.iter().all(|t| !t.icon.is_empty()));
        assert_eq!((c.pos, c.tab_size, c.width, c.height), (TabPos::Left, 44.0, None, None));
        assert_eq!(c.tab_gap, 6.0);
        assert!(c.preload);
        assert!(!HwCfg::from_table(&toml::parse("preload=false\n").unwrap()).preload);
    }

    #[test]
    fn config_tabs_and_sizes() {
        let src = "tab1=\"wifi\"\ntab1_icon=\"W\"\ntab2=\"Audio\"\ntab4=\"nonsense\"\ntab5=\"monitor\"\ntabs_position=\"bottom\"\nwidth=500\nheight=10\ntab_size=50\n";
        let c = HwCfg::from_table(&toml::parse(src).unwrap());
        assert_eq!(c.tabs.len(), 3);
        assert_eq!((c.tabs[0].module.as_str(), c.tabs[0].icon.as_str()), ("wifi", "W"));
        // názov sa berie bez ohľadu na veľkosť písmen, chýbajúca ikona = predvolená
        assert_eq!((c.tabs[1].module.as_str(), c.tabs[1].icon.as_str()), ("audio", default_icon("audio")));
        assert_eq!(c.tabs[2].module, "monitor");
        assert_eq!((c.pos, c.tab_size, c.width), (TabPos::Bottom, 50.0, Some(500.0)));
        // príliš malá výška sa zvýši na minimum
        assert_eq!(c.height, Some(120.0));
    }

    #[test]
    fn inverse_buttons() {
        let src = "tab_gap=10\ninversetab=true\ninversetab1=\"systemctl suspend\"\ninversetab1_icon=\"S\"\ninversetab2=\"menu:radio\"\ninversetab3=\"module:power\"\ninversetab4=\"  \"\ninversetab5=\"wifi\"\n";
        let c = HwCfg::from_table(&toml::parse(src).unwrap());
        let v: Vec<(&str, &str)> = c.inverse.iter().map(|b| (b.action.as_str(), b.icon.as_str())).collect();
        assert_eq!(v, vec![("systemctl suspend", "S"), ("radio", "\u{f0e7}"), ("power", "\u{f0e7}"), ("wifi", default_icon("wifi"))]);
        // vlastná medzera sa dedí z `tab_gap`, kým nie je zadaná
        assert_eq!(c.inverse_gap, 10.0);
        let c = HwCfg::from_table(&toml::parse("inversetab=\"true\"\ninversetab_gap=3\ninversetab1=\"x\"\n").unwrap());
        assert_eq!((c.inverse.len(), c.inverse_gap), (1, 3.0));
        // vypnuté (alebo bez kľúča) = žiadne tlačidlá, aj keď sú `inversetabN` zadané
        assert!(HwCfg::from_table(&toml::parse("inversetab=false\ninversetab1=\"x\"\n").unwrap()).inverse.is_empty());
        assert!(HwCfg::from_table(&toml::parse("inversetab1=\"x\"\n").unwrap()).inverse.is_empty());
    }

    #[test]
    fn opposite_position_and_two_bars() {
        assert_eq!((TabPos::Left.opposite(), TabPos::Bottom.opposite()), (TabPos::Right, TabPos::Top));
        // dve lišty: hrúbka sa sčíta, dĺžka je väčšia z oboch
        let (w, h) = window_size(TabPos::Left, (300.0, 100.0), 2.0 * 64.0, 200.0, None, None);
        assert_eq!((w, h), (428.0, 200.0));
        let win = (0.0, 0.0, 428.0, 200.0);
        let (bar1, c1) = split(TabPos::Left, win, 64.0);
        let (bar2, c2) = split(TabPos::Left.opposite(), c1, 64.0);
        assert_eq!(bar1, (0.0, 0.0, 64.0, 200.0));
        assert_eq!(bar2, (364.0, 0.0, 64.0, 200.0));
        assert_eq!(c2, (64.0, 0.0, 300.0, 200.0));
        let (_, c1) = split(TabPos::Top, (0.0, 0.0, 300.0, 300.0), 50.0);
        let (bar2, c2) = split(TabPos::Bottom, c1, 50.0);
        assert_eq!((bar2, c2), ((0.0, 250.0, 300.0, 50.0), (0.0, 50.0, 300.0, 200.0)));
    }

    #[test]
    fn menu_tabs() {
        assert_eq!(parse_tab_name("Wifi"), Some("wifi".into()));
        assert_eq!(parse_tab_name(" menu:Nastavenia "), Some("menu:Nastavenia".into()));
        assert_eq!(parse_tab_name("MENU: Apps"), Some("menu:Apps".into()));
        assert_eq!(parse_tab_name("menu:"), None);
        assert_eq!(parse_tab_name("menu:   "), None);
        assert_eq!(parse_tab_name("menu"), None);
        assert_eq!(parse_tab_name("foo"), None);
        let src = "tab1=\"wifi\"\ntab2=\"menu:Nastavenia\"\ntab2_icon=\"X\"\ntab3=\"menu:Apps\"\ntab4=\"menu:\"\n";
        let c = HwCfg::from_table(&toml::parse(src).unwrap());
        let t: Vec<(&str, &str)> = c.tabs.iter().map(|t| (t.module.as_str(), t.icon.as_str())).collect();
        // názov menu sa nelowercasuje, chýbajúca ikona = predvolená pre menu, prázdny názov menu sa preskočí
        assert_eq!(t, vec![("wifi", default_icon("wifi")), ("menu:Nastavenia", "X"), ("menu:Apps", default_icon("menu:Apps"))]);
        assert_eq!(default_icon("menu:Apps"), "\u{f0493}");
    }

    #[test]
    fn more_panels_as_tabs() {
        let src = "tab1=\"Trash\"\ntab2=\"clipboard\"\ntab3=\"notifications\"\ntab4=\"wallpaper\"\ntab5=\"media\"\ntab6=\"calendar\"\n";
        let c = HwCfg::from_table(&toml::parse(src).unwrap());
        let m: Vec<&str> = c.tabs.iter().map(|t| t.module.as_str()).collect();
        // media a kalendár sa nepovoľujú
        assert_eq!(m, vec!["trash", "clipboard", "notifications", "wallpaper"]);
        assert_eq!(c.tabs[0].icon, "\u{f1f8}");
        // predvolené záložky ostávajú štyri pôvodné
        assert_eq!(HwCfg::from_table(&toml::parse("").unwrap()).tabs.len(), 4);
    }

    #[test]
    fn unknown_only_gives_no_tabs() {
        let c = HwCfg::from_table(&toml::parse("tab1=\"foo\"\n").unwrap());
        assert!(c.tabs.is_empty());
    }

    #[test]
    fn sizes() {
        let (bt, bl) = (bar_thickness(44.0, 10.0), bar_length(4, 44.0, 6.0, 10.0));
        assert_eq!((bt, bl), (64.0, 214.0));
        // zvislá lišta: šírka = obsah + lišta; výška = obsah, ale lišta sa musí zmestiť
        assert_eq!(window_size(TabPos::Left, (400.0, 300.0), bt, bl, None, None), (464.0, 300.0));
        assert_eq!(window_size(TabPos::Right, (400.0, 100.0), bt, bl, None, None), (464.0, 214.0));
        // vodorovná lišta
        assert_eq!(window_size(TabPos::Top, (400.0, 300.0), bt, bl, None, None), (400.0, 364.0));
        assert_eq!(window_size(TabPos::Bottom, (100.0, 300.0), bt, bl, None, None), (214.0, 364.0));
        // pevné rozmery (vrátane lišty), ale nie menej, než potrebuje lišta
        assert_eq!(window_size(TabPos::Left, (400.0, 300.0), bt, bl, Some(600.0), Some(500.0)), (600.0, 500.0));
        assert_eq!(window_size(TabPos::Left, (400.0, 300.0), bt, bl, Some(10.0), Some(10.0)), (64.0, 214.0));
    }

    #[test]
    fn splitting() {
        let win = (10.0, 20.0, 464.0, 300.0);
        assert_eq!(split(TabPos::Left, win, 64.0), ((10.0, 20.0, 64.0, 300.0), (74.0, 20.0, 400.0, 300.0)));
        assert_eq!(split(TabPos::Right, win, 64.0), ((410.0, 20.0, 64.0, 300.0), (10.0, 20.0, 400.0, 300.0)));
        assert_eq!(split(TabPos::Top, win, 64.0), ((10.0, 20.0, 464.0, 64.0), (10.0, 84.0, 464.0, 236.0)));
        assert_eq!(split(TabPos::Bottom, win, 64.0), ((10.0, 256.0, 464.0, 64.0), (10.0, 20.0, 464.0, 236.0)));
        // lišta nikdy nepresiahne okno
        let (b, c) = split(TabPos::Left, (0.0, 0.0, 30.0, 100.0), 64.0);
        assert_eq!((b.2, c.2), (30.0, 0.0));
    }

    #[test]
    fn tab_positions() {
        let bar = (0.0, 0.0, 64.0, 300.0);
        let r = tab_rects(TabPos::Left, bar, 2, 44.0, 6.0, 10.0);
        assert_eq!(r, vec![(10.0, 10.0, 44.0, 44.0), (10.0, 60.0, 44.0, 44.0)]);
        let bar = (0.0, 0.0, 300.0, 64.0);
        let r = tab_rects(TabPos::Top, bar, 2, 44.0, 6.0, 10.0);
        assert_eq!(r, vec![(10.0, 10.0, 44.0, 44.0), (60.0, 10.0, 44.0, 44.0)]);
    }
}
