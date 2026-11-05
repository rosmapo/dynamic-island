//! Modul audio: hlasitosť a stlmenie výstupu/vstupu a výber predvoleného zariadenia.
//! Zoznam zariadení a predvolené zariadenie: `pactl` (PipeWire-pulse / WirePlumber), hlasitosť a stlmenie: `wpctl`.
use crate::data::Msg;
use crate::json;
use crate::sys::Tx;
use crate::toml::{Table, TableExt};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Nastavenia panela (kľúče v sekcii `[audio]`, vedľa nastavení modulu).
#[derive(Clone, Debug)]
pub struct AudioCfg {
    pub font_size: f32,
    pub width: f32,
    /// najviac zariadení zobrazených v jednej sekcii
    pub rows: usize,
}

impl AudioCfg {
    pub fn from_table(t: &Table) -> AudioCfg {
        AudioCfg {
            font_size: t.f64_or("panel_font_size", 15.0).clamp(8.0, 64.0) as f32,
            width: t.f64_or("panel_width", 380.0).clamp(200.0, 1000.0) as f32,
            rows: t.f64_or("panel_rows", 6.0).clamp(1.0, 30.0) as usize,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct AudioDev {
    pub name: String,
    pub desc: String,
    /// percentá (100 = nominálna hlasitosť)
    pub volume: u32,
    pub mute: bool,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct AudioState {
    pub sinks: Vec<AudioDev>,
    pub sources: Vec<AudioDev>,
    pub default_sink: String,
    pub default_source: String,
    pub error: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AudioCmd {
    Load,
    /// nastaví hlasitosť predvoleného zariadenia (`source` = vstup)
    Volume { source: bool, pct: u32 },
    ToggleMute { source: bool },
    SetDefault { source: bool, name: String },
}

/// Výstup `pactl -f json list sinks|sources` → zariadenia (zdroje bez „monitor“ vstupov).
pub fn parse_devices(out: &str, source: bool) -> Vec<AudioDev> {
    let Some(j) = json::parse(out) else { return Vec::new() };
    let Some(arr) = j.as_arr() else { return Vec::new() };
    let mut v = Vec::new();
    for d in arr {
        let name = d.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }
        let class = d.path(&["properties", "device.class"]).and_then(|x| x.as_str()).unwrap_or("");
        if source && (name.ends_with(".monitor") || class == "monitor") {
            continue;
        }
        // `pactl -f json` občas vráti v `description` doslovné "(null)"; vtedy skúsime properties a nakoniec názov
        let good = |s: &&str| !s.trim().is_empty() && *s != "(null)";
        let desc = d
            .get("description")
            .and_then(|x| x.as_str())
            .filter(good)
            .or_else(|| d.path(&["properties", "node.description"]).and_then(|x| x.as_str()).filter(good))
            .or_else(|| d.path(&["properties", "device.description"]).and_then(|x| x.as_str()).filter(good))
            .unwrap_or(&name)
            .to_string();
        let mute = d.get("mute").and_then(|x| x.as_bool()).unwrap_or(false);
        let volume = match d.get("volume") {
            Some(json::Json::Obj(ch)) => ch
                .first()
                .map(|(_, c)| {
                    c.get("value_percent")
                        .and_then(|p| p.as_str())
                        .and_then(|p| p.trim().trim_end_matches('%').trim().parse::<f64>().ok())
                        .or_else(|| c.get("value").and_then(|x| x.as_f64()).map(|n| n * 100.0 / 65536.0))
                        .unwrap_or(0.0)
                })
                .unwrap_or(0.0),
            _ => 0.0,
        };
        v.push(AudioDev { name, desc, volume: volume.round().max(0.0) as u32, mute });
    }
    v
}

fn out(cmd: &str, args: &[&str]) -> Option<String> {
    let o = Command::new(cmd).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

fn run_quiet(cmd: &str, args: &[&str]) {
    let _ = Command::new(cmd).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

fn load_state() -> AudioState {
    let sinks = out("pactl", &["-f", "json", "list", "sinks"]);
    let sources = out("pactl", &["-f", "json", "list", "sources"]);
    if sinks.is_none() && sources.is_none() {
        return AudioState { error: "pactl nie je dostupný (balík pulseaudio-utils)".into(), ..Default::default() };
    }
    AudioState {
        sinks: sinks.map(|s| parse_devices(&s, false)).unwrap_or_default(),
        sources: sources.map(|s| parse_devices(&s, true)).unwrap_or_default(),
        default_sink: out("pactl", &["get-default-sink"]).unwrap_or_default().trim().to_string(),
        default_source: out("pactl", &["get-default-source"]).unwrap_or_default().trim().to_string(),
        error: String::new(),
    }
}

fn target(source: bool) -> &'static str {
    if source {
        "@DEFAULT_AUDIO_SOURCE@"
    } else {
        "@DEFAULT_AUDIO_SINK@"
    }
}

// Posuvník generuje veľa zmien za sekundu: posledná hodnota vyhráva, `wpctl` beží najviac jeden naraz.
static PENDING: Mutex<[Option<u32>; 2]> = Mutex::new([None, None]);
static RUNNING: AtomicBool = AtomicBool::new(false);

fn volume_worker(limit: f64) {
    loop {
        let job = {
            let mut p = PENDING.lock().unwrap_or_else(|e| e.into_inner());
            let i = if p[0].is_some() { 0 } else { 1 };
            p[i].take().map(|v| (i == 1, v))
        };
        match job {
            Some((source, pct)) => {
                let lim = format!("{limit}");
                run_quiet("wpctl", &["set-volume", "-l", &lim, target(source), &format!("{pct}%")]);
            }
            None => {
                RUNNING.store(false, Ordering::SeqCst);
                let more = PENDING.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|p| p.is_some());
                if more && !RUNNING.swap(true, Ordering::SeqCst) {
                    continue;
                }
                return;
            }
        }
    }
}

/// Vykoná príkaz na pozadí; stav zariadení príde ako `Msg::AudioDev`.
pub fn run(cmd: AudioCmd, limit: f64, tx: Tx<Msg>) {
    match cmd {
        AudioCmd::Volume { source, pct } => {
            PENDING.lock().unwrap_or_else(|e| e.into_inner())[source as usize] = Some(pct);
            if !RUNNING.swap(true, Ordering::SeqCst) {
                std::thread::spawn(move || volume_worker(limit));
            }
        }
        AudioCmd::Load => {
            std::thread::spawn(move || tx.send(Msg::AudioDev(load_state())));
        }
        AudioCmd::ToggleMute { source } => {
            std::thread::spawn(move || {
                run_quiet("wpctl", &["set-mute", target(source), "toggle"]);
                tx.send(Msg::AudioDev(load_state()));
            });
        }
        AudioCmd::SetDefault { source, name } => {
            std::thread::spawn(move || {
                run_quiet("pactl", &[if source { "set-default-source" } else { "set-default-sink" }, &name]);
                tx.send(Msg::AudioDev(load_state()));
            });
        }
    }
}

/// Glyf (Nerd Font) tlačidla stlmenia.
pub fn icon(source: bool, muted: bool, volume: u32) -> &'static str {
    match (source, muted || volume == 0) {
        (false, true) => "\u{f0581}",
        (false, false) => {
            if volume < 34 {
                "\u{f057f}"
            } else if volume < 67 {
                "\u{f0580}"
            } else {
                "\u{f057e}"
            }
        }
        (true, true) => "\u{f036d}",
        (true, false) => "\u{f036c}",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SINKS: &str = r#"[{"index":50,"state":"RUNNING","name":"alsa_output.pci-0000_00_1f.3.analog-stereo","description":"Built-in Audio Analog Stereo","mute":false,"volume":{"front-left":{"value":39322,"value_percent":"60%","db":"-13.30 dB"},"front-right":{"value":39322,"value_percent":"60%","db":"-13.30 dB"}},"properties":{"device.class":"sound"}},{"index":51,"name":"bluez_output.AA","description":"Slúchadlá","mute":true,"volume":{"mono":{"value":65536,"value_percent":"100%"}},"properties":{}}]"#;
    const SOURCES: &str = r#"[{"index":1,"name":"alsa_output.pci.analog-stereo.monitor","description":"Monitor of Built-in","mute":false,"volume":{"front-left":{"value":65536,"value_percent":"100%"}},"properties":{"device.class":"monitor"}},{"index":2,"name":"alsa_input.pci.analog-stereo","description":"Mikrofón","mute":false,"volume":{"front-left":{"value":32768,"value_percent":"50%"}},"properties":{"device.class":"sound"}}]"#;

    #[test]
    fn parses_sinks() {
        let v = parse_devices(SINKS, false);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].volume, v[0].mute, v[0].desc.as_str()), (60, false, "Built-in Audio Analog Stereo"));
        assert_eq!((v[1].volume, v[1].mute), (100, true));
    }

    #[test]
    fn sources_skip_monitors() {
        let v = parse_devices(SOURCES, true);
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].desc.as_str(), v[0].volume), ("Mikrofón", 50));
        // bez filtra ostáva aj monitor
        assert_eq!(parse_devices(SOURCES, false).len(), 2);
    }

    #[test]
    fn bad_input_is_empty() {
        assert!(parse_devices("", false).is_empty());
        assert!(parse_devices("{}", false).is_empty());
    }

    #[test]
    fn config_and_icons() {
        let c = AudioCfg::from_table(&Table::new());
        assert_eq!((c.width, c.rows), (380.0, 6));
        assert_ne!(icon(false, true, 50), icon(false, false, 50));
        assert_ne!(icon(true, false, 50), icon(false, false, 50));
    }

    #[test]
    fn null_description_falls_back() {
        let j = r#"[{"name":"alsa_output.pci.analog-stereo","description":"(null)","mute":false,"volume":{"front-left":{"value":65536,"value_percent":"100%"}},"properties":{"node.description":"Vstavaný zvuk Analog Stereo"}},{"name":"x.y","description":"(null)","mute":false,"volume":{},"properties":{}}]"#;
        let v = parse_devices(j, false);
        assert_eq!(v[0].desc, "Vstavaný zvuk Analog Stereo");
        assert_eq!(v[1].desc, "x.y");
    }
}
