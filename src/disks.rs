//! Náhrada udiskie: UDisks2 cez systémovú zbernicu – automount, zoznam pripojených médií,
//! bezpečné odpojenie (unmount + eject/power-off) a notifikácie o tom.
use crate::data::{DiskInfo, Msg, Notification};
use crate::dbus::*;
use crate::sys::{self, PollFd, Tx, Waker, POLLIN};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

/// Príkazy z hlavnej slučky. Reťazec = objekt bloku (`DiskInfo::block`).
#[derive(Debug)]
pub enum DiskCmd {
    Eject(String),
    EjectAll,
}

#[derive(Clone, Copy, Debug)]
pub struct DiskCfg {
    pub automount: bool,
    pub notify_mount: bool,
    pub notify_unmount: bool,
    pub notify_error: bool,
    /// rovnaký pre všetky notifikácie diskov; 0 = nevyprší
    pub timeout_ms: i32,
}

const DEST: &str = "org.freedesktop.UDisks2";
const ROOT: &str = "/org/freedesktop/UDisks2";
const I_BLOCK: &str = "org.freedesktop.UDisks2.Block";
const I_FS: &str = "org.freedesktop.UDisks2.Filesystem";
const I_DRIVE: &str = "org.freedesktop.UDisks2.Drive";

type Props = HashMap<String, V>;
type Objects = HashMap<String, HashMap<String, Props>>;

pub fn spawn(cfg: DiskCfg, tx: Tx<Msg>, rx: Receiver<DiskCmd>, waker: Arc<Waker>) {
    std::thread::spawn(move || {
        let mut ids = 0x4000_0000u32;
        let mut quiet = false;
        loop {
            if let Err(e) = session(cfg, &tx, &rx, &waker, &mut ids) {
                if !quiet {
                    eprintln!("dynamic-island: disky nebežia: {e} (je nainštalovaný udisks2?)");
                    quiet = true;
                }
            }
            std::thread::sleep(Duration::from_secs(10));
        }
    });
}

// ------------------------------------------------------------ pomocné

fn unvar(v: V) -> V {
    match v {
        V::Var(b) => unvar(*b),
        o => o,
    }
}

fn gs(p: &Props, k: &str) -> String {
    match p.get(k) {
        Some(V::Str(s)) => s.clone(),
        _ => String::new(),
    }
}

fn gb(p: &Props, k: &str) -> bool {
    matches!(p.get(k), Some(V::Bool(true)))
}

