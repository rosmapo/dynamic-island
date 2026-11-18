//! Klient Wayfire IPC (unix socket, 4B dĺžka + JSON) – rozloženie klávesnice a autohide.
use crate::config::AutoCfg;
use crate::data::Msg;
use crate::json::{self, Json};
use crate::sys::{self, PollFd, Tx, POLLIN};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// true, kým beží odber `keyboard-modifier-state-changed` (rozloženie a Caps Lock idú cez udalosti, netreba ich pollovať).
pub static KB_EVENTS: AtomicBool = AtomicBool::new(false);

pub struct Ipc {
    sock: UnixStream,
}

impl Ipc {
    pub fn connect() -> Option<Ipc> {
        let path = std::env::var("WAYFIRE_SOCKET").ok()?;
        let sock = UnixStream::connect(path).ok()?;
        let _ = sock.set_read_timeout(Some(Duration::from_secs(2)));
        let _ = sock.set_write_timeout(Some(Duration::from_secs(2)));
        Some(Ipc { sock })
    }

    pub fn send(&mut self, method: &str, data: &str) -> std::io::Result<()> {
        let body = format!("{{\"method\":{},\"data\":{}}}", json::quote(method), if data.is_empty() { "{}" } else { data });
        let mut msg = (body.len() as u32).to_ne_bytes().to_vec();
        msg.extend_from_slice(body.as_bytes());
        self.sock.write_all(&msg)
    }

    pub fn recv(&mut self) -> std::io::Result<Json> {
        let mut hdr = [0u8; 4];
        self.sock.read_exact(&mut hdr)?;
        let n = u32::from_ne_bytes(hdr) as usize;
        if n > 16 * 1024 * 1024 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "príliš veľká odpoveď"));
        }
        let mut buf = vec![0u8; n];
        self.sock.read_exact(&mut buf)?;
        json::parse(&String::from_utf8_lossy(&buf)).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "neplatný JSON"))
    }

    pub fn call(&mut self, method: &str, data: &str) -> std::io::Result<Json> {
        self.send(method, data)?;
        self.recv()
    }

    /// Čaká na dáta na sockete najviac `ms` milisekúnd.
    fn readable(&self, ms: i32) -> bool {
        let mut fds = [PollFd { fd: self.sock.as_raw_fd(), events: POLLIN, revents: 0 }];
        sys::poll_in(&mut fds, ms) > 0
    }
}

/// (názov rozloženia, index, počet)
pub fn keyboard_state(ipc: &mut Ipc) -> Option<(String, usize, usize)> {
    let j = ipc.call("wayfire/get-keyboard-state", "").ok()?;
    let layout = j.get("layout")?.as_str()?.to_string();
    let idx = j.get("layout-index").and_then(|v| v.as_f64()).unwrap_or(0.0) as usize;
    let n = j.get("possible-layouts").and_then(|v| v.as_arr()).map(|a| a.len()).unwrap_or(0);
    Some((layout, idx, n))
}

/// Prepne na ďalšie rozloženie (ako keyboard-layout.sh toggle).
pub fn toggle_layout() {
    if let Some(mut ipc) = Ipc::connect() {
        if let Some((_, idx, n)) = keyboard_state(&mut ipc) {
            if n > 0 {
                let _ = ipc.call("wayfire/set-keyboard-state", &format!("{{\"layout-index\":{}}}", (idx + 1) % n));
            }
        }
    }
}

pub fn short_layout(name: &str, map: &std::collections::HashMap<String, String>) -> String {
    match map.get(name) {
        Some(s) => s.clone(),
        None => name.chars().take(2).collect::<String>().to_uppercase(),
    }
}

// ---------------------------------------------------------------- odber udalostí

