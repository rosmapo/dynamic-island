//! Modul `media` („Práve hrá“): prehrávače MPRIS (`org.mpris.MediaPlayer2.*`) na session D-Bus.
//!
//! Vlákno sleduje signály (`PropertiesChanged`, `NameOwnerChanged`) – žiadne cyklické čítanie. Pozícia v skladbe sa
//! číta iba kým je panel otvorený a prehrávač hrá (raz za sekundu). Obal (`mpris:artUrl`) sa prekonvertuje cez
//! ImageMagick na PNG (renderer vie iba PNG) do `~/.cache/dynamic-island/media-art/`.
use crate::data::{MediaInfo, Msg};
use crate::dbus::*;
use crate::sys::{self, PollFd, Tx, Waker, POLLIN};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Receiver;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const PREFIX: &str = "org.mpris.MediaPlayer2.";
const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const PROPS: &str = "org.freedesktop.DBus.Properties";

/// Príkazy z hlavnej slučky do vlákna MPRIS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaCmd {
    PlayPause,
    Next,
    Prev,
    /// panel je otvorený (`true`) / zatvorený: zapína sa čítanie pozície
    Watch(bool),
}

static CMD_TX: OnceLock<Mutex<Tx<MediaCmd>>> = OnceLock::new();

/// Pošle príkaz vláknu MPRIS (bez účinku, ak nebeží).
pub fn send(cmd: MediaCmd) {
    if let Some(t) = CMD_TX.get() {
        if let Ok(t) = t.lock() {
            t.send(cmd);
        }
    }
}

/// Adresár s prekonvertovanými obalmi.
pub fn art_dir() -> PathBuf {
    crate::nicons::cache_dir().with_file_name("media-art")
}

pub fn spawn(ignore: Vec<String>, tx: Tx<Msg>) {
    let w = Waker::new();
    let (ctx, crx) = sys::channel::<MediaCmd>(w.clone());
    let _ = CMD_TX.set(Mutex::new(ctx));
    std::thread::spawn(move || {
        crate::nicons::cleanup_dir(&art_dir(), 90);
        loop {
            if let Err(e) = session(&ignore, &tx, &crx, &w) {
                eprintln!("dynamic-island: MPRIS nebeží: {e} (opakujem o 5 s)");
            }
            tx.send(Msg::Media(None));
            std::thread::sleep(Duration::from_secs(5));
        }
    });
}

/// Stav jedného prehrávača.
#[derive(Clone, Debug, Default, PartialEq)]
struct Player {
    name: String,
    playing: bool,
    title: String,
    artist: String,
    album: String,
    art_url: String,
    length_us: i64,
    can_next: bool,
    can_prev: bool,
    can_play: bool,
}

fn as_i64(v: &V) -> Option<i64> {
    match v {
        V::I64(x) => Some(*x),
        V::U64(x) => Some(*x as i64),
        V::I32(x) => Some(*x as i64),
        V::U32(x) => Some(*x as i64),
        V::F64(x) => Some(*x as i64),
        _ => None,
    }
}

fn parse_meta(items: &[V], p: &mut Player) {
    for it in items {
        let V::Dict(k, val) = it else { continue };
        let V::Var(inner) = &**val else { continue };
        match k.as_str() {
            Some("xesam:title") => p.title = inner.as_str().unwrap_or("").to_string(),
            Some("xesam:album") => p.album = inner.as_str().unwrap_or("").to_string(),
            Some("xesam:artist") => {
                p.artist = match &**inner {
                    V::Arr(a) => a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "),
                    V::Str(s) => s.clone(),
                    _ => String::new(),
                }
            }
            Some("mpris:artUrl") => p.art_url = inner.as_str().unwrap_or("").to_string(),
            Some("mpris:length") => p.length_us = as_i64(inner).unwrap_or(0),
            _ => {}
        }
    }
}

/// `GetAll` rozhrania Player (`a{sv}`) → stav prehrávača.
fn parse_props(name: &str, v: &V) -> Option<Player> {
    let V::Arr(items) = v else { return None };
    let mut p = Player { name: name.to_string(), ..Default::default() };
    for it in items {
        let V::Dict(k, val) = it else { continue };
        let V::Var(inner) = &**val else { continue };
        match k.as_str() {
            Some("PlaybackStatus") => p.playing = inner.as_str() == Some("Playing"),
            Some("CanGoNext") => p.can_next = matches!(**inner, V::Bool(true)),
            Some("CanGoPrevious") => p.can_prev = matches!(**inner, V::Bool(true)),
            Some("CanPlay") | Some("CanPause") => p.can_play |= matches!(**inner, V::Bool(true)),
            Some("Metadata") => {
                if let V::Arr(m) = &**inner {
                    parse_meta(m, &mut p);
                }
            }
            _ => {}
        }
    }
    Some(p)
}

