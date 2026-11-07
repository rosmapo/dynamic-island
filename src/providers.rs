//! Zdroje dát: každý modul beží vo vlastnom vlákne a posiela `Msg` do hlavnej slučky.
use crate::config::{Config, CustomCfg};
use crate::data::*;
use crate::sys::{self, Tx};
use crate::wayfire;
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

pub fn spawn_all(cfg: &Config, tx: &Tx<Msg>) {
    spawn_net(cfg.clone(), tx.clone());
    spawn_audio(tx.clone());
    spawn_fast(cfg.clone(), tx.clone());
    for (i, c) in cfg.custom.iter().enumerate() {
        if c.exec.is_empty() {
            continue; // statické tlačidlo (`text`), netreba proces
        }
        spawn_custom(i, c.clone(), tx.clone());
    }
    if cfg.auto.enabled {
        wayfire::spawn_autohide(cfg.auto.clone(), tx.clone());
    }
}

fn sh(cmd: &str) -> Command {
    let mut c = Command::new("sh");
    c.arg("-c").arg(cmd).stdin(Stdio::null());
    c
}

/// Spustí príkaz tak, aby zanikol spolu s ostrovčekom.
fn with_parent_death(c: &mut Command) -> &mut Command {
    unsafe {
        c.pre_exec(|| {
            sys::die_with_parent();
            Ok(())
        })
    }
}

fn run_stdout(prog: &str, args: &[&str]) -> Option<String> {
    let o = Command::new(prog).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    if o.status.success() {
        Some(String::from_utf8_lossy(&o.stdout).into_owned())
    } else {
        None
    }
}

// -------------------------------------------------------------------- sieť

fn read_trim(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// Parsuje riadok z /proc/net/wireless pre rozhranie -> dBm.
pub fn parse_proc_wireless(src: &str, iface: &str) -> Option<i32> {
    for line in src.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix(iface).and_then(|r| r.strip_prefix(':')) {
            let f: Vec<&str> = rest.split_whitespace().collect();
            // status link level noise ...
            let lvl = f.get(2)?.trim_end_matches('.');
            let v: f64 = lvl.parse().ok()?;
            // niektoré ovládače uvádzajú 0..255 (-256 offset)
            return Some(if v > 0.0 { (v - 256.0) as i32 } else { v as i32 });
        }
    }
    None
}

pub fn dbm_to_pct(dbm: i32) -> u32 {
    ((dbm + 100) * 2).clamp(0, 100) as u32
}

/// Z výstupu `iw dev X link`: (ssid, frekvencia GHz, dBm)
pub fn parse_iw_link(s: &str) -> (String, String, Option<i32>) {
    let (mut ssid, mut freq, mut dbm) = (String::new(), String::new(), None);
    for l in s.lines() {
        let l = l.trim();
        if let Some(v) = l.strip_prefix("SSID:") {
            ssid = v.trim().to_string();
        } else if let Some(v) = l.strip_prefix("freq:") {
            if let Ok(f) = v.trim().parse::<f64>() {
                freq = format!("{:.3}", f / 1000.0).trim_end_matches('0').trim_end_matches('.').to_string();
            }
        } else if let Some(v) = l.strip_prefix("signal:") {
            dbm = v.split_whitespace().next().and_then(|x| x.parse().ok());
        }
    }
    (ssid, freq, dbm)
}

/// IPv4 adresa z `ip -4 -o addr show dev X`.
pub fn parse_ip_addr(s: &str) -> String {
    for l in s.lines() {
        let f: Vec<&str> = l.split_whitespace().collect();
        if let Some(p) = f.iter().position(|x| *x == "inet") {
            if let Some(a) = f.get(p + 1) {
                return a.split('/').next().unwrap_or("").to_string();
            }
        }
    }
    String::new()
}

fn parse_gateway(s: &str, _iface: &str) -> String {
    // "default via 192.168.1.1 dev wlan0 ..."
    for l in s.lines() {
        let f: Vec<&str> = l.split_whitespace().collect();
        if let Some(p) = f.iter().position(|x| *x == "via") {
            return f.get(p + 1).unwrap_or(&"").to_string();
        }
    }
    String::new()
}

