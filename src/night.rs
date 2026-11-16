//! Nočný režim: teplota farieb obrazovky cez `wlr-gamma-control-unstable-v1` (Wayfire, Sway, Hyprland…).
//!
//! * **ručne** – `dynamic-island --night on|off|toggle|get`, príkaz `night-toggle` v tlačidle / menu;
//! * **automaticky** (voliteľne) – podľa pevného času alebo východu / západu slnka z polohy, s plynulým prechodom;
//!   prepínač v paneli monitor potom zapína / vypína automatiku.
//!
//! Pri zapnutej automatike `toggle` (aj `on` / `off`) dočasne prebije jej hodnotu: `toggle` nastaví opak toho, čo
//! práve vidíš, a druhý `toggle` ho vráti. Ručný zásah platí do najbližšej zmeny automatiky (začiatok / koniec
//! noci), potom si automatika nastaví svoju hodnotu. Pri vypnutej automatike je `toggle` obyčajné ručné prepnutie.
//!
//! Vlákno má vlastné Wayland spojenie, zviaže všetky `wl_output` (aj pripojené neskôr) a mení gamma krivky.
//! Pri neutrálnej teplote ovládanie uvoľní, takže kompozitor vráti pôvodné krivky. Naraz môže gamma držať len
//! jeden klient – ak beží `wlsunset`/`gammastep`, nočný režim sa nezapne.
use crate::config::hud_dir;
use crate::data::Msg;
use crate::sys::{self, PollFd, Tx, Waker, POLLIN};
use crate::toml::{Table, TableExt};
use std::ffi::{c_char, c_int, c_uint};
use std::fs::File;
use std::io::{Seek, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::{Duration, Instant};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_wlr::gamma_control::v1::client::{
    zwlr_gamma_control_manager_v1 as manager, zwlr_gamma_control_v1 as control,
};

/// Neutrálna teplota (biela bez posunu), kelvin.
const NEUTRAL_K: f64 = 6500.0;
/// Interval snímok rýchleho prechodu (farby netreba viac ako ~30 fps).
const FRAME: Duration = Duration::from_millis(33);
/// Ako často sa prepočíta plán, keď beží automatika (pomalý prechod okolo západu stačí po pár sekundách).
const SCHED_TICK: Duration = Duration::from_secs(5);
/// Horná medza veľkosti gamma tabuľky (ochrana pred nezmyselnou hodnotou; Intel hlási aj 262145).
const MAX_RAMP: u32 = 1 << 20;
const DAY_MIN: f64 = 1440.0;

/// Plán automatiky.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Schedule {
    /// noc od západu do východu slnka (poloha z `latitude` / `longitude`)
    Sun { lat: f64, lon: f64 },
    /// noc od `start` do `end` (minúty od polnoci, môže prechádzať cez polnoc)
    Fixed { start: f64, end: f64 },
}

/// Nastavenia `[night]`.
#[derive(Clone, Debug, PartialEq)]
pub struct NightCfg {
    /// zapnúť natívny nočný režim (inak sa používa `night_switcher` z `[monitor]`)
    pub enabled: bool,
    /// teplota v nočnom režime, kelvin
    pub temperature: u32,
    /// trvanie rýchleho prechodu pri ručnom prepnutí, ms (0 = naraz)
    pub fade_ms: u64,
    /// po štarte (bez zapamätaného stavu) zapnúť ručne nočný režim
    pub start_on: bool,
    /// plán automatiky (`None` = len ručné prepínanie)
    pub schedule: Option<Schedule>,
    /// trvanie plynulého prechodu pri západe / východe, minúty
    pub transition_min: f64,
    /// automatika je po štarte (bez zapamätaného stavu) zapnutá
    pub auto: bool,
}

/// `HH:MM` → minúty od polnoci.
pub fn parse_hm(s: &str) -> Option<f64> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
    (h < 24 && m < 60).then(|| (h * 60 + m) as f64)
}

impl NightCfg {
    pub fn from_table(t: &Table) -> NightCfg {
        let schedule = match t.str_or("schedule", "none").trim() {
            "sun" => {
                let lat = t.f64_or("latitude", 0.0).clamp(-90.0, 90.0);
                let lon = t.f64_or("longitude", 0.0).clamp(-180.0, 180.0);
                if lat == 0.0 && lon == 0.0 {
                    eprintln!("dynamic-island: [night] schedule = \"sun\" potrebuje latitude a longitude");
                }
                Some(Schedule::Sun { lat, lon })
            }
            "fixed" => match (parse_hm(&t.str_or("start", "20:00")), parse_hm(&t.str_or("end", "07:00"))) {
                (Some(start), Some(end)) if start != end => Some(Schedule::Fixed { start, end }),
                _ => {
                    eprintln!("dynamic-island: [night] start / end musia byť rôzne časy HH:MM, automatika sa nepoužije");
                    None
                }
            },
            _ => None,
        };
        NightCfg {
            enabled: t.bool_or("enabled", false),
            temperature: t.f64_or("temperature", 3500.0).clamp(1000.0, 6500.0) as u32,
            fade_ms: t.f64_or("fade_ms", 1500.0).clamp(0.0, 30000.0) as u64,
            start_on: t.bool_or("start_on", false),
            schedule,
            transition_min: t.f64_or("transition_minutes", 30.0).clamp(0.0, 240.0),
            auto: t.bool_or("auto", true),
        }
    }
}

