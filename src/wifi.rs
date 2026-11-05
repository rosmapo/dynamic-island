//! Modul Wi-Fi: NetworkManager cez vlastný D-Bus klient (systémová zbernica).
//! Zoznam sietí, pripojenie (uložené / otvorené / heslo), odpojenie, zabudnutie siete, zapnutie/vypnutie rádia.
use crate::data::Msg;
use crate::dbus::*;
use crate::sys::Tx;
use crate::toml::{Table, TableExt};
use std::collections::HashMap;
use std::io;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const NM: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const IF_DEV: &str = "org.freedesktop.NetworkManager.Device";
const IF_WIFI: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const IF_AP: &str = "org.freedesktop.NetworkManager.AccessPoint";
const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const IF_SETTINGS: &str = "org.freedesktop.NetworkManager.Settings";
const IF_CONN: &str = "org.freedesktop.NetworkManager.Settings.Connection";
const IF_PROPS: &str = "org.freedesktop.DBus.Properties";

/// Nastavenia sekcie `[wifi]`.
#[derive(Clone, Debug)]
pub struct WifiCfg {
    pub font_size: f32,
    /// koľko sietí je vidieť naraz (viac = skrolovanie kolieskom)
    pub rows: usize,
    pub width: f32,
    /// príkaz tlačidla v hlavičke (správca pripojení)
    pub editor_command: String,
    pub editor_label: String,
}

