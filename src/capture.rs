//! Modul capture: snímka obrazovky / okna / oblasti (`grim`, `slurp`), nahrávanie videa (`wf-recorder`)
//! a OCR (`tesseract` + `wl-copy`).
//!
//! Aktívne okno a výstup sa zisťujú cez Wayfire IPC. Odznak REC (modul `recorder`) sa riadi interným súborom
//! (`rec_path()`, v konfigu sa nenastavuje): počas odpočtu v ňom je odpočet, počas nahrávania `REC m:ss`; po
//! skončení sa súbor zmaže.
use crate::data::Msg;
use crate::sys::{self, Tx};
use crate::toml::{Table, TableExt};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::{Duration, Instant};

/// Čo sa snímá.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Screen,
    Window,
    Region,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Screen, Mode::Window, Mode::Region];
    pub fn label(self) -> &'static str {
        match self {
            Mode::Screen => "Obrazovka",
            Mode::Window => "Okno",
            Mode::Region => "Región",
        }
    }
    fn parse(s: &str) -> Option<Mode> {
        match s.trim().to_lowercase().as_str() {
            "screen" | "obrazovka" | "output" => Some(Mode::Screen),
            "window" | "okno" => Some(Mode::Window),
            "region" | "area" | "oblast" => Some(Mode::Region),
            _ => None,
        }
    }
}

/// Obrázok, video alebo OCR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Image,
    Video,
    Ocr,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Image, Kind::Video, Kind::Ocr];
    pub fn label(self) -> &'static str {
        match self {
            Kind::Image => "Obrázok",
            Kind::Video => "Video",
            Kind::Ocr => "OCR",
        }
    }
    /// Text hlavného tlačidla podľa druhu.
    pub fn action_label(self) -> &'static str {
        match self {
            Kind::Image => "Odfotiť",
            Kind::Video => "Nahrávať",
            Kind::Ocr => "Skopírovať text",
        }
    }
}

/// Nastavenia `[capture]`.
#[derive(Clone, Debug)]
pub struct CaptureCfg {
    pub font_size: f32,
    pub width: f32,
    /// adresár snímok / videí (`~` sa rozvinie)
    pub dir: String,
    pub video_dir: String,
    /// vzor názvu súboru pre `strftime`
    pub name: String,
    /// ponúkané časovače v sekundách (0 = hneď)
    pub delays: Vec<u32>,
    pub mode: Mode,
    pub kind: Kind,
    pub cursor: bool,
    pub audio: bool,
    /// `png` | `jpeg`
    pub format: String,
    /// prípona videa (kontajner určí `wf-recorder` podľa nej): `mp4`, `mkv`, `webm`
    pub video_ext: String,
    /// ďalšie argumenty pre `wf-recorder` (oddelené medzerami)
    pub recorder_args: Vec<String>,
    /// argumenty pre `slurp` (oddelené medzerami, bez úvodzoviek), napr. `-b #ffffff30 -w 0`
    pub slurp_args: Vec<String>,
    /// skopírovať snímku do schránky (`wl-copy`)
    pub copy: bool,
    /// po uložení poslať notifikáciu (`notify-send`)
    pub notify: bool,
    /// pri snímke skryť ostrovček
    pub hide_bar: bool,
    /// čakanie na zatvorenie panela / skrytie ostrovčeka pred snímkou (ms)
    pub settle_ms: u64,
    /// interný súbor so stavom odznaku REC (`rec_path()`)
    pub rec_file: String,
}

/// Interný súbor so stavom odznaku REC: `$XDG_RUNTIME_DIR/dynamic-island-rec` (inak `/tmp/dynamic-island-rec`).
pub fn rec_path() -> String {
    match std::env::var("XDG_RUNTIME_DIR") {
        Ok(d) if !d.is_empty() => format!("{}/dynamic-island-rec", d.trim_end_matches('/')),
        _ => "/tmp/dynamic-island-rec".to_string(),
    }
}