/// SSID bez `iw`: NetworkManager (nmcli) alebo wireless-tools (iwgetid).
fn wifi_ssid_fallback(iface: &str) -> String {
    for (prog, args) in [
        ("iwgetid", vec!["-r", iface]),
        ("/usr/sbin/iwgetid", vec!["-r", iface]),
        ("nmcli", vec!["-g", "GENERAL.CONNECTION", "device", "show", iface]),
    ] {
        if let Some(s) = run_stdout(prog, &args) {
            let s = s.trim();
            if !s.is_empty() && s != "--" {
                return s.to_string();
            }
        }
    }
    String::new()
}

fn ip_of(iface: &str) -> String {
    run_stdout("ip", &["-4", "-o", "addr", "show", "dev", iface]).map(|s| parse_ip_addr(&s)).unwrap_or_default()
}

fn glob_match(pat: &str, name: &str) -> bool {
    match pat.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => pat == name,
    }
}

fn spawn_net(cfg: Config, tx: Tx<Msg>) {
    std::thread::spawn(move || {
        let iface = cfg.net_iface.clone();
        let mut last_net: Option<NetInfo> = None;
        let mut last_vpn: Option<Option<VpnInfo>> = None;
        let mut slow: u32 = 0;
        let mut cached = NetInfo::default();
        loop {
            // ---- hlavné rozhranie
            let base = format!("/sys/class/net/{iface}");
            let wifi = std::path::Path::new(&format!("{base}/wireless")).exists();
            let up = read_trim(&format!("{base}/operstate")).map(|s| s == "up").unwrap_or(false);
            let mut n = NetInfo { ifname: iface.clone(), wifi, up, ..Default::default() };
            if up {
                let mut dbm = std::fs::read_to_string("/proc/net/wireless").ok().and_then(|s| parse_proc_wireless(&s, &iface));
                if slow == 0 || cached.ifname != iface || !cached.up {
                    // pomalšie údaje (SSID, IP, brána) len občas
                    cached = n.clone();
                    cached.up = true;
                    if wifi {
                        let out = run_stdout("iw", &["dev", &iface, "link"]).or_else(|| run_stdout("/usr/sbin/iw", &["dev", &iface, "link"]));
                        if let Some(s) = out {
                            let (ssid, freq, d) = parse_iw_link(&s);
                            cached.ssid = ssid;
                            cached.freq_ghz = freq;
                            if dbm.is_none() {
                                dbm = d;
                            }
                        }
                        if cached.ssid.is_empty() {
                            cached.ssid = wifi_ssid_fallback(&iface);
                        }
                    }
                    cached.ip = ip_of(&iface);
                    cached.gateway = run_stdout("ip", &["-4", "route", "show", "default", "dev", &iface])
                        .map(|s| parse_gateway(&s, &iface))
                        .unwrap_or_default();
                }
                n.ssid = cached.ssid.clone();
                n.freq_ghz = cached.freq_ghz.clone();
                n.ip = cached.ip.clone();
                n.gateway = cached.gateway.clone();
                if let Some(d) = dbm {
                    n.dbm = d;
                    n.signal_pct = dbm_to_pct(d);
                }
            } else {
                cached = NetInfo::default();
            }
            slow = (slow + 1) % 5;
            if last_net.as_ref() != Some(&n) {
                last_net = Some(n.clone());
                tx.send(Msg::Net(n));
            }

            // ---- VPN
            let mut vpn: Option<VpnInfo> = None;
            if let Ok(rd) = std::fs::read_dir("/sys/class/net") {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if cfg.vpn_ifaces.iter().any(|p| glob_match(p, &name)) {
                        let flags = read_trim(&format!("/sys/class/net/{name}/flags"))
                            .and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                            .unwrap_or(0);
                        if flags & 1 != 0 {
                            vpn = Some(VpnInfo { ip: ip_of(&name), ifname: name });
                            break;
                        }
                    }
                }
            }
            if last_vpn.as_ref() != Some(&vpn) {
                last_vpn = Some(vpn.clone());
                tx.send(Msg::Vpn(vpn));
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

// --------------------------------------------------------------------- zvuk

/// "Volume: 0.45 [MUTED]" -> (muted, volume)
pub fn parse_wpctl(s: &str) -> Option<(bool, f32)> {
    let rest = s.trim().strip_prefix("Volume:")?;
    let vol: f32 = rest.split_whitespace().next()?.parse().ok()?;
    Some((rest.contains("[MUTED]"), vol))
}

fn read_audio() -> Option<(bool, f32)> {
    run_stdout("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]).and_then(|s| parse_wpctl(&s))
}

/// PID bežiaceho `pactl subscribe` (0 = žiadny) a príznak, že sa ostrovček chystá na `exec()` (reštart).
static AUDIO_SUB_PID: AtomicI32 = AtomicI32::new(0);
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

/// Volať tesne pred `exec()` (reštart po zmene configu): `exec` zachová PID procesu, takže nevyzdvihnutý potomok
/// (`pactl subscribe`, ktorého rodičovské vlákno `exec` zničí) by ostal ako zombie `[pactl] <defunct>` – nové vlákno
/// by naň nikdy nezavolalo `wait`. Preto sa `pactl` ukončí a vlákno zvuku ho ešte stihne vyzdvihnúť.
pub fn stop_children() {
    SHUTTING_DOWN.store(true, Ordering::SeqCst);
    sys::terminate(AUDIO_SUB_PID.load(Ordering::SeqCst));
    std::thread::sleep(Duration::from_millis(150));
}

fn spawn_audio(tx: Tx<Msg>) {
    std::thread::spawn(move || {
        let mut last: Option<(bool, f32)> = None;
        let emit = |last: &mut Option<(bool, f32)>| {
            if let Some(a) = read_audio() {
                if *last != Some(a) {
                    *last = Some(a);
                    tx.send(Msg::Audio { muted: a.0, volume: a.1 });
                }
            }
        };
        loop {
            if SHUTTING_DOWN.load(Ordering::SeqCst) {
                return;
            }
            emit(&mut last);
            // udalosti z PipeWire/PulseAudio; ak pactl nie je, zálohuj pollingom
            let child = with_parent_death(Command::new("pactl").arg("subscribe"))
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();
            match child {
                Ok(mut ch) => {
                    AUDIO_SUB_PID.store(ch.id() as i32, Ordering::SeqCst);
                    if SHUTTING_DOWN.load(Ordering::SeqCst) {
                        // reštart začal medzitým – `pactl` hneď ukončiť a vyzdvihnúť
                        let _ = ch.kill();
                    }
                    if let Some(out) = ch.stdout.take() {
                        for line in BufReader::new(out).lines().map_while(Result::ok) {
                            if line.contains("sink") || line.contains("server") {
                                emit(&mut last);
                            }
                        }
                    }
                    let _ = ch.wait();
                    AUDIO_SUB_PID.store(0, Ordering::SeqCst);
                    if SHUTTING_DOWN.load(Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
                Err(_) => std::thread::sleep(Duration::from_secs(1)),
            }
        }
    });
}

// ------------------------------------------------- caps / inhibitor / rec / layout

fn caps_led() -> Option<bool> {
    let rd = std::fs::read_dir("/sys/class/leds").ok()?;
    let mut found = false;
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if n.ends_with("::capslock") {
            found = true;
            if let Some(b) = read_trim(&format!("/sys/class/leds/{n}/brightness")) {
                if b != "0" {
                    return Some(true);
                }
            }
        }
    }
    found.then_some(false)
}

fn spawn_fast(cfg: Config, tx: Tx<Msg>) {
    std::thread::spawn(move || {
        let mut caps: Option<bool> = None;
        let mut inh: Option<bool> = None;
        let mut rec: Option<String> = None;
        let mut layout: Option<String> = None;
        let mut ipc: Option<wayfire::Ipc> = None;
        let mut ipc_retry = 0u32;
        loop {
            if let Some(c) = caps_led() {
                if caps != Some(c) {
                    caps = Some(c);
                    tx.send(Msg::Caps(c));
                }
            }
            let i = std::path::Path::new(&cfg.inh_file).exists();
            if inh != Some(i) {
                inh = Some(i);
                tx.send(Msg::Inhibit(i));
            }
            let r = read_trim(&cfg.rec_file).unwrap_or_default();
            if rec.as_ref() != Some(&r) {
                rec = Some(r.clone());
                tx.send(Msg::Rec(r));
            }
            if ipc.is_none() {
                if ipc_retry == 0 {
                    ipc = wayfire::Ipc::connect();
                }
                ipc_retry = (ipc_retry + 1) % 20;
            }
            if let Some(conn) = ipc.as_mut() {
                match wayfire::keyboard_state(conn) {
                    Some((name, _, _)) => {
                        let s = wayfire::short_layout(&name, &cfg.kb_names);
                        if layout.as_ref() != Some(&s) {
                            layout = Some(s.clone());
                            tx.send(Msg::Layout(s));
                        }
                    }
                    None => ipc = None, // spojenie padlo alebo IPC pravidlá chýbajú
                }
            }
            std::thread::sleep(Duration::from_millis(cfg.poll_ms));
        }
    });
}

// ------------------------------------------------------------ custom moduly

pub fn parse_custom_line(line: &str, json: bool) -> CustomOut {
    let line = line.trim();
    if json {
        if let Some(j) = crate::json::parse(line) {
            let g = |k: &str| j.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
            return CustomOut { text: g("text"), tooltip: unescape_markup(&g("tooltip")), class: g("class") };
        }
    }
    CustomOut { text: line.to_string(), ..Default::default() }
}

fn spawn_custom(idx: usize, c: CustomCfg, tx: Tx<Msg>) {
    std::thread::spawn(move || loop {
        let cmd = c.exec.clone();
        if c.interval <= 0.0 {
            // trvalý proces: každý riadok = nový stav
            let child = with_parent_death(&mut sh(&cmd)).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
            if let Ok(mut ch) = child {
                if let Some(out) = ch.stdout.take() {
                    for line in BufReader::new(out).lines().map_while(Result::ok) {
                        tx.send(Msg::Custom(idx, parse_custom_line(&line, c.json)));
                    }
                }
                let _ = ch.wait();
            }
            std::thread::sleep(Duration::from_secs(3));
        } else {
            if let Ok(o) = with_parent_death(&mut sh(&cmd)).stderr(Stdio::null()).output() {
                let s = String::from_utf8_lossy(&o.stdout);
                let line = s.lines().last().unwrap_or("");
                tx.send(Msg::Custom(idx, parse_custom_line(line, c.json)));
            }
            std::thread::sleep(Duration::from_secs_f64(c.interval));
        }
    });
}


#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wireless_and_iw() {
        let w = "Inter-| sta-|   Quality        |   Discarded packets\n face | tus | link level noise |\n wlan0: 0000   70.  -40.  -256        0      0\n";
        assert_eq!(parse_proc_wireless(w, "wlan0"), Some(-40));
        assert_eq!(parse_proc_wireless(w, "wlan1"), None);
        assert_eq!(dbm_to_pct(-40), 100);
        assert_eq!(dbm_to_pct(-80), 40);
        let (s, f, d) = parse_iw_link("Connected to aa:bb\n\tSSID: Doma 5G\n\tfreq: 5180\n\tsignal: -52 dBm\n");
        assert_eq!((s.as_str(), f.as_str(), d), ("Doma 5G", "5.18", Some(-52)));
    }
    #[test]
    fn ip_and_audio() {
        assert_eq!(parse_ip_addr("3: wlan0    inet 192.168.1.23/24 brd 192.168.1.255 scope global"), "192.168.1.23");
        assert_eq!(parse_wpctl("Volume: 0.45 [MUTED]\n"), Some((true, 0.45)));
        assert_eq!(parse_wpctl("Volume: 1.00"), Some((false, 1.0)));
        assert!(glob_match("tun*", "tun0") && !glob_match("tun*", "wlan0") && glob_match("wg0", "wg0"));
    }
    #[test]
    fn custom_lines() {
        let o = parse_custom_line(r#"{"text":" 2","tooltip":"A &amp; B","class":"has-devices"}"#, true);
        assert_eq!(o.text, "\u{f287} 2");
        assert_eq!(o.tooltip, "A & B");
        assert_eq!(parse_custom_line("hi", false).text, "hi");
    }
}