/// Prihlási spojenie na vybrané udalosti. Wayfire odmietne celý odber, ak niektorý názov nepozná
/// (`Event not found: "x"`), preto sa taký názov vyhodí a odber sa zopakuje (kompatibilita s inou verziou).
fn watch_events(ipc: &mut Ipc, events: &[&str]) -> Result<(), String> {
    let mut list: Vec<&str> = events.to_vec();
    for _ in 0..8 {
        let arr = list.iter().map(|e| json::quote(e)).collect::<Vec<_>>().join(",");
        let r = ipc.call("window-rules/events/watch", &format!("{{\"events\":[{arr}]}}")).map_err(|e| e.to_string())?;
        let Some(err) = r.get("error").and_then(|e| e.as_str()) else { return Ok(()) };
        // „Event not found: "názov"" -> odstráň názov a skús znova
        let bad = err.split('"').nth(1).map(str::to_string);
        match bad {
            Some(b) if err.starts_with("Event not found") && list.iter().any(|e| *e == b) => list.retain(|e| *e != b),
            _ => return Err(err.to_string()),
        }
    }
    Err("odber udalostí sa nepodarilo nastaviť".into())
}

// ---------------------------------------------------------------- rozloženie klávesnice a Caps Lock

/// Vlákno: rozloženie a Caps Lock podľa udalosti `keyboard-modifier-state-changed` (bez pollovania).
pub fn spawn_keyboard(names: HashMap<String, String>, tx: Tx<Msg>) {
    std::thread::spawn(move || loop {
        let r = keyboard_session(&names, &tx);
        KB_EVENTS.store(false, Ordering::Relaxed);
        if let Err(e) = r {
            if std::env::var_os("DI_DEBUG").is_some() {
                eprintln!("wayfire keyboard: {e}");
            }
        }
        std::thread::sleep(Duration::from_secs(3)); // Wayfire reštart / IPC nie je pripravené
    });
}

fn keyboard_session(names: &HashMap<String, String>, tx: &Tx<Msg>) -> Result<(), String> {
    let mut req = Ipc::connect().ok_or("WAYFIRE_SOCKET nie je dostupný")?;
    let mut watch = Ipc::connect().ok_or("druhé IPC spojenie zlyhalo")?;
    let _ = watch.sock.set_read_timeout(None); // čaká sa, kým príde udalosť
    watch_events(&mut watch, &["keyboard-modifier-state-changed"])?;

    let mut layout: Option<String> = None;
    let mut caps: Option<bool> = None;
    // Počiatočný stav.
    if let Some((name, _, _)) = keyboard_state(&mut req) {
        let s = short_layout(&name, names);
        layout = Some(s.clone());
        tx.send(Msg::Layout(s));
    }
    if let Some(c) = crate::providers::caps_led() {
        caps = Some(c);
        tx.send(Msg::Caps(c));
    }
    KB_EVENTS.store(true, Ordering::Relaxed);

    loop {
        let ev = watch.recv().map_err(|e| e.to_string())?;
        if ev.get("event").and_then(|e| e.as_str()) != Some("keyboard-modifier-state-changed") {
            continue;
        }
        if let Some(name) = ev.path(&["state", "layout"]).and_then(|l| l.as_str()) {
            let s = short_layout(name, names);
            if layout.as_ref() != Some(&s) {
                layout = Some(s.clone());
                tx.send(Msg::Layout(s));
            }
        }
        // LED sa po zmene modifikátora aktualizuje o chvíľu neskôr.
        std::thread::sleep(Duration::from_millis(30));
        if let Some(c) = crate::providers::caps_led() {
            if caps != Some(c) {
                caps = Some(c);
                tx.send(Msg::Caps(c));
            }
        }
    }
}

// ---------------------------------------------------------------- autohide

/// Udalosti, na ktoré sa autohide prihlási (Wayfire 0.11). Ostatné (napr. stlačenia kláves) vôbec nedostaneme.
const RELEVANT: &[&str] = &[
    "view-mapped",
    "view-unmapped",
    "view-tiled",
    "view-fullscreen",
    "output-added",
    "output-removed",
    "output-layout-changed",
    "view-minimized",
    "view-focused",
    "view-set-output",
    "view-geometry-changed",
    "view-wset-changed",
    "view-workspace-changed",
    "wset-workspace-changed",
    "output-gain-focus",
    "output-wset-changed",
    "view-sticky",
];

