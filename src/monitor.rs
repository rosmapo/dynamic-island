//! Modul monitor: jas, kontrast, teplota farieb, vstup a nočný režim cez ddcutil + wlsunset.
//! Bus a mapovania sú konfigurovateľné v sekcii `[monitor]`.
use crate::data::Msg;
use crate::sys::Tx;
use crate::toml::{Table, TableExt};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

/// Nastavenia panela `[monitor]`.
#[derive(Clone, Debug)]
pub struct MonitorCfg {
    pub font_size: f32,
    pub width: f32,
    /// I2C bus pre ddcutil (napr. "10")
    pub bus: String,
    /// VCP kódy pre temperature presets (poradie = poradie v `preset_labels`)
    pub presets: Vec<u8>,
    pub preset_labels: Vec<String>,
    /// VCP kódy pre vstupy
    pub inputs: Vec<u8>,
    pub input_labels: Vec<String>,
    /// teplota pre wlsunset (nočný režim)
    pub night_temp: u32,
}

impl MonitorCfg {
    pub fn from_table(t: &Table) -> MonitorCfg {
        let num_arr = |k: &str| -> Vec<u8> {
            t.get(k)
                .and_then(|v| v.as_arr())
                .map(|a| a.iter().filter_map(|v| v.as_f64().map(|n| n as u8)).collect())
                .unwrap_or_default()
        };
        let presets = num_arr("presets");
        let inputs = num_arr("inputs");
        let preset_labels = t.strs("preset_labels").unwrap_or_default();
        let input_labels = t.strs("input_labels").unwrap_or_default();

        // predvolené hodnoty pre Iiyama PL2493H (ako v pôvodnom skripte)
        let (presets, preset_labels) = if presets.is_empty() {
            (vec![0x05, 0x06, 0x08], vec!["6500 K".into(), "7500 K".into(), "9300 K".into()])
        } else {
            let labels = if preset_labels.len() == presets.len() {
                preset_labels
            } else {
                presets.iter().map(|c| format!("0x{c:02X}")).collect()
            };
            (presets, labels)
        };
        let (inputs, input_labels) = if inputs.is_empty() {
            (vec![0x0F, 0x11], vec!["DisplayPort".into(), "HDMI".into()])
        } else {
            let labels = if input_labels.len() == inputs.len() {
                input_labels
            } else {
                inputs.iter().map(|c| format!("0x{c:02X}")).collect()
            };
            (inputs, labels)
        };

        MonitorCfg {
            font_size: t.f64_or("font_size", 15.0).clamp(8.0, 64.0) as f32,
            width: t.f64_or("width", 380.0).clamp(240.0, 800.0) as f32,
            bus: t.str_or("bus", "10"),
            presets,
            preset_labels,
            inputs,
            input_labels,
            night_temp: t.f64_or("night_temp", 4500.0).clamp(1000.0, 6500.0) as u32,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct MonitorState {
    pub brightness: u32,
    pub contrast: u32,
    /// index do presets (alebo lená ak neznámy)
    pub preset: Option<usize>,
    /// index do inputs
    pub input: Option<usize>,
    pub night: bool,
    pub error: String,
    pub loaded: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MonitorCmd {
    Load,
    Brightness(u32),
    Contrast(u32),
    Preset(usize),
    Input(usize),
    /// explicitné zapnutie/vypnutie nočného režimu (nie toggle podľa pgrep)
    SetNight(bool),
}

// Debounce pre jas/kontrast – posledná hodnota vyhráva.
static PENDING_BR: Mutex<Option<u32>> = Mutex::new(None);
static PENDING_CT: Mutex<Option<u32>> = Mutex::new(None);
static RUNNING: AtomicBool = AtomicBool::new(false);

fn out(cmd: &str, args: &[&str]) -> Option<String> {
    let o = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

fn run_quiet(cmd: &str, args: &[&str]) {
    let _ = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Podporuje continuous (C) aj simple non-continuous (SNC) z `ddcutil --terse`.
fn get_vcp(bus: &str, feature: u8) -> Option<u32> {
    let code = format!("{feature:02x}");
    let stdout = out("ddcutil", &["getvcp", &code, "--bus", bus, "--terse"])?;
    let parts: Vec<&str> = stdout.split_whitespace().collect();
    if let Some(i) = parts.iter().position(|&p| p == "C") {
        return parts.get(i + 1).and_then(|s| s.parse().ok());
    }
    if let Some(i) = parts.iter().position(|&p| p == "SNC") {
        let raw = parts.get(i + 1)?.to_lowercase().replace("0x", "").replace('x', "");
        return u32::from_str_radix(&raw, 16).ok();
    }
    None
}

fn set_vcp(bus: &str, feature: u8, value: u32) {
    let code = format!("{feature:02x}");
    run_quiet(
        "ddcutil",
        &["setvcp", &code, &value.to_string(), "--bus", bus, "--noverify"],
    );
}

fn night_running() -> bool {
    Command::new("pgrep")
        .args(["-x", "wlsunset"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// wlsunset musí bežať mimo process group / cgroup ostrovčeka, inak
/// ostane visieť a `killall` zvonku nefunguje, kým ostrovček žije.
/// Preto všetko ide cez `sh -c '… &'` + nohup (shell hneď skončí).
fn set_night(enable: bool, temp: u32) {
    let script = if enable {
        format!(
            "pkill -x wlsunset >/dev/null 2>&1 || true; \
             sleep 0.08; \
             pkill -9 -x wlsunset >/dev/null 2>&1 || true; \
             sleep 0.05; \
             nohup wlsunset -t {temp} -T {t1} >/dev/null 2>&1 &",
            t1 = temp + 1
        )
    } else {
        "pkill -x wlsunset >/dev/null 2>&1 || true; \
         sleep 0.08; \
         pkill -9 -x wlsunset >/dev/null 2>&1 || true"
            .into()
    };
    // .status() – počkáme len na shell (okamžite skončí po &), nie na wlsunset
    let _ = Command::new("sh")
        .args(["-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn load_state(cfg: &MonitorCfg) -> MonitorState {
    // jas je zároveň test, či ddcutil existuje a monitor odpovedá (`ddcutil detect` by trvalo aj sekundy navyše)
    let Some(brightness) = get_vcp(&cfg.bus, 0x10) else {
        return MonitorState {
            error: "ddcutil nie je dostupný alebo monitor neodpovedá".into(),
            loaded: true,
            ..Default::default()
        };
    };
    let contrast = get_vcp(&cfg.bus, 0x12).unwrap_or(0);
    let raw_preset = get_vcp(&cfg.bus, 0x14);
    let raw_input = get_vcp(&cfg.bus, 0x60);

    let preset = raw_preset.and_then(|v| {
        cfg.presets
            .iter()
            .position(|&p| p as u32 == v)
    });
    let input = raw_input.and_then(|v| {
        cfg.inputs
            .iter()
            .position(|&p| p as u32 == v)
    });

    MonitorState {
        brightness,
        contrast,
        preset,
        input,
        night: night_running(),
        error: String::new(),
        loaded: true,
    }
}

fn continuous_worker(bus: String) {
    loop {
        let job = {
            let mut br = PENDING_BR.lock().unwrap_or_else(|e| e.into_inner());
            let mut ct = PENDING_CT.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(v) = br.take() {
                Some((0x10u8, v))
            } else if let Some(v) = ct.take() {
                Some((0x12u8, v))
            } else {
                None
            }
        };
        match job {
            Some((feat, val)) => set_vcp(&bus, feat, val),
            None => {
                RUNNING.store(false, Ordering::SeqCst);
                let more = PENDING_BR.lock().unwrap_or_else(|e| e.into_inner()).is_some()
                    || PENDING_CT.lock().unwrap_or_else(|e| e.into_inner()).is_some();
                if more && !RUNNING.swap(true, Ordering::SeqCst) {
                    continue;
                }
                return;
            }
        }
    }
}

/// Vykoná príkaz na pozadí; výsledok príde ako `Msg::Monitor`.
pub fn run(cmd: MonitorCmd, cfg: MonitorCfg, tx: Tx<Msg>) {
    match cmd {
        MonitorCmd::Load => {
            thread::spawn(move || tx.send(Msg::Monitor(load_state(&cfg))));
        }
        MonitorCmd::Brightness(pct) => {
            *PENDING_BR.lock().unwrap_or_else(|e| e.into_inner()) = Some(pct.min(100));
            if !RUNNING.swap(true, Ordering::SeqCst) {
                let bus = cfg.bus.clone();
                thread::spawn(move || continuous_worker(bus));
            }
        }
        MonitorCmd::Contrast(pct) => {
            *PENDING_CT.lock().unwrap_or_else(|e| e.into_inner()) = Some(pct.min(100));
            if !RUNNING.swap(true, Ordering::SeqCst) {
                let bus = cfg.bus.clone();
                thread::spawn(move || continuous_worker(bus));
            }
        }
        MonitorCmd::Preset(idx) => {
            if let Some(&code) = cfg.presets.get(idx) {
                thread::spawn(move || {
                    set_vcp(&cfg.bus, 0x14, code as u32);
                    // krátke oneskorenie, monitor niekedy potrebuje čas
                    thread::sleep(Duration::from_millis(120));
                    tx.send(Msg::Monitor(load_state(&cfg)));
                });
            }
        }
        MonitorCmd::Input(idx) => {
            if let Some(&code) = cfg.inputs.get(idx) {
                thread::spawn(move || {
                    set_vcp(&cfg.bus, 0x60, code as u32);
                    thread::sleep(Duration::from_millis(200));
                    tx.send(Msg::Monitor(load_state(&cfg)));
                });
            }
        }
        MonitorCmd::SetNight(enable) => {
            thread::spawn(move || {
                set_night(enable, cfg.night_temp);
                thread::sleep(Duration::from_millis(120));
                let mut st = load_state(&cfg);
                // dôveruj požadovanému stavu, pgrep môže ešte chvíľu lagovať
                st.night = enable;
                tx.send(Msg::Monitor(st));
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let c = MonitorCfg::from_table(&Table::new());
        assert_eq!(c.bus, "10");
        assert_eq!(c.presets, vec![0x05, 0x06, 0x08]);
        assert_eq!(c.preset_labels.len(), 3);
        assert_eq!(c.inputs, vec![0x0F, 0x11]);
        assert_eq!(c.width, 380.0);
    }
}