fn expand_home(p: &str) -> String {
    match (p.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(h)) if !h.is_empty() => format!("{h}/{rest}"),
        _ => p.to_string(),
    }
}

impl CaptureCfg {
    pub fn from_table(t: &Table, rec_file: &str) -> CaptureCfg {
        let mut delays: Vec<u32> = t
            .get("delays")
            .and_then(|v| v.as_arr())
            .map(|a| a.iter().filter_map(|v| v.as_f64()).map(|n| n.clamp(0.0, 120.0) as u32).collect())
            .unwrap_or_default();
        if delays.is_empty() {
            delays = vec![0, 3, 5, 10];
        }
        delays.truncate(6);
        let mode = Mode::parse(&t.str_or("mode", "region")).unwrap_or(Mode::Region);
        let kind = match t.str_or("kind", "image").trim().to_lowercase().as_str() {
            "video" => Kind::Video,
            "ocr" => Kind::Ocr,
            _ => Kind::Image,
        };
        let format = match t.str_or("format", "png").trim().to_lowercase().as_str() {
            "jpg" | "jpeg" => "jpeg",
            _ => "png",
        }
        .to_string();
        let video_ext = t.str_or("video_ext", "mp4").trim().trim_start_matches('.').to_lowercase();
        CaptureCfg {
            font_size: t.f64_or("font_size", 15.0).clamp(8.0, 64.0) as f32,
            width: t.f64_or("width", 380.0).clamp(240.0, 800.0) as f32,
            dir: expand_home(&t.str_or("dir", "~/Pictures/Screenshots")),
            video_dir: expand_home(&t.str_or("video_dir", "~/Videos/Recordings")),
            name: t.str_or("name", "%Y-%m-%d_%H-%M-%S"),
            delays,
            mode,
            kind,
            cursor: t.bool_or("cursor", false),
            audio: t.bool_or("audio", false),
            format,
            video_ext: if video_ext.is_empty() { "mp4".into() } else { video_ext },
            slurp_args: t.str_or("slurp_args", "-b #ffffff30 -w 0").split_whitespace().map(str::to_string).collect(),
            recorder_args: t.str_or("recorder_args", "").split_whitespace().map(str::to_string).collect(),
            copy: t.bool_or("copy", true),
            notify: t.bool_or("notify", true),
            hide_bar: t.bool_or("hide_bar", true),
            settle_ms: t.f64_or("settle_ms", 350.0).clamp(0.0, 3000.0) as u64,
            rec_file: rec_file.to_string(),
        }
    }
}

/// Výška panela (logické px) pre dané písmo – musí sedieť s `build_capture` v `ui.rs`.
pub fn panel_height(font: f32) -> f32 {
    let row = font + 20.0;
    // pad + 3 sekcie (popisok + rad) + prepínač + tlačidlo, s medzerami
    2.0 * 14.0 + 3.0 * (22.0 + row) + 4.0 * 12.0 + row + (row + 4.0)
}

/// Žiadosť o snímanie z panela.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub mode: Mode,
    pub kind: Kind,
    pub delay: u32,
    pub cursor: bool,
    pub audio: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CaptureCmd {
    Start(Request),
    /// zrušiť výber / odpočet alebo ukončiť nahrávanie
    Stop,
}

/// Udalosti pre UI.
#[derive(Clone, Debug, PartialEq)]
pub enum CaptureEvent {
    /// výber oblasti / okna (`slurp`)
    Selecting,
    /// odpočet: zostáva sekúnd
    Countdown(u32),
    /// nahrávanie: uplynulé sekundy
    Recording(u32),
    /// skryť / zobraziť ostrovček (kvôli snímke)
    Hide(bool),
    /// hotovo: cesta a či ide o video (pri OCR je path prázdna)
    Done { path: String, video: bool },
    /// zrušené používateľom
    Cancelled,
    Failed(String),
}

static BUSY: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);
static SELECTING: AtomicBool = AtomicBool::new(false);
static REC_PID: AtomicI32 = AtomicI32::new(0);

