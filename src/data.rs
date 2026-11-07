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

/// Indexy položiek, ktoré vyhovujú filtru (ako v rofi): slová oddelené medzerou sa hľadajú ako podreťazce
/// bez ohľadu na veľkosť písmen a všetky musia sedieť. Prázdny filter vráti všetky položky v pôvodnom poradí.
pub fn clip_filter(entries: &[ClipEntry], query: &str) -> Vec<usize> {
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            if words.is_empty() {
                return true;
            }
            let hay = e.text.to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .map(|(i, _)| i)
        .collect()
}

/// Z výstupu príkazu položky menu vyberie prvý neprázdny riadok (bez okrajových medzier a riadiacich znakov).
pub fn menu_exec_line(raw: &str) -> String {
    raw.lines()
        .map(|l| l.chars().filter(|c| !c.is_control()).collect::<String>())
        .map(|l| l.trim().to_string())
        .find(|l| !l.is_empty())
        .unwrap_or_default()
}

/// Text položky menu s `exec`. `out` = výstup príkazu (`None` = ešte nie je načítaný).
/// Ak `label` obsahuje `{}`, je to šablóna a `{}` nahradí výstup (kým nie je načítaný, je prázdny, takže rozloženie
/// ostane stabilné: `CPU: {} %` → `CPU:   %`). Inak sa do načítania zobrazuje `label` a potom samotný výstup.
pub fn menu_item_text(label: &str, out: Option<&str>) -> String {
    if label.contains("{}") {
        label.replace("{}", out.unwrap_or(""))
    } else {
        match out {
            Some(o) => o.to_string(),
            None => label.to_string(),
        }
    }
}

#[derive(Debug)]
pub enum Msg {
    /// výstup príkazu `exec` položky menu: (generácia otvorenia menu, index položky, prvý riadok výstupu)
    MenuExec(u64, usize, String),
    Disks(Vec<DiskInfo>),
    /// história schránky (najnovšie prvé)
    Clip(Vec<ClipEntry>),
    /// obsah koša zo všetkých diskov (najnovšie prvé) + hláška o práve dokončenej obnove / vyprázdnení (inak prázdna)
    Trash(Vec<crate::trash::TrashEntry>, String),
    /// zoznam obrázkov modulu wallpaper + cesta k aktuálnemu pozadiu
    Wall(Vec<crate::wallpaper::WallEntry>, String),
    /// náhľad obrázka s daným indexom je hotový
    WallThumb(usize, String),
    /// pozadie bolo úspešne nastavené (cesta)
    WallCurrent(String),
    /// témy z adresára (názov, cesta) a názov aktuálne nastavenej (prázdny = režim „wallpaper“)
    WallThemes(Vec<(String, String)>, String),
    /// téma bola úspešne nastavená (názov)
    WallTheme(String),
    /// stav Wi-Fi z NetworkManagera
    Wifi(crate::wifi::WifiState),
    /// stavová / chybová správa Wi-Fi panela (prázdna = vymazať)
    WifiNote(String),
    /// text zo schránky pre pole hesla
    WifiPaste(String),
    /// zoznam audio zariadení a predvolené zariadenia
    AudioDev(crate::audio::AudioState),
    /// stav monitora (jas, kontrast, preset, vstup, nočný režim)
    Monitor(crate::monitor::MonitorState),
    /// stav Bluetooth (BlueZ)
    Bluetooth(crate::bluetooth::BtState),
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
    /// sledovaný proces (položka menu s `track`) sa spustil: skupina, jeho číslo a zobrazovaný názov
    TrackStart { group: String, id: u64, name: String },
    /// sledovaný proces so zadaným číslom skončil (ukončený alebo skončil sám)
    TrackEnd { group: String, id: u64 },
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
    /// bežiace sledované procesy: skupina → (číslo procesu, názov)
    pub tracked: std::collections::HashMap<String, (u64, String)>,
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
    fn menu_exec_texts() {
        assert_eq!(menu_exec_line("\n  \n  23 %  \nzvyšok\n"), "23 %");
        assert_eq!(menu_exec_line("a\x1b[0mb"), "a[0mb");
        assert_eq!(menu_exec_line(""), "");
        assert_eq!(menu_exec_line("   \n\t\n"), "");
        // šablóna: kým výstup nie je, `{}` je prázdne
        assert_eq!(menu_item_text("CPU: {} %", None), "CPU:  %");
        assert_eq!(menu_item_text("CPU: {} %", Some("23")), "CPU: 23 %");
        // bez šablóny: label ako zástupný text, potom výstup
        assert_eq!(menu_item_text("Kôš", None), "Kôš");
        assert_eq!(menu_item_text("Kôš", Some("3 položky")), "3 položky");
        assert_eq!(menu_item_text("", None), "");
        assert_eq!(menu_item_text("Kôš", Some("")), "");
    }
    #[test]
    fn clip_filtering() {
        let e = |t: &str| ClipEntry { raw: format!("1\t{t}"), text: t.into() };
        let v = vec![e("Hello World"), e("foo bar"), e("world of FOO"), e("ružová")];
        assert_eq!(clip_filter(&v, ""), vec![0, 1, 2, 3]);
        assert_eq!(clip_filter(&v, "   "), vec![0, 1, 2, 3]);
        assert_eq!(clip_filter(&v, "WORLD"), vec![0, 2]);
        assert_eq!(clip_filter(&v, "world foo"), vec![2]);
        assert_eq!(clip_filter(&v, "foo  world"), vec![2]);
        assert_eq!(clip_filter(&v, "ruž"), vec![3]);
        assert_eq!(clip_filter(&v, "nič"), Vec::<usize>::new());
    }
    #[test]
    fn unescape() {
        assert_eq!(unescape_markup("A &amp; B &lt;x&gt;"), "A & B <x>");
    }
}
