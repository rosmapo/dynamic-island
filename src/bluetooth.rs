//! Modul Bluetooth: BlueZ cez D-Bus (systémová zbernica).
//! Prepínač adaptéra, skenovanie, pripojenie/odpojenie, zabudnutie zariadenia.
use crate::data::Msg;
use crate::dbus::*;
use crate::sys::Tx;
use crate::toml::{Table, TableExt};
use std::collections::HashMap;
use std::io;
use std::sync::Mutex;
use std::time::Duration;

const BZ: &str = "org.bluez";
const BZ_ROOT: &str = "/";
const IF_OM: &str = "org.freedesktop.DBus.ObjectManager";
const IF_PROPS: &str = "org.freedesktop.DBus.Properties";
const IF_ADAPTER: &str = "org.bluez.Adapter1";
const IF_DEVICE: &str = "org.bluez.Device1";

/// Nastavenia sekcie `[bluetooth]`.
#[derive(Clone, Debug)]
pub struct BtCfg {
    pub font_size: f32,
    pub rows: usize,
    pub width: f32,
    /// ako dlho beží skenovanie pri obnovení (ms)
    pub scan_ms: u64,
    /// voliteľný príkaz tlačidla v hlavičke (napr. blueman-manager)
    pub editor_command: String,
    pub editor_label: String,
}