// ---------------------------------------------------------------- čisté pomocné funkcie

/// Geometria `X,Y WxH` pre `grim -g` / `wf-recorder -g`.
pub fn geometry_arg(x: i32, y: i32, w: i32, h: i32) -> String {
    format!("{x},{y} {}x{}", w.max(1), h.max(1))
}

/// Overí výstup `slurp` (`X,Y WxH`) a vráti ho očistený.
pub fn parse_slurp(out: &str) -> Option<String> {
    let s = out.trim();
    let (pos, size) = s.split_once(' ')?;
    let (x, y) = pos.split_once(',')?;
    let (w, h) = size.split_once('x')?;
    let ok = [x, y, w, h].iter().all(|p| p.trim().trim_start_matches('-').parse::<u32>().is_ok());
    (ok && w.trim() != "0" && h.trim() != "0").then(|| s.to_string())
}

/// `m:ss` (alebo `h:mm:ss`) pre odznak.
pub fn format_elapsed(secs: u32) -> String {
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

pub fn rec_text(secs: u32) -> String {
    format!("REC {}", format_elapsed(secs))
}

pub fn countdown_text(n: u32) -> String {
    format!("\u{f017} {n}")
}

/// Úplná cesta k novému súboru.
pub fn target_path(cfg: &CaptureCfg, kind: Kind, tm: &sys::Tm) -> String {
    let stem = sys::strftime_str(&cfg.name, tm);
    let stem = if stem.trim().is_empty() { "capture".to_string() } else { stem.replace('/', "-") };
    match kind {
        Kind::Image | Kind::Ocr => {
            format!("{}/{}.{}", cfg.dir.trim_end_matches('/'), stem, if cfg.format == "jpeg" { "jpg" } else { "png" })
        }
        Kind::Video => format!("{}/{}.{}", cfg.video_dir.trim_end_matches('/'), stem, cfg.video_ext),
    }
}

/// Zo zoznamu okien (obdĺžniky `X,Y WxH`) vytvorí vstup pre `slurp -r`.
pub fn slurp_boxes(rects: &[(i32, i32, i32, i32)]) -> String {
    rects.iter().map(|&(x, y, w, h)| format!("{}\n", geometry_arg(x, y, w, h))).collect()
}

fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

// ---------------------------------------------------------------- vykonávanie

fn write_rec(cfg: &CaptureCfg, text: &str) {
    let _ = std::fs::write(&cfg.rec_file, text);
}

fn clear_rec(cfg: &CaptureCfg) {
    let _ = std::fs::remove_file(&cfg.rec_file);
}

/// Spí `ms` milisekúnd v krokoch; vráti false, ak bolo medzitým zrušené.
fn wait_ms(ms: u64) -> bool {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        if CANCEL.load(Ordering::SeqCst) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(25).min(end.saturating_duration_since(Instant::now())));
    }
    !CANCEL.load(Ordering::SeqCst)
}

/// Výber oblasti `slurp`-om; `boxes` = vopred dané obdĺžniky (výber okna). `None` = zrušené / chyba.
fn run_slurp(cfg: &CaptureCfg, boxes: Option<&str>) -> Result<Option<String>, String> {
    let mut c = Command::new("slurp");
    c.args(&cfg.slurp_args);
    if boxes.is_some() {
        c.arg("-r");
    }
    c.stdin(if boxes.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = c.spawn().map_err(|_| "slurp nie je nainštalovaný".to_string())?;
    if let (Some(b), Some(mut si)) = (boxes, child.stdin.take()) {
        use std::io::Write;
        let _ = si.write_all(b.as_bytes());
    }
    SELECTING.store(true, Ordering::SeqCst);
    let out = child.wait_with_output();
    SELECTING.store(false, Ordering::SeqCst);
    let out = out.map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Ok(None); // Esc / zrušené
    }
    Ok(parse_slurp(&String::from_utf8_lossy(&out.stdout)))
}

/// Cieľ snímania: celý výstup (voliteľne s názvom) alebo geometria.
enum Target {
    Output(Option<String>),
    Geometry(String),
}

