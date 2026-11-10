//! Natívne systémové zdroje pre položky menu: `exec = "builtin:ZDROJ[:ARGUMENT]"`.
//! Nahrádzajú skripty (`cpu.sh`, `ram.sh`, `net.sh`, `disk.sh`, `disk2.sh`, `temp.sh`) – bez spúšťania procesov,
//! výstup má rovnaký tvar. Beží vo vlákne, ktoré spúšťa `exec` položky (otvorené menu, `interval`), nie cyklicky.
//!
//! | Zdroj | Výstup |
//! |---|---|
//! | `cpu` | vyťaženie v % (celé číslo), rozdiel medzi dvoma vzorkami `/proc/stat` |
//! | `ram` | využitá pamäť (MemTotal − MemAvailable), `2.35 GB` (1 GB = 1024³ B) / `850 MB` |
//! | `ram-pct`, `swap` | % využitej pamäte, využitý swap |
//! | `temp[:NÁZOV]` | najvyššia teplota, `52 °C`; bez názvu z thermal zón, s názvom z hwmon / zón podľa mena |
//! | `disk[:CESTA]` | obsadené miesto ako v `df` / GNOME (1 GB = 10⁹ B) a obsadenosť v % ako `df --output=pcent`, oddelené väčšou medzerou: `8.4 GB     7 %` |
//! | `disk-free[:CESTA]`, `disk-total[:CESTA]` | voľné miesto (pre bežného používateľa), veľkosť |
//! | `net-down`, `net-up` | rýchlosť rozhrania s predvolenou trasou: `0 kB/s`, `1.5 MB/s` |
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Nastavenia zo `[system]`.
#[derive(Clone, Debug)]
pub struct Opts {
    /// desatinný oddeľovač vo výstupe (`","` ako pri slovenskej lokalite, `"."`)
    pub decimal: String,
    /// rozhranie pre `net-*`; prázdne = rozhranie s predvolenou trasou
    pub net_iface: String,
}

impl Default for Opts {
    fn default() -> Self {
        Opts { decimal: ",".into(), net_iface: String::new() }
    }
}

/// Vyhodnotí zdroj (`spec` bez predpony `builtin:`). Neznámy zdroj → `?`, nedostupný údaj → `n/a`.
pub fn eval(spec: &str, o: &Opts) -> String {
    let spec = spec.trim();
    let (name, arg) = match spec.split_once(':') {
        Some((n, a)) => (n, a),
        None => (spec, ""),
    };
    let out = match name {
        "cpu" => cpu().map(|p| p.to_string()),
        "ram" => meminfo().map(|m| fmt_ram(m.used())),
        "ram-pct" => meminfo().map(|m| (m.used() * 100 / m.total.max(1)).to_string()),
        "swap" => meminfo().map(|m| fmt_ram(m.swap_used())),
        "temp" => temp(arg).map(|t| format!("{} °C", t / 1000)),
        "disk" => disk(arg).map(|d| fmt_disk(&d)),
        "disk-free" => disk(arg).map(|d| fmt_si(d.avail)),
        "disk-total" => disk(arg).map(|d| fmt_si(d.total)),
        "net-down" => net_rate("down", 0, &o.net_iface).map(fmt_rate),
        "net-up" => net_rate("up", 8, &o.net_iface).map(fmt_rate),
        _ => return "?".into(),
    };
    let s = out.unwrap_or_else(|| "n/a".into());
    if o.decimal == "." {
        s
    } else {
        s.replace('.', &o.decimal)
    }
}

// ------------------------------------------------------------- formáty

/// Obsadené miesto ako vo výstupe `disk.sh` (SI jednotky, 1 desatinné miesto).
pub fn fmt_si(bytes: u64) -> String {
    let b = bytes as f64;
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", b / 1e9)
    } else if bytes >= 1_000_000 {
        format!("{:.1} MB", b / 1e6)
    } else {
        format!("{:.1} kB", b / 1e3)
    }
}

/// Pamäť: GB (1024³) na 2 desatinné miesta, pod 1 GB celé MB.
pub fn fmt_ram(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        format!("{:.2} GB", bytes as f64 / 1_073_741_824.0)
    } else {
        format!("{:.0} MB", bytes as f64 / 1_048_576.0)
    }
}

/// Rýchlosť siete ako `net.sh`: od 1 MB/s `1.5 MB/s`, inak celé kB/s (1 kB = 1024 B).
pub fn fmt_rate(bytes_per_s: f64) -> String {
    if bytes_per_s >= 1_048_576.0 {
        format!("{:.1} MB/s", bytes_per_s / 1_048_576.0)
    } else {
        format!("{} kB/s", (bytes_per_s.max(0.0) / 1024.0) as u64)
    }
}