/// Ktorý prehrávač zobraziť: zvolený zostáva, kým hrá alebo kým nehrá nikto iný; inak prvý hrajúci,
/// inak prvý použiteľný (pozastavený so skladbou).
fn choose(ps: &[Player], chosen: &str) -> Option<usize> {
    let usable = |p: &Player| p.playing || !p.title.is_empty();
    if let Some(i) = ps.iter().position(|p| p.name == chosen && usable(p)) {
        if ps[i].playing || !ps.iter().any(|p| p.playing) {
            return Some(i);
        }
    }
    ps.iter().position(|p| p.playing).or_else(|| ps.iter().position(usable))
}

fn list_players(c: &mut Conn, ignore: &[String]) -> std::io::Result<Vec<String>> {
    let r = c.call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "ListNames", "", Vec::new())?;
    let mut out = Vec::new();
    if let Some(V::Arr(a)) = Reader::new(&r.body, 0).value(b"as") {
        for n in a.iter().filter_map(|x| x.as_str()) {
            if n.starts_with(PREFIX) && !ignore.iter().any(|i| !i.is_empty() && n.contains(i.as_str())) {
                out.push(n.to_string());
            }
        }
    }
    out.sort();
    Ok(out)
}

fn fetch(c: &mut Conn, name: &str) -> Option<Player> {
    let mut b = W::default();
    b.string(PLAYER);
    let r = c.call(name, PATH, PROPS, "GetAll", "s", b.b).ok()?;
    let v = Reader::new(&r.body, 0).value(r.sig.as_bytes())?;
    parse_props(name, &v)
}

/// Pozícia v skladbe (µs); niektoré prehrávače ju nepodporujú.
fn position(c: &mut Conn, name: &str) -> Option<i64> {
    let mut b = W::default();
    b.string(PLAYER);
    b.string("Position");
    let r = c.call(name, PATH, PROPS, "Get", "ss", b.b).ok()?;
    match Reader::new(&r.body, 0).value(r.sig.as_bytes())? {
        V::Var(inner) => as_i64(&inner),
        _ => None,
    }
}

// ------------------------------------------------------------------ obal

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// `%20` → medzera atď. (cesty z `file://` URL).
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `Some(cesta)` = hotovo (aj prázdna = obal sa nedá použiť), `None` = súbor ešte neexistuje (skúsi sa znova).
fn convert_art(url: &str) -> Option<String> {
    let Some(rest) = url.strip_prefix("file://") else { return Some(String::new()) };
    let path = percent_decode(rest);
    let p = Path::new(&path);
    if !p.is_file() {
        return None;
    }
    if p.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("png")).unwrap_or(false) {
        return Some(path);
    }
    let mtime = std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    let dir = art_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return Some(String::new());
    }
    let out = dir.join(format!("{:016x}.png", crate::nicons::fnv(path.as_bytes(), (mtime & 0xffff_ffff) as u32, (mtime >> 32) as u32)));
    if !out.is_file() {
        for exe in ["magick", "convert"] {
            let ok = Command::new(exe)
                .arg(&path)
                .args(["-auto-orient", "-resize", "256x256>"])
                .arg(format!("PNG:{}", out.display()))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok && out.is_file() {
                break;
            }
        }
    }
    Some(if out.is_file() { out.to_string_lossy().into_owned() } else { String::new() })
}

fn art_png(url: &str, cache: &mut HashMap<String, String>) -> String {
    if url.is_empty() {
        return String::new();
    }
    if let Some(p) = cache.get(url) {
        return p.clone();
    }
    match convert_art(url) {
        Some(p) => {
            cache.insert(url.to_string(), p.clone());
            p
        }
        None => String::new(),
    }
}

// --------------------------------------------------------------- relácia