fn resolve_window(cfg: &CaptureCfg) -> Result<Target, String> {
    if let Some((x, y, w, h)) = crate::wayfire::focused_window_global() {
        return Ok(Target::Geometry(geometry_arg(x, y, w, h)));
    }
    // aktívne okno sa nepodarilo zistiť (napr. fokus má ostrovček) – okno sa vyberie klikom
    let rects = crate::wayfire::window_rects_global();
    if rects.is_empty() {
        return Err("nepodarilo sa zistiť aktívne okno (Wayfire IPC)".into());
    }
    match run_slurp(cfg, Some(&slurp_boxes(&rects)))? {
        Some(g) => Ok(Target::Geometry(g)),
        None => Err(String::new()),
    }
}

fn notify(cfg: &CaptureCfg, title: &str, body: &str) {
    if cfg.notify {
        let _ = Command::new("notify-send")
            .args(["-a", "dynamic-island", title, body])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn notify_icon(cfg: &CaptureCfg, title: &str, path: &str) {
    if cfg.notify {
        let _ = Command::new("notify-send")
            .args(["-a", "dynamic-island", "-i", path, title, base_name(path)])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Spustí `grim` s daným typom a cieľom; vráti stderr pri chybe.
fn run_grim(req: &Request, target: &Target, filetype: &str, path: &str) -> Result<(), String> {
    let mut c = Command::new("grim");
    if req.cursor {
        c.arg("-c");
    }
    c.args(["-t", filetype]);
    // JPEG: rozumná kvalita (grim default je 80)
    if filetype == "jpeg" {
        c.args(["-q", "90"]);
    }
    match target {
        Target::Geometry(g) => {
            c.args(["-g", g]);
        }
        Target::Output(Some(o)) => {
            c.args(["-o", o]);
        }
        Target::Output(None) => {}
    }
    c.arg(path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    let out = c.output().map_err(|_| "grim nie je nainštalovaný".to_string())?;
    if !out.status.success() {
        let e = String::from_utf8_lossy(&out.stderr);
        return Err(e.lines().last().unwrap_or("grim zlyhal").trim().to_string());
    }
    Ok(())
}

/// PNG → JPEG cez prvý dostupný nástroj (ImageMagick / ffmpeg).
fn png_to_jpeg(png: &str, jpg: &str) -> Result<(), String> {
    // ImageMagick 7
    if Command::new("magick")
        .args([png, "-quality", "90", jpg])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        return Ok(());
    }
    // ImageMagick 6
    if Command::new("convert")
        .args([png, "-quality", "90", jpg])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        return Ok(());
    }
    // ffmpeg
    if Command::new("ffmpeg")
        .args(["-y", "-i", png, "-q:v", "2", jpg])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        return Ok(());
    }
    Err("grim nemá podporu JPEG a chýba konvertor (nainštaluj ImageMagick alebo ffmpeg)".into())
}

fn shoot_image(cfg: &CaptureCfg, req: &Request, target: &Target, path: &str) -> Result<(), String> {
    let jpeg = cfg.format == "jpeg";
    if jpeg {
        // 1) skús priamy JPEG (ak má grim libjpeg)
        match run_grim(req, target, "jpeg", path) {
            Ok(()) => {}
            Err(e) => {
                // 2) fallback: PNG dočasne + konverzia
                let tmp = format!(
                    "/tmp/dynamic-island-cap-{}.png",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_nanos())
                        .unwrap_or(0)
                );
                if let Err(e2) = run_grim(req, target, "png", &tmp) {
                    return Err(e2);
                }
                let conv = png_to_jpeg(&tmp, path);
                let _ = std::fs::remove_file(&tmp);
                if let Err(ce) = conv {
                    // ak konverzia zlyhala, pôvodnú chybu z grim-u (väčšinou „jpeg support disabled“)
                    // doplníme o nápovedu
                    return Err(if e.to_lowercase().contains("jpeg") {
                        format!("{e}; {ce}")
                    } else {
                        ce
                    });
                }
            }
        }
    } else {
        run_grim(req, target, "png", path)?;
    }

    if cfg.copy {
        if let Ok(f) = std::fs::File::open(path) {
            let mime = if jpeg { "image/jpeg" } else { "image/png" };
            // wl-copy sa po načítaní sám odpojí do pozadia
            let _ = Command::new("wl-copy").args(["-t", mime]).stdin(f).stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
    }
    Ok(())
}

/// Snímka → tesseract → text do schránky (`wl-copy`). Dočasný súbor sa zmaže.
fn do_ocr(cfg: &CaptureCfg, req: &Request, target: &Target) -> Result<(), String> {
    let tmp = format!(
        "/tmp/dynamic-island-ocr-{}.png",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    // grim → PNG
    let mut c = Command::new("grim");
    if req.cursor {
        c.arg("-c");
    }
    c.args(["-t", "png"]);
    match target {
        Target::Geometry(g) => {
            c.args(["-g", g]);
        }
        Target::Output(Some(o)) => {
            c.args(["-o", o]);
        }
        Target::Output(None) => {}
    }
    c.arg(&tmp).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    let out = c.output().map_err(|_| "grim nie je nainštalovaný".to_string())?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&tmp);
        let e = String::from_utf8_lossy(&out.stderr);
        return Err(e.lines().last().unwrap_or("grim zlyhal").trim().to_string());
    }

    // tesseract -l slk+eng  (stdout = text)
    let tess = Command::new("tesseract")
        .args([&tmp, "-", "-l", "slk+eng"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| "tesseract nie je nainštalovaný".to_string())?;
    let _ = std::fs::remove_file(&tmp);

    if !tess.status.success() {
        return Err("tesseract zlyhal".into());
    }
    let text = String::from_utf8_lossy(&tess.stdout);
    // prázdny výstup považujeme za neúspech (nič na rozpoznanie)
    if text.trim().is_empty() {
        return Err("žiadny text".into());
    }

    // wl-copy text
    let mut copy = Command::new("wl-copy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "wl-copy nie je nainštalovaný".to_string())?;
    if let Some(mut si) = copy.stdin.take() {
        use std::io::Write;
        let _ = si.write_all(text.as_bytes());
    }
    let status = copy.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("wl-copy zlyhal".into());
    }
    Ok(())
}

fn record_video(cfg: &CaptureCfg, req: &Request, target: &Target, path: &str, tx: &Tx<Msg>) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let log = "/tmp/dynamic-island-capture.log";
    let mut c = Command::new("wf-recorder");
    if req.audio {
        c.arg("-a");
    }
    match target {
        Target::Geometry(g) => {
            c.args(["-g", g]);
        }
        Target::Output(Some(o)) => {
            c.args(["-o", o]);
        }
        Target::Output(None) => {}
    }
    c.args(&cfg.recorder_args).args(["-f", path]).stdin(Stdio::null()).stdout(Stdio::null());
    match std::fs::File::create(log) {
        Ok(f) => c.stderr(Stdio::from(f)),
        Err(_) => c.stderr(Stdio::null()),
    };
    // vlastná session: nahrávanie prežije aj reštart ostrovčeka
    unsafe {
        c.pre_exec(|| {
            sys::new_session();
            Ok(())
        });
    }
    let mut child = c.spawn().map_err(|_| "wf-recorder nie je nainštalovaný".to_string())?;
    REC_PID.store(child.id() as i32, Ordering::SeqCst);
    let start = Instant::now();
    let mut last = u32::MAX;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {}
            Err(e) => {
                REC_PID.store(0, Ordering::SeqCst);
                return Err(e.to_string());
            }
        }
        let secs = start.elapsed().as_secs() as u32;
        if secs != last {
            last = secs;
            write_rec(cfg, &rec_text(secs));
            tx.send(Msg::Capture(CaptureEvent::Recording(secs)));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    REC_PID.store(0, Ordering::SeqCst);
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        let msg = std::fs::read_to_string(log).ok().and_then(|s| s.lines().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_string()));
        return Err(msg.unwrap_or_else(|| format!("wf-recorder skončil ({status})")));
    }
    Ok(())
}

