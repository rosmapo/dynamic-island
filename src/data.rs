//! Dáta modulov a správy z pomocných vlákien do hlavnej slučky.

#[derive(Clone, Debug, PartialEq, Default)]
pub struct NetInfo {
    pub ifname: String,
    pub wifi: bool,
    pub up: bool,
    pub ssid: String,
    pub signal_pct: u32,
    pub dbm: i32,
    pub freq_ghz: String,
    pub ip: String,
    pub gateway: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct VpnInfo {
    pub ifname: String,
    pub ip: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct CustomOut {
    pub text: String,
    pub tooltip: String,
    pub class: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Notification {
    pub id: u32,
    pub app: String,
    pub summary: String,
    pub body: String,
    pub actions: Vec<(String, String)>,
    /// 0 nízka, 1 normálna, 2 kritická
    pub urgency: u8,
    pub timeout_ms: i32,
}

/// Pripojené externé médium (UDisks2).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct DiskInfo {
    /// objekt bloku v UDisks2 (identifikátor pre odpojenie)
    pub block: String,
    pub label: String,
    pub dev: String,
    pub mount: String,
    pub size: String,
}

/// Položka histórie schránky (cliphist): `raw` je celý riadok z `cliphist list` (ide na vstup
/// `cliphist decode`), `text` je náhľad na zobrazenie.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ClipEntry {
    pub raw: String,
    pub text: String,
}

/// Rozoberie výstup `cliphist list` (riadok = `id<TAB>náhľad`), najviac `max` najnovších položiek.
pub fn parse_clip_list(out: &str, max: usize) -> Vec<ClipEntry> {
    out.lines()
        .filter(|l| !l.trim().is_empty())
        .take(max)
        .map(|l| {
            let text = match l.split_once('\t') {
                Some((_, t)) => t,
                None => l,
            };
            ClipEntry { raw: l.to_string(), text: text.split_whitespace().collect::<Vec<_>>().join(" ") }
        })
        .collect()
}

#[derive(Debug)]
pub enum Msg {
    Disks(Vec<DiskInfo>),
    /// história schránky (najnovšie prvé)
    Clip(Vec<ClipEntry>),
    Notif(Notification),
    NotifClose(u32),
    Net(NetInfo),
    Vpn(Option<VpnInfo>),
    Layout(String),
    Audio { muted: bool, volume: f32 },
    /// HUD jasu (0.0..1.0) od externého skriptu
    Brightness(f32),
    Caps(bool),
    Inhibit(bool),
    Rec(String),
    Custom(usize, CustomOut),
    /// autohide: true = skryť
    AutoHide(bool),
    /// signály SIGUSR1 / SIGUSR2
    Hide,
    Show,
}

/// Aktuálny stav všetkých modulov.
#[derive(Clone, Debug, Default)]
pub struct Data {
    pub net: NetInfo,
    pub vpn: Option<VpnInfo>,
    pub layout: String,
    pub muted: bool,
    pub volume: f32,
    pub audio_known: bool,
    /// HUD: false = hlasitosť, true = jas
    pub hud_bright: bool,
    pub bright: f32,
    pub caps: bool,
    pub inhibit: bool,
    pub rec: String,
    pub custom: Vec<CustomOut>,
    pub disks: Vec<DiskInfo>,
    pub clock: String,
}

/// Pango/HTML entity z tooltipov (napr. z waybar-eject.py) -> obyčajný text.
pub fn unescape_markup(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// Odstráni značky (<b>, <a href=..>) a rozbalí entity z tela notifikácie.
pub fn strip_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    unescape_markup(&out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn markup() {
        assert_eq!(strip_markup("a <b>b</b> &amp; <a href=\"x\">c</a>"), "a b & c");
    }
    #[test]
    fn clip_list() {
        let out = "12\thello   world\n\n11\t[[ binary data 1 KiB png 10x10 ]]\n10\tx\n";
        let v = parse_clip_list(out, 2);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], ClipEntry { raw: "12\thello   world".into(), text: "hello world".into() });
        assert_eq!(v[1].text, "[[ binary data 1 KiB png 10x10 ]]");
        assert_eq!(parse_clip_list("", 5), vec![]);
    }
    #[test]
    fn unescape() {
        assert_eq!(unescape_markup("A &amp; B &lt;x&gt;"), "A & B <x>");
    }
}
