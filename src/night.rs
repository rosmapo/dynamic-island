//! Nočný režim: teplota farieb obrazovky cez `wlr-gamma-control-unstable-v1` (Wayfire, Sway, Hyprland…).
//! Prepína sa ručne (panel monitor, `dynamic-island --night on|off|toggle`), bez plánu podľa času / polohy.
//!
//! Vlákno má vlastné Wayland spojenie, zviaže všetky `wl_output` (aj pripojené neskôr) a pri prepnutí plynulo
//! prechádza medzi neutrálnou a nočnou teplotou. Pri vypnutí gamma ovládanie uvoľní, takže kompozitor vráti pôvodné
//! krivky (a uvoľní ich pre iné programy). Naraz môže gamma držať len jeden klient – ak beží `wlsunset`/`gammastep`,
//! nočný režim sa nezapne.
use crate::config::hud_dir;
use crate::data::Msg;
use crate::sys::{self, PollFd, Tx, Waker, POLLIN};
use crate::toml::{Table, TableExt};
use std::ffi::{c_char, c_int, c_uint};
use std::fs::File;
use std::io::{Seek, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd};
use std::sync::atomic::{AtomicBool, Ordering};
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
/// Interval snímok prechodu (farby netreba viac ako ~30 fps).
const FRAME: Duration = Duration::from_millis(33);
/// Horná medza veľkosti gamma tabuľky (ochrana pred nezmyselnou hodnotou; Intel hlási aj 262145).
const MAX_RAMP: u32 = 1 << 20;

/// Nastavenia `[night]`.
#[derive(Clone, Debug, PartialEq)]
pub struct NightCfg {
    /// zapnúť natívny nočný režim (inak sa používa `night_switcher` z `[monitor]`)
    pub enabled: bool,
    /// teplota v nočnom režime, kelvin
    pub temperature: u32,
    /// trvanie plynulého prechodu, ms (0 = naraz)
    pub fade_ms: u64,
    /// po štarte ostrovčeka zapnúť nočný režim
    pub start_on: bool,
}

impl NightCfg {
    pub fn from_table(t: &Table) -> NightCfg {
        NightCfg {
            enabled: t.bool_or("enabled", false),
            temperature: t.f64_or("temperature", 3500.0).clamp(1000.0, 6500.0) as u32,
            fade_ms: t.f64_or("fade_ms", 1500.0).clamp(0.0, 30000.0) as u64,
            start_on: t.bool_or("start_on", false),
        }
    }
}

/// Povel nočnému režimu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NightCmd {
    Set(bool),
    Toggle,
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

/// Plynulý náběh (smoothstep).
pub fn ease(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
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

// ------------------------------------------------------------- ovládanie
enum Cmd {
    Set(bool),
}

struct Handle {
    tx: mpsc::Sender<Cmd>,
    waker: Arc<Waker>,
    msgs: Tx<Msg>,
}

static HANDLE: OnceLock<Handle> = OnceLock::new();
/// Požadovaný stav (nie nutne dokončený prechod).
static ON: AtomicBool = AtomicBool::new(false);

fn state_file() -> std::path::PathBuf {
    hud_dir().join("night")
}

fn write_state(on: bool) {
    let dir = hud_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(state_file(), if on { "on\n" } else { "off\n" });
}

/// Stav zapamätaný bežiacim ostrovčekom (`on`/`off`), pre `--night get`.
pub fn saved_state() -> Option<bool> {
    match std::fs::read_to_string(state_file()).ok()?.trim() {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

/// Spustí vlákno nočného režimu (len ak je `[night] enabled`). Stav sa pamätá do odhlásenia, takže prežije aj
/// reštart ostrovčeka po zmene konfigurácie.
pub fn start(cfg: &NightCfg, msgs: &Tx<Msg>) {
    if !cfg.enabled || HANDLE.get().is_some() {
        return;
    }
    let (tx, rx) = mpsc::channel();
    let waker = Waker::new();
    let _ = HANDLE.set(Handle { tx, waker: waker.clone(), msgs: msgs.clone() });
    let (c, m) = (cfg.clone(), msgs.clone());
    let spawned = std::thread::Builder::new().name("night".into()).spawn(move || {
        if let Err(e) = run(rx, waker, c, m.clone()) {
            eprintln!("dynamic-island: nočný režim: {e}");
            ON.store(false, Ordering::SeqCst);
            write_state(false);
            m.send(Msg::Night(false));
        }
    });
    if spawned.is_err() {
        return;
    }
    if cfg.start_on || saved_state() == Some(true) {
        command(NightCmd::Set(true));
    } else {
        write_state(false);
    }
}

/// Zapne / vypne / prepne nočný režim. Bez `[night] enabled` nerobí nič.
pub fn command(cmd: NightCmd) {
    let Some(h) = HANDLE.get() else { return };
    let on = match cmd {
        NightCmd::Set(b) => b,
        NightCmd::Toggle => !ON.load(Ordering::SeqCst),
    };
    ON.store(on, Ordering::SeqCst);
    write_state(on);
    if h.tx.send(Cmd::Set(on)).is_err() {
        // vlákno skončilo (kompozitor nepodporuje gamma, spojenie padlo) – stav sa vráti
        ON.store(false, Ordering::SeqCst);
        write_state(false);
        h.msgs.send(Msg::Night(false));
        return;
    }
    h.waker.wake();
    h.msgs.send(Msg::Night(on));
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

fn run(rx: mpsc::Receiver<Cmd>, waker: Arc<Waker>, cfg: NightCfg, msgs: Tx<Msg>) -> Result<(), String> {
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

    let mut cur = NEUTRAL_K;
    let mut fade: Option<Fade> = None;
    let mut scratch: Vec<u8> = Vec::new();
    let night = cfg.temperature as f64;
    let dur = Duration::from_millis(cfg.fade_ms);
    loop {
        while let Ok(Cmd::Set(on)) = rx.try_recv() {
            let to = if on { night } else { NEUTRAL_K };
            fade = Some(Fade { from: cur, to, start: Instant::now(), dur });
        }
        if let Some(f) = &fade {
            let (v, done) = f.value(Instant::now());
            if (v - cur).abs() > 0.01 {
                cur = v;
                st.dirty = true;
            }
            if done {
                fade = None;
                st.dirty = true;
            }
        }
        if st.dirty {
            st.dirty = false;
            let neutral = fade.is_none() && (cur - NEUTRAL_K).abs() < 0.5;
            apply(&mut st, cur, neutral, &qh, &mut scratch);
            // žiadny výstup nedostal ovládanie (všetky obsadené) – stav sa vráti, nech prepínač neklame
            if !neutral && fade.is_none() && !st.outs.is_empty() && st.outs.iter().all(|o| o.busy) {
                ON.store(false, Ordering::SeqCst);
                write_state(false);
                msgs.send(Msg::Night(false));
                fade = Some(Fade { from: cur, to: NEUTRAL_K, start: Instant::now(), dur: Duration::ZERO });
                continue;
            }
        }
        conn.flush().map_err(|e| format!("Wayland: {e}"))?;
        queue.dispatch_pending(&mut st).map_err(|e| format!("Wayland: {e}"))?;
        let Some(guard) = queue.prepare_read() else { continue };
        let timeout = if fade.is_some() { FRAME.as_millis() as i32 } else { -1 };
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
    }
}