impl BtCfg {
    pub fn from_table(t: &Table) -> BtCfg {
        BtCfg {
            font_size: t.f64_or("font_size", 15.0).clamp(8.0, 64.0) as f32,
            rows: t.f64_or("rows", 8.0).clamp(1.0, 30.0) as usize,
            width: t.f64_or("width", 380.0).clamp(200.0, 1000.0) as f32,
            scan_ms: t.f64_or("scan_ms", 4000.0).clamp(500.0, 30000.0) as u64,
            editor_command: t.str_or("editor_command", ""),
            editor_label: t.str_or("editor_label", "Správca"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct BtDevice {
    pub path: String,
    pub address: String,
    pub name: String,
    pub paired: bool,
    pub connected: bool,
    pub trusted: bool,
    /// 0..100, None = neznáme
    pub battery: Option<u8>,
    pub icon: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct BtState {
    pub available: bool,
    pub powered: bool,
    pub discoverable: bool,
    pub discovering: bool,
    pub adapter: String,
    pub devices: Vec<BtDevice>,
    pub error: String,
    /// stavový riadok (párovanie, pripájanie…)
    pub note: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BtCmd {
    /// stav (+ voliteľné skenovanie nových zariadení)
    Load { scan: bool },
    SetPowered(bool),
    SetDiscoverable(bool),
    /// pripojiť; ak nie je spárované, najprv Pair + Trust
    Connect(String),
    Disconnect(String),
    /// len spárovať (bez connect)
    Pair(String),
    SetTrusted { path: String, trusted: bool },
    /// odpárovať / zabudnúť (path zariadenia)
    Forget(String),
}

static LOCK: Mutex<()> = Mutex::new(());

fn other(m: &str) -> io::Error {
    io::Error::other(m.to_string())
}

fn unvar(v: V) -> V {
    match v {
        V::Var(b) => *b,
        o => o,
    }
}

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

fn prop_bool(props: &HashMap<String, V>, key: &str) -> bool {
    match props.get(key) {
        Some(V::Bool(b)) => *b,
        Some(V::U32(n)) => *n != 0,
        _ => false,
    }
}

fn prop_str(props: &HashMap<String, V>, key: &str) -> String {
    props.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

fn prop_u8(props: &HashMap<String, V>, key: &str) -> Option<u8> {
    match props.get(key) {
        Some(V::Byte(b)) => Some(*b),
        Some(V::U32(n)) => Some(*n as u8),
        _ => None,
    }
}

fn props_get(c: &mut Conn, path: &str, iface: &str, prop: &str) -> io::Result<V> {
    let mut w = W::default();
    w.string(iface);
    w.string(prop);
    let m = c.call(BZ, path, IF_PROPS, "Get", "ss", w.b)?;
    match Reader::new(&m.body, 0).value(b"v") {
        Some(V::Var(b)) => Ok(*b),
        _ => Err(other("neplatná odpoveď Properties.Get")),
    }
}

fn props_set_bool(c: &mut Conn, path: &str, iface: &str, prop: &str, val: bool) -> io::Result<()> {
    let mut w = W::default();
    w.string(iface);
    w.string(prop);
    w.signature("b");
    w.u32(val as u32);
    c.call(BZ, path, IF_PROPS, "Set", "ssv", w.b)?;
    Ok(())
}

/// GetManagedObjects → adaptér + zariadenia.
fn snapshot(c: &mut Conn) -> io::Result<BtState> {
    let m = c.call(BZ, BZ_ROOT, IF_OM, "GetManagedObjects", "", Vec::new())?;
    let root = Reader::new(&m.body, 0)
        .value(b"a{oa{sa{sv}}}")
        .ok_or_else(|| other("neplatná odpoveď GetManagedObjects"))?;

    let mut adapter_path = String::new();
    let mut powered = false;
    let mut discoverable = false;
    let mut discovering = false;
    let mut devices = Vec::new();

    let objs = match root {
        V::Arr(a) => a,
        _ => return Err(other("očakávané pole objektov")),
    };

    for obj in objs {
        let V::Dict(path_v, ifaces_v) = obj else { continue };
        let path = match *path_v {
            V::Str(s) => s,
            _ => continue,
        };
        let ifaces = dict_of(*ifaces_v);

        if let Some(ad) = ifaces.get(IF_ADAPTER).cloned() {
            // prvý adaptér stačí
            if adapter_path.is_empty() {
                let props = dict_of(ad);
                adapter_path = path.clone();
                powered = prop_bool(&props, "Powered");
                discoverable = prop_bool(&props, "Discoverable");
                discovering = prop_bool(&props, "Discovering");
            }
        }

        if let Some(dev) = ifaces.get(IF_DEVICE).cloned() {
            let props = dict_of(dev);
            let name = {
                let alias = prop_str(&props, "Alias");
                if !alias.is_empty() {
                    alias
                } else {
                    let n = prop_str(&props, "Name");
                    if !n.is_empty() {
                        n
                    } else {
                        prop_str(&props, "Address")
                    }
                }
            };
            // Battery1 je samostatné rozhranie na tom istom path
            let battery = ifaces
                .get("org.bluez.Battery1")
                .map(|b| dict_of(b.clone()))
                .and_then(|p| prop_u8(&p, "Percentage"));

            devices.push(BtDevice {
                path: path.clone(),
                address: prop_str(&props, "Address"),
                name,
                paired: prop_bool(&props, "Paired"),
                connected: prop_bool(&props, "Connected"),
                trusted: prop_bool(&props, "Trusted"),
                battery,
                icon: prop_str(&props, "Icon"),
            });
        }
    }

    // poradie: pripojené, spárované, ostatné; v skupine podľa mena
    devices.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then(b.paired.cmp(&a.paired))
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    if adapter_path.is_empty() {
        return Ok(BtState {
            available: false,
            error: "Bluetooth adaptér sa nenašiel".into(),
            ..Default::default()
        });
    }

    Ok(BtState {
        available: true,
        powered,
        discoverable,
        discovering,
        adapter: adapter_path,
        devices,
        error: String::new(),
        note: String::new(),
    })
}

/// Jednoduchý agent na pozadí (Just Works / NoInputNoOutput).
/// Bez neho BlueZ pri párovaní nových zariadení často zlyhá.
fn ensure_agent() {
    use std::process::{Command, Stdio};
    // už beží?
    let running = Command::new("pgrep")
        .args(["-f", "bt-agent|bluetoothctl.*default-agent"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if running {
        return;
    }
    // preferuj bt-agent (bluez-tools), inak bluetoothctl
    let ok = Command::new("sh")
        .args([
            "-c",
            "command -v bt-agent >/dev/null 2>&1 && setsid bt-agent -c NoInputNoOutput >/dev/null 2>&1 &",
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        let _ = Command::new("sh")
            .args([
                "-c",
                "setsid sh -c 'printf \"agent NoInputNoOutput\\ndefault-agent\\n\" | bluetoothctl' >/dev/null 2>&1 &",
            ])
            .spawn();
    }
}

fn send_note(tx: &Tx<Msg>, c: &mut Conn, note: &str) {
    if let Ok(mut st) = snapshot(c) {
        st.note = note.into();
        tx.send(Msg::Bluetooth(st));
    }
}

fn device_paired(c: &mut Conn, path: &str) -> bool {
    props_get(c, path, IF_DEVICE, "Paired")
        .ok()
        .map(|v| matches!(v, V::Bool(true)) || matches!(v, V::U32(n) if n != 0))
        .unwrap_or(false)
}

fn execute(c: &mut Conn, cmd: BtCmd, scan_ms: u64, tx: &Tx<Msg>) -> io::Result<()> {
    match cmd {
        BtCmd::Load { scan } => {
            let st = snapshot(c)?;
            tx.send(Msg::Bluetooth(st.clone()));
            if scan && st.available && st.powered && !st.adapter.is_empty() {
                ensure_agent();
                // počas skenu byť viditeľný a prijímať párovanie
                let _ = props_set_bool(c, &st.adapter, IF_ADAPTER, "Pairable", true);
                let _ = props_set_bool(c, &st.adapter, IF_ADAPTER, "Discoverable", true);
                let _ = c.call(BZ, &st.adapter, IF_ADAPTER, "StartDiscovery", "", Vec::new());
                send_note(tx, c, "Hľadám zariadenia…");
                std::thread::sleep(Duration::from_millis(scan_ms));
                let _ = c.call(BZ, &st.adapter, IF_ADAPTER, "StopDiscovery", "", Vec::new());
                let mut st = snapshot(c)?;
                st.note.clear();
                tx.send(Msg::Bluetooth(st));
            }
        }
        BtCmd::SetPowered(on) => {
            let st = snapshot(c)?;
            if st.adapter.is_empty() {
                return Err(other("žiadny adaptér"));
            }
            props_set_bool(c, &st.adapter, IF_ADAPTER, "Powered", on)?;
            std::thread::sleep(Duration::from_millis(400));
            tx.send(Msg::Bluetooth(snapshot(c)?));
        }
        BtCmd::SetDiscoverable(on) => {
            let st = snapshot(c)?;
            if st.adapter.is_empty() {
                return Err(other("žiadny adaptér"));
            }
            props_set_bool(c, &st.adapter, IF_ADAPTER, "Discoverable", on)?;
            if on {
                let _ = props_set_bool(c, &st.adapter, IF_ADAPTER, "Pairable", true);
            }
            std::thread::sleep(Duration::from_millis(200));
            tx.send(Msg::Bluetooth(snapshot(c)?));
        }
        BtCmd::Connect(path) => {
            ensure_agent();
            let name = props_get(c, &path, IF_DEVICE, "Alias")
                .ok()
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "zariadenie".into());
            if !device_paired(c, &path) {
                send_note(tx, c, &format!("Páram {name}…"));
                if let Err(e) = c.call(BZ, &path, IF_DEVICE, "Pair", "", Vec::new()) {
                    return Err(other(&format!("párovanie zlyhalo: {e}")));
                }
                std::thread::sleep(Duration::from_millis(400));
                // po úspešnom páre nastav trust (ako blueman)
                let _ = props_set_bool(c, &path, IF_DEVICE, "Trusted", true);
            }
            send_note(tx, c, &format!("Pripájam {name}…"));
            c.call(BZ, &path, IF_DEVICE, "Connect", "", Vec::new())?;
            std::thread::sleep(Duration::from_millis(700));
            let mut st = snapshot(c)?;
            st.note.clear();
            tx.send(Msg::Bluetooth(st));
        }
        BtCmd::Disconnect(path) => {
            c.call(BZ, &path, IF_DEVICE, "Disconnect", "", Vec::new())?;
            std::thread::sleep(Duration::from_millis(400));
            tx.send(Msg::Bluetooth(snapshot(c)?));
        }
        BtCmd::Pair(path) => {
            ensure_agent();
            send_note(tx, c, "Páram…");
            c.call(BZ, &path, IF_DEVICE, "Pair", "", Vec::new())?;
            let _ = props_set_bool(c, &path, IF_DEVICE, "Trusted", true);
            std::thread::sleep(Duration::from_millis(400));
            let mut st = snapshot(c)?;
            st.note.clear();
            tx.send(Msg::Bluetooth(st));
        }
        BtCmd::SetTrusted { path, trusted } => {
            props_set_bool(c, &path, IF_DEVICE, "Trusted", trusted)?;
            tx.send(Msg::Bluetooth(snapshot(c)?));
        }
        BtCmd::Forget(path) => {
            let st = snapshot(c)?;
            if st.adapter.is_empty() {
                return Err(other("žiadny adaptér"));
            }
            let _ = c.call(BZ, &path, IF_DEVICE, "Disconnect", "", Vec::new());
            let mut w = W::default();
            w.string(&path);
            c.call(BZ, &st.adapter, IF_ADAPTER, "RemoveDevice", "o", w.b)?;
            std::thread::sleep(Duration::from_millis(300));
            tx.send(Msg::Bluetooth(snapshot(c)?));
        }
    }
    Ok(())
}

/// Vykoná príkaz na pozadí; výsledok príde ako `Msg::Bluetooth`.
pub fn run(cmd: BtCmd, cfg: BtCfg, tx: Tx<Msg>) {
    std::thread::spawn(move || {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut c = match Conn::connect_system() {
            Ok(c) => c,
            Err(e) => {
                tx.send(Msg::Bluetooth(BtState {
                    error: format!("Systémová zbernica nedostupná: {e}"),
                    ..Default::default()
                }));
                return;
            }
        };
        if let Err(e) = execute(&mut c, cmd, cfg.scan_ms, &tx) {
            // pri chybe ešte skús poslať aktuálny stav
            if let Ok(st) = snapshot(&mut c) {
                let mut st = st;
                if st.error.is_empty() {
                    st.error = format!("{e}");
                }
                tx.send(Msg::Bluetooth(st));
            } else {
                tx.send(Msg::Bluetooth(BtState {
                    error: format!("{e}"),
                    ..Default::default()
                }));
            }
        }
    });
}

/// Ikona podľa stavu (Nerd Font).
pub fn device_glyph(d: &BtDevice) -> &'static str {
    if d.connected {
        "󰂱" // connected
    } else if d.paired {
        "󰂯"
    } else {
        "󰂲"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let c = BtCfg::from_table(&Table::new());
        assert_eq!(c.rows, 8);
        assert!(c.width >= 200.0);
    }
}
