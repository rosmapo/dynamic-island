//! Stav a rozloženie ostrovčeka: položky, HUD, kalendár, tooltipy, animácie.
//! Všetky rozmery sa tu počítajú vo FYZICKÝCH pixeloch (logické * scale).
use crate::anim::Spring;
use crate::config::{Btn, Color, Config, MenuKind, Theme};
use crate::icons::IconCache;
use crate::data::*;
use crate::render::{Frame, Item, Rect};
use crate::sys;
use crate::text::{GlyphQuad, Text};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ModId {
    Network,
    Vpn,
    Clock,
    Keyboard,
    Audio,
    Caps,
    Inhibit,
    Recorder,
    Disks,
    Custom(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Run(String),
    InhibitToggle,
    LayoutToggle,
    AudioMute,
    DiskEject(String),
    DiskEjectAll,
    /// načítať históriu schránky (výsledok príde ako `Msg::Clip`)
    ClipLoad,
    /// skopírovať položku histórie (celý riadok z `cliphist list`)
    ClipCopy(String),
    /// načítať zoznam obrázkov a náhľady modulu wallpaper
    WallLoad,
    /// nastaviť pozadie skriptom (`wallpaper -v CESTA`)
    WallApply { path: String, background_only: bool },
    /// nastaviť tému (názov, cesta) alebo návrat do režimu „wallpaper“ (`None`)
    WallTheme(Option<(String, String)>),
    Audio(crate::audio::AudioCmd),
    Wifi(crate::wifi::WifiCmd),
    Monitor(crate::monitor::MonitorCmd),
    Bluetooth(crate::bluetooth::BtCmd),
    /// vložiť schránku do poľa hesla
    WifiPaste,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Panel {
    None,
    Calendar,
    Disks,
    Menu(usize),
    Clip,
    Wall,
    Wifi,
    Audio,
    Monitor,
    Bluetooth,
    Tip,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    Item(usize),
    Notif(usize),
    Disk(usize),
    DiskAll,
    MenuItem(usize),
    ClipItem(usize),
    WallItem(usize),
    AudioMute(bool),
    AudioSlider(bool),
    AudioDev(bool, usize),
    WallRollout,
    WallButton(usize),
    WallTheme(usize),
    WifiRow(usize),
    WifiToggle,
    WifiRefresh,
    WifiEditor,
    /// posuvník jasu (false) / kontrastu (true)
    MonSlider(bool),
    /// tlačidlo teploty farieb
    MonPreset(usize),
    /// tlačidlo vstupu
    MonInput(usize),
    /// prepínač nočného režimu
    MonNight,
    BtRow(usize),
    BtToggle,
    BtDiscoverable,
    BtRefresh,
    BtEditor,
    Panel,
}

/// Riadok vo flow layoute menu (indexy do `menu.items`).
enum MenuLayoutRow {
    Icons(Vec<usize>),
    Labels(Vec<usize>),
    Separator,
}

/// Udalosti pre D-Bus vlákno (zatvorenie / akcia notifikácie).
#[derive(Clone, Debug, PartialEq)]
pub enum NotifEvent {
    Closed(u32, u32),
    Action(u32, String),
}

struct NotifItem {
    n: Notification,
    expires: Option<Instant>,
    summary: String,
    lines: Vec<String>,
    h: f32,
}

struct ItemState {
    id: ModId,
    shown: String,
    live: String,
    tip: String,
    badge: Option<(Color, Color)>,
    fg: Option<Color>,
    pad: f32, // logické px
    btn: Btn,
    w: Spring,
    a: Spring,
    hover: Spring,
}

#[derive(Clone, Copy, Debug)]
struct Cell {
    mday: i32,
    other: bool,
    today: bool,
}

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

// logické rozmery kalendára
const CAL_CELL: f32 = 34.0;
const CAL_PAD: f32 = 14.0;
const CAL_HEAD: f32 = 40.0;
const CAL_NAMES: f32 = 26.0;
const CAL_ROW: f32 = 32.0;
const CAL_BOTTOM: f32 = 14.0;
// panel s diskami
const DISK_W: f32 = 340.0;
const DISK_HEAD: f32 = 40.0;
const DISK_ROW: f32 = 46.0;
const DISK_BOTTOM: f32 = 12.0;

// panel schránky (logické px)
/// po koľkých sekundách bez stlačenia klávesu sa zadávanie hesla zruší
const WIFI_PW_IDLE_SECS: u64 = 45;
const WIFI_PAD: f32 = 14.0;
const WIFI_HEAD: f32 = 44.0;
const WIFI_ROW_EXTRA: f32 = 20.0; // výška riadku = písmo + toto
const WIFI_FOOT: f32 = 30.0;
const WIFI_SCROLLBAR: f32 = 8.0;
const WIFI_SCROLL_UNIT: f32 = 10.0;
const AUDIO_PAD: f32 = 14.0;
const AUDIO_ROW_EXTRA: f32 = 20.0;
const AUDIO_LABEL: f32 = 26.0;
const AUDIO_GAP: f32 = 16.0;
const AUDIO_SCROLL_UNIT: f32 = 10.0;
const WALL_HEAD: f32 = 44.0;
const WALL_PAD: f32 = 14.0;
const WALL_GAP: f32 = 12.0;
const WALL_SCROLLBAR: f32 = 8.0;
const WALL_SCROLL_UNIT: f32 = 10.0;
const CLIP_PAD: f32 = 12.0;
const CLIP_ROW_EXTRA: f32 = 16.0; // výška riadku = písmo + toto
const CLIP_SCROLLBAR: f32 = 8.0;
/// jednotka osi kolieska zodpovedajúca jednému riadku
const CLIP_SCROLL_UNIT: f32 = 10.0;

pub struct Ui {
    pub cfg: Config,
    pub theme: Theme,
    pub scale: f32,
    pub data: Data,
    pub icons: IconCache,
    items: Vec<ItemState>,
    surf_w: f32,
    surf_h: f32,

    panel: Panel,       // cieľový panel
    panel_shown: Panel, // panel, ktorý sa práve kreslí (kým sa zatvára)
    big: Panel,         // veľký panel nahrádzajúci ostrovček (kalendár / disky)
    panel_h: Spring,
    panel_w: Spring,
    panel_a: Spring,
    radius: Spring,
    hud_t: Spring,
    hud_until: Option<Instant>,
    hide: Spring,
    manual_hidden: bool,
    auto_hidden: bool,

    cal_year: i32,
    cal_month: i32, // 0..11
    cal_cells: Vec<Cell>,
    cal_weeks: Vec<String>,
    cal_names: Vec<String>,
    cal_title: String,
    cal_key: (i32, i32, i32),
    tip_lines: Vec<String>,
    notifs: Vec<NotifItem>,
    notif_dirty: bool,
    notif_urgent: bool,
    notif_t: Spring,
    cal_t: Spring,
    notif_w: Spring,
    notif_h: Spring,
    events: Vec<NotifEvent>,
    /// história schránky (najnovšie prvé), prvý viditeľný riadok, nazbieraný posun kolieska
    clip: Vec<ClipEntry>,
    clip_loaded: bool,
    clip_scroll: usize,
    clip_acc: f32,
    /// modul wallpaper: obrázky, aktuálne pozadie, prvý viditeľný riadok mriežky, nazbieraný posun kolieska
    wall: Vec<crate::wallpaper::WallEntry>,
    wall_loaded: bool,
    wall_current: String,
    wall_scroll: usize,
    wall_acc: f32,
    /// témy (názov, cesta), aktuálna téma (prázdna = „wallpaper“), rozbalená ponuka tém a jej skrolovanie
    wall_themes: Vec<(String, String)>,
    wall_theme: String,
    wall_menu: bool,
    wall_menu_scroll: usize,
    /// modul audio: zariadenia, ťahaný posuvník (`false` = výstup, `true` = vstup), dráhy posuvníkov z posledného kreslenia
    audio: crate::audio::AudioState,
    audio_loaded: bool,
    audio_drag: Option<bool>,
    audio_sent: Instant,
    audio_unsent: bool,
    audio_rects: [Rect; 2],
    audio_acc: f32,
    /// modul Wi-Fi: stav, správa vo päte, skrolovanie, rozpísané heslo
    wifi: crate::wifi::WifiState,
    wifi_loaded: bool,
    wifi_note: String,
    wifi_scroll: usize,
    wifi_acc: f32,
    wifi_pw: Option<WifiPw>,
    /// poistka: zadávanie hesla sa po nečinnosti samo zruší (uvoľní klávesnicu)
    wifi_pw_until: Option<Instant>,
    /// modul monitor: jas/kontrast/preset/vstup/nočný režim
    monitor: crate::monitor::MonitorState,
    monitor_loaded: bool,
    /// ťahaný posuvník: false = jas, true = kontrast
    monitor_drag: Option<bool>,
    monitor_sent: Instant,
    monitor_unsent: bool,
    /// dráhy posuvníkov z posledného kreslenia [jas, kontrast]
    monitor_rects: [Rect; 2],
    /// modul Bluetooth
    bluetooth: crate::bluetooth::BtState,
    bluetooth_loaded: bool,
    bluetooth_scroll: usize,
    bluetooth_acc: f32,
    /// akcie, ktoré vznikli mimo `click` (otvorenie panela) – `click` ich vráti volajúcemu
    queued: Vec<Action>,

    ptr: Option<(f32, f32)>,
    hover: Option<Hit>,
    tip_pending: Option<(usize, Instant)>,
    collapse_at: Option<Instant>,
    /// po prepnutí panelu cez `module:` sa odchod myši ignoruje, kým myš nevstúpi do nového panela
    /// (nový panel môže mať inú veľkosť, takže myš po kliku často skončí mimo neho)
    /// Some(najskôr kedy ho pohyb myši môže zrušiť) = odklad zatvorenia je aktívny
    grace: Option<Instant>,

    pub pill: Rect,
    hits: Vec<(Rect, Hit)>,
}

/// Rozpísané zadávanie hesla pre zabezpečenú sieť.
struct WifiPw {
    ssid: String,
    ap: String,
    key_mgmt: String,
    text: String,
}

fn fade(c: Color, a: f32) -> Color {
    [c[0], c[1], c[2], c[3] * a.clamp(0.0, 1.0)]
}

pub fn days_in_month(year: i32, month0: i32) -> i32 {
    match month0 {
        0 | 2 | 4 | 6 | 7 | 9 | 11 => 31,
        3 | 5 | 8 | 10 => 30,
        _ => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
    }
}

/// Nahradí `{meno}` v šablóne tooltipu.
pub fn fill(fmt: &str, vars: &[(&str, String)]) -> String {
    let mut out = fmt.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

/// „názov (veľkosť)“
fn disk_title(d: &DiskInfo) -> String {
    if d.size.is_empty() {
        d.label.clone()
    } else {
        format!("{} ({})", d.label, d.size)
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}


fn menu_content_width(kind: MenuKind, icon_grid_w: f32, text_grid_w: f32, m: &crate::config::MenuCfg) -> f32 {
    match kind {
        MenuKind::Text => text_grid_w,
        MenuKind::Icons => icon_grid_w,
        MenuKind::Hybrid => {
            let has_icons = m.items.iter().any(|it| !it.separator && !it.icon.is_empty());
            let has_labels = m.items.iter().any(|it| !it.separator && it.icon.is_empty() && !it.label.is_empty());
            let mut w = 0.0f32;
            if has_icons {
                w = w.max(icon_grid_w);
            }
            if has_labels {
                w = w.max(text_grid_w);
            }
            if w > 0.0 {
                w
            } else {
                icon_grid_w
            }
        }
    }
}

impl Ui {
    pub fn new(cfg: Config, theme: Theme, scale: f32) -> Ui {
        let mut items = Vec::new();
        for name in &cfg.modules {
            let id = match name.as_str() {
                "network" => ModId::Network,
                "vpn" => ModId::Vpn,
                "clock" => ModId::Clock,
                "keyboard" => ModId::Keyboard,
                "audio" => ModId::Audio,
                "capslock" => ModId::Caps,
                "inhibitor" => ModId::Inhibit,
                "recorder" => ModId::Recorder,
                "disks" => ModId::Disks,
                other => match cfg.custom.iter().position(|c| c.name == other) {
                    Some(i) => ModId::Custom(i),
                    None => continue,
                },
            };
            let btn = match id {
                ModId::Network => cfg.net.clone(),
                ModId::Vpn => cfg.vpn.clone(),
                ModId::Clock => cfg.clock.clone(),
                ModId::Keyboard => cfg.kb.clone(),
                ModId::Audio => cfg.audio.clone(),
                ModId::Caps => Btn::default(),
                ModId::Inhibit => cfg.inh.clone(),
                ModId::Recorder => cfg.rec.clone(),
                ModId::Disks => cfg.disks.clone(),
                ModId::Custom(i) => cfg.custom[i].btn.clone(),
            };
            items.push(ItemState {
                id,
                shown: String::new(),
                live: String::new(),
                tip: String::new(),
                badge: None,
                fg: None,
                pad: 10.0,
                btn,
                w: Spring::with(0.0, 300.0, 26.0),
                a: Spring::with(0.0, 320.0, 30.0),
                hover: Spring::with(0.0, 500.0, 40.0),
            });
        }
        let now = sys::now_unix();
        let tm = sys::local_tm(now);
        let (sw, sh) = (cfg.surface_w as f32 * scale, cfg.surface_h as f32 * scale);
        let radius = cfg.radius;
        let mut ui = Ui {
            data: Data { custom: cfg.custom.iter().map(|c| CustomOut { text: c.text.clone(), ..Default::default() }).collect(), ..Default::default() },
            icons: {
                let dir = cfg.path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| std::path::PathBuf::from("."));
                let mut ic = IconCache::new(dir);
                let paths: Vec<String> = cfg.menus.iter().flat_map(|m| m.items.iter().map(|it| it.icon.clone())).filter(|s| !s.is_empty()).collect();
                ic.preload(paths.iter().map(|s| s.as_str()));
                ic
            },
            cfg,
            theme,
            scale,
            items,
            surf_w: sw,
            surf_h: sh,
            panel: Panel::None,
            panel_shown: Panel::None,
            big: Panel::Calendar,
            panel_h: Spring::with(0.0, 240.0, 26.0),
            panel_w: Spring::with(0.0, 240.0, 26.0),
            panel_a: Spring::with(0.0, 420.0, 42.0),
            radius: Spring::with(radius, 300.0, 30.0),
            hud_t: Spring::with(0.0, 260.0, 26.0),
            hud_until: None,
            hide: Spring::with(0.0, 200.0, 24.0),
            manual_hidden: false,
            auto_hidden: false,
            cal_year: tm.tm_year + 1900,
            cal_month: tm.tm_mon,
            cal_cells: Vec::new(),
            cal_weeks: Vec::new(),
            cal_names: Vec::new(),
            cal_title: String::new(),
            cal_key: (0, 0, 0),
            tip_lines: Vec::new(),
            notifs: Vec::new(),
            notif_dirty: false,
            notif_urgent: false,
            notif_t: Spring::with(0.0, 260.0, 26.0),
            cal_t: Spring::with(0.0, 260.0, 26.0),
            notif_w: Spring::with(0.0, 240.0, 26.0),
            notif_h: Spring::with(0.0, 240.0, 26.0),
            events: Vec::new(),
            clip: Vec::new(),
            clip_loaded: false,
            clip_scroll: 0,
            clip_acc: 0.0,
            wall: Vec::new(),
            wall_loaded: false,
            wall_current: String::new(),
            wall_scroll: 0,
            wall_acc: 0.0,
            wall_themes: Vec::new(),
            wall_theme: String::new(),
            wall_menu: false,
            wall_menu_scroll: 0,
            audio: Default::default(),
            audio_loaded: false,
            audio_drag: None,
            audio_sent: Instant::now(),
            audio_unsent: false,
            audio_rects: [Rect::new(0.0, 0.0, 0.0, 0.0); 2],
            audio_acc: 0.0,
            wifi: Default::default(),
            wifi_loaded: false,
            wifi_note: String::new(),
            wifi_scroll: 0,
            wifi_acc: 0.0,
            wifi_pw: None,
            wifi_pw_until: None,
            monitor: Default::default(),
            monitor_loaded: false,
            monitor_drag: None,
            monitor_sent: Instant::now(),
            monitor_unsent: false,
            monitor_rects: [Rect::new(0.0, 0.0, 0.0, 0.0); 2],
            bluetooth: Default::default(),
            bluetooth_loaded: false,
            bluetooth_scroll: 0,
            bluetooth_acc: 0.0,
            queued: Vec::new(),
            ptr: None,
            hover: None,
            tip_pending: None,
            collapse_at: None,
            grace: None,
            pill: Rect::new(0.0, 0.0, 0.0, 0.0),
            hits: Vec::new(),
        };
        ui.hide.snap(0.0);
        ui
    }

    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale;
        self.surf_w = self.cfg.surface_w as f32 * scale;
        self.surf_h = self.cfg.surface_h as f32 * scale;
        self.notif_dirty = true;
    }

    fn s(&self, v: f32) -> f32 {
        v * self.scale
    }

    // ------------------------------------------------------------- dáta

    pub fn set_clock(&mut self, text: String) {
        self.data.clock = text;
    }

    pub fn apply(&mut self, m: Msg, now: Instant) {
        match m {
            Msg::Notif(n) => self.add_notif(n, now),
            Msg::NotifClose(id) => {
                let before = self.notifs.len();
                self.notifs.retain(|x| x.n.id != id);
                if self.notifs.len() != before {
                    self.notif_dirty = true;
                }
            }
            Msg::Disks(d) => {
                self.data.disks = d;
                if self.data.disks.is_empty() && self.panel == Panel::Disks {
                    self.set_panel(Panel::None);
                }
            }
            Msg::Clip(v) => {
                self.clip = v;
                self.clip_loaded = true;
                let max_start = self.clip.len().saturating_sub(self.cfg.clip_rows);
                self.clip_scroll = self.clip_scroll.min(max_start);
            }
            Msg::Wall(v, current) => {
                self.wall = v;
                self.wall_loaded = true;
                self.wall_current = current;
                let max_start = self.wall_row_count().saturating_sub(self.cfg.wall.rows);
                self.wall_scroll = self.wall_scroll.min(max_start);
            }
            Msg::WallCurrent(path) => self.wall_current = path,
            Msg::WallThemes(list, selected) => {
                self.wall_themes = list;
                self.wall_theme = selected;
            }
            Msg::WallTheme(name) => self.wall_theme = name,
            Msg::Wifi(s) => {
                self.wifi = s;
                self.wifi_loaded = true;
                let max_start = self.wifi.aps.len().saturating_sub(self.cfg.wifi.rows);
                self.wifi_scroll = self.wifi_scroll.min(max_start);
            }
            Msg::WifiNote(n) => self.wifi_note = n,
            Msg::WifiPaste(t) => {
                if let Some(pw) = &mut self.wifi_pw {
                    pw.text.extend(t.trim_end_matches(['\n', '\r']).chars().filter(|c| !c.is_control()));
                }
            }
            Msg::WallThumb(i, thumb) => {
                if let Some(e) = self.wall.get_mut(i) {
                    if e.thumb == thumb {
                        e.ready = true;
                    }
                }
            }
            Msg::Net(n) => self.data.net = n,
            Msg::Vpn(v) => self.data.vpn = v,
            Msg::Layout(l) => self.data.layout = l,
            Msg::AudioDev(st) => {
                // počas ťahania posuvníka by nás staré hodnoty z `pactl` vracali späť
                if self.audio_drag.is_none() {
                    self.audio = st;
                }
                self.audio_loaded = true;
            }
            Msg::Monitor(st) => {
                if self.monitor_drag.is_none() {
                    self.monitor = st;
                } else {
                    // počas ťahania aktualizuj len to, čo nie je ťahané
                    let night = st.night;
                    let preset = st.preset;
                    let input = st.input;
                    let error = st.error;
                    let loaded = st.loaded;
                    if self.monitor_drag != Some(false) {
                        self.monitor.brightness = st.brightness;
                    }
                    if self.monitor_drag != Some(true) {
                        self.monitor.contrast = st.contrast;
                    }
                    self.monitor.night = night;
                    self.monitor.preset = preset;
                    self.monitor.input = input;
                    self.monitor.error = error;
                    self.monitor.loaded = loaded;
                }
                self.monitor_loaded = true;
            }
            Msg::Bluetooth(st) => {
                self.bluetooth = st;
                self.bluetooth_loaded = true;
                let max_start = self.bluetooth.devices.len().saturating_sub(self.cfg.bluetooth.rows);
                self.bluetooth_scroll = self.bluetooth_scroll.min(max_start);
            }
            Msg::Audio { muted, volume } => {
                if self.audio_drag.is_none() {
                    if let Some(i) = self.audio_default(false) {
                        self.audio.sinks[i].volume = (volume * 100.0).round().max(0.0) as u32;
                        self.audio.sinks[i].mute = muted;
                    }
                }
                let changed = self.data.audio_known && (self.data.muted != muted || (self.data.volume - volume).abs() > 0.004);
                self.data.muted = muted;
                self.data.volume = volume;
                self.data.audio_known = true;
                if changed && self.cfg.audio_hud && self.panel != Panel::Audio {
                    self.data.hud_bright = false;
                    self.hud_until = Some(now + Duration::from_millis(self.cfg.hud_ms));
                    self.hud_t.set(1.0);
                }
            }
            Msg::Brightness(b) => {
                self.data.bright = b.clamp(0.0, 1.0);
                self.data.hud_bright = true;
                self.hud_until = Some(now + Duration::from_millis(self.cfg.hud_ms));
                self.hud_t.set(1.0);
            }
            Msg::Caps(c) => self.data.caps = c,
            Msg::Inhibit(i) => self.data.inhibit = i,
            Msg::Rec(r) => self.data.rec = r,
            Msg::Custom(i, o) => {
                if i < self.data.custom.len() {
                    self.data.custom[i] = o;
                }
            }
            Msg::AutoHide(h) => self.auto_hidden = h,
            Msg::Hide => self.manual_hidden = true,
            Msg::Show => {
                self.manual_hidden = false;
                self.auto_hidden = false;
            }
        }
    }

    fn view(&self, id: ModId) -> (String, Option<(Color, Color)>, Option<Color>, String, f32) {
        let t = &self.theme;
        let d = &self.data;
        let c = &self.cfg;
        let badge_acc = Some((t.accent, t.on_accent));
        let none = (String::new(), None, None, String::new(), 12.0);
        match id {
            ModId::Network => {
                let n = &d.net;
                let vars = [
                    ("ssid", n.ssid.clone()),
                    ("ifname", n.ifname.clone()),
                    ("ip", n.ip.clone()),
                    ("gateway", n.gateway.clone()),
                    ("signal", n.signal_pct.to_string()),
                    ("dbm", n.dbm.to_string()),
                    ("freq", n.freq_ghz.clone()),
                ];
                if !n.up {
                    (c.net_icon_off.clone(), None, Some(t.alert), fill(&c.net_tip_off, &vars), 10.0)
                } else if n.wifi {
                    let k = c.net_icons.len().max(1);
                    let idx = ((n.signal_pct as usize * k) / 100).min(k - 1);
                    let icon = c.net_icons.get(idx).cloned().unwrap_or_default();
                    (icon, None, None, fill(&c.net_tip_wifi, &vars), 10.0)
                } else {
                    (c.net_icon_eth.clone(), None, None, fill(&c.net_tip_eth, &vars), 10.0)
                }
            }
            ModId::Vpn => match &d.vpn {
                Some(v) => {
                    let vars = [("ifname", v.ifname.clone()), ("ip", v.ip.clone())];
                    (c.vpn_text.clone(), badge_acc, None, fill(&c.vpn.tooltip, &vars), 10.0)
                }
                None => (String::new(), None, None, String::new(), 10.0),
            },
            ModId::Clock => {
                let tip = if c.clock.tooltip.is_empty() {
                    String::new()
                } else {
                    sys::strftime_str(&c.clock.tooltip, &sys::local_tm(sys::now_unix()))
                };
                (d.clock.clone(), None, None, tip, 14.0)
            }
            ModId::Keyboard => (d.layout.clone(), None, None, fill(&c.kb.tooltip, &[("layout", d.layout.clone())]), 10.0),
            ModId::Audio => {
                if d.muted {
                    let vars = [("volume", ((d.volume * 100.0).round() as i32).to_string())];
                    (c.audio_icon_muted.clone(), badge_acc, None, fill(&c.audio.tooltip, &vars), 12.0)
                } else {
                    none
                }
            }
            ModId::Caps => {
                if d.caps {
                    (c.caps_text.clone(), badge_acc, None, c.caps.tooltip.clone(), 12.0)
                } else {
                    none
                }
            }
            ModId::Inhibit => {
                if d.inhibit {
                    (c.inh_icon.clone(), badge_acc, None, c.inh.tooltip.clone(), 12.0)
                } else {
                    none
                }
            }
            ModId::Recorder => {
                if d.rec.is_empty() {
                    none
                } else {
                    (d.rec.clone(), Some((t.alert, t.on_alert)), None, fill(&c.rec.tooltip, &[("text", d.rec.clone())]), 12.0)
                }
            }
            ModId::Disks => {
                if d.disks.is_empty() {
                    none
                } else {
                    let list = d.disks.iter().map(|k| format!("{}  →  {}", disk_title(k), k.mount)).collect::<Vec<_>>().join("\n");
                    let vars = [("icon", c.disks_icon.clone()), ("count", d.disks.len().to_string()), ("label", d.disks[0].label.clone()), ("list", list)];
                    let fmt = if d.disks.len() > 1 { &c.disks_format_many } else { &c.disks_format };
                    (fill(fmt, &vars), None, None, fill(&c.disks.tooltip, &vars), 10.0)
                }
            }
            ModId::Custom(i) => {
                let o = d.custom.get(i).cloned().unwrap_or_default();
                let tip = if c.custom[i].btn.tooltip.is_empty() {
                    o.tooltip.clone()
                } else {
                    fill(&c.custom[i].btn.tooltip, &[("text", o.text.clone()), ("tooltip", o.tooltip.clone()), ("class", o.class.clone())])
                };
                (o.text, None, None, tip, 10.0)
            }
        }
    }

    // ------------------------------------------------------- animácie

    fn px_font(&self) -> f32 {
        self.cfg.font_size * self.scale
    }

    fn refresh_items(&mut self, text: &mut Text) {
        let px = self.px_font();
        for i in 0..self.items.len() {
            let (txt, badge, fg, tip, pad) = self.view(self.items[i].id);
            let sc = self.scale;
            let it = &mut self.items[i];
            it.live = txt.clone();
            it.badge = badge;
            it.fg = fg;
            it.tip = tip;
            it.pad = pad;
            if txt.is_empty() {
                it.w.set(0.0);
                it.a.set(0.0);
            } else {
                it.shown = txt;
                let w = text.measure(&it.shown, px) + 2.0 * pad * sc;
                it.w.set(w);
                it.a.set(1.0);
            }
        }
    }

    /// Posun animácií; vráti true, kým sa niečo hýbe.
    pub fn update(&mut self, dt: f32, now: Instant, text: &mut Text) -> bool {
        self.tick(now);
        self.refresh_items(text);
        if self.notif_dirty {
            self.relayout_notifs(text);
        }

        // notifikácia / otvorený veľký panel (menu, kalendár, disky) ostrovček vždy vysunie
        // inak by autohide pri pohybe kurzorom k dolným položkám menu skryl celý ostrovček
        let hidden = (self.manual_hidden || self.auto_hidden)
            && self.notifs.is_empty()
            && !self.big_open();
        self.hide.set(if hidden { 1.0 } else { 0.0 });
        if hidden && self.panel != Panel::None {
            self.set_panel(Panel::None);
        }

        let hovered = match self.hover {
            Some(Hit::Item(i)) => Some(i),
            _ => None,
        };
        let mut busy = false;
        for (i, it) in self.items.iter_mut().enumerate() {
            let clickable = it.badge.is_none() && !(it.btn.click.is_empty() && it.btn.right.is_empty() && it.btn.middle.is_empty());
            it.hover.set(if hovered == Some(i) && clickable { 1.0 } else { 0.0 });
            it.w.step(dt);
            it.a.step(dt);
            it.hover.step(dt);
            busy |= !(it.w.settled() && it.a.settled() && it.hover.settled());
        }
        let r_target = if self.panel != Panel::None || !self.notifs.is_empty() { self.cfg.radius_expanded } else { self.cfg.radius };
        self.radius.set(r_target);
        for sp in [&mut self.panel_h, &mut self.panel_w, &mut self.panel_a, &mut self.radius, &mut self.hud_t, &mut self.hide, &mut self.notif_t, &mut self.notif_w, &mut self.notif_h, &mut self.cal_t] {
            sp.step(dt);
            busy |= !sp.settled();
        }
        if self.panel == Panel::None && self.panel_h.x < 0.5 && self.panel_h.settled() {
            self.panel_shown = Panel::None;
        }
        busy
    }

    // ------------------------------------------------------ notifikácie

    /// Prvé pravidlo, ktoré sedí na notifikáciu.
    fn rule_for(&self, n: &Notification) -> Option<&crate::config::NotifRule> {
        self.cfg.notif_rules.iter().find(|r| {
            (!r.summary.is_empty() || !r.app.is_empty() || !r.body.is_empty())
                && n.summary.contains(&r.summary)
                && n.app.contains(&r.app)
                && n.body.contains(&r.body)
        })
    }

    fn add_notif(&mut self, mut n: Notification, now: Instant) {
        n.summary = strip_markup(&n.summary);
        n.body = strip_markup(&n.body);
        let rule_timeout = self.rule_for(&n).and_then(|r| r.timeout_ms);
        let timeout = if let Some(t) = rule_timeout {
            t
        } else if n.timeout_ms < 0 {
            if n.urgency == 2 { self.cfg.notif_urgent_timeout } else { self.cfg.notif_timeout }
        } else {
            n.timeout_ms
        };
        let expires = if timeout > 0 { Some(now + Duration::from_millis(timeout as u64)) } else { None };
        // rovnaké id (replaces_id) alebo rovnaký súhrn z tej istej aplikácie = nahradí staršiu
        self.notifs.retain(|x| x.n.id != n.id && !(x.n.app == n.app && x.n.summary == n.summary));
        self.notifs.insert(0, NotifItem { n, expires, summary: String::new(), lines: Vec::new(), h: 0.0 });
        while self.notifs.len() > 20 {
            if let Some(old) = self.notifs.pop() {
                self.events.push(NotifEvent::Closed(old.n.id, 1));
            }
        }
        self.notif_dirty = true;
    }

    fn dismiss(&mut self, k: usize, reason: u32) {
        if k < self.notifs.len() {
            let n = self.notifs.remove(k);
            self.events.push(NotifEvent::Closed(n.n.id, reason));
            self.notif_dirty = true;
            self.hover = None;
        }
    }

    pub fn take_events(&mut self) -> Vec<NotifEvent> {
        std::mem::take(&mut self.events)
    }

    fn ellipsize(text: &mut Text, s: &str, px: f32, maxw: f32) -> String {
        if text.measure(s, px) <= maxw {
            return s.to_string();
        }
        let mut chars: Vec<char> = s.chars().collect();
        while !chars.is_empty() {
            chars.pop();
            let t: String = chars.iter().collect::<String>() + "…";
            if text.measure(&t, px) <= maxw {
                return t;
            }
        }
        String::new()
    }

    fn wrap(text: &mut Text, s: &str, px: f32, maxw: f32, max_lines: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        for para in s.split('\n') {
            let mut cur = String::new();
            for word in para.split_whitespace() {
                let cand = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
                if text.measure(&cand, px) <= maxw || cur.is_empty() {
                    cur = cand;
                } else {
                    lines.push(std::mem::take(&mut cur));
                    cur = word.to_string();
                }
            }
            lines.push(cur);
        }
        while lines.last().map(|l| l.is_empty()).unwrap_or(false) {
            lines.pop();
        }
        if lines.len() > max_lines {
            lines.truncate(max_lines);
            let last = lines.pop().unwrap_or_default();
            lines.push(Self::ellipsize(text, &(last + " …"), px, maxw));
        }
        lines.into_iter().map(|l| Self::ellipsize(text, &l, px, maxw)).collect()
    }

    fn relayout_notifs(&mut self, text: &mut Text) {
        self.notif_dirty = false;
        if self.notifs.is_empty() {
            self.notif_t.set(0.0);
            return;
        }
        let s = self.scale;
        let px = self.cfg.notif_font * s;
        let width = self.cfg.notif_width * s;
        let pad = 12.0 * s;
        let inner = width - 2.0 * pad;
        let line_h = px * 1.4;
        let max_lines = self.cfg.notif_lines;
        let extra = self.notifs.len() > 1;
        let n = &mut self.notifs[0];
        n.summary = Self::ellipsize(text, &n.n.summary, px * 1.05, inner);
        n.lines = Self::wrap(text, &n.n.body, px, inner, max_lines);
        n.h = pad * 2.0 + line_h * 1.1 + line_h * n.lines.len() as f32 + if extra { 16.0 * s } else { 0.0 };
        self.notif_urgent = n.n.urgency == 2;
        let (w, h) = (width, n.h);
        if self.notif_t.x < 0.02 {
            self.notif_w.snap(w);
            self.notif_h.snap(h);
        }
        self.notif_w.set(w);
        self.notif_h.set(h);
        self.notif_t.set(1.0);
        if self.panel != Panel::None {
            self.set_panel(Panel::None);
        }
    }

    /// Notifikácia nahradí obsah ostrovčeka (ako HUD).
    fn build_notif(&mut self, frame: &mut Frame, text: &mut Text, pill: Rect, nt: f32) {
        let Some(top) = self.notifs.first() else { return };
        let (summary, lines) = (top.summary.clone(), top.lines.clone());
        let more = self.notifs.len() - 1;
        let s = self.scale;
        let t = self.theme;
        let px = self.cfg.notif_font * s;
        let pad = 12.0 * s;
        let line_h = px * 1.4;
        let center = self.cfg.notif_center;
        let col = if self.notif_urgent { t.urgent_text } else { t.fg };
        self.hits.push((pill, Hit::Notif(0)));
        if self.hover == Some(Hit::Notif(0)) {
            frame.content.push(Item::Rect { r: pill, radius: 14.0 * s, soft: 0.8, color: fade(t.accent, 0.08 * nt) });
        }
        let place = |text: &mut Text, out: &mut Vec<Item>, s_: &str, p: f32, cy: f32, c: Color| {
            let w = text.measure(s_, p);
            let x = if center { pill.x + (pill.w - w) * 0.5 } else { pill.x + pad };
            let (a, d) = text.line_metrics(p);
            text.layout_to(out, s_, p, x, cy + (a - d) * 0.5, c);
        };
        place(text, &mut frame.content, &summary, px * 1.05, pill.y + pad + line_h * 0.55, fade(col, nt));
        for (j, l) in lines.iter().enumerate() {
            place(text, &mut frame.content, l, px, pill.y + pad + line_h * 1.1 + line_h * (j as f32 + 0.5), fade(col, 0.85 * nt));
        }
        if more > 0 {
            let label = format!("+{more}");
            let p = px * 0.8;
            let w = text.measure(&label, p);
            let (a, d) = text.line_metrics(p);
            let cy = pill.y + pill.h - 14.0 * s;
            text.layout_to(&mut frame.content, &label, p, pill.x + (pill.w - w) * 0.5, cy + (a - d) * 0.5, fade(t.fg, 0.6 * nt));
        }
    }

    // ---------------------------------------------------------- panely

    fn rebuild_cal(&mut self) {
        let today = sys::local_tm(sys::now_unix());
        let key = (self.cal_year, self.cal_month, today.tm_mday + today.tm_mon * 40 + today.tm_year * 500);
        if key == self.cal_key && !self.cal_cells.is_empty() {
            return;
        }
        self.cal_key = key;
        let first = sys::make_tm(self.cal_year, self.cal_month, 1);
        let offset = (first.tm_wday + 6) % 7; // pondelok = 0
        self.cal_title = capitalize(&sys::strftime_str("%B %Y", &first));
        self.cal_cells.clear();
        self.cal_weeks.clear();
        for i in 0..42 {
            let tm = sys::make_tm(self.cal_year, self.cal_month, 1 - offset + i);
            let is_today = tm.tm_mday == today.tm_mday && tm.tm_mon == today.tm_mon && tm.tm_year == today.tm_year;
            self.cal_cells.push(Cell { mday: tm.tm_mday, other: tm.tm_mon != self.cal_month, today: is_today });
            if i % 7 == 0 {
                self.cal_weeks.push(sys::strftime_str("%V", &tm));
            }
        }
        // názvy dní: 1. január 2024 bol pondelok
        self.cal_names = (0..7).map(|d| sys::strftime_str("%a", &sys::make_tm(2024, 0, 1 + d)).chars().take(2).collect()).collect();
    }

    fn cal_size(&self) -> (f32, f32) {
        match self.big {
            Panel::Disks => {
                let n = self.data.disks.len().max(1) as f32;
                (DISK_W * self.scale, (DISK_HEAD + n * DISK_ROW + DISK_BOTTOM) * self.scale)
            }
            Panel::Menu(mi) => self.menu_size(mi),
            Panel::Audio => {
                let c = &self.cfg.audio_panel;
                let row = c.font_size + AUDIO_ROW_EXTRA;
                let sec = |len: usize| AUDIO_LABEL + row + len.min(c.rows).max(1) as f32 * row;
                let h = 2.0 * AUDIO_PAD + sec(self.audio.sinks.len()) + AUDIO_GAP + sec(self.audio.sources.len());
                (c.width * self.scale, h * self.scale)
            }
            Panel::Monitor => {
                let c = &self.cfg.monitor;
                let f = c.font_size;
                // 2 posuvníky + 2 rady tlačidiel (preset, input) + nočný režim
                let row = f + 20.0;
                let label = 22.0;
                let gap = 12.0;
                let h = 2.0 * 14.0
                    + label + row          // jas
                    + gap + label + row    // kontrast
                    + gap + label + row    // teplota
                    + gap + label + row    // vstup
                    + gap + row;           // nočný režim
                (c.width * self.scale, h * self.scale)
            }
            Panel::Bluetooth => {
                let c = &self.cfg.bluetooth;
                let f = c.font_size;
                let rows = self.bluetooth.devices.len().min(c.rows).max(1) as f32;
                let h = 2.0 * WIFI_PAD + WIFI_HEAD + rows * (f + WIFI_ROW_EXTRA) + WIFI_FOOT;
                (c.width * self.scale, h * self.scale)
            }
            Panel::Wifi => {
                let c = &self.cfg.wifi;
                let f = c.font_size;
                let h = if self.wifi_pw.is_some() {
                    2.0 * WIFI_PAD + WIFI_HEAD + 4.0 + (f + 18.0) + 10.0 + (f * 0.85 + 6.0)
                } else {
                    2.0 * WIFI_PAD + WIFI_HEAD + self.wifi_rows_shown() as f32 * (f + WIFI_ROW_EXTRA) + WIFI_FOOT
                };
                (c.width * self.scale, h * self.scale)
            }
            Panel::Wall => {
                let c = &self.cfg.wall;
                let (tw, th) = c.thumb_size();
                let sb = if self.wall_row_count() > c.rows { WALL_SCROLLBAR } else { 0.0 };
                let w = 2.0 * WALL_PAD + c.columns as f32 * tw + (c.columns - 1) as f32 * WALL_GAP + sb;
                let n = self.wall_rows_shown() as f32;
                let h = 2.0 * WALL_PAD + WALL_HEAD + WALL_GAP + n * th + (n - 1.0) * WALL_GAP;
                (w * self.scale, h * self.scale)
            }
            Panel::Clip => {
                let row_h = self.cfg.clip_font + CLIP_ROW_EXTRA;
                let h = 2.0 * CLIP_PAD + self.clip_rows() as f32 * row_h;
                (self.cfg.clip_width * self.scale, h * self.scale)
            }
            _ => {
                let w = 8.0 * CAL_CELL + 2.0 * CAL_PAD;
                let h = CAL_HEAD + CAL_NAMES + 6.0 * CAL_ROW + CAL_BOTTOM;
                (w * self.scale, h * self.scale)
            }
        }
    }

    fn menu_rows(menu: &crate::config::MenuCfg) -> Vec<MenuLayoutRow> {
        let icon_cols = menu.icon_columns.max(1);
        let text_cols = menu.text_columns.max(1);
        let mut rows: Vec<MenuLayoutRow> = Vec::new();
        let mut icons: Vec<usize> = Vec::new();
        let mut labels: Vec<usize> = Vec::new();

        let flush_icons = |icons: &mut Vec<usize>, rows: &mut Vec<MenuLayoutRow>| {
            if !icons.is_empty() {
                rows.push(MenuLayoutRow::Icons(std::mem::take(icons)));
            }
        };
        let flush_labels = |labels: &mut Vec<usize>, rows: &mut Vec<MenuLayoutRow>| {
            if !labels.is_empty() {
                rows.push(MenuLayoutRow::Labels(std::mem::take(labels)));
            }
        };

        for (i, it) in menu.items.iter().enumerate() {
            if it.separator {
                flush_icons(&mut icons, &mut rows);
                flush_labels(&mut labels, &mut rows);
                rows.push(MenuLayoutRow::Separator);
                continue;
            }
            match menu.kind {
                MenuKind::Text => {
                    // text menu: všetky položky ako labely (label, alebo icon ako nerd-font glyf)
                    if it.label.is_empty() && it.icon.is_empty() {
                        continue;
                    }
                    labels.push(i);
                    if labels.len() >= text_cols {
                        flush_labels(&mut labels, &mut rows);
                    }
                }
                MenuKind::Icons => {
                    if it.icon.is_empty() {
                        continue;
                    }
                    icons.push(i);
                    if icons.len() >= icon_cols {
                        flush_icons(&mut icons, &mut rows);
                    }
                }
                MenuKind::Hybrid => {
                    if !it.icon.is_empty() {
                        flush_labels(&mut labels, &mut rows);
                        icons.push(i);
                        if icons.len() >= icon_cols {
                            flush_icons(&mut icons, &mut rows);
                        }
                    } else if !it.label.is_empty() {
                        flush_icons(&mut icons, &mut rows);
                        labels.push(i);
                        if labels.len() >= text_cols {
                            flush_labels(&mut labels, &mut rows);
                        }
                    }
                }
            }
        }
        flush_icons(&mut icons, &mut rows);
        flush_labels(&mut labels, &mut rows);

        if let Some(max_rows) = menu.rows {
            let only_icons = menu.kind == MenuKind::Icons
                || (menu.kind == MenuKind::Hybrid
                    && menu.items.iter().all(|it| it.separator || !it.icon.is_empty() || it.label.is_empty()));
            if only_icons {
                rows.truncate(max_rows);
            }
        }
        rows
    }

    /// Logické → fyzické rozmery menu (hybrid / icons / text).
    fn menu_size(&self, mi: usize) -> (f32, f32) {
        let Some(m) = self.cfg.menus.get(mi) else {
            return (self.s(120.0), self.s(80.0));
        };
        let icon_cols = m.icon_columns.max(1);
        let text_cols = m.text_columns.max(1);
        let title_h = if m.title.is_empty() { 0.0 } else { 36.0 };
        let icon_cell = m.icon_size + 16.0;
        let label_h = m.text_size + 18.0;
        let sep_h = (m.separator_thickness + 12.0).max(14.0);

        let icon_grid_w = icon_cols as f32 * icon_cell + (icon_cols.saturating_sub(1)) as f32 * m.gap;
        let text_grid_w = text_cols as f32 * m.cell_width + (text_cols.saturating_sub(1)) as f32 * m.gap;
        // Hybrid: šírka len podľa typov, ktoré v menu skutočne sú (nie default cell_width navyše).
        let content_w = menu_content_width(m.kind, icon_grid_w, text_grid_w, m);

        let layout = Self::menu_rows(m);
        let mut body_h = 0.0f32;
        for (ri, row) in layout.iter().enumerate() {
            if ri > 0 {
                body_h += m.gap;
            }
            body_h += match row {
                MenuLayoutRow::Icons(_) => icon_cell,
                MenuLayoutRow::Labels(_) => label_h,
                MenuLayoutRow::Separator => sep_h,
            };
        }
        if layout.is_empty() {
            body_h = icon_cell;
        }

        let w = m.padding * 2.0 + content_w;
        let h = m.padding * 2.0 + title_h + body_h;
        (w * self.scale, h * self.scale)
    }

    fn set_panel(&mut self, p: Panel) {
        self.panel = p;
        if p == Panel::None {
            self.grace = None;
        }
        if Self::is_big(p) {
            self.big = p;
        }
        self.cal_t.set(if Self::is_big(p) { 1.0 } else { 0.0 });
        if p != Panel::Wifi {
            self.wifi_pw = None;
        }
        if p != Panel::Audio {
            self.audio_drag = None;
        }
        if p != Panel::Monitor {
            self.monitor_drag = None;
        }
        if p == Panel::Audio {
            self.audio_acc = 0.0;
            self.queued.push(Action::Audio(crate::audio::AudioCmd::Load));
        }
        if p == Panel::Monitor {
            self.queued.push(Action::Monitor(crate::monitor::MonitorCmd::Load));
        }
        if p == Panel::Bluetooth {
            self.bluetooth_scroll = 0;
            self.bluetooth_acc = 0.0;
            self.queued.push(Action::Bluetooth(crate::bluetooth::BtCmd::Load { scan: true }));
        }
        if p == Panel::Wifi {
            self.wifi_pw = None;
            self.wifi_scroll = 0;
            self.wifi_acc = 0.0;
            self.wifi_note.clear();
            self.queued.push(Action::Wifi(crate::wifi::WifiCmd::Load));
        }
        if p == Panel::Wall {
            // zoznam sa pri každom otvorení načíta znova (nové obrázky, dorobené náhľady)
            self.wall_scroll = 0;
            self.wall_acc = 0.0;
            self.wall_menu = false;
            self.wall_menu_scroll = 0;
            self.queued.push(Action::WallLoad);
        }
        if p == Panel::Clip {
            // pri každom otvorení znova načítať históriu (staré položky ostanú viditeľné, kým nepríde nová)
            self.clip_scroll = 0;
            self.clip_acc = 0.0;
            self.queued.push(Action::ClipLoad);
        }
        match p {
            Panel::None => {
                self.panel_h.set(0.0);
                self.panel_w.set(0.0);
                self.panel_a.set(0.0);
            }
            Panel::Calendar | Panel::Disks | Panel::Menu(_) | Panel::Clip | Panel::Wall | Panel::Wifi | Panel::Audio | Panel::Monitor | Panel::Bluetooth => {
                if p == Panel::Calendar {
                    self.rebuild_cal();
                }
                // veľký panel nahrádza ostrovček (ako HUD), nie je panelom pod riadkom
                self.panel_shown = Panel::None;
                self.panel_h.set(0.0);
                self.panel_w.set(0.0);
                self.panel_a.set(0.0);
            }
            Panel::Tip => {
                self.panel_shown = Panel::Tip;
                // šírka sa nastaví v `show_tip` (potrebuje meranie textu)
                self.panel_a.set(1.0);
            }
        }
    }

    fn show_tip(&mut self, idx: usize, text: &mut Text) {
        let tip = self.items[idx].tip.clone();
        if tip.is_empty() {
            return;
        }
        self.tip_lines = tip.lines().map(|l| l.to_string()).collect();
        let px = self.px_font() * 0.85;
        let widest = self.tip_lines.iter().map(|l| text.measure(l, px)).fold(0.0f32, f32::max);
        let line_h = self.s(22.0);
        self.panel_shown = Panel::Tip;
        self.panel = Panel::Tip;
        self.panel_w.set(widest + self.s(32.0));
        self.panel_h.set(self.tip_lines.len() as f32 * line_h + self.s(16.0));
        self.panel_a.set(1.0);
    }

    fn big_open(&self) -> bool {
        Self::is_big(self.panel)
    }

    fn is_big(p: Panel) -> bool {
        matches!(p, Panel::Calendar | Panel::Disks | Panel::Menu(_) | Panel::Clip | Panel::Wall | Panel::Wifi | Panel::Audio | Panel::Monitor | Panel::Bluetooth)
    }

    fn toggle_audio(&mut self) {
        if self.panel == Panel::Audio {
            self.set_panel(Panel::None);
        } else {
            self.set_panel(Panel::Audio);
        }
    }

    /// Najvyššia hodnota posuvníka v % (`limit` z `[audio]`).
    fn audio_max(&self) -> u32 {
        (self.cfg.vol_limit * 100.0).round().max(1.0) as u32
    }

    /// Index predvoleného zariadenia (alebo prvého, ak predvolené nepoznáme).
    fn audio_default(&self, src: bool) -> Option<usize> {
        let (list, def) = if src { (&self.audio.sources, &self.audio.default_source) } else { (&self.audio.sinks, &self.audio.default_sink) };
        if list.is_empty() {
            None
        } else {
            Some(list.iter().position(|d| &d.name == def).unwrap_or(0))
        }
    }

    /// Nastaví hlasitosť predvoleného zariadenia (hneď v zobrazení, príkaz sa posiela najviac každých 40 ms).
    fn audio_set_volume(&mut self, src: bool, pct: u32, force: bool) {
        let Some(i) = self.audio_default(src) else { return };
        let dev = if src { &mut self.audio.sources[i] } else { &mut self.audio.sinks[i] };
        let changed = dev.volume != pct;
        dev.volume = pct;
        self.audio_unsent |= changed;
        let now = Instant::now();
        if self.audio_unsent && (force || now.duration_since(self.audio_sent) >= Duration::from_millis(40)) {
            self.audio_unsent = false;
            self.audio_sent = now;
            self.queued.push(Action::Audio(crate::audio::AudioCmd::Volume { source: src, pct }));
        }
    }

    /// Z polohy kurzora (fyzické px) na hodnotu posuvníka.
    fn audio_drag_to(&mut self, src: bool, x: f32, force: bool) {
        let r = self.audio_rects[src as usize];
        if r.w <= 1.0 {
            return;
        }
        let pct = (((x - r.x) / r.w).clamp(0.0, 1.0) * self.audio_max() as f32).round() as u32;
        self.audio_set_volume(src, pct, force);
    }

    /// Po skončení ťahania pošle poslednú hodnotu, ak ešte nebola odoslaná.
    fn audio_flush(&mut self, src: bool) {
        if !self.audio_unsent {
            return;
        }
        if let Some(i) = self.audio_default(src) {
            let pct = if src { self.audio.sources[i].volume } else { self.audio.sinks[i].volume };
            self.audio_unsent = false;
            self.audio_sent = Instant::now();
            self.queued.push(Action::Audio(crate::audio::AudioCmd::Volume { source: src, pct }));
        }
    }

    fn monitor_set_value(&mut self, contrast: bool, pct: u32, force: bool) {
        let pct = pct.min(100);
        let cur = if contrast { self.monitor.contrast } else { self.monitor.brightness };
        let changed = cur != pct;
        if contrast {
            self.monitor.contrast = pct;
        } else {
            self.monitor.brightness = pct;
        }
        self.monitor_unsent |= changed;
        let now = Instant::now();
        if self.monitor_unsent && (force || now.duration_since(self.monitor_sent) >= Duration::from_millis(80)) {
            self.monitor_unsent = false;
            self.monitor_sent = now;
            if contrast {
                self.queued.push(Action::Monitor(crate::monitor::MonitorCmd::Contrast(pct)));
            } else {
                self.queued.push(Action::Monitor(crate::monitor::MonitorCmd::Brightness(pct)));
            }
        }
    }

    fn monitor_drag_to(&mut self, contrast: bool, x: f32, force: bool) {
        let r = self.monitor_rects[contrast as usize];
        if r.w <= 1.0 {
            return;
        }
        let pct = (((x - r.x) / r.w).clamp(0.0, 1.0) * 100.0).round() as u32;
        self.monitor_set_value(contrast, pct, force);
    }

    fn monitor_flush(&mut self, contrast: bool) {
        if !self.monitor_unsent {
            return;
        }
        let pct = if contrast { self.monitor.contrast } else { self.monitor.brightness };
        self.monitor_unsent = false;
        self.monitor_sent = Instant::now();
        if contrast {
            self.queued.push(Action::Monitor(crate::monitor::MonitorCmd::Contrast(pct)));
        } else {
            self.queued.push(Action::Monitor(crate::monitor::MonitorCmd::Brightness(pct)));
        }
    }

    /// Uvoľnenie tlačidla myši (koniec ťahania posuvníka).
    pub fn release(&mut self, button: u32, now: Instant) {
        if button != BTN_LEFT {
            return;
        }
        if let Some(src) = self.audio_drag.take() {
            self.audio_flush(src);
            let outside = self.ptr.map(|(x, y)| self.hit_at(x, y).is_none()).unwrap_or(true);
            if outside {
                self.collapse_at = Some(now + Duration::from_millis(self.cfg.collapse_ms));
            }
        }
        if let Some(contrast) = self.monitor_drag.take() {
            self.monitor_flush(contrast);
            let outside = self.ptr.map(|(x, y)| self.hit_at(x, y).is_none()).unwrap_or(true);
            if outside {
                self.collapse_at = Some(now + Duration::from_millis(self.cfg.collapse_ms));
            }
        }
    }

    /// Akcie vzniknuté mimo `click` (ťahanie posuvníka, koliesko).
    pub fn take_queued(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.queued)
    }

    /// Počet riadkov zoznamu sietí, ktoré sú vidieť (aspoň 1 – kvôli správe „žiadne siete“).
    fn wifi_rows_shown(&self) -> usize {
        if !self.wifi.available || !self.wifi.enabled {
            return 1;
        }
        self.wifi.aps.len().min(self.cfg.wifi.rows).max(1)
    }

    fn toggle_wifi(&mut self) {
        if self.panel == Panel::Wifi {
            self.set_panel(Panel::None);
        } else {
            self.set_panel(Panel::Wifi);
        }
    }

    /// Klávesnicu potrebujeme len pri zadávaní hesla.
    pub fn wants_keyboard(&self) -> bool {
        self.panel == Panel::Wifi && self.wifi_pw.is_some()
    }

    /// Klávesnicu sa nepodarilo použiť (chýba mapa kláves) – zadávanie hesla sa zruší.
    pub fn keyboard_unavailable(&mut self) {
        self.wifi_pw = None;
        self.wifi_note = "Klávesnica nie je dostupná (pozri stderr)".into();
    }

    pub fn key(&mut self, k: &crate::wl::KeyEvent) -> Vec<Action> {
        self.wifi_pw_until = Some(Instant::now() + Duration::from_secs(WIFI_PW_IDLE_SECS));
        let mut out = Vec::new();
        let Some(pw) = &mut self.wifi_pw else { return out };
        match k.sym {
            0xff1b => self.wifi_pw = None, // Escape
            0xff08 => {
                pw.text.pop();
            }
            0xff0d | 0xff8d => {
                // Enter
                if pw.text.is_empty() {
                    return out;
                }
                if pw.key_mgmt == "wpa-psk" && pw.text.chars().count() < 8 {
                    self.wifi_note = "Heslo WPA musí mať aspoň 8 znakov".into();
                    return out;
                }
                let pw = self.wifi_pw.take().unwrap_or(WifiPw { ssid: String::new(), ap: String::new(), key_mgmt: String::new(), text: String::new() });
                self.wifi_note = format!("Pripájam k {}…", pw.ssid);
                out.push(Action::Wifi(crate::wifi::WifiCmd::Connect { ssid: pw.ssid, ap: pw.ap, saved: None, password: Some(pw.text), key_mgmt: pw.key_mgmt }));
            }
            0x76 | 0x56 if k.ctrl => out.push(Action::WifiPaste),
            _ if !k.ctrl => pw.text.extend(k.text.chars().filter(|c| !c.is_control())),
            _ => {}
        }
        out
    }

    /// Počet riadkov celej mriežky náhľadov.
    fn wall_row_count(&self) -> usize {
        self.wall.len().div_ceil(self.cfg.wall.columns)
    }

    /// Počet práve viditeľných riadkov mriežky (aspoň 1 – aj pre hlášku o prázdnom adresári).
    fn wall_rows_shown(&self) -> usize {
        self.wall_row_count().min(self.cfg.wall.rows).max(1)
    }

    fn toggle_wall(&mut self) {
        if self.panel == Panel::Wall {
            self.set_panel(Panel::None);
        } else {
            self.set_panel(Panel::Wall);
        }
    }

    /// Počet riadkov panela schránky, ktoré sú práve vidieť.
    fn clip_rows(&self) -> usize {
        self.cfg.clip_rows.min(self.clip.len()).max(1)
    }

    fn toggle_clip(&mut self) {
        if self.panel == Panel::Clip {
            self.set_panel(Panel::None);
        } else {
            self.set_panel(Panel::Clip);
        }
    }

    fn toggle_disks(&mut self) {
        if self.panel == Panel::Disks || self.data.disks.is_empty() {
            self.set_panel(Panel::None);
        } else {
            self.set_panel(Panel::Disks);
        }
    }

    fn toggle_calendar(&mut self) {
        if self.panel == Panel::Calendar {
            self.set_panel(Panel::None);
        } else {
            let tm = sys::local_tm(sys::now_unix());
            self.cal_year = tm.tm_year + 1900;
            self.cal_month = tm.tm_mon;
            self.set_panel(Panel::Calendar);
        }
    }

    fn toggle_menu(&mut self, idx: usize) {
        if self.panel == Panel::Menu(idx) {
            self.set_panel(Panel::None);
        } else if idx < self.cfg.menus.len() {
            self.set_panel(Panel::Menu(idx));
        }
    }

    /// Po otvorení veľkého panela dostane myš čas dôjsť k nemu (pozri `pointer_leave`).
    fn start_grace(&mut self, now: Instant) {
        self.grace = if self.big_open() { Some(now + Duration::from_millis(500)) } else { None };
    }

    fn menu_index(&self, name: &str) -> Option<usize> {
        self.cfg.menus.iter().position(|m| m.name == name)
    }

    fn cal_shift(&mut self, delta: i32) {
        let m = self.cal_month + delta;
        self.cal_year += m.div_euclid(12);
        self.cal_month = m.rem_euclid(12);
        self.rebuild_cal();
    }

    // ------------------------------------------------------ časovače

    pub fn tick(&mut self, now: Instant) -> bool {
        let mut changed = false;
        let mut i = 0;
        while i < self.notifs.len() {
            if self.notifs[i].expires.map(|t| now >= t).unwrap_or(false) {
                let n = self.notifs.remove(i);
                self.events.push(NotifEvent::Closed(n.n.id, 1));
                self.notif_dirty = true;
                changed = true;
            } else {
                i += 1;
            }
        }
        if let Some(t) = self.hud_until {
            if now >= t {
                self.hud_until = None;
                self.hud_t.set(0.0);
                changed = true;
            }
        }
        if let Some(t) = self.wifi_pw_until {
            if now >= t {
                self.wifi_pw_until = None;
                if self.wifi_pw.take().is_some() {
                    self.wifi_note = "Zadávanie hesla vypršalo".into();
                    changed = true;
                }
            }
        }
        if let Some(t) = self.collapse_at {
            if now >= t {
                self.collapse_at = None;
                if self.panel != Panel::None {
                    self.set_panel(Panel::None);
                    changed = true;
                }
            }
        }
        changed
    }

    /// Tooltip čaká na zobrazenie – volá sa z hlavnej slučky, ktorá má `Text`.
    pub fn tick_tip(&mut self, now: Instant, text: &mut Text) -> bool {
        if let Some((idx, t)) = self.tip_pending {
            if now >= t {
                self.tip_pending = None;
                if self.hover == Some(Hit::Item(idx)) && !self.big_open() {
                    self.show_tip(idx, text);
                    return true;
                }
            }
        }
        false
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        [self.hud_until, self.collapse_at, self.wifi_pw.as_ref().and(self.wifi_pw_until), self.tip_pending.map(|t| t.1), self.notifs.iter().filter_map(|n| n.expires).min()].into_iter().flatten().min()
    }

    // ------------------------------------------------------- ukazovateľ

    fn hit_at(&self, x: f32, y: f32) -> Option<Hit> {
        for (r, h) in self.hits.iter().rev() {
            if r.contains(x, y) {
                return Some(*h);
            }
        }
        if self.pill.contains(x, y) {
            return Some(Hit::Panel);
        }
        None
    }

    pub fn pointer_move(&mut self, lx: f32, ly: f32, now: Instant) {
        let (x, y) = (lx * self.scale, ly * self.scale);
        self.ptr = Some((x, y));
        if let Some(src) = self.audio_drag {
            self.audio_drag_to(src, x, false);
            return;
        }
        if let Some(contrast) = self.monitor_drag {
            self.monitor_drag_to(contrast, x, false);
            return;
        }
        let h = self.hit_at(x, y);
        // odklad zruší až vstup myši do panela, a to až po dobehnutí animácie jeho veľkosti
        // (počas nej ešte pohyb zasahuje starý rozmer ostrovčeka)
        if h.is_some() && self.grace.map(|t| now >= t).unwrap_or(false) {
            self.grace = None;
        }
        if h == self.hover {
            return;
        }
        self.hover = h;
        match h {
            Some(Hit::Item(i)) => {
                self.collapse_at = None;
                if self.big_open() {
                    return;
                }
                if !self.items[i].tip.is_empty() {
                    let delay = if self.panel == Panel::Tip { 0 } else { self.cfg.hover_ms };
                    self.tip_pending = Some((i, now + Duration::from_millis(delay)));
                } else {
                    self.tip_pending = None;
                    if self.panel == Panel::Tip {
                        self.collapse_at = Some(now + Duration::from_millis(250));
                    }
                }
            }
            Some(_) => {
                self.tip_pending = None;
                self.collapse_at = None;
            }
            None => {}
        }
    }

    pub fn pointer_leave(&mut self, now: Instant) {
        self.ptr = None;
        self.hover = None;
        self.tip_pending = None;
        // práve otvorený panel (kliknutím na modul / položku menu): nový panel býva menší ako ostrovček,
        // takže myš po kliku zvyčajne skončí mimo neho – odchod sa ignoruje, kým do panela nevstúpi
        if self.grace.is_some() {
            return;
        }
        // počas písania hesla panel nezatvárame (zavrie ho Esc / odoslanie)
        if self.wants_keyboard() || self.audio_drag.is_some() || self.monitor_drag.is_some() {
            return;
        }
        match self.panel {
            Panel::Calendar | Panel::Disks | Panel::Menu(_) | Panel::Clip | Panel::Wall | Panel::Wifi | Panel::Audio | Panel::Monitor | Panel::Bluetooth => {
                self.collapse_at = Some(now + Duration::from_millis(self.cfg.collapse_ms))
            }
            Panel::Tip => self.collapse_at = Some(now + Duration::from_millis(200)),
            Panel::None => {}
        }
    }

    fn action_for(cmd: &str) -> Option<Action> {
        match cmd {
            "" | "calendar" | "menu" | "clipboard" | "wallpaper" | "wifi" | "audio" | "monitor" | "bluetooth" => None,
            "eject-all" => Some(Action::DiskEjectAll),
            "inhibit-toggle" => Some(Action::InhibitToggle),
            "layout-toggle" => Some(Action::LayoutToggle),
            "audio-mute" => Some(Action::AudioMute),
            other => Some(Action::Run(other.to_string())),
        }
    }

    /// `module:NAZOV` v príkaze položky menu = vnútorná akcia ostrovčeka namiesto shell príkazu.
    /// NAZOV: `calendar`, `disks` (zoznam diskov), meno iného `[[menu]]`, alebo vstavaná akcia
    /// (`audio-mute`, `layout-toggle`, `inhibit-toggle`, `eject-all`). Neznámy názov sa ignoruje.
    fn run_module(&mut self, name: &str) -> Option<Action> {
        match name.trim() {
            "calendar" => {
                let tm = sys::local_tm(sys::now_unix());
                self.cal_year = tm.tm_year + 1900;
                self.cal_month = tm.tm_mon;
                self.set_panel(Panel::Calendar);
                None
            }
            "disks" => {
                if !self.data.disks.is_empty() {
                    self.set_panel(Panel::Disks);
                }
                None
            }
            "clipboard" => {
                self.set_panel(Panel::Clip);
                None
            }
            "wallpaper" => {
                self.set_panel(Panel::Wall);
                None
            }
            "wifi" => {
                self.set_panel(Panel::Wifi);
                None
            }
            "audio" => {
                self.set_panel(Panel::Audio);
                None
            }
            "monitor" => {
                self.set_panel(Panel::Monitor);
                None
            }
            "bluetooth" => {
                self.set_panel(Panel::Bluetooth);
                None
            }
            other => match self.menu_index(other) {
                Some(mi) => {
                    self.set_panel(Panel::Menu(mi));
                    None
                }
                None => Self::action_for(other).filter(|a| !matches!(a, Action::Run(_))),
            },
        }
    }

    pub fn click(&mut self, button: u32, now: Instant) -> Vec<Action> {
        let mut out = self.click_inner(button, now);
        out.extend(self.queued.drain(..));
        out
    }

    fn click_inner(&mut self, button: u32, now: Instant) -> Vec<Action> {
        let mut out = Vec::new();
        let Some((x, y)) = self.ptr else { return out };

        // custom menu: zatvorí sa klikom na položku alebo odchodom myši
        if matches!(self.panel, Panel::Menu(_)) {
            if button == BTN_LEFT {
                let hit = self.hit_at(x, y);
                let mut cmd = String::new();
                if let (Some(Hit::MenuItem(k)), Panel::Menu(mi)) = (hit, self.panel) {
                    if let Some(item) = self.cfg.menus.get(mi).and_then(|m| m.items.get(k)) {
                        cmd = item.command.clone();
                    }
                }
                // `module:...` – otvorí iný panel ostrovčeka (kalendár, iné menu) namiesto shell príkazu
                if let Some(name) = cmd.strip_prefix("module:") {
                    let name = name.to_string();
                    self.set_panel(Panel::None);
                    self.collapse_at = None;
                    self.tip_pending = None;
                    out.extend(self.run_module(&name));
                    self.start_grace(now);
                    return out;
                }
                if !cmd.is_empty() {
                    out.push(Action::Run(cmd));
                }
                // klik na položku zatvorí menu; klik na okraj/nadpis ho nechá otvorené (zavrie ho odchod myši)
                if matches!(hit, Some(Hit::MenuItem(_))) {
                    self.set_panel(Panel::None);
                    self.collapse_at = None;
                }
            }
            return out;
        }

        let Some(h) = self.hit_at(x, y) else { return out };
        match h {
            Hit::Item(i) => {
                let cmd = match button {
                    BTN_LEFT => self.items[i].btn.click.clone(),
                    BTN_RIGHT => self.items[i].btn.right.clone(),
                    BTN_MIDDLE => self.items[i].btn.middle.clone(),
                    _ => String::new(),
                };
                if cmd == "menu" {
                    self.tip_pending = None;
                    if self.data.disks.len() == 1 {
                        // jediný disk: rovno odpojiť
                        out.push(Action::DiskEject(self.data.disks[0].block.clone()));
                        if self.panel == Panel::Tip {
                            self.set_panel(Panel::None);
                        }
                    } else {
                        self.toggle_disks();
                    }
                    self.collapse_at = None;
                    self.start_grace(now);
                } else if cmd == "calendar" {
                    self.tip_pending = None;
                    self.toggle_calendar();
                    self.collapse_at = None;
                    self.start_grace(now);
                } else if cmd == "clipboard" {
                    self.tip_pending = None;
                    self.toggle_clip();
                    self.collapse_at = None;
                    self.start_grace(now);
                } else if cmd == "audio" {
                    self.tip_pending = None;
                    self.toggle_audio();
                    self.collapse_at = None;
                    self.start_grace(now);
                } else if cmd == "wifi" {
                    self.tip_pending = None;
                    self.toggle_wifi();
                    self.collapse_at = None;
                    self.start_grace(now);
                } else if cmd == "wallpaper" {
                    self.tip_pending = None;
                    self.toggle_wall();
                    self.collapse_at = None;
                    self.start_grace(now);
                } else if let Some(mi) = self.menu_index(&cmd) {
                    self.tip_pending = None;
                    self.toggle_menu(mi);
                    self.collapse_at = None;
                    self.start_grace(now);
                } else if let Some(a) = Self::action_for(&cmd) {
                    if self.panel == Panel::Tip {
                        self.set_panel(Panel::None);
                    }
                    self.tip_pending = None;
                    out.push(a);
                }
            }
            Hit::Disk(k) => {
                if button == BTN_LEFT {
                    if let Some(d) = self.data.disks.get(k) {
                        out.push(Action::DiskEject(d.block.clone()));
                    }
                }
            }
            Hit::DiskAll => {
                if button == BTN_LEFT {
                    out.push(Action::DiskEjectAll);
                }
            }
            Hit::ClipItem(i) => {
                if button == BTN_LEFT {
                    if let Some(e) = self.clip.get(i) {
                        out.push(Action::ClipCopy(e.raw.clone()));
                    }
                    self.set_panel(Panel::None);
                    self.collapse_at = None;
                }
            }
            Hit::WifiToggle => {
                if button == BTN_LEFT {
                    self.wifi_note.clear();
                    out.push(Action::Wifi(crate::wifi::WifiCmd::SetEnabled(!self.wifi.enabled)));
                }
            }
            Hit::AudioMute(src) => {
                if button == BTN_LEFT {
                    if let Some(i) = self.audio_default(src) {
                        let d = if src { &mut self.audio.sources[i] } else { &mut self.audio.sinks[i] };
                        d.mute = !d.mute;
                    }
                    out.push(Action::Audio(crate::audio::AudioCmd::ToggleMute { source: src }));
                }
            }
            Hit::AudioDev(src, i) => {
                if button == BTN_LEFT {
                    let name = if src { self.audio.sources.get(i).map(|d| d.name.clone()) } else { self.audio.sinks.get(i).map(|d| d.name.clone()) };
                    if let Some(name) = name {
                        if src {
                            self.audio.default_source = name.clone();
                        } else {
                            self.audio.default_sink = name.clone();
                        }
                        out.push(Action::Audio(crate::audio::AudioCmd::SetDefault { source: src, name }));
                    }
                }
            }
            Hit::AudioSlider(src) => {
                if button == BTN_LEFT {
                    self.audio_drag = Some(src);
                    self.audio_drag_to(src, x, true);
                }
            }
            Hit::MonSlider(contrast) => {
                if button == BTN_LEFT {
                    self.monitor_drag = Some(contrast);
                    self.monitor_drag_to(contrast, x, true);
                }
            }
            Hit::MonPreset(i) => {
                if button == BTN_LEFT {
                    self.monitor.preset = Some(i);
                    out.push(Action::Monitor(crate::monitor::MonitorCmd::Preset(i)));
                }
            }
            Hit::MonInput(i) => {
                if button == BTN_LEFT {
                    self.monitor.input = Some(i);
                    out.push(Action::Monitor(crate::monitor::MonitorCmd::Input(i)));
                }
            }
            Hit::MonNight => {
                if button == BTN_LEFT {
                    let enable = !self.monitor.night;
                    self.monitor.night = enable;
                    out.push(Action::Monitor(crate::monitor::MonitorCmd::SetNight(enable)));
                }
            }
            Hit::BtToggle => {
                if button == BTN_LEFT {
                    let on = !self.bluetooth.powered;
                    self.bluetooth.powered = on;
                    out.push(Action::Bluetooth(crate::bluetooth::BtCmd::SetPowered(on)));
                }
            }
            Hit::BtDiscoverable => {
                if button == BTN_LEFT {
                    let on = !self.bluetooth.discoverable;
                    self.bluetooth.discoverable = on;
                    out.push(Action::Bluetooth(crate::bluetooth::BtCmd::SetDiscoverable(on)));
                }
            }
            Hit::BtRefresh => {
                if button == BTN_LEFT {
                    out.push(Action::Bluetooth(crate::bluetooth::BtCmd::Load { scan: true }));
                }
            }
            Hit::BtEditor => {
                if button == BTN_LEFT && !self.cfg.bluetooth.editor_command.is_empty() {
                    out.push(Action::Run(self.cfg.bluetooth.editor_command.clone()));
                    self.set_panel(Panel::None);
                    self.collapse_at = None;
                }
            }
            Hit::BtRow(i) => {
                if let Some(d) = self.bluetooth.devices.get(i).cloned() {
                    if button == BTN_RIGHT {
                        // zabudnúť vždy, ak je v zozname (aj nespárované z discovery)
                        out.push(Action::Bluetooth(crate::bluetooth::BtCmd::Forget(d.path)));
                    } else if button == BTN_MIDDLE {
                        out.push(Action::Bluetooth(crate::bluetooth::BtCmd::SetTrusted {
                            path: d.path,
                            trusted: !d.trusted,
                        }));
                    } else if button == BTN_LEFT {
                        if d.connected {
                            out.push(Action::Bluetooth(crate::bluetooth::BtCmd::Disconnect(d.path)));
                        } else {
                            // Connect sám sparuje, ak treba (náhrada blueman)
                            out.push(Action::Bluetooth(crate::bluetooth::BtCmd::Connect(d.path)));
                        }
                    }
                }
            }
            Hit::WifiEditor => {
                if button == BTN_LEFT {
                    out.push(Action::Run(self.cfg.wifi.editor_command.clone()));
                    self.set_panel(Panel::None);
                    self.collapse_at = None;
                }
            }
            Hit::WifiRefresh => {
                if button == BTN_LEFT {
                    self.wifi_note = "Hľadám siete…".into();
                    out.push(Action::Wifi(crate::wifi::WifiCmd::Load));
                }
            }
            Hit::WifiRow(i) => {
                if let Some(ap) = self.wifi.aps.get(i).cloned() {
                    if button == BTN_RIGHT {
                        if let Some(conn) = ap.saved {
                            out.push(Action::Wifi(crate::wifi::WifiCmd::Forget(conn)));
                            self.wifi_note = format!("Zabúdam {}…", ap.ssid);
                        }
                    } else if button == BTN_LEFT {
                        if ap.active {
                            out.push(Action::Wifi(crate::wifi::WifiCmd::Disconnect));
                            self.wifi_note = "Odpájam…".into();
                        } else if ap.saved.is_some() || !ap.secured {
                            self.wifi_note = format!("Pripájam k {}…", ap.ssid);
                            out.push(Action::Wifi(crate::wifi::WifiCmd::Connect { ssid: ap.ssid, ap: ap.ap, saved: ap.saved, password: None, key_mgmt: ap.key_mgmt }));
                        } else if ap.supported {
                            self.wifi_note.clear();
                            self.collapse_at = None;
                            self.wifi_pw = Some(WifiPw { ssid: ap.ssid, ap: ap.ap, key_mgmt: ap.key_mgmt, text: String::new() });
                            self.wifi_pw_until = Some(now + Duration::from_secs(WIFI_PW_IDLE_SECS));
                        } else {
                            self.wifi_note = "Tento typ zabezpečenia ostrovček nepodporuje".into();
                        }
                    }
                }
            }
            Hit::WallItem(i) => {
                if button == BTN_LEFT {
                    if let Some(e) = self.wall.get(i) {
                        // pri vybranej téme sa mení len pozadie (bez matugenu), v režime „wallpaper“ aj farby
                        out.push(Action::WallApply { path: e.path.clone(), background_only: !self.wall_theme.is_empty() });
                    }
                    self.set_panel(Panel::None);
                    self.collapse_at = None;
                }
            }
            Hit::WallRollout => {
                if button == BTN_LEFT {
                    self.wall_menu = !self.wall_menu;
                    self.wall_menu_scroll = 0;
                }
            }
            Hit::WallTheme(i) => {
                if button == BTN_LEFT {
                    self.wall_menu = false;
                    if i == 0 {
                        // pseudotéma: len prepne režim (zmaže zapamätanú tému), farby sa zmenia pri ďalšom nastavení pozadia
                        self.wall_theme.clear();
                        out.push(Action::WallTheme(None));
                    } else if let Some((name, path)) = self.wall_themes.get(i - 1).cloned() {
                        out.push(Action::WallTheme(Some((name, path))));
                    }
                }
            }
            Hit::WallButton(i) => {
                if button == BTN_LEFT {
                    if let Some(b) = self.cfg.wall.buttons.get(i) {
                        if !b.command.is_empty() {
                            out.push(Action::Run(b.command.clone()));
                        }
                    }
                    self.set_panel(Panel::None);
                    self.collapse_at = None;
                }
            }
            Hit::Notif(k) => match button {
                BTN_LEFT => {
                    let rule_click = self.notifs.get(k).and_then(|n| self.rule_for(&n.n)).map(|r| r.on_click.clone()).filter(|c| !c.is_empty());
                    let c = rule_click.unwrap_or_else(|| self.cfg.notif_click.clone());
                    if c == "default-action" {
                        if let Some(n) = self.notifs.get(k) {
                            if n.n.actions.iter().any(|a| a.0 == "default") {
                                self.events.push(NotifEvent::Action(n.n.id, "default".into()));
                            }
                        }
                    } else if c != "dismiss" && !c.is_empty() {
                        out.push(Action::Run(c));
                    }
                    self.dismiss(k, 2);
                }
                BTN_RIGHT => {
                    if self.cfg.notif_right == "dismiss" || self.cfg.notif_right.is_empty() {
                        self.dismiss(k, 2);
                    } else {
                        out.push(Action::Run(self.cfg.notif_right.clone()));
                        self.dismiss(k, 2);
                    }
                }
                BTN_MIDDLE => {
                    while !self.notifs.is_empty() {
                        self.dismiss(0, 2);
                    }
                }
                _ => {}
            },
            _ => {}
        }
        let _ = now;
        out
    }

    pub fn scroll(&mut self, value: f32) {
        if self.panel == Panel::Audio {
            // koliesko nad posuvníkom mení hlasitosť o `step` % (hore = viac)
            if let Some(Hit::AudioSlider(src)) = self.hover {
                self.audio_acc += value;
                let steps = (self.audio_acc / AUDIO_SCROLL_UNIT).trunc();
                self.audio_acc -= steps * AUDIO_SCROLL_UNIT;
                if steps != 0.0 {
                    if let Some(i) = self.audio_default(src) {
                        let cur = if src { self.audio.sources[i].volume } else { self.audio.sinks[i].volume } as f32;
                        let new = (cur - steps * self.cfg.vol_step as f32).clamp(0.0, self.audio_max() as f32);
                        self.audio_set_volume(src, new.round() as u32, true);
                    }
                }
            }
        }
        if self.panel == Panel::Calendar && self.hover.is_some() {
            self.cal_shift(if value > 0.0 { 1 } else { -1 });
        }
        if self.panel == Panel::Wifi && self.hover.is_some() && self.wifi_pw.is_none() {
            self.wifi_acc += value;
            let rows = (self.wifi_acc / WIFI_SCROLL_UNIT).trunc();
            self.wifi_acc -= rows * WIFI_SCROLL_UNIT;
            let max_start = self.wifi.aps.len().saturating_sub(self.cfg.wifi.rows) as f32;
            self.wifi_scroll = (self.wifi_scroll as f32 + rows).clamp(0.0, max_start) as usize;
        }
        if self.panel == Panel::Bluetooth && self.hover.is_some() {
            self.bluetooth_acc += value;
            let rows = (self.bluetooth_acc / WIFI_SCROLL_UNIT).trunc();
            self.bluetooth_acc -= rows * WIFI_SCROLL_UNIT;
            let max_start = self.bluetooth.devices.len().saturating_sub(self.cfg.bluetooth.rows) as f32;
            self.bluetooth_scroll = (self.bluetooth_scroll as f32 + rows).clamp(0.0, max_start) as usize;
        }
        if self.panel == Panel::Wall && self.hover.is_some() && self.wall_menu {
            self.wall_acc += value;
            let steps = (self.wall_acc / WALL_SCROLL_UNIT).trunc();
            self.wall_acc -= steps * WALL_SCROLL_UNIT;
            let max_start = (self.wall_themes.len() + 1).saturating_sub(1) as f32;
            self.wall_menu_scroll = (self.wall_menu_scroll as f32 + steps).clamp(0.0, max_start) as usize;
        } else if self.panel == Panel::Wall && self.hover.is_some() {
            self.wall_acc += value;
            let rows = (self.wall_acc / WALL_SCROLL_UNIT).trunc();
            self.wall_acc -= rows * WALL_SCROLL_UNIT;
            let max_start = self.wall_row_count().saturating_sub(self.cfg.wall.rows) as f32;
            self.wall_scroll = (self.wall_scroll as f32 + rows).clamp(0.0, max_start) as usize;
        }
        if self.panel == Panel::Clip && self.hover.is_some() {
            // koliesko aj touchpad: posun sa zbiera a každá celá jednotka = jeden riadok
            self.clip_acc += value;
            let rows = (self.clip_acc / CLIP_SCROLL_UNIT).trunc();
            self.clip_acc -= rows * CLIP_SCROLL_UNIT;
            let max_start = self.clip.len().saturating_sub(self.cfg.clip_rows) as f32;
            self.clip_scroll = (self.clip_scroll as f32 + rows).clamp(0.0, max_start) as usize;
        }
    }

    /// Oblasť, ktorá prijíma vstup (logické px) – zvyšok povrchu je priehľadný pre myš.
    pub fn input_rect(&self) -> Option<(i32, i32, i32, i32)> {
        if self.hide.x > 0.92 || self.pill.w < 1.0 {
            return None;
        }
        let k = 1.0 / self.scale;
        let x0 = ((self.pill.x - 2.0) * k).floor().max(0.0) as i32;
        let y0 = ((self.pill.y - 2.0) * k).floor().max(0.0) as i32;
        let x1 = ((self.pill.x + self.pill.w + 2.0) * k).ceil() as i32;
        let y1 = ((self.pill.y + self.pill.h + 2.0) * k).ceil() as i32;
        Some((x0, y0, x1 - x0, y1 - y0))
    }

    /// Ukážkové stavy pre `--snapshot`.
    pub fn preview(&mut self, what: &str) {
        match what {
            "calendar" => self.toggle_calendar(),
            "disks" => {
                self.data.disks = vec![
                    DiskInfo { block: "a".into(), label: "FOTKY 2TB".into(), dev: "/dev/sdb1".into(), mount: "/run/media/mm/FOTKY".into(), size: "1.8T".into() },
                    DiskInfo { block: "b".into(), label: "USB KLÚČ".into(), dev: "/dev/sdc1".into(), mount: "/run/media/mm/USB".into(), size: "14.6G".into() },
                ];
                self.set_panel(Panel::Disks);
            }
            "hud" => {
                self.hud_until = Some(Instant::now() + Duration::from_secs(600));
                self.hud_t.set(1.0);
            }
            "clear" => {
                self.hud_until = None;
                self.hud_t.set(0.0);
            }
            "hide" => self.manual_hidden = true,
            _ => {}
        }
    }

    pub fn preview_tip(&mut self, text: &mut Text) {
        self.refresh_items(text);
        if let Some(i) = self.items.iter().position(|it| !it.tip.is_empty()) {
            self.show_tip(i, text);
        }
    }

    // ------------------------------------------------------ kreslenie

    pub fn build(&mut self, text: &mut Text) -> Frame {
        let s = self.scale;
        let px = self.px_font();
        let row_h = self.s(self.cfg.row_height);
        let pad_x = self.s(6.0);
        let gap = self.s(2.0);
        let item_h = row_h - self.s(8.0);
        let t = self.theme;

        // šírka riadka z animovaných šírok položiek
        let mut row_w = 0.0f32;
        let mut n_vis = 0;
        for it in &self.items {
            if it.w.x > 0.5 {
                row_w += it.w.x;
                n_vis += 1;
            }
        }
        if n_vis > 1 {
            row_w += gap * (n_vis - 1) as f32;
        }

        // HUD (hlasitosť alebo jas)
        let bright = self.data.hud_bright;
        let hud_val = if bright { self.data.bright } else { self.data.volume };
        let hud_muted = !bright && self.data.muted;
        let hud_icon = if bright {
            let k = self.cfg.bright_icons.len().max(1);
            let idx = ((hud_val * k as f32) as usize).min(k - 1);
            self.cfg.bright_icons.get(idx).cloned().unwrap_or_default()
        } else if hud_muted || hud_val <= 0.001 {
            self.cfg.audio_icon_muted.clone()
        } else {
            let k = self.cfg.audio_icons.len().max(1);
            let idx = ((hud_val * k as f32) as usize).min(k - 1);
            self.cfg.audio_icons.get(idx).cloned().unwrap_or_default()
        };
        let hud_pct = format!("{}%", (hud_val * 100.0).round() as i32);
        let icon_w = text.measure(&hud_icon, px * 1.1).max(self.s(18.0));
        let pct_w = text.measure("100%", px);
        let bar_w = self.s(120.0);
        let hud_w = icon_w + self.s(10.0) + bar_w + self.s(10.0) + pct_w;
        let ht = self.hud_t.x.clamp(0.0, 1.0);
        let content_w = row_w + (hud_w - row_w) * ht;

        let nt = self.notif_t.x.clamp(0.0, 1.0);
        let pill_w0 = (content_w + 2.0 * pad_x).max(self.panel_w.x).max(self.s(40.0));
        let pill_h0 = row_h + self.panel_h.x.max(0.0);
        let pill_w = pill_w0 + (self.notif_w.x - pill_w0) * nt;
        let pill_h = pill_h0 + (self.notif_h.x - pill_h0) * nt;
        let ct = self.cal_t.x.clamp(0.0, 1.0);
        let (cal_w, cal_h) = self.cal_size();
        let pill_w = pill_w + (cal_w - pill_w) * ct;
        let pill_h = pill_h + (cal_h - pill_h) * ct;
        let hide_off = -(pill_h + self.s(self.cfg.top_margin) + self.s(24.0)) * self.hide.x;
        let pill = Rect::new(((self.surf_w - pill_w) * 0.5).round(), self.s(self.cfg.top_margin) + hide_off, pill_w, pill_h);
        self.pill = pill;
        self.hits.clear();

        let mut frame = Frame::default();
        let radius = (self.radius.x * s).min(pill_h * 0.5).min(pill_w * 0.5);

        // tieň + pozadie
        if self.cfg.shadow {
            frame.back.push(Item::Rect {
                r: Rect::new(pill.x + self.s(2.0), pill.y + self.s(5.0), pill.w - self.s(4.0), pill.h),
                radius,
                soft: self.s(12.0),
                color: [0.0, 0.0, 0.0, 0.32],
            });
        }
        let mut bg = t.bg;
        bg[3] *= self.cfg.opacity;
        let ut = if self.notif_urgent { nt } else { 0.0 };
        if ut > 0.01 {
            frame.back.push(Item::Rect { r: pill, radius, soft: 0.8, color: fade(t.urgent_border, ut) });
            let b = self.s(self.cfg.notif_border) * ut;
            bg[3] += (1.0 - bg[3]) * ut;
            frame.back.push(Item::Rect { r: Rect::new(pill.x + b, pill.y + b, pill.w - 2.0 * b, pill.h - 2.0 * b), radius: (radius - b).max(0.0), soft: 0.8, color: bg });
        } else {
            frame.back.push(Item::Rect { r: pill, radius, soft: 0.8, color: bg });
        }
        frame.clip = Some(Rect::new(pill.x, pill.y, pill.w, pill.h));

        // ---- riadok s modulmi
        let row_alpha = (1.0 - ht) * (1.0 - nt) * (1.0 - ct);
        if row_alpha > 0.01 {
            let (asc, desc) = text.line_metrics(px);
            let row_cy = pill.y + row_h * 0.5;
            let baseline = row_cy + (asc - desc) * 0.5;
            let mut x = pill.x + (pill_w - row_w) * 0.5;
            for i in 0..self.items.len() {
                let (w, a, hv) = {
                    let it = &self.items[i];
                    (it.w.x, it.a.x, it.hover.x)
                };
                if w <= 0.5 {
                    continue;
                }
                let it = &self.items[i];
                let ia = (a * row_alpha).clamp(0.0, 1.0);
                let r = Rect::new(x, row_cy - item_h * 0.5, w, item_h);
                self.hits.push((r, Hit::Item(i)));
                if let Some((bgc, fgc)) = it.badge {
                    frame.content.push(Item::Rect { r, radius: item_h * 0.5, soft: 0.8, color: fade(bgc, ia) });
                    text.layout_to(&mut frame.content, &it.shown, px, x + it.pad * s, baseline, fade(fgc, ia));
                } else {
                    if hv > 0.01 {
                        frame.content.push(Item::Rect {
                            r,
                            radius: self.s(12.0).min(item_h * 0.5),
                            soft: 0.8,
                            color: fade(t.accent, 0.20 * hv * ia),
                        });
                    }
                    let fg = it.fg.unwrap_or(t.fg);
                    text.layout_to(&mut frame.content, &it.shown, px, x + it.pad * s, baseline, fade(fg, ia));
                }
                x += w + gap;
            }
        }

        // ---- HUD
        if ht > 0.01 && nt < 0.99 {
            let hud_a = ht * (1.0 - nt) * (1.0 - ct);
            let (asc, desc) = text.line_metrics(px);
            let cy = pill.y + row_h * 0.5;
            let baseline = cy + (asc - desc) * 0.5;
            let mut x = pill.x + (pill_w - hud_w) * 0.5;
            text.layout_to(&mut frame.content, &hud_icon, px * 1.1, x, baseline, fade(t.fg, hud_a));
            x += icon_w + self.s(10.0);
            let bh = self.s(6.0);
            let track = Rect::new(x, cy - bh * 0.5, bar_w, bh);
            frame.content.push(Item::Rect { r: track, radius: bh * 0.5, soft: 0.8, color: fade(t.accent, 0.25 * hud_a) });
            let fill_w = (bar_w * hud_val.clamp(0.0, 1.0)).max(if hud_val > 0.001 { bh } else { 0.0 });
            if fill_w > 0.0 {
                let col = if hud_muted { fade(t.fg, 0.45 * hud_a) } else { fade(t.accent, hud_a) };
                frame.content.push(Item::Rect { r: Rect::new(x, cy - bh * 0.5, fill_w, bh), radius: bh * 0.5, soft: 0.8, color: col });
            }
            x += bar_w + self.s(10.0);
            let pw = text.measure(&hud_pct, px);
            text.layout_to(&mut frame.content, &hud_pct, px, x + pct_w - pw, baseline, fade(t.fg, hud_a));
        }

        if nt > 0.01 {
            self.build_notif(&mut frame, text, pill, nt);
        }

        // ---- kalendár / disky / menu (nahrádza obsah ostrovčeka)
        if ct > 0.01 {
            self.hits.push((pill, Hit::Panel));
            match self.big {
                Panel::Disks => self.build_disks(&mut frame, text, pill, ct),
                Panel::Menu(mi) => self.build_menu(&mut frame, text, pill, mi, ct),
                Panel::Clip => self.build_clip(&mut frame, text, pill, ct),
                Panel::Wall => self.build_wall(&mut frame, text, pill, ct),
                Panel::Wifi => self.build_wifi(&mut frame, text, pill, ct),
                Panel::Audio => self.build_audio(&mut frame, text, pill, ct),
                Panel::Monitor => self.build_monitor(&mut frame, text, pill, ct),
                Panel::Bluetooth => self.build_bluetooth(&mut frame, text, pill, ct),
                _ => self.build_calendar(&mut frame, text, pill, pill.y, ct),
            }
        }

        // ---- panel pod riadkom (tooltip)
        let pa = self.panel_a.x.clamp(0.0, 1.0);
        if self.panel_shown != Panel::None && self.panel_h.x > 2.0 && pa > 0.01 {
            let top = pill.y + row_h;
            self.hits.push((Rect::new(pill.x, top, pill.w, pill.h - row_h), Hit::Panel));
            match self.panel_shown {
                Panel::Tip => {
                    let px2 = px * 0.85;
                    let (asc, desc) = text.line_metrics(px2);
                    let line_h = self.s(22.0);
                    let lines = self.tip_lines.clone();
                    for (k, l) in lines.iter().enumerate() {
                        let cy = top + self.s(8.0) + line_h * (k as f32 + 0.5);
                        let col = if k == 0 { fade(t.fg, pa) } else { fade(t.fg, 0.8 * pa) };
                        text.layout_to(&mut frame.content, l, px2, pill.x + self.s(16.0), cy + (asc - desc) * 0.5, col);
                    }
                }
                Panel::None | Panel::Calendar | Panel::Disks | Panel::Menu(_) | Panel::Clip | Panel::Wall | Panel::Wifi | Panel::Audio | Panel::Monitor | Panel::Bluetooth => {}
            }
        }
        frame
    }

    fn build_menu(&mut self, frame: &mut Frame, text: &mut Text, pill: Rect, mi: usize, pa: f32) {
        let Some(menu) = self.cfg.menus.get(mi).cloned() else { return };
        let s = self.scale;
        let t = self.theme;
        let icon_cols = menu.icon_columns.max(1);
        let text_cols = menu.text_columns.max(1);
        let pad = menu.padding * s;
        let gap = menu.gap * s;
        let title_h = if menu.title.is_empty() { 0.0 } else { 36.0 * s };
        let icon_cell = (menu.icon_size + 16.0) * s;
        let label_h = (menu.text_size + 18.0) * s;
        let label_cell_w = menu.cell_width * s;
        let sep_h = ((menu.separator_thickness + 12.0).max(14.0)) * s;
        let sep_thick = menu.separator_thickness * s;

        let icon_grid_w = icon_cols as f32 * icon_cell + (icon_cols.saturating_sub(1)) as f32 * gap;
        let text_grid_w = text_cols as f32 * label_cell_w + (text_cols.saturating_sub(1)) as f32 * gap;
        let content_w = menu_content_width(menu.kind, icon_grid_w, text_grid_w, &menu);

        let icon_px = menu.icon_size * s;
        let text_px = menu.text_size * s;
        let (asc_t, desc_t) = text.line_metrics(text_px);
        let (asc_h, desc_h) = text.line_metrics(self.px_font());
        let layout = Self::menu_rows(&menu);

        if !menu.title.is_empty() {
            let px = self.px_font();
            let tw = text.measure(&menu.title, px);
            let cy = pill.y + title_h * 0.5;
            text.layout_to(
                &mut frame.content,
                &menu.title,
                px,
                pill.x + (pill.w - tw) * 0.5,
                cy + (asc_h - desc_h) * 0.5,
                fade(t.fg, pa),
            );
        }

        let grid_top = pill.y + pad + title_h;
        let grid_left = pill.x + (pill.w - content_w) * 0.5;

        let mut y = grid_top;
        for (ri, row) in layout.iter().enumerate() {
            if ri > 0 {
                y += gap;
            }
            match row {
                MenuLayoutRow::Icons(indices) => {
                    let n = indices.len();
                    let row_w = n as f32 * icon_cell + (n.saturating_sub(1)) as f32 * gap;
                    // centrovať riadok v rámci content_w (neúplný aj pri užšej ikonovej mriežke)
                    let row_left = grid_left + (content_w - row_w) * 0.5;
                    for (c, &i) in indices.iter().enumerate() {
                        let x = row_left + c as f32 * (icon_cell + gap);
                        let cell = Rect::new(x, y, icon_cell, icon_cell);
                        let hov = self.hover == Some(Hit::MenuItem(i));
                        self.hits.push((cell, Hit::MenuItem(i)));
                        if hov {
                            frame.content.push(Item::Rect {
                                r: cell,
                                radius: 10.0 * s,
                                soft: 0.8,
                                color: fade(t.fg, 0.10 * pa),
                            });
                        }
                        if let Some(it) = menu.items.get(i) {
                            if !it.icon.is_empty() {
                                if let Some(icon) = self.icons.get(&it.icon) {
                                    let max = icon_px;
                                    let sc = (max / icon.w as f32).min(max / icon.h as f32);
                                    let dw = icon.w as f32 * sc;
                                    let dh = icon.h as f32 * sc;
                                    let ix = x + (icon_cell - dw) * 0.5;
                                    let iy = y + (icon_cell - dh) * 0.5;
                                    frame.content.push(Item::Image {
                                        r: Rect::new(ix, iy, dw, dh),
                                        path: it.icon.clone(),
                                        color: [1.0, 1.0, 1.0, pa],
                                    });
                                }
                            }
                        }
                    }
                    y += icon_cell;
                }
                MenuLayoutRow::Labels(indices) => {
                    let n = indices.len();
                    let row_w = n as f32 * label_cell_w + (n.saturating_sub(1)) as f32 * gap;
                    let row_left = grid_left + (content_w - row_w) * 0.5;
                    for (c, &i) in indices.iter().enumerate() {
                        let x = row_left + c as f32 * (label_cell_w + gap);
                        let cell = Rect::new(x, y, label_cell_w, label_h);
                        let hov = self.hover == Some(Hit::MenuItem(i));
                        self.hits.push((cell, Hit::MenuItem(i)));
                        let (bg_col, fg_col) = if hov {
                            (fade(t.menu_selected, pa), fade(t.menu_selected_text, pa))
                        } else {
                            (fade(t.fg, 0.0), fade(t.fg, pa))
                        };
                        if hov {
                            frame.content.push(Item::Rect {
                                r: cell,
                                radius: 10.0 * s,
                                soft: 0.8,
                                color: bg_col,
                            });
                        }
                        if let Some(it) = menu.items.get(i) {
                            let label = if !it.label.is_empty() {
                                it.label.as_str()
                            } else {
                                it.icon.as_str()
                            };
                            if !label.is_empty() {
                                let maxw = label_cell_w - 12.0 * s;
                                let shown = Self::ellipsize(text, label, text_px, maxw);
                                let tw = text.measure(&shown, text_px);
                                let cx = x + (label_cell_w - tw) * 0.5;
                                let cy = y + label_h * 0.5;
                                text.layout_to(
                                    &mut frame.content,
                                    &shown,
                                    text_px,
                                    cx,
                                    cy + (asc_t - desc_t) * 0.5,
                                    fg_col,
                                );
                            }
                        }
                    }
                    y += label_h;
                }
                MenuLayoutRow::Separator => {
                    let line_w = content_w * menu.separator_length;
                    let lx = grid_left + (content_w - line_w) * 0.5;
                    let ly = y + (sep_h - sep_thick) * 0.5;
                    frame.content.push(Item::Rect {
                        r: Rect::new(lx, ly, line_w, sep_thick),
                        radius: sep_thick * 0.5,
                        soft: 0.6,
                        color: fade(t.fg, 0.28 * pa),
                    });
                    y += sep_h;
                }
            }
        }
    }

    fn build_clip(&mut self, frame: &mut Frame, text: &mut Text, pill: Rect, pa: f32) {
        let s = self.scale;
        let t = self.theme;
        let px = self.cfg.clip_font * s;
        let (asc, desc) = text.line_metrics(px);
        let pad = CLIP_PAD * s;
        let row_h = (self.cfg.clip_font + CLIP_ROW_EXTRA) * s;
        let n = self.clip.len();

        if n == 0 {
            let msg = if self.clip_loaded { "Schránka je prázdna" } else { "Načítavam…" };
            let tw = text.measure(msg, px);
            let cy = pill.y + pill.h * 0.5;
            text.layout_to(&mut frame.content, msg, px, pill.x + (pill.w - tw) * 0.5, cy + (asc - desc) * 0.5, fade(t.fg, 0.7 * pa));
            return;
        }

        let rows = self.clip_rows();
        let scrollable = n > rows;
        let sb = if scrollable { CLIP_SCROLLBAR * s } else { 0.0 };
        let start = self.clip_scroll.min(n - rows);
        let inset = 10.0 * s;
        let cell_w = pill.w - 2.0 * pad - sb;

        for k in 0..rows {
            let i = start + k;
            let y = pill.y + pad + k as f32 * row_h;
            let cell = Rect::new(pill.x + pad, y, cell_w, row_h);
            self.hits.push((cell, Hit::ClipItem(i)));
            let hov = self.hover == Some(Hit::ClipItem(i));
            let fg = if hov {
                frame.content.push(Item::Rect { r: cell, radius: 10.0 * s, soft: 0.8, color: fade(t.menu_selected, pa) });
                fade(t.menu_selected_text, pa)
            } else {
                fade(t.fg, pa)
            };
            let label = Self::ellipsize(text, &self.clip[i].text, px, cell_w - 2.0 * inset);
            text.layout_to(&mut frame.content, &label, px, cell.x + inset, y + row_h * 0.5 + (asc - desc) * 0.5, fg);
        }

        if scrollable {
            // posuvník: dráha vpravo, bežec ukazuje polohu a podiel viditeľných riadkov
            let track_h = rows as f32 * row_h;
            let thumb_h = (track_h * rows as f32 / n as f32).max(16.0 * s).min(track_h);
            let frac = start as f32 / (n - rows) as f32;
            let thumb_y = pill.y + pad + (track_h - thumb_h) * frac;
            let w = 4.0 * s;
            let x = pill.x + pill.w - pad - sb + (sb - w) * 0.5;
            frame.content.push(Item::Rect { r: Rect::new(x, thumb_y, w, thumb_h), radius: w * 0.5, soft: 0.8, color: fade(t.fg, 0.35 * pa) });
        }
    }

    fn build_wifi(&mut self, frame: &mut Frame, text: &mut Text, pill: Rect, pa: f32) {
        let s = self.scale;
        let t = self.theme;
        let f = self.cfg.wifi.font_size;
        let px = f * s;
        let (asc, desc) = text.line_metrics(px);
        let pad = WIFI_PAD * s;
        let head = WIFI_HEAD * s;
        let row_h = (f + WIFI_ROW_EXTRA) * s;
        let x0 = pill.x + pad;
        let w_in = pill.w - 2.0 * pad;
        let inset = 12.0 * s;
        let hy = pill.y + pad;
        // mierne vyššie ako geometrický stred – opticky vycentruje prepínač / text / tlačidlá
        let hcy = hy + head * 0.5 - 6.0 * s;
        let base = |cy: f32| cy + (asc - desc) * 0.5;

        // ---- zadávanie hesla
        if let Some(pw) = &self.wifi_pw {
            let title = format!("\u{f033e}  Heslo pre {}", pw.ssid);
            let title = Self::ellipsize(text, &title, px, w_in - 2.0 * inset);
            text.layout_to(&mut frame.content, &title, px, x0 + inset, base(hcy), fade(t.fg, pa));
            let bx = Rect::new(x0, hy + head + 4.0 * s, w_in, (f + 18.0) * s);
            frame.content.push(Item::Rect { r: bx, radius: 10.0 * s, soft: 0.8, color: fade(t.fg, 0.12 * pa) });
            let max_w = bx.w - 2.0 * inset;
            let mut dots: String = "\u{25cf}".repeat(pw.text.chars().count());
            while !dots.is_empty() && text.measure(&dots, px) > max_w {
                dots.remove(0);
            }
            let cy = bx.y + bx.h * 0.5;
            let mut cx = bx.x + inset;
            if dots.is_empty() {
                text.layout_to(&mut frame.content, "Heslo", px, cx, base(cy), fade(t.fg, 0.4 * pa));
            } else {
                text.layout_to(&mut frame.content, &dots, px, cx, base(cy), fade(t.fg, pa));
                cx += text.measure(&dots, px);
            }
            frame.content.push(Item::Rect { r: Rect::new(cx + 2.0 * s, bx.y + 8.0 * s, 1.5 * s, bx.h - 16.0 * s), radius: 0.7 * s, soft: 0.8, color: fade(t.accent, pa) });
            let px2 = px * 0.85;
            let (a2, d2) = text.line_metrics(px2);
            let msg = if self.wifi_note.is_empty() { "Enter – pripojiť · Esc – zrušiť · Ctrl+V – vložiť" } else { self.wifi_note.as_str() };
            let msg = Self::ellipsize(text, msg, px2, w_in - 2.0 * inset);
            let hy2 = bx.y + bx.h + 10.0 * s + (f * 0.85 + 6.0) * s * 0.5;
            text.layout_to(&mut frame.content, &msg, px2, x0 + inset, hy2 + (a2 - d2) * 0.5, fade(t.fg, 0.65 * pa));
            return;
        }

        // ---- hlavička: vľavo prepínač rádia + obnoviť, v strede pripojená sieť, vpravo správca pripojení
        let usable = self.wifi.available && self.wifi.error.is_empty();
        let on = usable && self.wifi.enabled;
        let active = self.wifi.aps.iter().find(|a| a.active).cloned();
        let tw = 40.0 * s;
        let th = 22.0 * s;
        let tog = Rect::new(x0 + inset * 0.5, hcy - th * 0.5, tw, th);
        let ref_sz = px * 1.7;
        let rbtn = Rect::new(tog.x + tog.w + 6.0 * s, hcy - ref_sz * 0.5, ref_sz, ref_sz);
        let ed_label = format!("\u{f0493}  {}", self.cfg.wifi.editor_label);
        let ed_w = text.measure(&ed_label, px) + 2.0 * inset;
        let ed_h = ref_sz * 1.1;
        let ed = Rect::new(pill.x + pill.w - pad - ed_w, hcy - ed_h * 0.5, ed_w, ed_h);
        if !self.cfg.wifi.editor_command.is_empty() {
            self.hits.push((ed, Hit::WifiEditor));
            let hov = self.hover == Some(Hit::WifiEditor);
            let bg = if hov { fade(t.menu_selected, pa) } else { fade(t.fg, 0.12 * pa) };
            frame.content.push(Item::Rect { r: ed, radius: ed.h * 0.5, soft: 0.8, color: bg });
            text.layout_to(&mut frame.content, &ed_label, px, ed.x + inset, base(hcy), fade(if hov { t.menu_selected_text } else { t.fg }, pa));
        }
        if self.wifi.available {
            self.hits.push((tog, Hit::WifiToggle));
            let track = if self.wifi.enabled { t.accent } else { t.fg };
            frame.content.push(Item::Rect { r: tog, radius: th * 0.5, soft: 0.8, color: fade(track, (if self.wifi.enabled { 1.0 } else { 0.3 }) * pa) });
            let kn = th - 6.0 * s;
            let kx = if self.wifi.enabled { tog.x + tog.w - 3.0 * s - kn } else { tog.x + 3.0 * s };
            frame.content.push(Item::Rect { r: Rect::new(kx, tog.y + 3.0 * s, kn, kn), radius: kn * 0.5, soft: 0.8, color: fade(if self.wifi.enabled { t.on_accent } else { t.fg }, pa) });
            if on {
                self.hits.push((rbtn, Hit::WifiRefresh));
                let hov = self.hover == Some(Hit::WifiRefresh);
                if hov {
                    frame.content.push(Item::Rect { r: rbtn, radius: 8.0 * s, soft: 0.8, color: fade(t.menu_selected, pa) });
                }
                let gw = text.measure("\u{f0450}", px);
                text.layout_to(&mut frame.content, "\u{f0450}", px, rbtn.x + (rbtn.w - gw) * 0.5, base(hcy), fade(if hov { t.menu_selected_text } else { t.fg }, pa));
            }
        }
        // stred: názov pripojenej siete (so silou signálu) alebo stav
        let (mid, mid_col) = if !self.wifi.error.is_empty() {
            ("Wi-Fi nedostupné".to_string(), 0.7)
        } else if !self.wifi_loaded {
            ("Načítavam…".to_string(), 0.7)
        } else if !self.wifi.available {
            ("Wi-Fi zariadenie nenájdené".to_string(), 0.7)
        } else if !self.wifi.enabled {
            ("Wi-Fi vypnuté".to_string(), 0.7)
        } else if let Some(a) = &active {
            (format!("{}  {}", crate::wifi::signal_glyph(a.strength), a.ssid), 1.0)
        } else {
            ("Nepripojené".to_string(), 0.7)
        };
        let left = rbtn.x + rbtn.w + 10.0 * s;
        let right = ed.x - 10.0 * s;
        if right > left {
            let mid = Self::ellipsize(text, &mid, px, right - left);
            let mw = text.measure(&mid, px);
            let col = if active.is_some() && on { t.accent } else { t.fg };
            text.layout_to(&mut frame.content, &mid, px, left + (right - left - mw) * 0.5, base(hcy), fade(col, mid_col * pa));
        }

        // ---- zoznam sietí
        let top = pill.y + pad + head;
        let n = self.wifi.aps.len();
        let list_on = usable && self.wifi.enabled && n > 0;
        let rows = self.wifi_rows_shown();
        if !list_on {
            let msg = if !self.wifi.error.is_empty() {
                self.wifi.error.clone()
            } else if !self.wifi_loaded {
                "Načítavam…".to_string()
            } else if !self.wifi.available {
                "Nenašiel som žiadne Wi-Fi zariadenie".to_string()
            } else if !self.wifi.enabled {
                "Wi-Fi je vypnuté".to_string()
            } else {
                "Žiadne siete".to_string()
            };
            let msg = Self::ellipsize(text, &msg, px, w_in - 2.0 * inset);
            let mw = text.measure(&msg, px);
            text.layout_to(&mut frame.content, &msg, px, x0 + (w_in - mw) * 0.5, base(top + row_h * 0.5), fade(t.fg, 0.7 * pa));
        } else {
            let scrollable = n > rows;
            let sb = if scrollable { WIFI_SCROLLBAR * s } else { 0.0 };
            let start = self.wifi_scroll.min(n - rows.min(n));
            let cell_w = w_in - sb;
            let px_small = px * 0.8;
            let (a3, d3) = text.line_metrics(px_small);
            // šírka stĺpca zabezpečenia = najširší popis v zozname; vpravo je pevné miesto na značku pripojenia
            let check_w = text.measure("\u{f012c}", px) + 6.0 * s;
            let sec_w = self.wifi.aps.iter().map(|a| text.measure(if a.sec.is_empty() { "Otvorená" } else { &a.sec }, px_small)).fold(0.0f32, f32::max);
            for k in 0..rows.min(n) {
                let i = start + k;
                let y = top + k as f32 * row_h;
                let cell = Rect::new(x0, y, cell_w, row_h);
                self.hits.push((cell, Hit::WifiRow(i)));
                let hov = self.hover == Some(Hit::WifiRow(i));
                let ap = &self.wifi.aps[i];
                let fg = if hov {
                    frame.content.push(Item::Rect { r: cell, radius: 10.0 * s, soft: 0.8, color: fade(t.menu_selected, pa) });
                    t.menu_selected_text
                } else if ap.active {
                    // aktuálne pripojená sieť: jemné podfarbenie a farba zvýraznenia
                    frame.content.push(Item::Rect { r: cell, radius: 10.0 * s, soft: 0.8, color: fade(t.accent, 0.18 * pa) });
                    t.accent
                } else {
                    t.fg
                };
                let cy = y + row_h * 0.5;
                text.layout_to(&mut frame.content, crate::wifi::signal_glyph(ap.strength), px, cell.x + inset, base(cy), fade(fg, pa));
                // zprava doľava: značka pripojenia, stĺpec zabezpečenia, „uložená“
                let mut rx = cell.x + cell.w - inset - check_w;
                if ap.active {
                    text.layout_to(&mut frame.content, "\u{f012c}", px, rx, base(cy), fade(fg, pa));
                }
                rx -= 8.0 * s;
                let (label, dim) = if ap.sec.is_empty() { ("Otvorená", 0.5) } else { (ap.sec.as_str(), 0.75) };
                let lw = text.measure(label, px_small);
                // zarovnané vľavo v stĺpci, aby si popisy boli pod sebou
                let col_x = rx - sec_w;
                text.layout_to(&mut frame.content, label, px_small, col_x, cy + (a3 - d3) * 0.5, fade(fg, dim * pa));
                let _ = lw;
                rx = col_x - 10.0 * s;
                if ap.saved.is_some() && !ap.active {
                    let label = "uložená";
                    let gw = text.measure(label, px_small);
                    rx -= gw;
                    text.layout_to(&mut frame.content, label, px_small, rx, cy + (a3 - d3) * 0.5, fade(fg, 0.55 * pa));
                    rx -= 8.0 * s;
                }
                let name_x = cell.x + inset + px * 1.9;
                let name = Self::ellipsize(text, &ap.ssid, px, rx - name_x);
                text.layout_to(&mut frame.content, &name, px, name_x, base(cy), fade(fg, pa));
            }
            if scrollable {
                let track_h = rows as f32 * row_h;
                let thumb_h = (track_h * rows as f32 / n as f32).max(16.0 * s).min(track_h);
                let frac = start as f32 / (n - rows) as f32;
                let w = 4.0 * s;
                let x = x0 + cell_w + (sb - w) * 0.5;
                frame.content.push(Item::Rect { r: Rect::new(x, top + (track_h - thumb_h) * frac, w, thumb_h), radius: w * 0.5, soft: 0.8, color: fade(t.fg, 0.35 * pa) });
            }
        }

        // ---- päta: stav / nápoveda k riadku pod kurzorom
        let hint = match self.hover {
            Some(Hit::WifiRow(i)) => self.wifi.aps.get(i).map(|ap| {
                if ap.active {
                    "Klik – odpojiť"
                } else if ap.saved.is_some() {
                    "Klik – pripojiť · pravý klik – zabudnúť sieť"
                } else if !ap.supported {
                    "Nepodporované zabezpečenie"
                } else if ap.secured {
                    "Klik – zadať heslo"
                } else {
                    "Klik – pripojiť"
                }
            }),
            _ => None,
        };
        let msg = if !self.wifi_note.is_empty() { Some(self.wifi_note.as_str()) } else { hint };
        if let Some(m) = msg {
            let px2 = px * 0.85;
            let (a2, d2) = text.line_metrics(px2);
            let m = Self::ellipsize(text, m, px2, w_in - 2.0 * inset);
            let cy = pill.y + pill.h - pad - WIFI_FOOT * s * 0.5;
            text.layout_to(&mut frame.content, &m, px2, x0 + inset, cy + (a2 - d2) * 0.5, fade(t.fg, 0.65 * pa));
        }
    }

    fn build_bluetooth(&mut self, frame: &mut Frame, text: &mut Text, pill: Rect, pa: f32) {
        let s = self.scale;
        let t = self.theme;
        let f = self.cfg.bluetooth.font_size;
        let px = f * s;
        let (asc, desc) = text.line_metrics(px);
        let pad = WIFI_PAD * s;
        let head = WIFI_HEAD * s;
        let row_h = (f + WIFI_ROW_EXTRA) * s;
        let x0 = pill.x + pad;
        let w_in = pill.w - 2.0 * pad;
        let inset = 12.0 * s;
        let hy = pill.y + pad;
        let hcy = hy + head * 0.5 - 6.0 * s;
        let base = |cy: f32| cy + (asc - desc) * 0.5;

        // hlavička: prepínač + obnoviť + voliteľný editor
        let tw = 40.0 * s;
        let th = 22.0 * s;
        let tog = Rect::new(x0 + inset * 0.5, hcy - th * 0.5, tw, th);
        let ref_sz = px * 1.7;
        let rbtn = Rect::new(tog.x + tog.w + 6.0 * s, hcy - ref_sz * 0.5, ref_sz, ref_sz);

        let mut ed = Rect::new(0.0, 0.0, 0.0, 0.0);
        if !self.cfg.bluetooth.editor_command.is_empty() {
            let ed_label = format!("\u{f0493}  {}", self.cfg.bluetooth.editor_label);
            let ed_w = text.measure(&ed_label, px) + 2.0 * inset;
            let ed_h = ref_sz * 1.1;
            ed = Rect::new(pill.x + pill.w - pad - ed_w, hcy - ed_h * 0.5, ed_w, ed_h);
            self.hits.push((ed, Hit::BtEditor));
            let hov = self.hover == Some(Hit::BtEditor);
            let bg = if hov { fade(t.menu_selected, pa) } else { fade(t.fg, 0.12 * pa) };
            frame.content.push(Item::Rect { r: ed, radius: ed.h * 0.5, soft: 0.8, color: bg });
            text.layout_to(
                &mut frame.content,
                &ed_label,
                px,
                ed.x + inset,
                base(hcy),
                fade(if hov { t.menu_selected_text } else { t.fg }, pa),
            );
        }

        if self.bluetooth.available {
            self.hits.push((tog, Hit::BtToggle));
            let on = self.bluetooth.powered;
            let track = if on { t.accent } else { t.fg };
            frame.content.push(Item::Rect {
                r: tog,
                radius: th * 0.5,
                soft: 0.8,
                color: fade(track, (if on { 1.0 } else { 0.3 }) * pa),
            });
            let kn = th - 6.0 * s;
            let kx = if on {
                tog.x + tog.w - 3.0 * s - kn
            } else {
                tog.x + 3.0 * s
            };
            frame.content.push(Item::Rect {
                r: Rect::new(kx, tog.y + 3.0 * s, kn, kn),
                radius: kn * 0.5,
                soft: 0.8,
                color: fade(if on { t.on_accent } else { t.fg }, pa),
            });

            if on {
                self.hits.push((rbtn, Hit::BtRefresh));
                let hov = self.hover == Some(Hit::BtRefresh);
                if hov {
                    frame.content.push(Item::Rect {
                        r: rbtn,
                        radius: 8.0 * s,
                        soft: 0.8,
                        color: fade(t.menu_selected, pa),
                    });
                }
                let gw = text.measure("\u{f0450}", px);
                text.layout_to(
                    &mut frame.content,
                    "\u{f0450}",
                    px,
                    rbtn.x + (rbtn.w - gw) * 0.5,
                    base(hcy),
                    fade(if hov { t.menu_selected_text } else { t.fg }, pa),
                );

                // viditeľnosť (discoverable) – aby ťa našli nové zariadenia
                let dlab = if self.bluetooth.discoverable { "Viditeľný" } else { "Skrytý" };
                let dw = text.measure(dlab, px * 0.85) + 16.0 * s;
                let dh = ref_sz * 1.0;
                let dbtn = Rect::new(rbtn.x + rbtn.w + 8.0 * s, hcy - dh * 0.5, dw, dh);
                self.hits.push((dbtn, Hit::BtDiscoverable));
                let dhov = self.hover == Some(Hit::BtDiscoverable);
                let dbg = if self.bluetooth.discoverable {
                    fade(t.accent, 0.35 * pa)
                } else if dhov {
                    fade(t.menu_selected, pa)
                } else {
                    fade(t.fg, 0.12 * pa)
                };
                frame.content.push(Item::Rect {
                    r: dbtn,
                    radius: dh * 0.5,
                    soft: 0.8,
                    color: dbg,
                });
                let (a2, d2) = text.line_metrics(px * 0.85);
                let lw = text.measure(dlab, px * 0.85);
                text.layout_to(
                    &mut frame.content,
                    dlab,
                    px * 0.85,
                    dbtn.x + (dbtn.w - lw) * 0.5,
                    hcy + (a2 - d2) * 0.5,
                    fade(if self.bluetooth.discoverable { t.on_accent } else if dhov { t.menu_selected_text } else { t.fg }, pa),
                );
            }
        }

        // stred: stav
        let (mid, mid_col) = if !self.bluetooth.note.is_empty() {
            (self.bluetooth.note.clone(), 0.9)
        } else if !self.bluetooth.error.is_empty() {
            (self.bluetooth.error.clone(), 0.7)
        } else if !self.bluetooth_loaded {
            ("Načítavam…".into(), 0.7)
        } else if !self.bluetooth.available {
            ("Adaptér nenájdený".into(), 0.7)
        } else if !self.bluetooth.powered {
            ("Bluetooth vypnutý".into(), 0.7)
        } else if self.bluetooth.discovering {
            ("Hľadám zariadenia…".into(), 0.85)
        } else {
            let n = self.bluetooth.devices.iter().filter(|d| d.connected).count();
            if n == 0 {
                ("Žiadne pripojené · ↻ hľadať".into(), 0.7)
            } else if n == 1 {
                let name = self
                    .bluetooth
                    .devices
                    .iter()
                    .find(|d| d.connected)
                    .map(|d| d.name.as_str())
                    .unwrap_or("");
                (name.to_string(), 1.0)
            } else {
                (format!("{n} pripojené"), 1.0)
            }
        };
        // ľavý okraj textu za tlačidlami (refresh + discoverable)
        let left = if self.bluetooth.powered {
            rbtn.x + rbtn.w + 8.0 * s + 90.0 * s
        } else {
            rbtn.x + rbtn.w + 10.0 * s
        };
        let right = if ed.w > 0.0 { ed.x - 10.0 * s } else { x0 + w_in };
        if right > left {
            let mid = Self::ellipsize(text, &mid, px, right - left);
            let mw = text.measure(&mid, px);
            text.layout_to(
                &mut frame.content,
                &mid,
                px,
                left + (right - left - mw) * 0.5,
                base(hcy),
                fade(t.fg, mid_col * pa),
            );
        }

        // zoznam
        let top = pill.y + pad + head;
        let n = self.bluetooth.devices.len();
        let rows = n.min(self.cfg.bluetooth.rows).max(1);
        if !self.bluetooth.available || !self.bluetooth.powered || n == 0 {
            let msg = if !self.bluetooth.error.is_empty() {
                self.bluetooth.error.clone()
            } else if !self.bluetooth_loaded {
                "Načítavam…".into()
            } else if !self.bluetooth.available {
                "Bluetooth adaptér sa nenašiel".into()
            } else if !self.bluetooth.powered {
                "Bluetooth je vypnutý".into()
            } else {
                "Žiadne zariadenia – skús obnoviť".into()
            };
            let msg = Self::ellipsize(text, &msg, px, w_in - 2.0 * inset);
            let mw = text.measure(&msg, px);
            text.layout_to(
                &mut frame.content,
                &msg,
                px,
                x0 + (w_in - mw) * 0.5,
                base(top + row_h * 0.5),
                fade(t.fg, 0.7 * pa),
            );
        } else {
            let scrollable = n > rows;
            let sb = if scrollable { WIFI_SCROLLBAR * s } else { 0.0 };
            let start = self.bluetooth_scroll.min(n.saturating_sub(rows));
            let cell_w = w_in - sb;
            for k in 0..rows.min(n) {
                let i = start + k;
                let d = &self.bluetooth.devices[i];
                let y = top + k as f32 * row_h;
                let cell = Rect::new(x0, y, cell_w, row_h);
                self.hits.push((cell, Hit::BtRow(i)));
                let hov = self.hover == Some(Hit::BtRow(i));
                if hov {
                    frame.content.push(Item::Rect {
                        r: cell,
                        radius: 10.0 * s,
                        soft: 0.8,
                        color: fade(t.menu_selected, pa),
                    });
                } else if d.connected {
                    frame.content.push(Item::Rect {
                        r: cell,
                        radius: 10.0 * s,
                        soft: 0.8,
                        color: fade(t.accent, 0.18 * pa),
                    });
                }
                let fg = if hov {
                    t.menu_selected_text
                } else if d.connected {
                    t.accent
                } else {
                    t.fg
                };
                let cy = cell.y + row_h * 0.5;
                let glyph = crate::bluetooth::device_glyph(d);
                let gw = text.measure(glyph, px);
                text.layout_to(&mut frame.content, glyph, px, cell.x + inset, base(cy), fade(fg, pa));
                let mut right = cell.x + cell.w - inset;
                if d.connected {
                    let mark = "\u{f012c}";
                    let mw = text.measure(mark, px);
                    right -= mw;
                    text.layout_to(&mut frame.content, mark, px, right, base(cy), fade(fg, pa));
                    right -= 8.0 * s;
                }
                if let Some(bat) = d.battery {
                    let bt = format!("{bat}%");
                    let bw = text.measure(&bt, px * 0.85);
                    right -= bw;
                    let (a2, d2) = text.line_metrics(px * 0.85);
                    text.layout_to(
                        &mut frame.content,
                        &bt,
                        px * 0.85,
                        right,
                        cy + (a2 - d2) * 0.5,
                        fade(fg, 0.75 * pa),
                    );
                    right -= 8.0 * s;
                }
                let name = Self::ellipsize(text, &d.name, px, right - (cell.x + inset + gw + 8.0 * s));
                text.layout_to(
                    &mut frame.content,
                    &name,
                    px,
                    cell.x + inset + gw + 8.0 * s,
                    base(cy),
                    fade(fg, pa),
                );
            }
            if scrollable {
                let track_h = rows as f32 * row_h;
                let thumb_h = (track_h * rows as f32 / n as f32).max(16.0 * s).min(track_h);
                let frac = start as f32 / (n - rows) as f32;
                let w = 4.0 * s;
                let x = pill.x + pill.w - pad - (WIFI_SCROLLBAR * s - w) * 0.5 - w;
                let y = top + (track_h - thumb_h) * frac;
                frame.content.push(Item::Rect {
                    r: Rect::new(x, y, w, thumb_h),
                    radius: w * 0.5,
                    soft: 0.8,
                    color: fade(t.fg, 0.35 * pa),
                });
            }
        }

        // päta
        let hint = if !self.bluetooth.note.is_empty() {
            Some(self.bluetooth.note.as_str())
        } else if !self.bluetooth.powered {
            Some("Zapni Bluetooth prepínačom vľavo")
        } else if self.hover == Some(Hit::BtRefresh) {
            Some("Hľadať nové zariadenia (sken)")
        } else if self.hover == Some(Hit::BtDiscoverable) {
            Some("Viditeľný = iné zariadenia ťa nájdu")
        } else {
            match self.hover {
                Some(Hit::BtRow(i)) => self.bluetooth.devices.get(i).map(|d| {
                    if d.connected {
                        "Klik – odpojiť · stred – trust · pravý – zabudnúť"
                    } else if d.paired {
                        "Klik – pripojiť · stred – trust · pravý – zabudnúť"
                    } else {
                        "Klik – spárovať a pripojiť · pravý – odstrániť"
                    }
                }),
                _ => Some("Klik = spárovať/pripojiť · ↻ = hľadať nové"),
            }
        };
        if let Some(m) = hint {
            let px2 = px * 0.85;
            let (a2, d2) = text.line_metrics(px2);
            let m = Self::ellipsize(text, m, px2, w_in - 2.0 * inset);
            let cy = pill.y + pill.h - pad - WIFI_FOOT * s * 0.5;
            text.layout_to(
                &mut frame.content,
                &m,
                px2,
                x0 + inset,
                cy + (a2 - d2) * 0.5,
                fade(t.fg, 0.65 * pa),
            );
        }
    }

    fn build_audio(&mut self, frame: &mut Frame, text: &mut Text, pill: Rect, pa: f32) {
        let s = self.scale;
        let t = self.theme;
        let f = self.cfg.audio_panel.font_size;
        let px = f * s;
        let px_small = px * 0.8;
        let (asc, desc) = text.line_metrics(px);
        let (a3, d3) = text.line_metrics(px_small);
        let base = |cy: f32| cy + (asc - desc) * 0.5;
        let pad = AUDIO_PAD * s;
        let row_h = (f + AUDIO_ROW_EXTRA) * s;
        let inset = 12.0 * s;
        let x0 = pill.x + pad;
        let w_in = pill.w - 2.0 * pad;
        let max_pct = self.audio_max() as f32;
        let mut y = pill.y + pad;

        for src in [false, true] {
            let list_len = if src { self.audio.sources.len() } else { self.audio.sinks.len() };
            let shown = list_len.min(self.cfg.audio_panel.rows).max(1);
            let di = self.audio_default(src);
            let (vol, muted) = match di {
                Some(i) => {
                    let d = if src { &self.audio.sources[i] } else { &self.audio.sinks[i] };
                    (d.volume, d.mute)
                }
                None => (0, false),
            };

            // názov sekcie
            let label = if src { "Vstup" } else { "Výstup" };
            let lh = AUDIO_LABEL * s;
            text.layout_to(&mut frame.content, label, px_small, x0 + inset, y + lh * 0.5 + (a3 - d3) * 0.5, fade(t.fg, 0.6 * pa));
            y += lh;

            // riadok posuvníka: tlačidlo stlmenia, dráha, percentá
            let cy = y + row_h * 0.5;
            let mb = Rect::new(x0, y, row_h, row_h);
            self.hits.push((mb, Hit::AudioMute(src)));
            let mhov = self.hover == Some(Hit::AudioMute(src));
            if mhov {
                frame.content.push(Item::Rect { r: mb, radius: 10.0 * s, soft: 0.8, color: fade(t.menu_selected, pa) });
            }
            let glyph = crate::audio::icon(src, muted, vol);
            let gw = text.measure(glyph, px);
            text.layout_to(&mut frame.content, glyph, px, mb.x + (mb.w - gw) * 0.5, base(cy), fade(if mhov { t.menu_selected_text } else if muted { t.alert } else { t.fg }, pa));
            let pct_w = text.measure("150%", px);
            let track_x = mb.x + mb.w + 8.0 * s;
            let track = Rect::new(track_x, cy - 3.0 * s, (x0 + w_in - pct_w - 12.0 * s - track_x).max(10.0 * s), 6.0 * s);
            self.audio_rects[src as usize] = track;
            self.hits.push((Rect::new(track.x - 8.0 * s, y, track.w + 16.0 * s, row_h), Hit::AudioSlider(src)));
            frame.content.push(Item::Rect { r: track, radius: 3.0 * s, soft: 0.8, color: fade(t.fg, 0.2 * pa) });
            let frac = (vol as f32 / max_pct).clamp(0.0, 1.0);
            let fill = (track.w * frac).max(if vol > 0 { 6.0 * s } else { 0.0 });
            let fill_col = if muted { fade(t.fg, 0.45 * pa) } else { fade(t.accent, pa) };
            if fill > 0.0 {
                frame.content.push(Item::Rect { r: Rect::new(track.x, track.y, fill, track.h), radius: 3.0 * s, soft: 0.8, color: fill_col });
            }
            let kn = 14.0 * s;
            let dragging = self.audio_drag == Some(src);
            let khov = dragging || self.hover == Some(Hit::AudioSlider(src));
            let kd = if khov { kn + 2.0 * s } else { kn };
            frame.content.push(Item::Rect { r: Rect::new(track.x + track.w * frac - kd * 0.5, cy - kd * 0.5, kd, kd), radius: kd * 0.5, soft: 0.8, color: fade(if muted { t.fg } else { t.accent }, pa) });
            let ptxt = format!("{vol}%");
            let pw = text.measure(&ptxt, px);
            text.layout_to(&mut frame.content, &ptxt, px, x0 + w_in - pw, base(cy), fade(t.fg, (if muted { 0.5 } else { 1.0 }) * pa));
            y += row_h;

            // zoznam zariadení
            if list_len == 0 {
                let msg = if !self.audio.error.is_empty() {
                    self.audio.error.clone()
                } else if !self.audio_loaded {
                    "Načítavam…".to_string()
                } else {
                    "Žiadne zariadenie".to_string()
                };
                let msg = Self::ellipsize(text, &msg, px, w_in - 2.0 * inset);
                text.layout_to(&mut frame.content, &msg, px, x0 + inset, base(y + row_h * 0.5), fade(t.fg, 0.6 * pa));
            } else {
                for k in 0..shown.min(list_len) {
                    let cell = Rect::new(x0, y + k as f32 * row_h, w_in, row_h);
                    self.hits.push((cell, Hit::AudioDev(src, k)));
                    let hov = self.hover == Some(Hit::AudioDev(src, k));
                    let d = if src { &self.audio.sources[k] } else { &self.audio.sinks[k] };
                    let is_def = Some(k) == di;
                    let fg = if hov {
                        frame.content.push(Item::Rect { r: cell, radius: 10.0 * s, soft: 0.8, color: fade(t.menu_selected, pa) });
                        t.menu_selected_text
                    } else if is_def {
                        frame.content.push(Item::Rect { r: cell, radius: 10.0 * s, soft: 0.8, color: fade(t.accent, 0.18 * pa) });
                        t.accent
                    } else {
                        t.fg
                    };
                    let ccy = cell.y + row_h * 0.5;
                    let mut right = cell.x + cell.w - inset;
                    if is_def {
                        let cw = text.measure("\u{f012c}", px);
                        right -= cw;
                        text.layout_to(&mut frame.content, "\u{f012c}", px, right, base(ccy), fade(fg, pa));
                        right -= 6.0 * s;
                    }
                    let name = Self::ellipsize(text, &d.desc, px, right - cell.x - inset);
                    text.layout_to(&mut frame.content, &name, px, cell.x + inset, base(ccy), fade(fg, pa));
                }
            }
            y += shown as f32 * row_h + AUDIO_GAP * s;
        }
    }

    fn build_monitor(&mut self, frame: &mut Frame, text: &mut Text, pill: Rect, pa: f32) {
        let s = self.scale;
        let t = self.theme;
        let c = &self.cfg.monitor;
        let f = c.font_size;
        let px = f * s;
        let px_small = px * 0.8;
        let (asc, desc) = text.line_metrics(px);
        let (a3, d3) = text.line_metrics(px_small);
        let base = |cy: f32| cy + (asc - desc) * 0.5;
        let pad = 14.0 * s;
        let row_h = (f + 20.0) * s;
        let label_h = 22.0 * s;
        let gap = 12.0 * s;
        let inset = 12.0 * s;
        let x0 = pill.x + pad;
        let w_in = pill.w - 2.0 * pad;
        let mut y = pill.y + pad;

        if !self.monitor.error.is_empty() {
            let msg = Self::ellipsize(text, &self.monitor.error, px, w_in);
            text.layout_to(
                &mut frame.content,
                &msg,
                px,
                x0 + inset,
                base(y + row_h * 0.5),
                fade(t.alert, pa),
            );
            return;
        }

        if !self.monitor_loaded {
            text.layout_to(
                &mut frame.content,
                "Načítavam…",
                px,
                x0 + inset,
                base(y + row_h * 0.5),
                fade(t.fg, 0.6 * pa),
            );
            return;
        }

        // --- helper: sekcia s popiskom
        let mut section_label = |out: &mut Vec<Item>, text: &mut Text, y: &mut f32, label: &str| {
            text.layout_to(
                out,
                label,
                px_small,
                x0 + inset,
                *y + label_h * 0.5 + (a3 - d3) * 0.5,
                fade(t.fg, 0.6 * pa),
            );
            *y += label_h;
        };

        // --- jas
        section_label(&mut frame.content, text, &mut y, "Jas");
        {
            let cy = y + row_h * 0.5;
            let pct = self.monitor.brightness;
            let pct_w = text.measure("100%", px);
            let track_x = x0 + inset;
            let track = Rect::new(
                track_x,
                cy - 3.0 * s,
                (w_in - 2.0 * inset - pct_w - 10.0 * s).max(20.0 * s),
                6.0 * s,
            );
            self.monitor_rects[0] = track;
            self.hits.push((
                Rect::new(track.x - 4.0 * s, y, track.w + 8.0 * s, row_h),
                Hit::MonSlider(false),
            ));
            let fill_w = track.w * (pct as f32 / 100.0);
            frame.content.push(Item::Rect {
                r: track,
                radius: 3.0 * s,
                soft: 0.8,
                color: fade(t.fg, 0.18 * pa),
            });
            if fill_w > 0.5 {
                frame.content.push(Item::Rect {
                    r: Rect::new(track.x, track.y, fill_w, track.h),
                    radius: 3.0 * s,
                    soft: 0.8,
                    color: fade(t.accent, pa),
                });
            }
            // gulička
            let kx = track.x + fill_w - 5.0 * s;
            frame.content.push(Item::Rect {
                r: Rect::new(kx.max(track.x), cy - 7.0 * s, 10.0 * s, 14.0 * s),
                radius: 5.0 * s,
                soft: 0.8,
                color: fade(t.fg, pa),
            });
            let ptxt = format!("{pct}%");
            let pw = text.measure(&ptxt, px);
            text.layout_to(
                &mut frame.content,
                &ptxt,
                px,
                x0 + w_in - pw,
                base(cy),
                fade(t.fg, pa),
            );
            y += row_h;
        }

        y += gap;

        // --- kontrast
        section_label(&mut frame.content, text, &mut y, "Kontrast");
        {
            let cy = y + row_h * 0.5;
            let pct = self.monitor.contrast;
            let pct_w = text.measure("100%", px);
            let track_x = x0 + inset;
            let track = Rect::new(
                track_x,
                cy - 3.0 * s,
                (w_in - 2.0 * inset - pct_w - 10.0 * s).max(20.0 * s),
                6.0 * s,
            );
            self.monitor_rects[1] = track;
            self.hits.push((
                Rect::new(track.x - 4.0 * s, y, track.w + 8.0 * s, row_h),
                Hit::MonSlider(true),
            ));
            let fill_w = track.w * (pct as f32 / 100.0);
            frame.content.push(Item::Rect {
                r: track,
                radius: 3.0 * s,
                soft: 0.8,
                color: fade(t.fg, 0.18 * pa),
            });
            if fill_w > 0.5 {
                frame.content.push(Item::Rect {
                    r: Rect::new(track.x, track.y, fill_w, track.h),
                    radius: 3.0 * s,
                    soft: 0.8,
                    color: fade(t.accent, pa),
                });
            }
            let kx = track.x + fill_w - 5.0 * s;
            frame.content.push(Item::Rect {
                r: Rect::new(kx.max(track.x), cy - 7.0 * s, 10.0 * s, 14.0 * s),
                radius: 5.0 * s,
                soft: 0.8,
                color: fade(t.fg, pa),
            });
            let ptxt = format!("{pct}%");
            let pw = text.measure(&ptxt, px);
            text.layout_to(
                &mut frame.content,
                &ptxt,
                px,
                x0 + w_in - pw,
                base(cy),
                fade(t.fg, pa),
            );
            y += row_h;
        }

        y += gap;

        // --- teplota farieb
        section_label(&mut frame.content, text, &mut y, "Teplota farieb");
        {
            let n = c.preset_labels.len().max(1) as f32;
            let btn_gap = 8.0 * s;
            let btn_w = ((w_in - 2.0 * inset - (n - 1.0) * btn_gap) / n).max(40.0 * s);
            let bh = (row_h * 0.85).min(32.0 * s);
            let by = y + (row_h - bh) * 0.5;
            for (i, label) in c.preset_labels.iter().enumerate() {
                let bx = x0 + inset + i as f32 * (btn_w + btn_gap);
                let r = Rect::new(bx, by, btn_w, bh);
                self.hits.push((r, Hit::MonPreset(i)));
                let selected = self.monitor.preset == Some(i);
                let hov = self.hover == Some(Hit::MonPreset(i));
                let bg = if selected {
                    fade(t.accent, 0.35 * pa)
                } else if hov {
                    fade(t.menu_selected, pa)
                } else {
                    fade(t.fg, 0.12 * pa)
                };
                frame.content.push(Item::Rect {
                    r,
                    radius: bh * 0.5,
                    soft: 0.8,
                    color: bg,
                });
                let fg = if selected {
                    t.on_accent
                } else if hov {
                    t.menu_selected_text
                } else {
                    t.fg
                };
                let lab = Self::ellipsize(text, label, px_small, btn_w - 10.0 * s);
                let lw = text.measure(&lab, px_small);
                text.layout_to(
                    &mut frame.content,
                    &lab,
                    px_small,
                    r.x + (r.w - lw) * 0.5,
                    by + bh * 0.5 + (a3 - d3) * 0.5,
                    fade(fg, pa),
                );
            }
            y += row_h;
        }

        y += gap;

        // --- vstup
        section_label(&mut frame.content, text, &mut y, "Vstup");
        {
            let n = c.input_labels.len().max(1) as f32;
            let btn_gap = 8.0 * s;
            let btn_w = ((w_in - 2.0 * inset - (n - 1.0) * btn_gap) / n).max(40.0 * s);
            let bh = (row_h * 0.85).min(32.0 * s);
            let by = y + (row_h - bh) * 0.5;
            for (i, label) in c.input_labels.iter().enumerate() {
                let bx = x0 + inset + i as f32 * (btn_w + btn_gap);
                let r = Rect::new(bx, by, btn_w, bh);
                self.hits.push((r, Hit::MonInput(i)));
                let selected = self.monitor.input == Some(i);
                let hov = self.hover == Some(Hit::MonInput(i));
                let bg = if selected {
                    fade(t.accent, 0.35 * pa)
                } else if hov {
                    fade(t.menu_selected, pa)
                } else {
                    fade(t.fg, 0.12 * pa)
                };
                frame.content.push(Item::Rect {
                    r,
                    radius: bh * 0.5,
                    soft: 0.8,
                    color: bg,
                });
                let fg = if selected {
                    t.on_accent
                } else if hov {
                    t.menu_selected_text
                } else {
                    t.fg
                };
                let lab = Self::ellipsize(text, label, px_small, btn_w - 10.0 * s);
                let lw = text.measure(&lab, px_small);
                text.layout_to(
                    &mut frame.content,
                    &lab,
                    px_small,
                    r.x + (r.w - lw) * 0.5,
                    by + bh * 0.5 + (a3 - d3) * 0.5,
                    fade(fg, pa),
                );
            }
            y += row_h;
        }

        y += gap;

        // --- nočný režim
        {
            let cy = y + row_h * 0.5;
            let label = "Nočný režim (4500 K)";
            text.layout_to(
                &mut frame.content,
                label,
                px,
                x0 + inset,
                base(cy),
                fade(t.fg, pa),
            );
            // switch vpravo
            let tw = 44.0 * s;
            let th = 24.0 * s;
            let tx = x0 + w_in - tw;
            let ty = cy - th * 0.5;
            let tog = Rect::new(tx, ty, tw, th);
            self.hits.push((tog, Hit::MonNight));
            let on = self.monitor.night;
            let track = if on { t.accent } else { t.fg };
            frame.content.push(Item::Rect {
                r: tog,
                radius: th * 0.5,
                soft: 0.8,
                color: fade(track, (if on { 1.0 } else { 0.3 }) * pa),
            });
            let kn = th - 6.0 * s;
            let kx = if on {
                tog.x + tog.w - 3.0 * s - kn
            } else {
                tog.x + 3.0 * s
            };
            frame.content.push(Item::Rect {
                r: Rect::new(kx, tog.y + 3.0 * s, kn, kn),
                radius: kn * 0.5,
                soft: 0.8,
                color: fade(if on { t.on_accent } else { t.fg }, pa),
            });
        }
    }

    fn build_wall(&mut self, frame: &mut Frame, text: &mut Text, pill: Rect, pa: f32) {
        let s = self.scale;
        let t = self.theme;
        let (tw, th) = self.cfg.wall.thumb_size();
        let (tw, th, gap, pad) = (tw * s, th * s, WALL_GAP * s, WALL_PAD * s);
        let cols = self.cfg.wall.columns;
        let n = self.wall.len();
        let px = self.px_font();
        let (asc, desc) = text.line_metrics(px);
        let base = |cy: f32| cy + (asc - desc) * 0.5;
        let inset = 12.0 * s;
        let x0 = pill.x + pad;
        let w_in = pill.w - 2.0 * pad;

        // ---- hlavička: tlačidlá + rollout s témou
        let head = WALL_HEAD * s;
        let hcy = pill.y + pad + head * 0.5;
        let bh = (px * 1.9).min(head);
        let rows = self.wall_rows_shown();
        let (rx, rw, bxs) = crate::wallpaper::header_layout(x0, w_in, self.cfg.wall.theme_width * s, self.cfg.wall.buttons.len(), self.cfg.wall.button_max_width.map(|w| w * s), gap);
        for (i, (bx, bw)) in bxs.iter().enumerate() {
            let r = Rect::new(*bx, hcy - bh * 0.5, *bw, bh);
            self.hits.push((r, Hit::WallButton(i)));
            let hov = self.hover == Some(Hit::WallButton(i));
            frame.content.push(Item::Rect { r, radius: bh * 0.5, soft: 0.8, color: if hov { fade(t.menu_selected, pa) } else { fade(t.fg, 0.12 * pa) } });
            let label = Self::ellipsize(text, &self.cfg.wall.buttons[i].label, px, bw - 2.0 * inset);
            let lw = text.measure(&label, px);
            text.layout_to(&mut frame.content, &label, px, r.x + (r.w - lw) * 0.5, base(hcy), fade(if hov { t.menu_selected_text } else { t.fg }, pa));
        }
        let rr = Rect::new(rx, hcy - bh * 0.5, rw, bh);
        self.hits.push((rr, Hit::WallRollout));
        let rhov = self.hover == Some(Hit::WallRollout);
        let rbg = if rhov || self.wall_menu { fade(t.accent, 0.25 * pa) } else { fade(t.fg, 0.12 * pa) };
        frame.content.push(Item::Rect { r: rr, radius: bh * 0.5, soft: 0.8, color: rbg });
        let chev = if self.wall_menu { "\u{f0143}" } else { "\u{f0140}" };
        let cw = text.measure(chev, px);
        text.layout_to(&mut frame.content, chev, px, rr.x + rr.w - inset - cw, base(hcy), fade(t.fg, pa));
        let cur = if self.wall_theme.is_empty() { "wallpaper" } else { self.wall_theme.as_str() };
        let cur = Self::ellipsize(text, cur, px, rw - 3.0 * inset - cw);
        text.layout_to(&mut frame.content, &cur, px, rr.x + inset, base(hcy), fade(t.fg, pa));

        // ---- oblasť pod hlavičkou: zoznam tém (rozbalený rollout) alebo mriežka náhľadov
        let top = pill.y + pad + head + gap;
        let area_h = rows as f32 * th + (rows - 1) as f32 * gap;

        if self.wall_menu {
            let row_h = px * 1.9;
            let vis = ((area_h / row_h).floor() as usize).max(1);
            let total = self.wall_themes.len() + 1; // + pseudotéma „wallpaper“
            let start = self.wall_menu_scroll.min(total.saturating_sub(vis));
            let sb = if total > vis { WALL_SCROLLBAR * s } else { 0.0 };
            for k in 0..vis.min(total) {
                let i = start + k;
                let name = if i == 0 { "wallpaper" } else { self.wall_themes[i - 1].0.as_str() };
                let sel = if i == 0 { self.wall_theme.is_empty() } else { self.wall_theme == name };
                let cell = Rect::new(x0, top + k as f32 * row_h, w_in - sb, row_h);
                self.hits.push((cell, Hit::WallTheme(i)));
                let hov = self.hover == Some(Hit::WallTheme(i));
                let fg = if hov {
                    frame.content.push(Item::Rect { r: cell, radius: 10.0 * s, soft: 0.8, color: fade(t.menu_selected, pa) });
                    t.menu_selected_text
                } else if sel {
                    frame.content.push(Item::Rect { r: cell, radius: 10.0 * s, soft: 0.8, color: fade(t.accent, 0.18 * pa) });
                    t.accent
                } else {
                    t.fg
                };
                let mark = if sel { "\u{f012c}" } else { "" };
                let mw = if sel { text.measure(mark, px) } else { 0.0 };
                if sel {
                    text.layout_to(&mut frame.content, mark, px, cell.x + cell.w - inset - mw, base(cell.y + row_h * 0.5), fade(fg, pa));
                }
                let label = Self::ellipsize(text, name, px, cell.w - 3.0 * inset - mw);
                text.layout_to(&mut frame.content, &label, px, cell.x + inset, base(cell.y + row_h * 0.5), fade(fg, pa));
            }
            if total > vis {
                let track_h = vis as f32 * row_h;
                let thumb_h = (track_h * vis as f32 / total as f32).max(16.0 * s).min(track_h);
                let frac = start as f32 / (total - vis) as f32;
                let w = 4.0 * s;
                let x = x0 + w_in - sb + (sb - w) * 0.5;
                frame.content.push(Item::Rect { r: Rect::new(x, top + (track_h - thumb_h) * frac, w, thumb_h), radius: w * 0.5, soft: 0.8, color: fade(t.fg, 0.35 * pa) });
            }
            return;
        }

        if n == 0 {
            let msg = if self.wall_loaded { "Žiadne obrázky" } else { "Načítavam…" };
            let mw = text.measure(msg, px);
            let cy = top + area_h * 0.5;
            text.layout_to(&mut frame.content, msg, px, pill.x + (pill.w - mw) * 0.5, base(cy), fade(t.fg, 0.7 * pa));
            return;
        }

        let total_rows = self.wall_row_count();
        let scrollable = total_rows > rows;
        let start = self.wall_scroll.min(total_rows - rows);

        for r in 0..rows {
            for c in 0..cols {
                let i = (start + r) * cols + c;
                if i >= n {
                    break;
                }
                let cell = Rect::new(x0 + c as f32 * (tw + gap), top + r as f32 * (th + gap), tw, th);
                self.hits.push((cell, Hit::WallItem(i)));
                let e = &self.wall[i];
                let hov = self.hover == Some(Hit::WallItem(i));
                let current = !self.wall_current.is_empty() && e.path == self.wall_current;
                // rámik okolo náhľadu: zvýraznenie pod kurzorom / aktuálne pozadie
                if hov || current {
                    let ring = if hov { t.menu_selected } else { t.accent };
                    let g = 3.0 * s;
                    let outer = Rect::new(cell.x - g, cell.y - g, cell.w + 2.0 * g, cell.h + 2.0 * g);
                    frame.content.push(Item::Rect { r: outer, radius: (crate::wallpaper::THUMB_RADIUS + g / s) * s, soft: 0.8, color: fade(ring, (if hov { 1.0 } else { 0.8 }) * pa) });
                }
                if e.ready {
                    frame.content.push(Item::Image { r: cell, path: e.thumb.clone(), color: [1.0, 1.0, 1.0, pa] });
                } else {
                    frame.content.push(Item::Rect { r: cell, radius: crate::wallpaper::THUMB_RADIUS * s, soft: 0.8, color: fade(t.fg, 0.12 * pa) });
                }
            }
        }

        if scrollable {
            let track_h = rows as f32 * th + (rows - 1) as f32 * gap;
            let thumb_h = (track_h * rows as f32 / total_rows as f32).max(16.0 * s).min(track_h);
            let frac = start as f32 / (total_rows - rows) as f32;
            let w = 4.0 * s;
            let x = pill.x + pill.w - pad - (WALL_SCROLLBAR * s - w) * 0.5 - w;
            let y = top + (track_h - thumb_h) * frac;
            frame.content.push(Item::Rect { r: Rect::new(x, y, w, thumb_h), radius: w * 0.5, soft: 0.8, color: fade(t.fg, 0.35 * pa) });
        }
    }

    fn build_disks(&mut self, frame: &mut Frame, text: &mut Text, pill: Rect, pa: f32) {
        let s = self.scale;
        let t = self.theme;
        let px = self.px_font();
        let px_s = px * 0.8;
        let (asc, desc) = text.line_metrics(px);
        let (asc_s, desc_s) = text.line_metrics(px_s);
        let pad = 16.0 * s;
        let head_cy = pill.y + DISK_HEAD * s * 0.5 + 2.0 * s;
        let title = "Disky";
        text.layout_to(&mut frame.content, title, px, pill.x + pad, head_cy + (asc - desc) * 0.5, fade(t.fg, pa));
        let disks = self.data.disks.clone();
        if disks.len() > 1 {
            let all = "Odpojiť všetky";
            let aw = text.measure(all, px_s);
            let ax = pill.x + pill.w - pad - aw;
            let hov = self.hover == Some(Hit::DiskAll);
            text.layout_to(&mut frame.content, all, px_s, ax, head_cy + (asc_s - desc_s) * 0.5, fade(t.accent, (if hov { 1.0 } else { 0.7 }) * pa));
            self.hits.push((Rect::new(ax - 6.0 * s, head_cy - 14.0 * s, aw + 12.0 * s, 28.0 * s), Hit::DiskAll));
        }
        let eject_icon = "\u{f052}";
        let iw = text.measure(eject_icon, px);
        for (k, d) in disks.iter().enumerate() {
            let top = pill.y + (DISK_HEAD + k as f32 * DISK_ROW) * s;
            let row = Rect::new(pill.x + 8.0 * s, top, pill.w - 16.0 * s, DISK_ROW * s - 4.0 * s);
            let hov = self.hover == Some(Hit::Disk(k));
            if hov {
                frame.content.push(Item::Rect { r: row, radius: 12.0 * s, soft: 0.8, color: fade(t.fg, 0.10 * pa) });
            }
            self.hits.push((row, Hit::Disk(k)));
            let maxw = pill.w - 2.0 * pad - iw - 16.0 * s;
            let name = Self::ellipsize(text, &disk_title(d), px, maxw);
            let sub = Self::ellipsize(text, &d.mount, px_s, maxw);
            text.layout_to(&mut frame.content, &name, px, pill.x + pad, top + 16.0 * s + (asc - desc) * 0.5, fade(t.fg, pa));
            text.layout_to(&mut frame.content, &sub, px_s, pill.x + pad, top + 35.0 * s + (asc_s - desc_s) * 0.5 - 2.0 * s, fade(t.fg, 0.55 * pa));
            let cy = top + (DISK_ROW * s - 4.0 * s) * 0.5;
            text.layout_to(&mut frame.content, eject_icon, px, pill.x + pill.w - pad - iw, cy + (asc - desc) * 0.5, fade(t.accent, (if hov { 1.0 } else { 0.75 }) * pa));
        }
    }

    fn build_calendar(&mut self, frame: &mut Frame, text: &mut Text, pill: Rect, top: f32, pa: f32) {
        self.rebuild_cal();
        let s = self.scale;
        let t = self.theme;
        let px = self.px_font();
        let px_s = px * 0.9;
        let (cw, _) = self.cal_size();
        let x0 = pill.x + (pill.w - cw) * 0.5 + CAL_PAD * s;
        let cell = CAL_CELL * s;
        let (asc, desc) = text.line_metrics(px);
        let (asc_s, desc_s) = text.line_metrics(px_s);

        // hlavička: mesiac a rok vycentrované (mesiace sa prepínajú kolieskom myši)
        let head_cy = top + CAL_HEAD * s * 0.5 + 2.0 * s;
        let tw = text.measure(&self.cal_title, px);
        let title_x = x0 + (8.0 * cell - tw) * 0.5;
        text.layout_to(&mut frame.content, &self.cal_title, px, title_x, head_cy + (asc - desc) * 0.5, fade(t.fg, pa));

        // názvy dní
        let names_cy = top + CAL_HEAD * s + CAL_NAMES * s * 0.5;
        for (d, n) in self.cal_names.clone().iter().enumerate() {
            let w = text.measure(n, px_s);
            let cx = x0 + (d as f32 + 1.5) * cell;
            text.layout_to(&mut frame.content, n, px_s, cx - w * 0.5, names_cy + (asc_s - desc_s) * 0.5, fade(t.fg, 0.55 * pa));
        }

        // mriežka
        let grid_top = top + (CAL_HEAD + CAL_NAMES) * s;
        let row_h = CAL_ROW * s;
        for r in 0..6 {
            let cy = grid_top + (r as f32 + 0.5) * row_h;
            let wk = self.cal_weeks.get(r).cloned().unwrap_or_default();
            let w = text.measure(&wk, px_s);
            text.layout_to(&mut frame.content, &wk, px_s, x0 + cell * 0.5 - w * 0.5, cy + (asc_s - desc_s) * 0.5, fade(t.fg, 0.4 * pa));
            for c in 0..7 {
                let cell_data = self.cal_cells[r * 7 + c];
                let cx = x0 + (c as f32 + 1.5) * cell;
                let label = cell_data.mday.to_string();
                let w = text.measure(&label, px);
                let col = if cell_data.today {
                    let rr = Rect::new(cx - cell * 0.5 + 2.0 * s, cy - row_h * 0.5 + 2.0 * s, cell - 4.0 * s, row_h - 4.0 * s);
                    frame.content.push(Item::Rect { r: rr, radius: 9.0 * s, soft: 0.8, color: fade(t.accent, pa) });
                    fade(t.on_accent, pa)
                } else if cell_data.other {
                    fade(t.fg, 0.35 * pa)
                } else {
                    fade(t.fg, pa)
                };
                text.layout_to(&mut frame.content, &label, px, cx - w * 0.5, cy + (asc - desc) * 0.5, col);
            }
        }
    }
}

impl Text {
    /// Rozloží reťazec priamo do zoznamu položiek.
    pub fn layout_to(&mut self, out: &mut Vec<Item>, s: &str, px: f32, x: f32, baseline: f32, color: Color) {
        let mut q: Vec<GlyphQuad> = Vec::with_capacity(s.len());
        self.layout(s, px, x, baseline, color, &mut q);
        out.extend(q.into_iter().map(Item::Glyph));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn month_lengths() {
        assert_eq!(days_in_month(2024, 1), 29);
        assert_eq!(days_in_month(2026, 1), 28);
        assert_eq!(days_in_month(2026, 10), 30);
        assert_eq!(days_in_month(2100, 1), 28);
        assert_eq!(days_in_month(2000, 1), 29);
    }
    #[test]
    fn caps() {
        assert_eq!(capitalize("november 2026"), "November 2026");
        assert_eq!(capitalize(""), "");
    }
}