fn session(ignore: &[String], tx: &Tx<Msg>, rx: &Receiver<MediaCmd>, w: &Waker) -> std::io::Result<()> {
    let mut c = Conn::connect()?;
    c.set_read_timeout(Some(Duration::from_secs(5)));
    for rule in [
        format!("type='signal',interface='{PROPS}',path='{PATH}'"),
        "type='signal',sender='org.freedesktop.DBus',member='NameOwnerChanged',arg0namespace='org.mpris.MediaPlayer2'".to_string(),
    ] {
        let mut b = W::default();
        b.string(&rule);
        c.call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "AddMatch", "s", b.b)?;
    }

    let mut art_cache: HashMap<String, String> = HashMap::new();
    let mut chosen = String::new();
    let mut last: Option<MediaInfo> = None;
    let mut watching = false;
    let mut need = true;
    let mut settle = false;
    let mut pos_due = false;
    loop {
        if need {
            need = false;
            let mut players = Vec::new();
            for n in list_players(&mut c, ignore)? {
                if let Some(p) = fetch(&mut c, &n) {
                    players.push(p);
                }
            }
            let info = choose(&players, &chosen).map(|i| {
                let p = &players[i];
                chosen = p.name.clone();
                MediaInfo {
                    player: p.name.clone(),
                    title: p.title.clone(),
                    artist: p.artist.clone(),
                    album: p.album.clone(),
                    playing: p.playing,
                    art: art_png(&p.art_url, &mut art_cache),
                    length_us: p.length_us,
                    can_next: p.can_next,
                    can_prev: p.can_prev,
                    can_play: p.can_play,
                }
            });
            if info.is_none() {
                chosen.clear();
            }
            // obal sa mohol objaviť až po signáli (súbor ešte nebol zapísaný) → skúsiť ešte raz
            if info.as_ref().map(|i| i.art.is_empty()).unwrap_or(false) && players.iter().any(|p| p.name == chosen && !p.art_url.is_empty() && !art_cache.contains_key(&p.art_url)) {
                settle = true;
            }
            if info != last {
                last = info.clone();
                tx.send(Msg::Media(info));
                if watching {
                    pos_due = true;
                }
            }
        }

        let tick = watching && last.as_ref().map(|i| i.playing).unwrap_or(false);
        let mut fds = [PollFd { fd: c.fd(), events: POLLIN, revents: 0 }, PollFd { fd: w.fd(), events: POLLIN, revents: 0 }];
        let n = sys::poll_in(&mut fds, if settle { 200 } else if tick { 1000 } else { -1 });
        if n == 0 {
            if settle {
                settle = false;
                need = true;
                continue;
            }
            if tick {
                pos_due = true;
            }
        }
        if fds[1].revents & POLLIN != 0 {
            w.drain();
            while let Ok(cmd) = rx.try_recv() {
                let method = match cmd {
                    MediaCmd::Watch(on) => {
                        watching = on;
                        if on {
                            pos_due = true;
                        }
                        continue;
                    }
                    MediaCmd::PlayPause => "PlayPause",
                    MediaCmd::Next => "Next",
                    MediaCmd::Prev => "Previous",
                };
                if let Some(i) = &last {
                    let _ = c.call(&i.player, PATH, PLAYER, method, "", Vec::new());
                    settle = true;
                }
            }
        }
        if fds[0].revents & POLLIN != 0 {
            let m = c.recv()?;
            if m.typ == SIGNAL {
                settle = true;
            }
        }
        if pos_due {
            pos_due = false;
            if let Some(i) = &last {
                if let Some(p) = position(&mut c, &i.player) {
                    tx.send(Msg::MediaPos(p));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(k: &str, v: V) -> V {
        V::Dict(Box::new(V::Str(k.into())), Box::new(V::Var(Box::new(v))))
    }

    #[test]
    fn parses_player_props() {
        let meta = V::Arr(vec![
            dict("xesam:title", V::Str("Relax".into())),
            dict("xesam:artist", V::Arr(vec![V::Str("Frankie".into()), V::Str("Holly".into())])),
            dict("xesam:album", V::Str("Dance".into())),
            dict("mpris:artUrl", V::Str("file:///x/a%20b.jpg".into())),
            dict("mpris:length", V::I64(233_593_000)),
        ]);
        let props = V::Arr(vec![
            dict("PlaybackStatus", V::Str("Playing".into())),
            dict("CanGoNext", V::Bool(true)),
            dict("CanGoPrevious", V::Bool(false)),
            dict("CanPause", V::Bool(true)),
            dict("Metadata", meta),
        ]);
        let p = parse_props("org.mpris.MediaPlayer2.fooyin", &props).unwrap();
        assert!(p.playing && p.can_next && !p.can_prev && p.can_play);
        assert_eq!((p.title.as_str(), p.artist.as_str(), p.album.as_str()), ("Relax", "Frankie, Holly", "Dance"));
        assert_eq!(p.length_us, 233_593_000);
        assert_eq!(p.art_url, "file:///x/a%20b.jpg");
    }

    #[test]
    fn artist_as_plain_string_and_u64_length() {
        let meta = vec![dict("xesam:artist", V::Str("Solo".into())), dict("mpris:length", V::U64(5_000_000))];
        let mut p = Player::default();
        parse_meta(&meta, &mut p);
        assert_eq!((p.artist.as_str(), p.length_us), ("Solo", 5_000_000));
    }

    #[test]
    fn choose_prefers_playing_and_keeps_choice() {
        let mk = |n: &str, playing: bool, title: &str| Player { name: n.into(), playing, title: title.into(), ..Default::default() };
        let ps = vec![mk("a", false, "x"), mk("b", true, "y")];
        assert_eq!(choose(&ps, ""), Some(1));
        // zvolený pozastavený prehrávač sa vymení, ak iný hrá
        assert_eq!(choose(&ps, "a"), Some(1));
        // nikto nehrá: zostane zvolený
        let ps = vec![mk("a", false, "x"), mk("b", false, "y")];
        assert_eq!(choose(&ps, "b"), Some(1));
        assert_eq!(choose(&ps, ""), Some(0));
        // prehrávač bez skladby sa nepoužije
        let ps = vec![mk("a", false, "")];
        assert_eq!(choose(&ps, ""), None);
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("/home/mm/a%20b%C5%A1.jpg"), "/home/mm/a bš.jpg");
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn non_file_art_is_skipped() {
        assert_eq!(convert_art("https://x/y.jpg"), Some(String::new()));
        assert_eq!(convert_art("file:///nonexistent/z.jpg"), None);
    }
}