fn bytes_to_string(v: &V) -> String {
    let mut out = Vec::new();
    if let V::Arr(a) = v {
        for b in a {
            if let V::Byte(c) = b {
                if *c == 0 {
                    break;
                }
                out.push(*c);
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn mounts_of(p: &Props) -> Vec<String> {
    match p.get("MountPoints") {
        Some(V::Arr(a)) => a.iter().map(bytes_to_string).filter(|s| !s.is_empty()).collect(),
        _ => Vec::new(),
    }
}

fn managed(c: &mut Conn) -> std::io::Result<Objects> {
    let r = c.call(DEST, ROOT, "org.freedesktop.DBus.ObjectManager", "GetManagedObjects", "", Vec::new())?;
    let v = Reader::new(&r.body, 0).value(b"a{oa{sa{sv}}}").ok_or_else(|| std::io::Error::other("neplatná odpoveď UDisks2"))?;
    Ok(parse_objects(v))
}

fn parse_objects(v: V) -> Objects {
    let mut out = Objects::new();
    if let V::Arr(items) = v {
        for it in items {
            let V::Dict(k, val) = it else { continue };
            let (V::Str(path), V::Arr(ifs)) = (*k, *val) else { continue };
            let mut m = HashMap::new();
            for i in ifs {
                let V::Dict(ik, iv) = i else { continue };
                let (V::Str(iname), V::Arr(props)) = (*ik, *iv) else { continue };
                let mut pm = Props::new();
                for p in props {
                    if let V::Dict(pk, pv) = p {
                        if let V::Str(name) = *pk {
                            pm.insert(name, unvar(*pv));
                        }
                    }
                }
                m.insert(iname, pm);
            }
            out.insert(path, m);
        }
    }
    out
}

fn label_of(block: &Props, drive: Option<&Props>) -> String {
    let l = gs(block, "IdLabel");
    if !l.is_empty() {
        return l;
    }
    if let Some(d) = drive {
        let s = format!("{} {}", gs(d, "Vendor").trim(), gs(d, "Model").trim()).trim().to_string();
        if !s.is_empty() {
            return s;
        }
    }
    let dev = bytes_to_string(block.get("PreferredDevice").unwrap_or(&V::Arr(Vec::new())));
    dev.rsplit('/').next().unwrap_or("").to_string()
}

struct Snap {
    disks: Vec<DiskInfo>,
    /// (blok, názov) ešte nepripojených vhodných súborových systémov
    unmounted: Vec<(String, String)>,
}

/// Externé (nie systémové) súborové systémy.
fn snapshot(o: &Objects) -> Snap {
    let mut groups: Vec<(String, DiskInfo)> = Vec::new();
    let mut unmounted = Vec::new();
    let mut paths: Vec<&String> = o.keys().filter(|p| p.contains("/block_devices/")).collect();
    paths.sort();
    for path in paths {
        let ifs = &o[path];
        let (Some(block), Some(fs)) = (ifs.get(I_BLOCK), ifs.get(I_FS)) else { continue };
        if gb(block, "HintIgnore") || gb(block, "HintSystem") {
            continue;
        }
        let drive = o.get(&gs(block, "Drive")).and_then(|d| d.get(I_DRIVE));
        let label = label_of(block, drive);
        let dev = bytes_to_string(block.get("Device").unwrap_or(&V::Arr(Vec::new())));
        let mounts = mounts_of(fs);
        if !mounts.is_empty() {
            // jeden záznam na fyzický disk (nie na oddiel)
            let key = gs(block, "Drive");
            let key = if key.is_empty() || key == "/" { path.clone() } else { key };
            let size = drive.and_then(|d| if let Some(V::U64(n)) = d.get("Size") { Some(*n) } else { None }).map(human_size).unwrap_or_default();
            match groups.iter_mut().find(|g| g.0 == key) {
                Some(g) => {
                    if !g.1.label.split(" + ").any(|l| l == label) {
                        g.1.label = format!("{} + {}", g.1.label, label);
                    }
                    g.1.mount = format!("{}, {}", g.1.mount, mounts.join(", "));
                }
                None => groups.push((key, DiskInfo { block: path.clone(), label, dev, mount: mounts.join(", "), size })),
            }
        } else {
            unmounted.push((path.clone(), label));
        }
    }
    Snap { disks: groups.into_iter().map(|g| g.1).collect(), unmounted }
}

/// Veľkosť ako v `lsblk` (1024-based): 14.6G, 500M, 1T.
fn human_size(n: u64) -> String {
    let units = ["B", "K", "M", "G", "T", "P"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < units.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    let s = format!("{v:.1}");
    format!("{}{}", s.strip_suffix(".0").unwrap_or(&s), units[u])
}

fn empty_opts() -> Vec<u8> {
    let mut w = W::default();
    w.array(8, |_| {});
    w.b
}

fn notice(tx: &Tx<Msg>, ids: &mut u32, timeout_ms: i32, summary: &str, body: &str, urgency: u8) {
    *ids = ids.wrapping_add(1).max(0x4000_0000);
    tx.send(Msg::Notif(Notification {
        id: *ids,
        app: "disks".into(),
        summary: summary.into(),
        body: body.into(),
        actions: Vec::new(),
        urgency,
        timeout_ms,
        icon: String::new(),
    }));
}

/// Text chyby bez dlhého názvu D-Bus chyby.
fn short_err(e: &std::io::Error) -> String {
    let s = e.to_string();
    s.rsplit(':').next().unwrap_or(&s).trim().to_string()
}

fn eject(c: &mut Conn, cfg: &DiskCfg, tx: &Tx<Msg>, ids: &mut u32, blocks: &[String]) -> std::io::Result<()> {
    let o = managed(c)?;
    // všetky pripojené časti rovnakých fyzických zariadení
    let mut drives: Vec<String> = Vec::new();
    let mut targets: Vec<String> = Vec::new();
    for b in blocks {
        if let Some(d) = o.get(b).and_then(|i| i.get(I_BLOCK)).map(|p| gs(p, "Drive")) {
            if !d.is_empty() && d != "/" && !drives.contains(&d) {
                drives.push(d);
            }
        }
        if !targets.contains(b) {
            targets.push(b.clone());
        }
    }
    for (p, ifs) in &o {
        if let (Some(bl), Some(fs)) = (ifs.get(I_BLOCK), ifs.get(I_FS)) {
            if drives.contains(&gs(bl, "Drive")) && !mounts_of(fs).is_empty() && !targets.contains(p) {
                targets.push(p.clone());
            }
        }
    }
    for t in &targets {
        let Some(ifs) = o.get(t) else { continue };
        if ifs.get(I_FS).map(|f| mounts_of(f).is_empty()).unwrap_or(true) {
            continue;
        }
        let label = label_of(&ifs[I_BLOCK], o.get(&gs(&ifs[I_BLOCK], "Drive")).and_then(|d| d.get(I_DRIVE)));
        if let Err(e) = c.call(DEST, t, I_FS, "Unmount", "a{sv}", empty_opts()) {
            if cfg.notify_error {
                notice(tx, ids, cfg.timeout_ms, &format!("Nepodarilo sa odpojiť {label}"), &short_err(&e), 2);
            }
            return Ok(());
        }
    }
    let mut label = String::new();
    for d in &drives {
        let Some(dp) = o.get(d).and_then(|i| i.get(I_DRIVE)) else { continue };
        label = format!("{} {}", gs(dp, "Vendor").trim(), gs(dp, "Model").trim()).trim().to_string();
        let method = if gb(dp, "CanPowerOff") {
            "PowerOff"
        } else if gb(dp, "Ejectable") {
            "Eject"
        } else {
            continue;
        };
        if let Err(e) = c.call(DEST, d, I_DRIVE, method, "a{sv}", empty_opts()) {
            if cfg.notify_error {
                notice(tx, ids, cfg.timeout_ms, "Zariadenie sa nepodarilo vysunúť", &short_err(&e), 2);
            }
            return Ok(());
        }
    }
    if cfg.notify_unmount {
        let name = targets.first().and_then(|t| o.get(t)).map(|i| label_of(&i[I_BLOCK], None)).filter(|s| !s.is_empty()).unwrap_or(label);
        notice(tx, ids, cfg.timeout_ms, "Médium odpojené", &format!("{name} môžeš bezpečne vybrať."), 1);
    }
    Ok(())
}

// ------------------------------------------------------------- relácia

fn session(cfg: DiskCfg, tx: &Tx<Msg>, rx: &Receiver<DiskCmd>, waker: &Waker, ids: &mut u32) -> std::io::Result<()> {
    let mut c = Conn::connect_system()?;
    c.set_read_timeout(Some(Duration::from_secs(120)));
    let mut w = W::default();
    w.string("type='signal',sender='org.freedesktop.UDisks2'");
    c.call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "AddMatch", "s", w.b)?;

    let mut tried: HashSet<String> = HashSet::new();
    let mut last: Option<Vec<DiskInfo>> = None;
    let mut need = true;
    let mut settle = false;
    loop {
        if need {
            need = false;
            let o = managed(&mut c)?;
            let snap = snapshot(&o);
            let present: HashSet<&String> = snap.unmounted.iter().map(|u| &u.0).chain(snap.disks.iter().map(|d| &d.block)).collect();
            tried.retain(|t| present.contains(t));
            let mut mounted_any = false;
            if cfg.automount {
                for (blk, label) in &snap.unmounted {
                    if !tried.insert(blk.clone()) {
                        continue;
                    }
                    match c.call(DEST, blk, I_FS, "Mount", "a{sv}", empty_opts()) {
                        Ok(_) => mounted_any = true,
                        Err(e) => {
                            if cfg.notify_error {
                                notice(tx, ids, cfg.timeout_ms, &format!("Nepodarilo sa pripojiť {label}"), &short_err(&e), 2);
                            }
                        }
                    }
                }
            }
            if mounted_any {
                // znovu načítať a oznámiť nové pripojenia
                let o = managed(&mut c)?;
                let snap2 = snapshot(&o);
                if cfg.notify_mount {
                    for d in &snap2.disks {
                        if !snap.disks.iter().any(|x| x.block == d.block) {
                            notice(tx, ids, cfg.timeout_ms, &format!("Pripojené: {}", d.label), &d.mount, 1);
                        }
                    }
                }
                publish(tx, &mut last, snap2.disks);
            } else {
                publish(tx, &mut last, snap.disks);
            }
        }

        let mut fds = [PollFd { fd: c.fd(), events: POLLIN, revents: 0 }, PollFd { fd: waker.fd(), events: POLLIN, revents: 0 }];
        let n = sys::poll_in(&mut fds, if settle { 250 } else { -1 });
        if n == 0 && settle {
            settle = false;
            need = true;
            continue;
        }
        if fds[1].revents & POLLIN != 0 {
            waker.drain();
            while let Ok(cmd) = rx.try_recv() {
                let blocks: Vec<String> = match cmd {
                    DiskCmd::Eject(b) => vec![b],
                    DiskCmd::EjectAll => last.as_ref().map(|l| l.iter().map(|d| d.block.clone()).collect()).unwrap_or_default(),
                };
                if !blocks.is_empty() {
                    eject(&mut c, &cfg, tx, ids, &blocks)?;
                    need = true;
                }
            }
        }
        if fds[0].revents & POLLIN != 0 {
            let m = c.recv()?;
            if m.typ == SIGNAL {
                settle = true;
            }
        }
    }
}

fn publish(tx: &Tx<Msg>, last: &mut Option<Vec<DiskInfo>>, disks: Vec<DiskInfo>) {
    if last.as_ref() != Some(&disks) {
        *last = Some(disks.clone());
        tx.send(Msg::Disks(disks));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prop(k: &str, v: V) -> V {
        V::Dict(Box::new(V::Str(k.into())), Box::new(V::Var(Box::new(v))))
    }
    fn iface(n: &str, p: Vec<V>) -> V {
        V::Dict(Box::new(V::Str(n.into())), Box::new(V::Arr(p)))
    }
    fn obj(path: &str, i: Vec<V>) -> V {
        V::Dict(Box::new(V::Str(path.into())), Box::new(V::Arr(i)))
    }
    fn cstr(s: &str) -> V {
        let mut b: Vec<V> = s.bytes().map(V::Byte).collect();
        b.push(V::Byte(0));
        V::Arr(b)
    }

    #[test]
    fn external_only() {
        let usb = obj(
            "/org/freedesktop/UDisks2/block_devices/sdb1",
            vec![
                iface(I_BLOCK, vec![prop("IdLabel", V::Str("FOTKY".into())), prop("Device", cstr("/dev/sdb1")), prop("Drive", V::Str("/d/usb".into())), prop("HintSystem", V::Bool(false))]),
                iface(I_FS, vec![prop("MountPoints", V::Arr(vec![cstr("/run/media/mm/FOTKY")]))]),
            ],
        );
        let usb2 = obj(
            "/org/freedesktop/UDisks2/block_devices/sdb2",
            vec![
                iface(I_BLOCK, vec![prop("IdLabel", V::Str("DATA".into())), prop("Device", cstr("/dev/sdb2")), prop("Drive", V::Str("/d/usb".into()))]),
                iface(I_FS, vec![prop("MountPoints", V::Arr(vec![cstr("/run/media/mm/DATA")]))]),
            ],
        );
        let drv = obj("/d/usb", vec![iface(I_DRIVE, vec![prop("Size", V::U64(2_000_000_000_000))])]);
        let sys = obj(
            "/org/freedesktop/UDisks2/block_devices/nvme0n1p2",
            vec![
                iface(I_BLOCK, vec![prop("IdLabel", V::Str("root".into())), prop("HintSystem", V::Bool(true))]),
                iface(I_FS, vec![prop("MountPoints", V::Arr(vec![cstr("/")]))]),
            ],
        );
        let idle = obj(
            "/org/freedesktop/UDisks2/block_devices/sdc1",
            vec![iface(I_BLOCK, vec![prop("IdLabel", V::Str("PRAZDNY".into())), prop("Drive", V::Str("/d/x".into()))]), iface(I_FS, vec![prop("MountPoints", V::Arr(vec![]))])],
        );
        let o = parse_objects(V::Arr(vec![usb, usb2, drv, sys, idle]));
        let s = snapshot(&o);
        assert_eq!(s.disks.len(), 1);
        assert_eq!(s.disks[0].label, "FOTKY + DATA");
        assert_eq!(s.disks[0].size, "1.8T");
        assert_eq!(s.disks[0].mount, "/run/media/mm/FOTKY, /run/media/mm/DATA");
        assert_eq!(s.disks[0].dev, "/dev/sdb1");
        assert_eq!(s.unmounted.len(), 1);
        assert_eq!(s.unmounted[0].1, "PRAZDNY");
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(500 * 1024 * 1024), "500M");
        assert_eq!(human_size(1024u64.pow(4)), "1T");
        assert_eq!(human_size(15_664_000_000), "14.6G");
    }
}