impl WifiCfg {
    pub fn from_table(t: &Table) -> WifiCfg {
        WifiCfg {
            font_size: t.f64_or("font_size", 15.0).clamp(8.0, 64.0) as f32,
            rows: t.f64_or("rows", 8.0).clamp(1.0, 30.0) as usize,
            width: t.f64_or("width", 420.0).clamp(200.0, 1000.0) as f32,
            editor_command: t.str_or("editor_command", "nm-connection-editor"),
            editor_label: t.str_or("editor_label", "Pripojenia"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct WifiAp {
    pub ssid: String,
    /// 0..100
    pub strength: u8,
    pub secured: bool,
    /// false = podnikové (802.1X), WEP, OWE – tie ostrovček nevie pripojiť
    pub supported: bool,
    /// `wpa-psk` alebo `sae` (prázdne = otvorená sieť)
    pub key_mgmt: String,
    /// popis zabezpečenia na zobrazenie (WPA2, WPA3, …; prázdne = otvorená)
    pub sec: String,
    /// objekt prístupového bodu (cieľ pripojenia)
    pub ap: String,
    /// objekt uloženého spojenia v NetworkManageri
    pub saved: Option<String>,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct WifiState {
    /// rádio Wi-Fi je zapnuté
    pub enabled: bool,
    /// existuje Wi-Fi zariadenie
    pub available: bool,
    pub device: String,
    /// stav zariadenia podľa NetworkManagera (100 = pripojené)
    pub dev_state: u32,
    pub aps: Vec<WifiAp>,
    /// chyba komunikácie (NetworkManager nebeží …)
    pub error: String,
}

/// Príkazy z používateľského rozhrania.
#[derive(Clone, Debug, PartialEq)]
pub enum WifiCmd {
    /// stav + vyhľadanie sietí
    Load,
    Connect { ssid: String, ap: String, saved: Option<String>, password: Option<String>, key_mgmt: String },
    Forget(String),
    Disconnect,
    SetEnabled(bool),
}

// ---------------------------------------------------------------- pomocníci

fn other(m: &str) -> io::Error {
    io::Error::other(m.to_string())
}

fn unvar(v: V) -> V {
    match v {
        V::Var(b) => *b,
        o => o,
    }
}

/// `a{sv}` / `a{sa{sv}}` … → mapa kľúč → hodnota (varianty sa rozbalia).
fn dict_of(v: V) -> HashMap<String, V> {
    let mut m = HashMap::new();
    if let V::Arr(items) = v {
        for it in items {
            if let V::Dict(k, val) = it {
                if let V::Str(k) = *k {
                    m.insert(k, unvar(*val));
                }
            }
        }
    }
    m
}

fn bytes_of(v: &V) -> Vec<u8> {
    match v {
        V::Arr(a) => a.iter().filter_map(|b| if let V::Byte(x) = b { Some(*x) } else { None }).collect(),
        _ => Vec::new(),
    }
}

fn obj_list(m: &Message) -> Vec<String> {
    match Reader::new(&m.body, 0).value(b"ao") {
        Some(V::Arr(a)) => a.into_iter().filter_map(|v| if let V::Str(s) = v { Some(s) } else { None }).collect(),
        _ => Vec::new(),
    }
}

fn props_get(c: &mut Conn, path: &str, iface: &str, prop: &str) -> io::Result<V> {
    let mut w = W::default();
    w.string(iface);
    w.string(prop);
    let m = c.call(NM, path, IF_PROPS, "Get", "ss", w.b)?;
    match Reader::new(&m.body, 0).value(b"v") {
        Some(V::Var(b)) => Ok(*b),
        _ => Err(other("neplatná odpoveď Properties.Get")),
    }
}

fn props_all(c: &mut Conn, path: &str, iface: &str) -> io::Result<HashMap<String, V>> {
    let mut w = W::default();
    w.string(iface);
    let m = c.call(NM, path, IF_PROPS, "GetAll", "s", w.b)?;
    Ok(dict_of(Reader::new(&m.body, 0).value(b"a{sv}").ok_or_else(|| other("neplatná odpoveď GetAll"))?))
}

fn find_device(c: &mut Conn) -> io::Result<Option<String>> {
    let m = c.call(NM, NM_PATH, NM, "GetDevices", "", Vec::new())?;
    for d in obj_list(&m) {
        // 2 = NM_DEVICE_TYPE_WIFI
        if props_get(c, &d, IF_DEV, "DeviceType").ok().and_then(|v| v.as_u32()) == Some(2) {
            return Ok(Some(d));
        }
    }
    Ok(None)
}

/// (zabezpečená, podporovaná, key-mgmt) z príznakov prístupového bodu.
pub fn security(flags: u32, wpa: u32, rsn: u32) -> (bool, bool, &'static str) {
    const PSK: u32 = 0x100;
    const EAP: u32 = 0x200;
    const SAE: u32 = 0x400;
    let secured = flags & 1 != 0 || wpa != 0 || rsn != 0;
    if !secured {
        return (false, true, "");
    }
    let km = wpa | rsn;
    if km & PSK != 0 {
        (true, true, "wpa-psk")
    } else if km & SAE != 0 {
        (true, true, "sae")
    } else if km & EAP != 0 {
        (true, false, "")
    } else {
        // WEP, OWE, neznáme
        (true, false, "")
    }
}

/// Popis zabezpečenia pre zoznam: WPA2, WPA3, WPA2/WPA3, WPA, WEP, …; prázdny reťazec = otvorená sieť.
pub fn security_label(flags: u32, wpa: u32, rsn: u32) -> String {
    const PSK: u32 = 0x100;
    const EAP: u32 = 0x200;
    const SAE: u32 = 0x400;
    const OWE: u32 = 0x800;
    if flags & 1 == 0 && wpa == 0 && rsn == 0 {
        return String::new();
    }
    if rsn != 0 {
        let base = if rsn & SAE != 0 && rsn & PSK != 0 {
            "WPA2/WPA3"
        } else if rsn & SAE != 0 {
            "WPA3"
        } else if rsn & OWE != 0 {
            "OWE"
        } else if wpa & PSK != 0 {
            "WPA/WPA2"
        } else {
            "WPA2"
        };
        return if rsn & EAP != 0 && rsn & (PSK | SAE) == 0 { format!("{base}-EAP") } else { base.to_string() };
    }
    if wpa != 0 {
        return if wpa & EAP != 0 && wpa & PSK == 0 { "WPA-EAP".into() } else { "WPA".into() };
    }
    "WEP".into()
}

/// Z výsledku `GetSettings` (`a{sa{sv}}`) vytiahne SSID, ak ide o Wi-Fi spojenie.
fn wifi_ssid_of_settings(v: V) -> Option<String> {
    let sections = dict_of(v);
    let conn = dict_of(sections.get("connection")?.clone());
    if conn.get("type")?.as_str()? != "802-11-wireless" {
        return None;
    }
    let wl = dict_of(sections.get("802-11-wireless")?.clone());
    let ssid = bytes_of(wl.get("ssid")?);
    Some(String::from_utf8_lossy(&ssid).into_owned())
}

/// SSID → objekt uloženého Wi-Fi spojenia.
fn saved_connections(c: &mut Conn) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(m) = c.call(NM, SETTINGS_PATH, IF_SETTINGS, "ListConnections", "", Vec::new()) else { return out };
    for p in obj_list(&m) {
        let Ok(r) = c.call(NM, &p, IF_CONN, "GetSettings", "", Vec::new()) else { continue };
        if let Some(v) = Reader::new(&r.body, 0).value(b"a{sa{sv}}") {
            if let Some(ssid) = wifi_ssid_of_settings(v) {
                out.entry(ssid).or_insert(p);
            }
        }
    }
    out
}

fn snapshot(c: &mut Conn) -> io::Result<WifiState> {
    let mut st = WifiState { enabled: matches!(props_get(c, NM_PATH, NM, "WirelessEnabled")?, V::Bool(true)), ..Default::default() };
    let Some(dev) = find_device(c)? else { return Ok(st) };
    st.available = true;
    st.dev_state = props_get(c, &dev, IF_DEV, "State").ok().and_then(|v| v.as_u32()).unwrap_or(0);
    let active_ap = match props_get(c, &dev, IF_WIFI, "ActiveAccessPoint") {
        Ok(V::Str(s)) => s,
        _ => "/".to_string(),
    };
    let saved = saved_connections(c);
    let m = c.call(NM, &dev, IF_WIFI, "GetAccessPoints", "", Vec::new())?;
    let mut index: HashMap<String, usize> = HashMap::new();
    for ap in obj_list(&m) {
        let Ok(p) = props_all(c, &ap, IF_AP) else { continue };
        let ssid = String::from_utf8_lossy(&bytes_of(p.get("Ssid").unwrap_or(&V::Arr(Vec::new())))).into_owned();
        if ssid.is_empty() {
            continue; // skryté siete zatiaľ nie
        }
        let strength = match p.get("Strength") {
            Some(V::Byte(b)) => *b,
            _ => 0,
        };
        let get_u32 = |k: &str| p.get(k).and_then(|v| v.as_u32()).unwrap_or(0);
        let (secured, supported, km) = security(get_u32("Flags"), get_u32("WpaFlags"), get_u32("RsnFlags"));
        let item = WifiAp {
            saved: saved.get(&ssid).cloned(),
            ssid: ssid.clone(),
            strength,
            secured,
            supported,
            key_mgmt: km.to_string(),
            sec: security_label(get_u32("Flags"), get_u32("WpaFlags"), get_u32("RsnFlags")),
            active: ap == active_ap,
            ap,
        };
        match index.get(&ssid) {
            Some(&i) => {
                let old = &mut st.aps[i];
                let was_active = old.active;
                if item.active || item.strength > old.strength {
                    *old = item;
                }
                old.active |= was_active;
            }
            None => {
                index.insert(ssid, st.aps.len());
                st.aps.push(item);
            }
        }
    }
    st.aps.sort_by(|a, b| b.active.cmp(&a.active).then(b.strength.cmp(&a.strength)).then(a.ssid.to_lowercase().cmp(&b.ssid.to_lowercase())));
    Ok(st)
}

fn send_snapshot(c: &mut Conn, tx: &Tx<Msg>) {
    match snapshot(c) {
        Ok(s) => tx.send(Msg::Wifi(s)),
        Err(e) => tx.send(Msg::Wifi(WifiState { error: format!("NetworkManager: {e}"), ..Default::default() })),
    }
}

// ---------------------------------------------------------------- zápis

fn section(w: &mut W, name: &str, f: impl FnOnce(&mut W)) {
    w.align(8);
    w.string(name);
    w.array(8, f);
}

fn entry_str(w: &mut W, key: &str, val: &str) {
    w.align(8);
    w.string(key);
    w.signature("s");
    w.string(val);
}

fn entry_bytes(w: &mut W, key: &str, val: &[u8]) {
    w.align(8);
    w.string(key);
    w.signature("ay");
    w.array(1, |w| w.b.extend_from_slice(val));
}

/// Telo `AddAndActivateConnection(a{sa{sv}} settings, o device, o specific_object)`.
fn add_and_activate_body(ssid: &str, secret: Option<(&str, &str)>, dev: &str, ap: &str) -> Vec<u8> {
    let mut w = W::default();
    w.array(8, |w| {
        section(w, "connection", |w| {
            entry_str(w, "type", "802-11-wireless");
            entry_str(w, "id", ssid);
        });
        section(w, "802-11-wireless", |w| entry_bytes(w, "ssid", ssid.as_bytes()));
        if let Some((key_mgmt, psk)) = secret {
            section(w, "802-11-wireless-security", |w| {
                entry_str(w, "key-mgmt", key_mgmt);
                entry_str(w, "psk", psk);
            });
        }
    });
    w.string(dev);
    w.string(ap);
    w.b
}

fn active_ssid(c: &mut Conn, dev: &str) -> Option<String> {
    let ap = match props_get(c, dev, IF_WIFI, "ActiveAccessPoint").ok()? {
        V::Str(s) if s != "/" => s,
        _ => return None,
    };
    let v = props_get(c, &ap, IF_AP, "Ssid").ok()?;
    Some(String::from_utf8_lossy(&bytes_of(&v)).into_owned())
}

/// Čaká na výsledok pripájania: Ok = pripojené k `ssid`, Err = text chyby.
fn wait_connected(c: &mut Conn, dev: &str, ssid: &str) -> Result<(), String> {
    let start = Instant::now();
    let mut seen_connecting = false;
    loop {
        std::thread::sleep(Duration::from_millis(400));
        let state = props_get(c, dev, IF_DEV, "State").ok().and_then(|v| v.as_u32()).unwrap_or(0);
        match state {
            100 => {
                if active_ssid(c, dev).as_deref() == Some(ssid) {
                    return Ok(());
                }
            }
            40..=90 => seen_connecting = true,
            120 => return Err("Pripojenie zlyhalo (skontroluj heslo)".into()),
            0..=30 if seen_connecting || start.elapsed() > Duration::from_secs(6) => return Err("Pripojenie zlyhalo".into()),
            _ => {}
        }
        if start.elapsed() > Duration::from_secs(40) {
            return Err("Časový limit pripájania".into());
        }
    }
}

static LOCK: Mutex<()> = Mutex::new(());

/// Vykoná príkaz vo vlastnom vlákne; výsledky chodia ako `Msg::Wifi` / `Msg::WifiNote`.
pub fn run(cmd: WifiCmd, tx: Tx<Msg>) {
    std::thread::spawn(move || {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut c = match Conn::connect_system() {
            Ok(c) => c,
            Err(e) => {
                tx.send(Msg::Wifi(WifiState { error: format!("Systémová zbernica nedostupná: {e}"), ..Default::default() }));
                return;
            }
        };
        if let Err(e) = execute(&mut c, cmd, &tx) {
            tx.send(Msg::WifiNote(format!("Chyba: {e}")));
            send_snapshot(&mut c, &tx);
        }
    });
}

fn execute(c: &mut Conn, cmd: WifiCmd, tx: &Tx<Msg>) -> io::Result<()> {
    match cmd {
        WifiCmd::Load => {
            send_snapshot(c, tx);
            if let Some(dev) = find_device(c)? {
                let mut w = W::default();
                w.array(8, |_| {});
                // príliš časté skenovanie NM odmietne – nevadí, zoznam sa aj tak obnoví
                let _ = c.call(NM, &dev, IF_WIFI, "RequestScan", "a{sv}", w.b);
                std::thread::sleep(Duration::from_millis(3000));
                send_snapshot(c, tx);
            }
        }
        WifiCmd::SetEnabled(on) => {
            let mut w = W::default();
            w.string(NM);
            w.string("WirelessEnabled");
            w.signature("b");
            w.u32(on as u32);
            c.call(NM, NM_PATH, IF_PROPS, "Set", "ssv", w.b)?;
            std::thread::sleep(Duration::from_millis(1500));
            send_snapshot(c, tx);
        }
        WifiCmd::Disconnect => {
            if let Some(dev) = find_device(c)? {
                c.call(NM, &dev, IF_DEV, "Disconnect", "", Vec::new())?;
            }
            std::thread::sleep(Duration::from_millis(800));
            tx.send(Msg::WifiNote(String::new()));
            send_snapshot(c, tx);
        }
        WifiCmd::Forget(conn) => {
            c.call(NM, &conn, IF_CONN, "Delete", "", Vec::new())?;
            tx.send(Msg::WifiNote(String::new()));
            send_snapshot(c, tx);
        }
        WifiCmd::Connect { ssid, ap, saved, password, key_mgmt } => {
            let dev = find_device(c)?.ok_or_else(|| other("Wi-Fi zariadenie sa nenašlo"))?;
            tx.send(Msg::WifiNote(format!("Pripájam k {ssid}…")));
            // nové spojenie vytvorené touto akciou (pri zlyhaní ho zmažeme, aby nezostal profil so zlým heslom)
            let mut created: Option<String> = None;
            match &saved {
                Some(conn) => {
                    let mut w = W::default();
                    w.string(conn);
                    w.string(&dev);
                    w.string(&ap);
                    c.call(NM, NM_PATH, NM, "ActivateConnection", "ooo", w.b)?;
                }
                None => {
                    let secret = password.as_deref().map(|p| (key_mgmt.as_str(), p));
                    let body = add_and_activate_body(&ssid, secret, &dev, &ap);
                    let r = c.call(NM, NM_PATH, NM, "AddAndActivateConnection", "a{sa{sv}}oo", body)?;
                    created = Reader::new(&r.body, 0).string();
                }
            }
            match wait_connected(c, &dev, &ssid) {
                Ok(()) => tx.send(Msg::WifiNote(String::new())),
                Err(e) => {
                    if let Some(p) = created {
                        let _ = c.call(NM, &p, IF_CONN, "Delete", "", Vec::new());
                    }
                    tx.send(Msg::WifiNote(e));
                }
            }
            send_snapshot(c, tx);
        }
    }
    Ok(())
}

/// Vloženie schránky (Ctrl+V) do poľa hesla: `wl-paste --no-newline`.
pub fn paste(tx: Tx<Msg>) {
    std::thread::spawn(move || {
        if let Ok(o) = Command::new("wl-paste").arg("--no-newline").stdin(Stdio::null()).stderr(Stdio::null()).output() {
            if o.status.success() {
                tx.send(Msg::WifiPaste(String::from_utf8_lossy(&o.stdout).into_owned()));
            }
        }
    });
}

/// Glyf sily signálu (Nerd Font).
pub fn signal_glyph(strength: u8) -> &'static str {
    match strength {
        0..=24 => "\u{f091f}",
        25..=49 => "\u{f0922}",
        50..=74 => "\u{f0925}",
        _ => "\u{f0928}",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults() {
        let c = WifiCfg::from_table(&Table::new());
        assert_eq!((c.rows, c.width, c.font_size), (8, 420.0, 15.0));
        assert_eq!(c.editor_command, "nm-connection-editor");
    }

    #[test]
    fn security_flags() {
        assert_eq!(security(0, 0, 0), (false, true, ""));
        assert_eq!(security(1, 0x188, 0x188), (true, true, "wpa-psk")); // PSK
        assert_eq!(security(1, 0, 0x400), (true, true, "sae"));
        assert_eq!(security(1, 0, 0x400 | 0x100), (true, true, "wpa-psk"));
        assert_eq!(security(1, 0, 0x200), (true, false, ""));
        assert_eq!(security(1, 0, 0), (true, false, "")); // WEP
    }

    #[test]
    fn security_labels() {
        assert_eq!(security_label(0, 0, 0), "");
        assert_eq!(security_label(1, 0, 0x188), "WPA2");
        assert_eq!(security_label(1, 0x188, 0x188), "WPA/WPA2");
        assert_eq!(security_label(1, 0, 0x400), "WPA3");
        assert_eq!(security_label(1, 0, 0x500), "WPA2/WPA3");
        assert_eq!(security_label(1, 0, 0x200), "WPA2-EAP");
        assert_eq!(security_label(1, 0x188, 0), "WPA");
        assert_eq!(security_label(1, 0, 0), "WEP");
    }

    #[test]
    fn add_and_activate_roundtrip() {
        let body = add_and_activate_body("Moja WiFi", Some(("wpa-psk", "tajne heslo")), "/org/freedesktop/NetworkManager/Devices/2", "/org/freedesktop/NetworkManager/AccessPoint/9");
        let mut r = Reader::new(&body, 0);
        let v = r.value(b"a{sa{sv}}").expect("settings");
        let dev = r.string().unwrap();
        let ap = r.string().unwrap();
        assert!(dev.ends_with("Devices/2") && ap.ends_with("AccessPoint/9"));
        let sections = dict_of(v);
        let conn = dict_of(sections["connection"].clone());
        assert_eq!(conn["type"].as_str(), Some("802-11-wireless"));
        assert_eq!(conn["id"].as_str(), Some("Moja WiFi"));
        let wl = dict_of(sections["802-11-wireless"].clone());
        assert_eq!(bytes_of(&wl["ssid"]), b"Moja WiFi");
        let sec = dict_of(sections["802-11-wireless-security"].clone());
        assert_eq!(sec["key-mgmt"].as_str(), Some("wpa-psk"));
        assert_eq!(sec["psk"].as_str(), Some("tajne heslo"));
        // otvorená sieť: bez sekcie zabezpečenia
        let open = add_and_activate_body("Cafe", None, "/d", "/a");
        let v = Reader::new(&open, 0).value(b"a{sa{sv}}").unwrap();
        assert!(!dict_of(v).contains_key("802-11-wireless-security"));
    }

    #[test]
    fn ssid_from_saved_settings() {
        // zostav odpoveď GetSettings rovnakým zapisovačom
        let mut w = W::default();
        w.array(8, |w| {
            section(w, "connection", |w| {
                entry_str(w, "type", "802-11-wireless");
                entry_str(w, "id", "x");
            });
            section(w, "802-11-wireless", |w| entry_bytes(w, "ssid", "Doma".as_bytes()));
        });
        let v = Reader::new(&w.b, 0).value(b"a{sa{sv}}").unwrap();
        assert_eq!(wifi_ssid_of_settings(v), Some("Doma".to_string()));
        let mut w = W::default();
        w.array(8, |w| section(w, "connection", |w| entry_str(w, "type", "vpn")));
        let v = Reader::new(&w.b, 0).value(b"a{sa{sv}}").unwrap();
        assert_eq!(wifi_ssid_of_settings(v), None);
    }

    #[test]
    fn glyphs_by_strength() {
        assert_ne!(signal_glyph(10), signal_glyph(90));
    }
}