/// Povel nočnému režimu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NightCmd {
    /// `on` / `off`: pri zapnutej automatike dočasný zásah, inak ručný stav
    Set(bool),
    /// opak toho, čo je práve vidieť
    Toggle,
    /// prepínač v paneli monitor: zapne / vypne automatiku (bez plánu = ručný stav)
    Panel(bool),
}

impl NightCmd {
    /// `on` / `off` / `toggle` (aj `1`/`0`).
    pub fn parse(arg: &str) -> Option<NightCmd> {
        match arg.trim() {
            "on" | "1" | "true" => Some(NightCmd::Set(true)),
            "off" | "0" | "false" => Some(NightCmd::Set(false)),
            "toggle" => Some(NightCmd::Toggle),
            _ => None,
        }
    }
}

// ------------------------------------------------------------------ farby
/// Biela pre `kelvin` ako násobiteľ každého kanála v 0..=1 (6500 K = presne biela).
/// Približný model čierneho telesa (Tanner Helland), normalizovaný na neutrálnu teplotu.
pub fn white_point(kelvin: f64) -> [f64; 3] {
    fn raw(k: f64) -> [f64; 3] {
        let t = k.clamp(1000.0, 40000.0) / 100.0;
        let r = if t <= 66.0 { 255.0 } else { 329.698_727_446 * (t - 60.0).powf(-0.133_204_759_2) };
        let g = if t <= 66.0 {
            99.470_802_586_1 * t.ln() - 161.119_568_166_1
        } else {
            288.122_169_528_3 * (t - 60.0).powf(-0.075_514_849_2)
        };
        let b = if t >= 66.0 {
            255.0
        } else if t <= 19.0 {
            0.0
        } else {
            138.517_731_223_1 * (t - 10.0).ln() - 305.044_792_730_7
        };
        [r, g, b].map(|c| c.clamp(0.0, 255.0))
    }
    let (c, n) = (raw(kelvin), raw(NEUTRAL_K));
    [0, 1, 2].map(|i| (c[i] / n[i].max(1.0)).clamp(0.0, 1.0))
}

/// Teplota v čase `t` (0..=1) prechodu z `from` do `to`; interpoluje sa v miredoch (rovnomernejšie pre oko).
pub fn lerp_kelvin(from: f64, to: f64, t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    let (a, b) = (1.0e6 / from.max(1.0), 1.0e6 / to.max(1.0));
    1.0e6 / (a + (b - a) * t)
}

/// Gamma tabuľka v rozložení wlr-gamma-control: najprv všetky červené, potom zelené, potom modré (`u16`, natívne poradie).
pub fn fill_ramp(size: usize, kelvin: f64, out: &mut Vec<u8>) {
    out.clear();
    let last = size.saturating_sub(1).max(1) as f64;
    for scale in white_point(kelvin) {
        for i in 0..size {
            let v = ((i as f64 / last) * scale).clamp(0.0, 1.0);
            out.extend_from_slice(&((v * u16::MAX as f64).round() as u16).to_ne_bytes());
        }
    }
}

struct Fade {
    from: f64,
    to: f64,
    start: Instant,
    dur: Duration,
}

impl Fade {
    /// Teplota v čase `now` a či prechod skončil.
    fn value(&self, now: Instant) -> (f64, bool) {
        if self.dur.is_zero() {
            return (self.to, true);
        }
        let t = now.saturating_duration_since(self.start).as_secs_f64() / self.dur.as_secs_f64();
        if t >= 1.0 {
            (self.to, true)
        } else {
            (lerp_kelvin(self.from, self.to, ease(t)), false)
        }
    }
}

// ------------------------------------------------------------------ plán
/// Východ a západ slnka (UTC minúty od polnoci) pre polohu a deň v roku (1–366), alebo polárny deň / noc.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SunDay {
    Normal { rise: f64, set: f64 },
    PolarDay,
    PolarNight,
}