fn job(cfg: CaptureCfg, req: Request, tx: Tx<Msg>) {
    let send = |e: CaptureEvent| tx.send(Msg::Capture(e));
    let video = req.kind == Kind::Video;
    let ocr = req.kind == Kind::Ocr;
    let result = (|| -> Result<Option<String>, String> {
        // panel sa zatvára – počkať na dokončenie animácie
        if !wait_ms(cfg.settle_ms) {
            return Ok(None);
        }
        // 1) výber oblasti pred odpočtom
        let mut target = match req.mode {
            Mode::Region => {
                send(CaptureEvent::Selecting);
                match run_slurp(&cfg, None)? {
                    Some(g) => Some(Target::Geometry(g)),
                    None => return Ok(None),
                }
            }
            _ => None,
        };
        // 2) odpočet v odznaku
        let mut n = req.delay;
        while n > 0 {
            write_rec(&cfg, &countdown_text(n));
            send(CaptureEvent::Countdown(n));
            if !wait_ms(1000) {
                clear_rec(&cfg);
                return Ok(None);
            }
            n -= 1;
        }
        clear_rec(&cfg);
        // 3) okno / obrazovka sa určuje až po odpočte
        if target.is_none() {
            target = Some(match req.mode {
                Mode::Window => match resolve_window(&cfg) {
                    Ok(t) => t,
                    Err(e) if e.is_empty() => return Ok(None),
                    Err(e) => return Err(e),
                },
                _ => Target::Output(crate::wayfire::focused_output_name()),
            });
        }
        let target = target.unwrap_or(Target::Output(None));
        // 4) snímanie / OCR / nahrávanie
        if video {
            let path = target_path(&cfg, req.kind, &sys::local_tm(sys::now_unix()));
            if let Some(dir) = std::path::Path::new(&path).parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("adresár {}: {e}", dir.display()))?;
            }
            record_video(&cfg, &req, &target, &path, &tx)?;
            Ok(Some(path))
        } else if ocr {
            let hide = cfg.hide_bar;
            if hide {
                send(CaptureEvent::Hide(true));
                wait_ms(cfg.settle_ms);
            }
            let r = do_ocr(&cfg, &req, &target);
            if hide {
                send(CaptureEvent::Hide(false));
            }
            r?;
            Ok(Some(String::new())) // OCR: žiadny trvalý súbor
        } else {
            let path = target_path(&cfg, req.kind, &sys::local_tm(sys::now_unix()));
            if let Some(dir) = std::path::Path::new(&path).parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("adresár {}: {e}", dir.display()))?;
            }
            let hide = cfg.hide_bar;
            if hide {
                send(CaptureEvent::Hide(true));
                wait_ms(cfg.settle_ms);
            }
            let r = shoot_image(&cfg, &req, &target, &path);
            if hide {
                send(CaptureEvent::Hide(false));
            }
            r?;
            Ok(Some(path))
        }
    })();
    clear_rec_if_video(&cfg, video);
    match result {
        Ok(Some(path)) => {
            if ocr {
                notify(&cfg, "OCR", "Text bol skopírovaný do schránky.");
            } else if video {
                notify_icon(&cfg, "Nahrávka uložená", &path);
            } else {
                notify_icon(&cfg, "Snímka uložená", &path);
            }
            send(CaptureEvent::Done { path, video });
        }
        Ok(None) => send(CaptureEvent::Cancelled),
        Err(e) => {
            if cfg.notify && !e.is_empty() {
                let (title, body) = if ocr {
                    ("OCR", "Rozpoznávanie textu zlyhalo.")
                } else {
                    ("Snímanie zlyhalo", e.as_str())
                };
                let _ = Command::new("notify-send")
                    .args(["-a", "dynamic-island", "-u", "critical", title, body])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            send(CaptureEvent::Failed(e));
        }
    }
    CANCEL.store(false, Ordering::SeqCst);
    BUSY.store(false, Ordering::SeqCst);
}