/// Počet medzier medzi údajom v GB a v % pri zdroji `disk`.
const DISK_GAP: usize = 5;

/// `disk`: obsadené miesto a obsadenosť v %, oddelené väčšou medzerou (`8.4 GB     7 %`).
pub fn fmt_disk(d: &Disk) -> String {
    format!("{}{}{} %", fmt_si(d.used), " ".repeat(DISK_GAP), pct_ceil(d.used as u128, d.avail as u128))
}

/// Percento obsadenosti ako `df`: `used / (used + avail)` zaokrúhlené nahor.
pub fn pct_ceil(used: u128, avail: u128) -> u64 {
    let total = used + avail;
    if total == 0 {
        return 0;
    }
    let u100 = used * 100;
    (u100 / total + u128::from(u100 % total != 0)) as u64
}

// ----------------------------------------------------------------- CPU

static CPU_PREV: Mutex<Option<(Instant, (u64, u64))>> = Mutex::new(None);

/// (súčet, nečinnosť) z prvého riadku `/proc/stat` – rovnaké polia ako `cpu.sh` (bez steal/guest).
pub fn parse_cpu_times(stat: &str) -> Option<(u64, u64)> {
    let line = stat.lines().next()?;
    let mut it = line.split_whitespace();
    if it.next()? != "cpu" {
        return None;
    }
    let v: Vec<u64> = it.take(7).filter_map(|x| x.parse().ok()).collect();
    if v.len() < 7 {
        return None;
    }
    Some((v.iter().sum(), v[3] + v[4]))
}

/// Vyťaženie v % medzi dvoma vzorkami (celé číslo, ako v `cpu.sh`).
pub fn cpu_delta(a: (u64, u64), b: (u64, u64)) -> u64 {
    let total = b.0.saturating_sub(a.0);
    let idle = b.1.saturating_sub(a.1);
    if total == 0 {
        0
    } else {
        100 * total.saturating_sub(idle) / total
    }
}

fn cpu_now() -> Option<(u64, u64)> {
    parse_cpu_times(&std::fs::read_to_string("/proc/stat").ok()?)
}

/// Rozdiel oproti predošlému vyhodnoteniu (keď je čerstvé: 0,2–6 s); inak sa odoberú dve vzorky s odstupom 0,5 s.
fn cpu() -> Option<u64> {
    let (now, cur) = (Instant::now(), cpu_now()?);
    let prev = CPU_PREV.lock().ok().and_then(|mut g| g.replace((now, cur)));
    if let Some((t, p)) = prev {
        let age = now.duration_since(t);
        if age >= Duration::from_millis(200) && age <= Duration::from_secs(6) {
            return Some(cpu_delta(p, cur));
        }
    }
    std::thread::sleep(Duration::from_millis(500));
    let (now2, cur2) = (Instant::now(), cpu_now()?);
    if let Ok(mut g) = CPU_PREV.lock() {
        *g = Some((now2, cur2));
    }
    Some(cpu_delta(cur, cur2))
}

// ------------------------------------------------------------- pamäť

#[derive(Debug, Default, PartialEq)]
pub struct MemInfo {
    pub total: u64,
    pub available: u64,
    pub swap_total: u64,
    pub swap_free: u64,
}

impl MemInfo {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }
    pub fn swap_used(&self) -> u64 {
        self.swap_total.saturating_sub(self.swap_free)
    }
}

/// `/proc/meminfo` → bajty (hodnoty sú v kB = 1024 B).
pub fn parse_meminfo(s: &str) -> MemInfo {
    let mut m = MemInfo::default();
    for l in s.lines() {
        let mut it = l.split_whitespace();
        let (Some(k), Some(v)) = (it.next(), it.next().and_then(|v| v.parse::<u64>().ok())) else { continue };
        let b = v * 1024;
        match k {
            "MemTotal:" => m.total = b,
            "MemAvailable:" => m.available = b,
            "SwapTotal:" => m.swap_total = b,
            "SwapFree:" => m.swap_free = b,
            _ => {}
        }
    }
    m
}

fn meminfo() -> Option<MemInfo> {
    let m = parse_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?);
    (m.total > 0).then_some(m)
}

// ------------------------------------------------------------- disk

pub struct Disk {
    pub used: u64,
    pub avail: u64,
    pub total: u64,
}

fn disk(path: &str) -> Option<Disk> {
    let path = if path.is_empty() { "/".to_string() } else { expand_home(path) };
    let s = crate::sys::statvfs(&path)?;
    let fr = s.frsize;
    Some(Disk { used: s.blocks.saturating_sub(s.bfree) * fr, avail: s.bavail * fr, total: s.blocks * fr })
}

