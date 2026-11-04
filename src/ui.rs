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
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Panel {
    None,
    Calendar,
    Disks,
    Menu(usize),
    Tip,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    Item(usize),
    Notif(usize),
    Disk(usize),
    DiskAll,
    MenuItem(usize),
    Panel,
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
            Msg::Net(n) => self.data.net = n,
            Msg::Vpn(v) => self.data.vpn = v,
            Msg::Layout(l) => self.data.layout = l,
            Msg::Audio { muted, volume } => {
                let changed = self.data.audio_known && (self.data.muted != muted || (self.data.volume - volume).abs() > 0.004);
                self.data.muted = muted;
                self.data.volume = volume;
                self.data.audio_known = true;
                if changed && self.cfg.audio_hud {
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
            let clickable = it.badge.is_none() && !(it.btn.click.is_empty() && it.btn.right.is_empty());
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
            _ => {
                let w = 8.0 * CAL_CELL + 2.0 * CAL_PAD;
                let h = CAL_HEAD + CAL_NAMES + 6.0 * CAL_ROW + CAL_BOTTOM;
                (w * self.scale, h * self.scale)
            }
        }
    }

    /// Logické → fyzické rozmery mriežky menu.
    fn menu_size(&self, mi: usize) -> (f32, f32) {
        let Some(m) = self.cfg.menus.get(mi) else {
            return (self.s(120.0), self.s(80.0));
        };
        let cols = m.columns.max(1);
        let n = m.items.len();
        let rows = m.rows.unwrap_or_else(|| ((n + cols - 1) / cols).max(1));
        let title_h = if m.title.is_empty() { 0.0 } else { 36.0 };
        let (cell_w, cell_h) = match m.kind {
            MenuKind::Icons => (m.icon_size + 16.0, m.icon_size + 16.0),
            MenuKind::Text => (m.cell_width, m.text_size + 18.0),
        };
        let w = m.padding * 2.0 + cols as f32 * cell_w + (cols.saturating_sub(1)) as f32 * m.gap;
        let h = m.padding * 2.0 + title_h + rows as f32 * cell_h + (rows.saturating_sub(1)) as f32 * m.gap;
        (w * self.scale, h * self.scale)
    }

    fn set_panel(&mut self, p: Panel) {
        self.panel = p;
        if p == Panel::None {
            self.grace = None;
        }
        if matches!(p, Panel::Calendar | Panel::Disks | Panel::Menu(_)) {
            self.big = p;
        }
        self.cal_t.set(if matches!(p, Panel::Calendar | Panel::Disks | Panel::Menu(_)) { 1.0 } else { 0.0 });
        match p {
            Panel::None => {
                self.panel_h.set(0.0);
                self.panel_w.set(0.0);
                self.panel_a.set(0.0);
            }
            Panel::Calendar | Panel::Disks | Panel::Menu(_) => {
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
        matches!(self.panel, Panel::Calendar | Panel::Disks | Panel::Menu(_))
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

    /// Otvorené menu má `autoclose = true` (zatvára sa aj odchodom myši).
    fn menu_autoclose(&self) -> bool {
        match self.panel {
            Panel::Menu(mi) => self.cfg.menus.get(mi).map(|m| m.autoclose).unwrap_or(false),
            _ => false,
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
        [self.hud_until, self.collapse_at, self.tip_pending.map(|t| t.1), self.notifs.iter().filter_map(|n| n.expires).min()].into_iter().flatten().min()
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
        match self.panel {
            // menu ostáva otvorené po odchode myši (zatvára sa klikom na položku alebo mimo),
            // pokiaľ nemá `autoclose = true` – vtedy sa zavrie ako kalendár
            Panel::Menu(mi) => {
                if self.cfg.menus.get(mi).map(|m| m.autoclose).unwrap_or(false) {
                    self.collapse_at = Some(now + Duration::from_millis(self.cfg.collapse_ms));
                }
            }
            Panel::Calendar | Panel::Disks => {
                self.collapse_at = Some(now + Duration::from_millis(self.cfg.collapse_ms))
            }
            Panel::Tip => self.collapse_at = Some(now + Duration::from_millis(200)),
            Panel::None => {}
        }
    }

    fn action_for(cmd: &str) -> Option<Action> {
        match cmd {
            "" | "calendar" | "menu" => None,
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
        let mut out = Vec::new();
        let Some((x, y)) = self.ptr else { return out };

        // custom menu: zatvorí sa klikom na položku alebo mimo (nie odchodom myši)
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
                // položka aj klik mimo → zatvoriť; pri `autoclose` je klik na okraj/nadpis
                // v paneli ešte „v menu“, takže ho nezatvára (zavrie ho položka alebo odchod myši)
                let on_item = matches!(hit, Some(Hit::MenuItem(_)));
                if on_item || !self.menu_autoclose() {
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
                0x112 => {
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
        if self.panel == Panel::Calendar && self.hover.is_some() {
            self.cal_shift(if value > 0.0 { 1 } else { -1 });
        }
    }

    /// Oblasť, ktorá prijíma vstup (logické px) – zvyšok povrchu je priehľadný pre myš.
    pub fn input_rect(&self) -> Option<(i32, i32, i32, i32)> {
        if self.hide.x > 0.92 || self.pill.w < 1.0 {
            return None;
        }
        let k = 1.0 / self.scale;
        // otvorené menu: celý povrch prijíma klik (zatvorenie klikom mimo); menu s `autoclose`
        // to nepotrebuje – vstup len na paneli, aby odchod myši vyvolal zatvorenie
        if matches!(self.panel, Panel::Menu(_)) && !self.menu_autoclose() {
            let w = (self.surf_w * k).ceil().max(1.0) as i32;
            let h = (self.surf_h * k).ceil().max(1.0) as i32;
            return Some((0, 0, w, h));
        }
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
                Panel::None | Panel::Calendar | Panel::Disks | Panel::Menu(_) => {}
            }
        }
        frame
    }

    fn build_menu(&mut self, frame: &mut Frame, text: &mut Text, pill: Rect, mi: usize, pa: f32) {
        let Some(menu) = self.cfg.menus.get(mi).cloned() else { return };
        let s = self.scale;
        let t = self.theme;
        let cols = menu.columns.max(1);
        let n = menu.items.len();
        let rows = menu.rows.unwrap_or_else(|| ((n + cols - 1) / cols).max(1));
        let pad = menu.padding * s;
        let gap = menu.gap * s;
        let title_h = if menu.title.is_empty() { 0.0 } else { 36.0 * s };
        let (cell_w, cell_h) = match menu.kind {
            MenuKind::Icons => ((menu.icon_size + 16.0) * s, (menu.icon_size + 16.0) * s),
            MenuKind::Text => (menu.cell_width * s, (menu.text_size + 18.0) * s),
        };
        let icon_px = menu.icon_size * s;
        let text_px = menu.text_size * s;
        let (asc_t, desc_t) = text.line_metrics(text_px);
        let (asc_h, desc_h) = text.line_metrics(self.px_font());

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
        let grid_left = pill.x + pad;
        let total = rows * cols;
        for i in 0..total {
            let r = i / cols;
            let c = i % cols;
            let x = grid_left + c as f32 * (cell_w + gap);
            let y = grid_top + r as f32 * (cell_h + gap);
            let cell = Rect::new(x, y, cell_w, cell_h);
            let item = menu.items.get(i);
            let hov = self.hover == Some(Hit::MenuItem(i));

            if item.is_some() {
                self.hits.push((cell, Hit::MenuItem(i)));
            }

            match menu.kind {
                MenuKind::Icons => {
                    if hov && item.is_some() {
                        frame.content.push(Item::Rect {
                            r: cell,
                            radius: 10.0 * s,
                            soft: 0.8,
                            color: fade(t.fg, 0.10 * pa),
                        });
                    }
                    if let Some(it) = item {
                        if !it.icon.is_empty() {
                            // PNG: názov súboru → config/icons/, alebo plná cesta
                            if let Some(icon) = self.icons.get(&it.icon) {
                                let max = icon_px;
                                let sc = (max / icon.w as f32).min(max / icon.h as f32);
                                let dw = icon.w as f32 * sc;
                                let dh = icon.h as f32 * sc;
                                let ix = x + (cell_w - dw) * 0.5;
                                let iy = y + (cell_h - dh) * 0.5;
                                frame.content.push(Item::Image {
                                    r: Rect::new(ix, iy, dw, dh),
                                    path: it.icon.clone(),
                                    color: [1.0, 1.0, 1.0, pa],
                                });
                            }
                        }
                    }
                }
                MenuKind::Text => {
                    let (bg_col, fg_col) = if hov && item.is_some() {
                        (fade(t.accent, pa), fade(t.on_accent, pa))
                    } else {
                        (fade(t.fg, 0.0), fade(t.fg, pa))
                    };
                    if hov && item.is_some() {
                        frame.content.push(Item::Rect {
                            r: cell,
                            radius: 10.0 * s,
                            soft: 0.8,
                            color: bg_col,
                        });
                    }
                    if let Some(it) = item {
                        let label = if it.label.is_empty() { &it.icon } else { &it.label };
                        if !label.is_empty() {
                            let maxw = cell_w - 12.0 * s;
                            let shown = Self::ellipsize(text, label, text_px, maxw);
                            let tw = text.measure(&shown, text_px);
                            let cx = x + (cell_w - tw) * 0.5;
                            let cy = y + cell_h * 0.5;
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
            }
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