/// NOAA zjednodušený výpočet (presnosť ~ minúty, stačí na nočný režim).
pub fn sun_day(lat: f64, lon: f64, yday: f64) -> SunDay {
    let g = 2.0 * std::f64::consts::PI / 365.0 * (yday - 1.0);
    let eqtime = 229.18 * (0.000075 + 0.001868 * g.cos() - 0.032077 * g.sin() - 0.014615 * (2.0 * g).cos() - 0.040849 * (2.0 * g).sin());
    let decl = 0.006918 - 0.399912 * g.cos() + 0.070257 * g.sin() - 0.006758 * (2.0 * g).cos() + 0.000907 * (2.0 * g).sin()
        - 0.002697 * (3.0 * g).cos()
        + 0.00148 * (3.0 * g).sin();
    let la = lat.to_radians();
    let arg = 90.833_f64.to_radians().cos() / (la.cos() * decl.cos()) - la.tan() * decl.tan();
    if arg > 1.0 {
        return SunDay::PolarNight;
    }
    if arg < -1.0 {
        return SunDay::PolarDay;
    }
    let ha = arg.acos().to_degrees();
    SunDay::Normal { rise: 720.0 - 4.0 * (lon + ha) - eqtime, set: 720.0 - 4.0 * (lon - ha) - eqtime }
}

fn ease(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Miera noci (0 = deň, 1 = noc) pre noc od `start` do `end` (minúty od polnoci, cez polnoc) s prechodom
/// `fade` minút, ktorý začína o `start` a o `end`.
pub fn night_factor(now: f64, start: f64, end: f64, fade: f64) -> f64 {
    let wrap = |m: f64| m.rem_euclid(DAY_MIN);
    let night_len = wrap(end - start);
    if night_len <= 0.0 {
        return 0.0;
    }
    let fade = fade.max(0.0).min(night_len).min(DAY_MIN - night_len);
    let since_start = wrap(now - start);
    if since_start < night_len {
        return if fade > 0.0 { ease(since_start / fade) } else { 1.0 };
    }
    let since_end = wrap(now - end);
    if fade > 0.0 && since_end < fade {
        1.0 - ease(since_end / fade)
    } else {
        0.0
    }
}

/// Fáza plánu: ručný zásah platí do jej zmeny.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Day,
    Night,
    Fade,
}

/// Čas na výpočet plánu: miestne minúty od polnoci, deň v roku, posun od UTC v minútach.
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    pub minutes: f64,
    pub yday: f64,
    pub utc_offset_min: f64,
}

impl Clock {
    pub fn now() -> Clock {
        let tm = sys::local_tm(sys::now_unix());
        Clock {
            minutes: tm.tm_hour as f64 * 60.0 + tm.tm_min as f64 + tm.tm_sec as f64 / 60.0,
            yday: tm.tm_yday as f64 + 1.0,
            utc_offset_min: tm.tm_gmtoff as f64 / 60.0,
        }
    }
}

/// Miera noci podľa plánu a jej fáza.
pub fn schedule_factor(s: &Schedule, fade_min: f64, c: Clock) -> (f64, Phase) {
    let f = match *s {
        Schedule::Fixed { start, end } => night_factor(c.minutes, start, end, fade_min),
        Schedule::Sun { lat, lon } => match sun_day(lat, lon, c.yday) {
            SunDay::PolarDay => 0.0,
            SunDay::PolarNight => 1.0,
            SunDay::Normal { rise, set } => {
                let local = |utc: f64| (utc + c.utc_offset_min).rem_euclid(DAY_MIN);
                night_factor(c.minutes, local(set), local(rise), fade_min)
            }
        },
    };
    let phase = if f <= 0.0 {
        Phase::Day
    } else if f >= 1.0 {
        Phase::Night
    } else {
        Phase::Fade
    };
    (f, phase)
}

// ------------------------------------------------------------------ stav
/// Čo sa má zobraziť: ručný stav, automatika a dočasný zásah. Čistá logika (bez Waylandu), aby sa dala testovať.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Ctl {
    pub auto: bool,
    pub manual: bool,
    /// dočasný zásah pri automatike: (chcená noc, fáza plánu, v ktorej vznikol)
    pub over: Option<(bool, Phase)>,
}

impl Ctl {
    /// Miera noci, ktorá sa má zobraziť pri miere noci `f` podľa plánu.
    pub fn factor(&self, f: f64) -> f64 {
        if self.auto {
            match self.over {
                Some((night, _)) => night as u8 as f64,
                None => f,
            }
        } else {
            self.manual as u8 as f64
        }
    }