fn expand_home(p: &str) -> String {
    match (p.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(h)) if !h.is_empty() => format!("{h}/{rest}"),
        _ => p.to_string(),
    }
}

// ---------------------------------------------------------------- sieť

static NET_PREV: Mutex<Vec<(String, Instant, u64)>> = Mutex::new(Vec::new());

/// Rozhranie s predvolenou trasou z `/proc/net/route` (cieľ aj maska samé nuly).
pub fn default_iface(route: &str) -> Option<String> {
    route.lines().skip(1).find_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        (f.len() >= 8 && f[1] == "00000000" && f[7] == "00000000").then(|| f[0].to_string())
    })
}

/// Počítadlo bajtov rozhrania z `/proc/net/dev`: stĺpec 0 = prijaté, 8 = odoslané.
pub fn parse_net_dev(s: &str, iface: &str, col: usize) -> Option<u64> {
    s.lines().find_map(|l| {
        let (name, rest) = l.split_once(':')?;
        if name.trim() != iface {
            return None;
        }
        rest.split_whitespace().nth(col)?.parse().ok()
    })
}

fn net_bytes(iface: &str, col: usize) -> Option<u64> {
    parse_net_dev(&std::fs::read_to_string("/proc/net/dev").ok()?, iface, col)
}

/// Rýchlosť (B/s) v rozhraní; ako pri CPU – z predošlého vyhodnotenia alebo z dvoch vzoriek po 0,5 s.
fn net_rate(key: &str, col: usize, forced: &str) -> Option<f64> {
    let iface = if forced.is_empty() { default_iface(&std::fs::read_to_string("/proc/net/route").ok()?)? } else { forced.to_string() };
    let tag = format!("{key}:{iface}");
    let (now, cur) = (Instant::now(), net_bytes(&iface, col)?);
    let prev = NET_PREV.lock().ok().and_then(|mut g| {
        let old = g.iter().position(|e| e.0 == tag).map(|i| g.remove(i));
        g.push((tag.clone(), now, cur));
        old
    });
    if let Some((_, t, b)) = prev {
        let age = now.duration_since(t);
        if age >= Duration::from_millis(200) && age <= Duration::from_secs(6) {
            return Some(cur.saturating_sub(b) as f64 / age.as_secs_f64());
        }
    }
    std::thread::sleep(Duration::from_millis(500));
    let (now2, cur2) = (Instant::now(), net_bytes(&iface, col)?);
    if let Ok(mut g) = NET_PREV.lock() {
        g.retain(|e| e.0 != tag);
        g.push((tag, now2, cur2));
    }
    Some(cur2.saturating_sub(cur) as f64 / now2.duration_since(now).as_secs_f64().max(0.001))
}

// ------------------------------------------------------------ teplota

/// Záznam snímača: (zo thermal zóny?, názov, teplota v m°C).
pub type Sensor = (bool, String, i64);

/// Bez filtra najvyššia teplota z thermal zón (ako `temp.sh`), inak z hwmon; s filtrom najvyššia zo snímačov,
/// ktorých názov obsahuje `filter` (bez ohľadu na veľkosť písmen).
pub fn pick_temp(sensors: &[Sensor], filter: &str) -> Option<i64> {
    let f = filter.trim().to_lowercase();
    let pool: Vec<i64> = if f.is_empty() {
        let zones: Vec<i64> = sensors.iter().filter(|s| s.0).map(|s| s.2).collect();
        if zones.is_empty() { sensors.iter().map(|s| s.2).collect() } else { zones }
    } else {
        sensors.iter().filter(|s| s.1.to_lowercase().contains(&f)).map(|s| s.2).collect()
    };
    pool.into_iter().filter(|t| *t > 0).max()
}

fn read_trim(p: &std::path::Path) -> Option<String> {
    Some(std::fs::read_to_string(p).ok()?.trim().to_string())
}

fn read_sensors() -> Vec<Sensor> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir("/sys/class/thermal") {
        for e in rd.flatten() {
            let p = e.path();
            if !e.file_name().to_string_lossy().starts_with("thermal_zone") {
                continue;
            }
            if let Some(t) = read_trim(&p.join("temp")).and_then(|v| v.parse::<i64>().ok()) {
                out.push((true, read_trim(&p.join("type")).unwrap_or_default(), t));
            }
        }
    }
    if let Ok(rd) = std::fs::read_dir("/sys/class/hwmon") {
        for e in rd.flatten() {
            let dir = e.path();
            let chip = read_trim(&dir.join("name")).unwrap_or_default();
            let Ok(files) = std::fs::read_dir(&dir) else { continue };
            for f in files.flatten() {
                let fname = f.file_name().to_string_lossy().into_owned();
                let Some(stem) = fname.strip_suffix("_input").filter(|s| s.starts_with("temp")) else { continue };
                if let Some(t) = read_trim(&f.path()).and_then(|v| v.parse::<i64>().ok()) {
                    let label = read_trim(&dir.join(format!("{stem}_label"))).unwrap_or_default();
                    out.push((false, format!("{chip} {label}").trim().to_string(), t));
                }
            }
        }
    }
    out
}