#[derive(Clone, Copy, Debug)]
struct Geo {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

fn geo(j: &Json) -> Option<Geo> {
    Some(Geo {
        x: j.get("x")?.as_f64()?,
        y: j.get("y")?.as_f64()?,
        w: j.get("width")?.as_f64()?,
        h: j.get("height")?.as_f64()?,
    })
}

// ---------------------------------------------------------------- snímanie obrazovky (modul capture)

/// Prienik obdĺžnika `g` (súradnice výstupu) s výstupom `out`, prepočítaný na globálne súradnice (celé px).
fn to_global(g: Geo, out: Geo) -> Option<(i32, i32, i32, i32)> {
    let x0 = (g.x + out.x).max(out.x);
    let y0 = (g.y + out.y).max(out.y);
    let x1 = (g.x + out.x + g.w).min(out.x + out.w);
    let y1 = (g.y + out.y + g.h).min(out.y + out.h);
    (x1 - x0 >= 1.0 && y1 - y0 >= 1.0).then(|| (x0.round() as i32, y0.round() as i32, (x1 - x0).round() as i32, (y1 - y0).round() as i32))
}

/// Názov aktívneho výstupu (pre `grim -o` / `wf-recorder -o`).
pub fn focused_output_name() -> Option<String> {
    let mut ipc = Ipc::connect()?;
    let j = ipc.call("window-rules/get-focused-output", "").ok()?;
    j.path(&["info", "name"]).or_else(|| j.get("name")).and_then(|n| n.as_str()).filter(|n| !n.is_empty()).map(str::to_string)
}

fn focused_output_geo(ipc: &mut Ipc) -> Option<Geo> {
    let j = ipc.call("window-rules/get-focused-output", "").ok()?;
    j.path(&["info", "geometry"]).or_else(|| j.get("geometry")).and_then(geo)
}

/// Geometria aktívneho okna v globálnych súradnicích (pre `grim -g`), orezaná na aktívny výstup.
pub fn focused_window_global() -> Option<(i32, i32, i32, i32)> {
    let mut ipc = Ipc::connect()?;
    let og = focused_output_geo(&mut ipc)?;
    let j = ipc.call("window-rules/get-focused-view", "").ok()?;
    let info = j.get("info").unwrap_or(&j);
    if matches!(info, Json::Null) {
        return None;
    }
    if info.get("type").and_then(|t| t.as_str()).map_or(false, |t| t != "toplevel") {
        return None;
    }
    if info.get("mapped").and_then(|t| t.as_bool()) == Some(false) {
        return None;
    }
    to_global(info.get("geometry").and_then(geo)?, og)
}

/// Viditeľné okná aktívneho výstupu v globálnych súradnicích (výber klikom, keď aktívne okno nie je známe).
pub fn window_rects_global() -> Vec<(i32, i32, i32, i32)> {
    let Some(mut ipc) = Ipc::connect() else { return Vec::new() };
    let Some(og) = focused_output_geo(&mut ipc) else { return Vec::new() };
    let Ok(views) = ipc.call("window-rules/list-views", "") else { return Vec::new() };
    let mut out = Vec::new();
    for v in views.as_arr().unwrap_or(&[]) {
        if v.get("type").and_then(|t| t.as_str()) != Some("toplevel") {
            continue;
        }
        if v.get("mapped").and_then(|t| t.as_bool()) != Some(true) || v.get("minimized").and_then(|t| t.as_bool()) == Some(true) {
            continue;
        }
        let Some(g) = v.get("geometry").and_then(geo) else { continue };
        let (cx, cy) = (g.x + g.w / 2.0, g.y + g.h / 2.0);
        let sticky = v.get("sticky").and_then(|s| s.as_bool()) == Some(true);
        if !(sticky || (cx >= 0.0 && cx < og.w && cy >= 0.0 && cy < og.h)) {
            continue;
        }
        if let Some(r) = to_global(g, og) {
            out.push(r);
        }
    }
    out
}

/// Zasahuje niektoré okno aktívneho workspace do pásma ostrovčeka?
fn obstructed(req: &mut Ipc, out: Geo, cfg: &AutoCfg) -> bool {
    let band = cfg.bar_height + cfg.margin;
    let views = match req.call("window-rules/list-views", "") {
        Ok(v) => v,
        Err(_) => return false,
    };
    let Some(list) = views.as_arr() else { return false };
    for v in list {
        if v.get("type").and_then(|t| t.as_str()) != Some("toplevel") {
            continue;
        }
        if v.get("mapped").and_then(|t| t.as_bool()) != Some(true) || v.get("minimized").and_then(|t| t.as_bool()) == Some(true) {
            continue;
        }
        if let Some(app) = v.get("app-id").and_then(|a| a.as_str()) {
            if cfg.ignore.iter().any(|i| i == app) {
                continue;
            }
        }
        let Some(g) = v.get("geometry").and_then(geo) else { continue };
        let (cx, cy) = (g.x + g.w / 2.0, g.y + g.h / 2.0);
        let sticky = v.get("sticky").and_then(|s| s.as_bool()) == Some(true);
        if !(sticky || (cx >= 0.0 && cx < out.w && cy >= 0.0 && cy < out.h)) {
            continue;
        }
        if g.y < band && g.y + g.h > 0.0 && g.x < out.w && g.x + g.w > 0.0 {
            return true;
        }
    }
    false
}

/// Vlákno autohide: sleduje, či nejaké okno zasahuje do pásma ostrovčeka (`Msg::Obstructed`). Kurzor sa nesleduje:
/// skrytie a zobrazenie riadi samotný ostrovček podľa vstupu a odchodu kurzora na jeho povrchu (pruh pri hornom okraji).
pub fn spawn_autohide(cfg: AutoCfg, tx: Tx<Msg>) {
    std::thread::spawn(move || loop {
        let _ = autohide_session(&cfg, &tx);
        std::thread::sleep(Duration::from_secs(3)); // Wayfire reštart / IPC nie je pripravené
    });
}

fn is_relevant(ev: &Json) -> bool {
    ev.get("event").and_then(|e| e.as_str()).is_some_and(|name| RELEVANT.contains(&name))
}

fn autohide_session(cfg: &AutoCfg, tx: &Tx<Msg>) -> Result<(), String> {
    let mut req = Ipc::connect().ok_or("WAYFIRE_SOCKET nie je dostupný")?;
    let mut watch = Ipc::connect().ok_or("druhé IPC spojenie zlyhalo")?;
    let _ = watch.sock.set_read_timeout(None); // čaká sa, kým príde udalosť
    watch_events(&mut watch, RELEVANT)?;

    let mut obstructed_now = false;
    let mut sent: Option<bool> = None;
    loop {
        if let Ok(j) = req.call("window-rules/get-focused-output", "") {
            if let Some(g) = j.path(&["info", "geometry"]).and_then(geo) {
                obstructed_now = obstructed(&mut req, g, cfg);
            }
        }
        if sent != Some(obstructed_now) {
            sent = Some(obstructed_now);
            tx.send(Msg::Obstructed(obstructed_now));
        }
        // čakaj na udalosť a zlúč dávku (ťahanie okna posiela veľa udalostí)
        let mut dirty = false;
        while !dirty {
            dirty = is_relevant(&watch.recv().map_err(|e| e.to_string())?);
        }
        let mut budget = 200;
        while budget > 0 && watch.readable(30) {
            budget -= 1;
            watch.recv().map_err(|e| e.to_string())?;
        }
    }
}