    /// Zásah končí, keď sa fáza plánu zmení (začiatok / koniec noci). Vráti `true`, ak ho zrušil.
    pub fn expire(&mut self, phase: Phase) -> bool {
        match self.over {
            Some((_, p)) if p != phase => {
                self.over = None;
                true
            }
            _ => false,
        }
    }

    /// Vykoná povel; `has_schedule` = je nastavený plán, `f` / `phase` = stav plánu teraz.
    pub fn apply(&mut self, cmd: NightCmd, has_schedule: bool, f: f64, phase: Phase) {
        match cmd {
            NightCmd::Toggle => {
                if self.auto {
                    let visible_night = self.factor(f) >= 0.5;
                    self.over = Some((!visible_night, phase));
                } else {
                    self.manual = !self.manual;
                }
            }
            NightCmd::Set(b) => {
                if self.auto {
                    self.over = Some((b, phase));
                } else {
                    self.manual = b;
                }
            }
            NightCmd::Panel(b) => {
                if has_schedule {
                    self.auto = b;
                    self.over = None;
                } else {
                    self.manual = b;
                }
            }
        }
    }

    /// Stav, ktorý ukazuje prepínač v paneli.
    pub fn switch(&self, has_schedule: bool) -> bool {
        if has_schedule {
            self.auto
        } else {
            self.manual
        }
    }
}

// ------------------------------------------------------------- ovládanie
struct Handle {
    tx: mpsc::Sender<NightCmd>,
    waker: Arc<Waker>,
    msgs: Tx<Msg>,
}

static HANDLE: OnceLock<Handle> = OnceLock::new();

fn state_file() -> std::path::PathBuf {
    hud_dir().join("night")
}

/// Zapíše stav: 1. riadok = čo je práve vidieť (`on`/`off`, pre `--night get`), potom `auto=` a `manual=`.
fn write_state(visible_night: bool, ctl: &Ctl) {
    let dir = hud_dir();
    let _ = std::fs::create_dir_all(&dir);
    let onoff = |b: bool| if b { "on" } else { "off" };
    let _ = std::fs::write(state_file(), format!("{}\nauto={}\nmanual={}\n", onoff(visible_night), onoff(ctl.auto), onoff(ctl.manual)));
}