fn temp(filter: &str) -> Option<i64> {
    pick_temp(&read_sensors(), filter)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_formats_like_gnome() {
        assert_eq!(fmt_si(8_400_000_000), "8.4 GB");
        assert_eq!(fmt_si(63_200_000), "63.2 MB");
        assert_eq!(fmt_si(950), "0.9 kB");
        assert_eq!(fmt_si(189_800_000_000), "189.8 GB");
    }

    #[test]
    fn disk_shows_size_and_percent_with_gap() {
        let d = Disk { used: 8_400_000_000, avail: 111_600_000_000, total: 120_000_000_000 };
        assert_eq!(fmt_disk(&d), "8.4 GB     7 %");
        let d = Disk { used: 71, avail: 929, total: 1000 };
        assert_eq!(fmt_disk(&d), "0.1 kB     8 %");
    }

    #[test]
    fn df_percent_rounds_up() {
        assert_eq!(pct_ceil(7, 93), 7);
        assert_eq!(pct_ceil(71, 929), 8); // 7,1 % → 8 %
        assert_eq!(pct_ceil(0, 0), 0);
        assert_eq!(pct_ceil(100, 0), 100);
    }

    #[test]
    fn eval_replaces_decimal_point() {
        let o = Opts::default();
        assert_eq!(eval("nonsense", &o), "?");
        assert_eq!(fmt_ram(2_523_000_000).replace('.', &o.decimal), "2,35 GB");
    }

    #[test]
    fn ram_and_rate_formats() {
        assert_eq!(fmt_ram(2_523_136_000), "2.35 GB");
        assert_eq!(fmt_ram(800 * 1_048_576), "800 MB");
        assert_eq!(fmt_rate(0.0), "0 kB/s");
        assert_eq!(fmt_rate(2048.0 * 10.0), "20 kB/s");
        assert_eq!(fmt_rate(1_572_864.0), "1.5 MB/s");
    }

    #[test]
    fn cpu_parsing_and_delta() {
        let a = parse_cpu_times("cpu  100 0 100 700 100 0 0 0 0 0\ncpu0 1 2 3").unwrap();
        assert_eq!(a, (1000, 800));
        let b = parse_cpu_times("cpu  200 0 200 750 150 0 0 0 0 0").unwrap();
        assert_eq!(cpu_delta(a, b), 66); // súčet +300, nečinnosť +100
        assert_eq!(cpu_delta(a, a), 0);
        assert!(parse_cpu_times("intr 1 2").is_none());
    }

    #[test]
    fn meminfo_parsing() {
        let m = parse_meminfo("MemTotal:       16000000 kB\nMemFree: 1 kB\nMemAvailable:   12000000 kB\nSwapTotal: 100 kB\nSwapFree: 40 kB\n");
        assert_eq!(m.total, 16_000_000 * 1024);
        assert_eq!(m.used(), 4_000_000 * 1024);
        assert_eq!(m.swap_used(), 60 * 1024);
    }

    #[test]
    fn route_and_netdev() {
        let route = "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\nwlan0\t00000000\t0100A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\nwlan0\t0000A8C0\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0\n";
        assert_eq!(default_iface(route).as_deref(), Some("wlan0"));
        let dev = "Inter-|   Receive\n face |bytes packets\n    lo: 1 2 3 4 5 6 7 8 9 10 11 12\n wlan0:1234567890 22 0 0 0 0 0 0 555 3 0 0 0 0 0 0\n";
        assert_eq!(parse_net_dev(dev, "wlan0", 0), Some(1_234_567_890));
        assert_eq!(parse_net_dev(dev, "wlan0", 8), Some(555));
        assert_eq!(parse_net_dev(dev, "eth9", 0), None);
    }

    #[test]
    fn temperature_picking() {
        let s: Vec<Sensor> = vec![(true, "acpitz".into(), 41_000), (true, "x86_pkg_temp".into(), 55_000), (false, "coretemp Package id 0".into(), 60_000)];
        assert_eq!(pick_temp(&s, ""), Some(55_000));
        assert_eq!(pick_temp(&s, "CORETEMP"), Some(60_000));
        assert_eq!(pick_temp(&s, "nvme"), None);
        // bez thermal zón sa berie hwmon
        assert_eq!(pick_temp(&s[2..], ""), Some(60_000));
    }
}