fn clear_rec_if_video(cfg: &CaptureCfg, video: bool) {
    if video {
        clear_rec(cfg);
    }
}

/// Vykoná príkaz na pozadí; priebeh hlási cez `Msg::Capture`.
pub fn run(cmd: CaptureCmd, cfg: CaptureCfg, tx: Tx<Msg>) {
    match cmd {
        CaptureCmd::Start(req) => {
            if BUSY.swap(true, Ordering::SeqCst) {
                return;
            }
            CANCEL.store(false, Ordering::SeqCst);
            std::thread::spawn(move || job(cfg, req, tx));
        }
        CaptureCmd::Stop => {
            let pid = REC_PID.load(Ordering::SeqCst);
            if BUSY.load(Ordering::SeqCst) {
                CANCEL.store(true, Ordering::SeqCst);
                if pid > 1 {
                    sys::interrupt(pid); // SIGINT: wf-recorder dokončí súbor
                } else if SELECTING.load(Ordering::SeqCst) {
                    let _ = Command::new("pkill").args(["-x", "slurp"]).status();
                }
            } else {
                // nahrávanie spustené mimo ostrovčeka (vlastný skript)
                let _ = Command::new("pkill").args(["-INT", "-x", "wf-recorder"]).status();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let c = CaptureCfg::from_table(&Table::new(), "/tmp/RECORDING");
        assert_eq!(c.delays, vec![0, 3, 5, 10]);
        assert_eq!(c.mode, Mode::Region);
        assert_eq!(c.kind, Kind::Image);
        assert_eq!(c.format, "png");
        assert_eq!(c.video_ext, "mp4");
        assert!(c.copy && c.notify && c.hide_bar);
        assert_eq!(c.slurp_args, vec!["-b", "#ffffff30", "-w", "0"]);
        assert!(c.dir.ends_with("Pictures/Screenshots"));
    }

    #[test]
    fn kind_labels() {
        assert_eq!(Kind::Image.label(), "Obrázok");
        assert_eq!(Kind::Video.label(), "Video");
        assert_eq!(Kind::Ocr.label(), "OCR");
        assert_eq!(Kind::Image.action_label(), "Odfotiť");
        assert_eq!(Kind::Video.action_label(), "Nahrávať");
        assert_eq!(Kind::Ocr.action_label(), "Skopírovať text");
        assert_eq!(Kind::ALL.len(), 3);
    }

    #[test]
    fn slurp_output() {
        assert_eq!(parse_slurp("10,20 300x200\n").as_deref(), Some("10,20 300x200"));
        assert_eq!(parse_slurp("-5,20 300x200").as_deref(), Some("-5,20 300x200"));
        assert_eq!(parse_slurp("10,20 0x200"), None);
        assert_eq!(parse_slurp(""), None);
        assert_eq!(parse_slurp("selection cancelled"), None);
    }

    #[test]
    fn elapsed_and_texts() {
        assert_eq!(format_elapsed(5), "0:05");
        assert_eq!(format_elapsed(125), "2:05");
        assert_eq!(format_elapsed(3725), "1:02:05");
        assert_eq!(rec_text(65), "REC 1:05");
    }

    #[test]
    fn geometry_and_boxes() {
        assert_eq!(geometry_arg(0, 40, 800, 600), "0,40 800x600");
        assert_eq!(geometry_arg(0, 0, 0, -3), "0,0 1x1");
        assert_eq!(slurp_boxes(&[(1, 2, 3, 4), (5, 6, 7, 8)]), "1,2 3x4\n5,6 7x8\n");
    }

    #[test]
    fn panel_height_matches_layout() {
        // 5 radov (f + 20) + pevné konštanty
        let f = 15.0;
        assert_eq!(panel_height(f), 146.0 + 5.0 * (f + 20.0));
    }
}