/// Čo je práve vidieť (`on` = nočná teplota), podľa súboru bežiaceho ostrovčeka; pre `--night get`.
pub fn saved_state() -> Option<bool> {
    match std::fs::read_to_string(state_file()).ok()?.lines().next()?.trim() {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

/// Zapamätaný stav automatiky a ručného prepnutia (prežije reštart ostrovčeka po zmene konfigurácie).
fn saved_ctl() -> (Option<bool>, Option<bool>) {
    let s = std::fs::read_to_string(state_file()).unwrap_or_default();
    let get = |key: &str| {
        s.lines().find_map(|l| l.strip_prefix(key)).and_then(|v| match v.trim() {
            "on" => Some(true),
            "off" => Some(false),
            _ => None,
        })
    };
    (get("auto="), get("manual="))
}

/// Spustí vlákno nočného režimu (len ak je `[night] enabled`).
pub fn start(cfg: &NightCfg, msgs: &Tx<Msg>) {
    if !cfg.enabled || HANDLE.get().is_some() {
        return;
    }
    let (tx, rx) = mpsc::channel();
    let waker = Waker::new();
    let _ = HANDLE.set(Handle { tx, waker: waker.clone(), msgs: msgs.clone() });
    let (saved_auto, saved_manual) = saved_ctl();
    let ctl = Ctl {
        auto: cfg.schedule.is_some() && saved_auto.unwrap_or(cfg.auto),
        manual: saved_manual.unwrap_or(cfg.start_on),
        over: None,
    };
    let (c, m) = (cfg.clone(), msgs.clone());
    let _ = std::thread::Builder::new().name("night".into()).spawn(move || {
        if let Err(e) = run(rx, waker, c, ctl, m.clone()) {
            eprintln!("dynamic-island: nočný režim: {e}");
            write_state(false, &Ctl::default());
            m.send(Msg::Night(false));
        }
    });
}

/// Pošle povel nočnému režimu. Bez `[night] enabled` nerobí nič.
pub fn command(cmd: NightCmd) {
    let Some(h) = HANDLE.get() else { return };
    if h.tx.send(cmd).is_err() {
        // vlákno skončilo (kompozitor nepodporuje gamma, spojenie padlo) – prepínač sa vráti
        h.msgs.send(Msg::Night(false));
        return;
    }
    h.waker.wake();
}

// --------------------------------------------------------------- wayland
struct Out {
    global: u32,
    proxy: wl_output::WlOutput,
    control: Option<control::ZwlrGammaControlV1>,
    /// počet položiek tabuľky na kanál (kým ho kompozitor nenahlási, je `None`)
    size: Option<u32>,
    /// gamma tohto výstupu drží iný program
    busy: bool,
}

struct State {
    outs: Vec<Out>,
    manager: Option<manager::ZwlrGammaControlManagerV1>,
    dirty: bool,
}

impl State {
    fn add(&mut self, registry: &wl_registry::WlRegistry, global: u32, qh: &QueueHandle<Self>) {
        let proxy: wl_output::WlOutput = registry.bind(global, 1, qh, global);
        self.outs.push(Out { global, proxy, control: None, size: None, busy: false });
        self.dirty = true;
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        st: &mut Self,
        registry: &wl_registry::WlRegistry,
        ev: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match ev {
            wl_registry::Event::Global { name, interface, .. } if interface == wl_output::WlOutput::interface().name => {
                st.add(registry, name, qh)
            }
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(i) = st.outs.iter().position(|o| o.global == name) {
                    let o = st.outs.remove(i);
                    if let Some(c) = o.control {
                        c.destroy();
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for State {
    fn event(_: &mut Self, _: &wl_output::WlOutput, _: wl_output::Event, _: &u32, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<manager::ZwlrGammaControlManagerV1, ()> for State {
    fn event(_: &mut Self, _: &manager::ZwlrGammaControlManagerV1, _: manager::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<control::ZwlrGammaControlV1, u32> for State {
    fn event(st: &mut Self, proxy: &control::ZwlrGammaControlV1, ev: control::Event, global: &u32, _: &Connection, _: &QueueHandle<Self>) {
        let Some(o) = st.outs.iter_mut().find(|o| o.global == *global) else { return };
        match ev {
            control::Event::GammaSize { size } => {
                if size > 0 && size <= MAX_RAMP {
                    o.size = Some(size);
                    st.dirty = true;
                }
            }
            control::Event::Failed => {
                eprintln!("dynamic-island: nočný režim: gamma výstupu drží iný program (wlsunset, gammastep…)");
                proxy.destroy();
                o.control = None;
                o.size = None;
                o.busy = true;
                st.dirty = true;
            }
            _ => {}
        }
    }
}

extern "C" {
    fn memfd_create(name: *const c_char, flags: c_uint) -> c_int;
}
const MFD_CLOEXEC: c_uint = 1;

/// Zapíše tabuľku do memfd a pošle ju kompozitoru.
fn send_ramp(c: &control::ZwlrGammaControlV1, size: u32, kelvin: f64, scratch: &mut Vec<u8>) -> std::io::Result<()> {
    fill_ramp(size as usize, kelvin, scratch);
    let fd = unsafe { memfd_create(c"island-gamma".as_ptr(), MFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.write_all(scratch)?;
    file.rewind()?;
    c.set_gamma(file.as_fd());
    Ok(())
}

/// Pošle teplotu všetkým výstupom; pri neutrálnej ovládanie uvoľní (kompozitor obnoví pôvodné krivky).
fn apply(st: &mut State, kelvin: f64, neutral: bool, qh: &QueueHandle<State>, scratch: &mut Vec<u8>) {
    let Some(mgr) = st.manager.clone() else { return };
    for o in &mut st.outs {
        if neutral {
            if let Some(c) = o.control.take() {
                c.destroy();
            }
            o.size = None;
            o.busy = false;
            continue;
        }
        if o.busy {
            continue;
        }
        match (&o.control, o.size) {
            (None, _) => o.control = Some(mgr.get_gamma_control(&o.proxy, qh, o.global)),
            (Some(c), Some(size)) => {
                if let Err(e) = send_ramp(c, size, kelvin, scratch) {
                    eprintln!("dynamic-island: nočný režim: {e}");
                }
            }
            _ => {}
        }
    }
}

fn run(rx: mpsc::Receiver<NightCmd>, waker: Arc<Waker>, cfg: NightCfg, mut ctl: Ctl, msgs: Tx<Msg>) -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("Wayland: {e}"))?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(|e| format!("Wayland: {e}"))?;
    let qh = queue.handle();
    let mgr: manager::ZwlrGammaControlManagerV1 = globals
        .bind(&qh, 1..=1, ())
        .map_err(|_| "kompozitor nepodporuje wlr-gamma-control".to_string())?;
    let mut st = State { outs: Vec::new(), manager: Some(mgr), dirty: false };
    let existing: Vec<u32> = globals
        .contents()
        .with_list(|l| l.iter().filter(|g| g.interface == wl_output::WlOutput::interface().name).map(|g| g.name).collect());
    for g in existing {
        st.add(globals.registry(), g, &qh);
    }
    queue.roundtrip(&mut st).map_err(|e| format!("Wayland: {e}"))?;

    let has_schedule = cfg.schedule.is_some();
    let night_k = cfg.temperature as f64;
    let dur = Duration::from_millis(cfg.fade_ms);
    let sched = |c: Clock| match &cfg.schedule {
        Some(s) => schedule_factor(s, cfg.transition_min, c),
        None => (0.0, Phase::Day),
    };

    let mut cur = NEUTRAL_K;
    let mut fade: Option<Fade> = None;
    let mut scratch: Vec<u8> = Vec::new();
    let mut jump = true; // prvý priebeh: prejsť na zapamätaný stav
    let mut published: Option<(bool, Ctl)> = None;
    loop {
        let (f, phase) = sched(Clock::now());
        let mut changed = false;
        while let Ok(cmd) = rx.try_recv() {
            ctl.apply(cmd, has_schedule, f, phase);
            changed = true;
        }
        if ctl.expire(phase) {
            changed = true;
        }
        let want_f = ctl.factor(f);
        let want_k = lerp_kelvin(NEUTRAL_K, night_k, want_f);
        if changed || jump {
            jump = false;
            fade = Some(Fade { from: cur, to: want_k, start: Instant::now(), dur });
        } else if fade.is_none() && (want_k - cur).abs() > 0.5 {
            // pomalý prechod plánu: teplota sa len posunie
            cur = want_k;
            st.dirty = true;
        }
        if let Some(fd) = &fade {
            let (v, done) = fd.value(Instant::now());
            if (v - cur).abs() > 0.01 {
                cur = v;
                st.dirty = true;
            }
            if done {
                fade = None;
                st.dirty = true;
            }
        }
        // stav pre `--night get` a prepínač v paneli
        let visible = want_f >= 0.5;
        if published != Some((visible, ctl)) {
            published = Some((visible, ctl));
            write_state(visible, &ctl);
            msgs.send(Msg::Night(ctl.switch(has_schedule)));
        }
        if st.dirty {
            st.dirty = false;
            let neutral = fade.is_none() && (cur - NEUTRAL_K).abs() < 0.5;
            apply(&mut st, cur, neutral, &qh, &mut scratch);
            // všetky výstupy má obsadené iný program – nočný režim sa vráti, nech prepínač neklame
            if !neutral && fade.is_none() && !st.outs.is_empty() && st.outs.iter().all(|o| o.busy) {
                ctl = Ctl::default();
                jump = true;
                continue;
            }
        }
        conn.flush().map_err(|e| format!("Wayland: {e}"))?;
        queue.dispatch_pending(&mut st).map_err(|e| format!("Wayland: {e}"))?;
        let Some(guard) = queue.prepare_read() else { continue };
        let timeout = if fade.is_some() {
            FRAME.as_millis() as i32
        } else if ctl.auto {
            SCHED_TICK.as_millis() as i32
        } else {
            -1
        };
        let mut fds = [
            PollFd { fd: guard.connection_fd().as_raw_fd(), events: POLLIN, revents: 0 },
            PollFd { fd: waker.fd(), events: POLLIN, revents: 0 },
        ];
        sys::poll_in(&mut fds, timeout);
        if fds[0].revents != 0 {
            if let Err(e) = guard.read() {
                if !matches!(&e, wayland_client::backend::WaylandError::Io(io) if io.kind() == std::io::ErrorKind::WouldBlock) {
                    return Err(format!("Wayland: {e}"));
                }
            }
        } else {
            drop(guard);
        }
        waker.drain();
        queue.dispatch_pending(&mut st).map_err(|e| format!("Wayland: {e}"))?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_is_white_and_warm_drops_blue_first() {
        assert_eq!(white_point(NEUTRAL_K), [1.0, 1.0, 1.0]);
        let mut last_b = 1.0;
        for k in [5500.0, 4500.0, 3500.0, 2700.0, 1900.0] {
            let [r, g, b] = white_point(k);
            assert!((r - 1.0).abs() < 1e-9, "červená zostáva plná pri {k}: {r}");
            assert!(g >= b && b <= last_b, "{k}: {g} {b}");
            last_b = b;
        }
        let [_, g, b] = white_point(3500.0);
        assert!((0.6..0.95).contains(&g) && (0.3..0.8).contains(&b), "{g} {b}");
    }

    #[test]
    fn lerp_runs_in_mireds() {
        assert!((lerp_kelvin(6500.0, 3000.0, 0.0) - 6500.0).abs() < 1e-6);
        assert!((lerp_kelvin(6500.0, 3000.0, 1.0) - 3000.0).abs() < 1e-6);
        let mid = lerp_kelvin(6500.0, 3000.0, 0.5);
        // mired stred 6500 K a 3000 K ≈ 4105 K (obyčajný priemer by bol 4750)
        assert!((4050.0..4160.0).contains(&mid), "{mid}");
    }

    #[test]
    fn ramp_has_wlr_layout() {
        let mut out = Vec::new();
        fill_ramp(256, NEUTRAL_K, &mut out);
        assert_eq!(out.len(), 3 * 256 * 2);
        let at = |i: usize| u16::from_ne_bytes([out[2 * i], out[2 * i + 1]]);
        assert_eq!((at(0), at(255), at(256 + 255), at(512 + 255)), (0, u16::MAX, u16::MAX, u16::MAX));
        fill_ramp(256, 3000.0, &mut out);
        let at = |i: usize| u16::from_ne_bytes([out[2 * i], out[2 * i + 1]]);
        assert!(at(255) > at(256 + 255) && at(256 + 255) > at(512 + 255), "R > G > B v teplej farbe");
        fill_ramp(1, 3000.0, &mut out);
        assert_eq!(out.len(), 6, "aj tabuľka s jednou položkou je v poriadku");
    }

    #[test]
    fn fade_goes_from_start_to_end() {
        let t0 = Instant::now();
        let f = Fade { from: 6500.0, to: 3500.0, start: t0, dur: Duration::from_millis(1000) };
        let (v0, d0) = f.value(t0);
        assert!((v0 - 6500.0).abs() < 1.0 && !d0);
        let (vm, dm) = f.value(t0 + Duration::from_millis(500));
        assert!(vm < 6500.0 && vm > 3500.0 && !dm);
        let (v1, d1) = f.value(t0 + Duration::from_millis(1000));
        assert_eq!((v1, d1), (3500.0, true));
        let instant = Fade { from: 6500.0, to: 3500.0, start: t0, dur: Duration::ZERO };
        assert_eq!(instant.value(t0), (3500.0, true));
    }

    #[test]
    fn parses_commands_and_config() {
        assert_eq!(NightCmd::parse("on"), Some(NightCmd::Set(true)));
        assert_eq!(NightCmd::parse("off"), Some(NightCmd::Set(false)));
        assert_eq!(NightCmd::parse("toggle"), Some(NightCmd::Toggle));
        assert_eq!(NightCmd::parse("nieco"), None);
        let c = NightCfg::from_table(&Table::new());
        assert_eq!((c.enabled, c.temperature, c.fade_ms, c.start_on), (false, 3500, 1500, false));
        assert_eq!((c.schedule, c.transition_min, c.auto), (None, 30.0, true));
        assert_eq!(parse_hm("07:30"), Some(450.0));
        assert_eq!(parse_hm("24:00"), None);
        assert_eq!(parse_hm("7pm"), None);
    }

    #[test]
    fn config_reads_schedules() {
        let mut t = Table::new();
        t.insert("schedule".into(), crate::toml::Value::Str("fixed".into()));
        t.insert("start".into(), crate::toml::Value::Str("21:00".into()));
        t.insert("end".into(), crate::toml::Value::Str("06:30".into()));
        let c = NightCfg::from_table(&t);
        assert_eq!(c.schedule, Some(Schedule::Fixed { start: 1260.0, end: 390.0 }));
        let mut t = Table::new();
        t.insert("schedule".into(), crate::toml::Value::Str("sun".into()));
        t.insert("latitude".into(), crate::toml::Value::Num(48.15));
        t.insert("longitude".into(), crate::toml::Value::Num(17.11));
        assert_eq!(NightCfg::from_table(&t).schedule, Some(Schedule::Sun { lat: 48.15, lon: 17.11 }));
    }

    #[test]
    fn sun_times_are_plausible() {
        // Bratislava, 16. novembra (yday 320): východ ≈ 06:00 UTC, západ ≈ 15:10 UTC
        match sun_day(48.15, 17.11, 320.0) {
            SunDay::Normal { rise, set } => {
                assert!((330.0..390.0).contains(&rise), "východ {rise}");
                assert!((890.0..950.0).contains(&set), "západ {set}");
            }
            other => panic!("{other:?}"),
        }
        // leto je dlhší deň než zima
        let len = |y| match sun_day(48.15, 17.11, y) {
            SunDay::Normal { rise, set } => set - rise,
            _ => 0.0,
        };
        assert!(len(172.0) > len(355.0) + 300.0);
        // Svalbard: v decembri polárna noc, v júli polárny deň
        assert_eq!(sun_day(78.2, 15.6, 355.0), SunDay::PolarNight);
        assert_eq!(sun_day(78.2, 15.6, 182.0), SunDay::PolarDay);
    }

    #[test]
    fn night_factor_wraps_midnight_and_fades() {
        let (start, end, fade) = (20.0 * 60.0, 7.0 * 60.0, 30.0);
        assert_eq!(night_factor(12.0 * 60.0, start, end, fade), 0.0);
        assert_eq!(night_factor(23.0 * 60.0, start, end, fade), 1.0);
        assert_eq!(night_factor(3.0 * 60.0, start, end, fade), 1.0);
        let dusk = night_factor(20.0 * 60.0 + 15.0, start, end, fade);
        assert!((dusk - 0.5).abs() < 1e-3, "{dusk}");
        let dawn = night_factor(7.0 * 60.0 + 15.0, start, end, fade);
        assert!((dawn - 0.5).abs() < 1e-3, "{dawn}");
        assert_eq!(night_factor(7.0 * 60.0 + 31.0, start, end, fade), 0.0);
        assert_eq!(night_factor(20.0 * 60.0, start, end, 0.0), 1.0, "bez prechodu sa prepne naraz");
        assert_eq!(night_factor(13.0 * 60.0, 12.0 * 60.0, 14.0 * 60.0, 0.0), 1.0);
        assert_eq!(night_factor(15.0 * 60.0, 12.0 * 60.0, 14.0 * 60.0, 0.0), 0.0);
    }

    #[test]
    fn schedule_phases() {
        let s = Schedule::Fixed { start: 1200.0, end: 420.0 };
        let at = |m: f64| schedule_factor(&s, 30.0, Clock { minutes: m, yday: 1.0, utc_offset_min: 0.0 });
        assert_eq!(at(720.0), (0.0, Phase::Day));
        assert_eq!(at(1380.0), (1.0, Phase::Night));
        assert_eq!(at(1215.0).1, Phase::Fade);
    }

    #[test]
    fn toggle_without_auto_is_a_plain_manual_switch() {
        let mut c = Ctl::default();
        c.apply(NightCmd::Toggle, false, 0.0, Phase::Day);
        assert_eq!(c.factor(0.0), 1.0);
        c.apply(NightCmd::Toggle, false, 0.0, Phase::Day);
        assert_eq!(c.factor(0.0), 0.0);
        c.apply(NightCmd::Set(true), false, 0.0, Phase::Day);
        assert!(c.manual && c.over.is_none());
        assert!(c.switch(false) && !c.switch(true));
    }

    #[test]
    fn toggle_with_auto_flips_what_you_see_and_back() {
        // v noci: automatika 3500 K → prvý toggle 6500 K, druhý späť
        let mut c = Ctl { auto: true, ..Default::default() };
        c.apply(NightCmd::Toggle, true, 1.0, Phase::Night);
        assert_eq!(c.factor(1.0), 0.0);
        c.apply(NightCmd::Toggle, true, 1.0, Phase::Night);
        assert_eq!(c.factor(1.0), 1.0);
        // cez deň: automatika 6500 K → prvý toggle 3500 K, druhý späť
        let mut c = Ctl { auto: true, ..Default::default() };
        c.apply(NightCmd::Toggle, true, 0.0, Phase::Day);
        assert_eq!(c.factor(0.0), 1.0);
        c.apply(NightCmd::Toggle, true, 0.0, Phase::Day);
        assert_eq!(c.factor(0.0), 0.0);
        // automatika zostáva zapnutá
        assert!(c.auto);
    }

    #[test]
    fn override_ends_when_the_schedule_changes_phase() {
        let mut c = Ctl { auto: true, ..Default::default() };
        c.apply(NightCmd::Toggle, true, 0.0, Phase::Day); // cez deň zapnutý ručne
        assert!(!c.expire(Phase::Day), "pokým je deň, zásah platí");
        assert_eq!(c.factor(0.0), 1.0);
        assert!(c.expire(Phase::Fade), "so začiatkom západu ho automatika prevezme");
        assert_eq!(c.factor(0.5), 0.5);
        assert!(!c.expire(Phase::Night));
    }

    #[test]
    fn panel_switch_controls_auto_when_a_schedule_exists() {
        let mut c = Ctl { auto: true, over: Some((true, Phase::Day)), ..Default::default() };
        c.apply(NightCmd::Panel(false), true, 0.0, Phase::Day);
        assert!(!c.auto && c.over.is_none());
        assert_eq!(c.factor(1.0), 0.0, "bez automatiky platí ručný stav");
        c.apply(NightCmd::Panel(true), true, 1.0, Phase::Night);
        assert_eq!(c.factor(1.0), 1.0);
        // bez plánu prepínač nastavuje ručný stav
        let mut c = Ctl::default();
        c.apply(NightCmd::Panel(true), false, 0.0, Phase::Day);
        assert!(c.manual && !c.auto);
    }
}
