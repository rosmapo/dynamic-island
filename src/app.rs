//! Hlavná slučka: poll nad Waylandom + budík vlákien + inotify + časovače.
use crate::config::{Config, Theme};
use crate::data::Msg;
use crate::fonts::Fonts;
use crate::gl::Egl;
use crate::providers;
use crate::render::Renderer;
use crate::sys::{self, Inotify, Tx, Waker};
use crate::text::Text;
use crate::disks::{self, DiskCmd};
use crate::history::History;
use crate::notify::{self, NotifCmd};
use crate::ui::{Action, NotifEvent, Ui};
use crate::wayfire;
use crate::wl::{PtrEvent, Wl};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::time::{Duration, Instant};

static SIG_FLAGS: AtomicU32 = AtomicU32::new(0);
static SIG_FD: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_sigusr1(_: i32) {
    SIG_FLAGS.fetch_or(1, Ordering::SeqCst);
    sys::raw_wake(SIG_FD.load(Ordering::SeqCst));
}
extern "C" fn on_sigusr2(_: i32) {
    SIG_FLAGS.fetch_or(2, Ordering::SeqCst);
    sys::raw_wake(SIG_FD.load(Ordering::SeqCst));
}

/// Rozdelí príkaz na slová, ak neobsahuje žiadnu shell syntax (pipe, presmerovanie, úvodzovky, premenné,
/// `~`, zástupné znaky, `VAR=hodnota` na začiatku…). Inak `None` a príkaz sa pošle do `sh -c`.
fn simple_command(cmd: &str) -> Option<Vec<String>> {
    const SPECIAL: &[char] = &[
        '|', '&', ';', '<', '>', '(', ')', '$', '`', '"', '\'', '\\', '*', '?', '[', ']', '{', '}', '~', '#', '!', '\n',
    ];
    if cmd.contains(SPECIAL) {
        return None;
    }
    let words: Vec<String> = cmd.split_whitespace().map(str::to_string).collect();
    let first = words.first()?;
    if first.contains('=') || matches!(first.as_str(), "exec" | "cd" | "export" | "." | "source" | "eval" | "set" | "umask") {
        return None;
    }
    Some(words)
}

/// Príkaz pripravený na spustenie oddelene od ostrovčeka (vlastná session = vlastná skupina procesov).
pub(crate) fn session_command(cmd: &str) -> Command {
    // Jednoduchý príkaz (len program a argumenty, bez shell syntaxe) sa spustí priamo – inak by v zozname
    // procesov ostal medzistupeň `sh -c program` (dash nerobí `exec` sám). Zložitejšie príkazy idú cez `sh -c`.
    let mut c = match simple_command(cmd) {
        Some(words) => {
            let mut c = Command::new(&words[0]);
            c.args(&words[1..]);
            c
        }
        None => {
            let mut c = Command::new("sh");
            c.arg("-c").arg(cmd);
            c
        }
    };
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe {
        c.pre_exec(|| {
            sys::new_session();
            Ok(())
        });
    }
    c
}

/// Spustí príkaz oddelene od ostrovčeka (vlastná session, bez čakania).
pub fn spawn_detached(cmd: &str) {
    if let Ok(mut child) = session_command(cmd).spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

// ------------------------------------------------------------ sledované procesy (položky menu s `track`)

/// Bežiaci sledovaný proces: číslo (rozlišuje ho od predošlého v tej istej skupine), PID vedúceho skupiny
/// a rúra, cez ktorú čakajúce vlákno oznámi, že proces skončil.
struct Tracked {
    id: u64,
    pid: i32,
    done: std::sync::mpsc::Receiver<()>,
}

static TRACKED: std::sync::Mutex<Vec<(String, Tracked)>> = std::sync::Mutex::new(Vec::new());
/// Štarty sa vykonávajú po jednom: nový proces počká, kým sa predošlý zo skupiny skutočne ukončí.
static TRACK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
static TRACK_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Ukončí sledovaný proces skupiny (celú jeho skupinu procesov). S `wait` počká, kým skutočne skončí
/// (najviac 2 s, potom SIGKILL).
fn stop_group(group: &str, wait: bool) {
    let prev = {
        let mut v = TRACKED.lock().unwrap_or_else(|e| e.into_inner());
        v.iter().position(|(g, _)| g == group).map(|i| v.remove(i).1)
    };
    let Some(t) = prev else { return };
    sys::terminate_group(t.pid);
    if wait && t.done.recv_timeout(Duration::from_secs(2)).is_err() {
        sys::kill_group(t.pid);
        let _ = t.done.recv_timeout(Duration::from_secs(1));
    }
}

/// Ukončí všetky sledované procesy (pred `exec()` pri reštarte): čakajúce vlákna ich ešte stihnú vyzdvihnúť,
/// takže nezostanú zombie ani bez indikácie.
pub fn stop_tracked() {
    let groups: Vec<String> = TRACKED.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(g, _)| g.clone()).collect();
    for g in groups {
        stop_group(&g, false);
    }
}

/// Spustí `cmd` ako sledovaný proces skupiny `group`: predošlý zo skupiny sa najprv ukončí, ostrovček dostane
/// `Msg::TrackStart` a po skončení procesu `Msg::TrackEnd`. Na koniec procesu sa len blokujúco čaká (`wait`),
/// nič sa nezisťuje cyklicky.
fn start_tracked(group: String, name: String, cmd: String, tx: Tx<Msg>) {
    std::thread::spawn(move || {
        let guard = TRACK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        stop_group(&group, true);
        let Ok(mut child) = session_command(&cmd).spawn() else { return };
        let id = TRACK_ID.fetch_add(1, Ordering::SeqCst) + 1;
        let (dtx, drx) = std::sync::mpsc::channel::<()>();
        TRACKED.lock().unwrap_or_else(|e| e.into_inner()).push((group.clone(), Tracked { id, pid: child.id() as i32, done: drx }));
        tx.send(Msg::TrackStart { group: group.clone(), id, name });
        drop(guard);
        let _ = child.wait();
        let _ = dtx.send(());
        TRACKED.lock().unwrap_or_else(|e| e.into_inner()).retain(|(_, t)| t.id != id);
        tx.send(Msg::TrackEnd { group, id });
    });
}

/// Spustí `sh -c cmd` a pošle mu `input` (+ nový riadok) na vstup; beží oddelene od ostrovčeka.
fn spawn_with_stdin(cmd: &str, input: String) {
    let mut c = Command::new("sh");
    c.arg("-c").arg(cmd).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe {
        c.pre_exec(|| {
            sys::new_session();
            Ok(())
        });
    }
    if let Ok(mut child) = c.spawn() {
        std::thread::spawn(move || {
            if let Some(mut si) = child.stdin.take() {
                use std::io::Write;
                let _ = si.write_all(input.as_bytes());
                let _ = si.write_all(b"\n");
            }
            let _ = child.wait();
        });
    }
}

/// Najdlhší čas, ktorý má príkaz `exec` položky menu na dokončenie; potom sa ukončí a výstup ostane prázdny.
const MENU_EXEC_TIMEOUT: Duration = Duration::from_secs(5);
/// najviac záznamov, ktoré načíta panel histórie notifikácií
const HIST_PANEL_MAX: usize = 300;

/// Spustí `cmd` cez `sh -c` a vráti jeho štandardný výstup (blokuje, volať z vlákna). Pri prekročení `timeout`
/// príkaz ukončí a vráti prázdny reťazec.
fn run_capture(cmd: &str, timeout: Duration) -> String {
    use std::io::Read;
    let mut c = Command::new("sh");
    c.arg("-c").arg(cmd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    unsafe {
        c.pre_exec(|| {
            sys::die_with_parent();
            Ok(())
        });
    }
    let Ok(mut child) = c.spawn() else { return String::new() };
    let Some(mut out) = child.stdout.take() else { return String::new() };
    // čítanie v samostatnom vlákne po kúskoch: potomok, ktorý by držal rúru otvorenú, nás nezablokuje
    // a už prečítaný výstup nestratíme
    let (rtx, rrx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = out.read(&mut buf) {
            if n == 0 || rtx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let start = Instant::now();
    let finished = loop {
        match child.try_wait() {
            Ok(Some(_)) => break true,
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(15)),
            _ => break false,
        }
    };
    if !finished {
        let _ = child.kill();
        let _ = child.wait();
        return String::new();
    }
    // príkaz skončil: pozbierať, čo prišlo (koniec rúry alebo najviac 300 ms čakania na potomkov)
    let mut all = Vec::new();
    let until = Instant::now() + Duration::from_millis(300);
    while let Ok(chunk) = rrx.recv_timeout(until.saturating_duration_since(Instant::now())) {
        all.extend_from_slice(&chunk);
    }
    String::from_utf8_lossy(&all).into_owned()
}

fn run_action(a: Action, cfg: &Config, disk_tx: &Option<Tx<DiskCmd>>, tx: &Tx<Msg>) {
    match a {
        Action::MenuExec { gen, idx, cmd } => {
            // výstup príde na pozadí ako `Msg::MenuExec`; hlavná slučka medzitým nečaká
            let tx = tx.clone();
            let opts = crate::sysinfo::Opts { decimal: cfg.sys_decimal.clone(), net_iface: cfg.sys_net_iface.clone() };
            std::thread::spawn(move || {
                // `builtin:ZDROJ` = natívny zdroj (CPU, RAM, disk, sieť, teplota), inak príkaz cez shell
                let raw = match cmd.trim().strip_prefix("builtin:") {
                    Some(spec) => crate::sysinfo::eval(spec, &opts),
                    None => run_capture(&cmd, MENU_EXEC_TIMEOUT),
                };
                tx.send(Msg::MenuExec(gen, idx, crate::data::menu_exec_line(&raw)));
            });
        }
        Action::ClipLoad => {
            // história sa načíta na pozadí, hlavná slučka dostane `Msg::Clip`
            let (cmd, max, tx) = (cfg.clip_list.clone(), cfg.clip_max, tx.clone());
            std::thread::spawn(move || {
                let out = Command::new("sh").arg("-c").arg(&cmd).stdin(Stdio::null()).stderr(Stdio::null()).output();
                let entries = match out {
                    Ok(o) => crate::data::parse_clip_list(&String::from_utf8_lossy(&o.stdout), max),
                    Err(_) => Vec::new(),
                };
                tx.send(Msg::Clip(entries));
            });
        }
        Action::ClipCopy(line) => spawn_with_stdin(&cfg.clip_copy, line),
        Action::Media(cmd) => crate::mpris::send(cmd),
        Action::HistLoad => {
            // súbor sa číta na pozadí, hlavná slučka dostane `Msg::NotifHist`
            let (path, tx) = (cfg.history_file.clone(), tx.clone());
            std::thread::spawn(move || {
                tx.send(Msg::NotifHist(crate::history::load(&path, HIST_PANEL_MAX)));
            });
        }
        Action::HistRemove(time, id) => {
            crate::history::remove(&cfg.history_file, time, id);
        }
        Action::HistClear => {
            crate::history::clear(&cfg.history_file);
        }
        Action::TrashLoad => {
            let tx = tx.clone();
            std::thread::spawn(move || {
                tx.send(Msg::Trash(crate::trash::load_all(), String::new()));
            });
        }
        Action::TrashRestore(items) => {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let note = crate::trash::restore_all(&items);
                tx.send(Msg::Trash(crate::trash::load_all(), note));
            });
        }
        Action::TrashEmpty(items) => {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let note = crate::trash::empty_all(&items);
                tx.send(Msg::Trash(crate::trash::load_all(), note));
            });
        }
        Action::Audio(cmd) => crate::audio::run(cmd, cfg.vol_limit, tx.clone()),
        Action::Wifi(cmd) => crate::wifi::run(cmd, tx.clone()),
        Action::Monitor(cmd) => crate::monitor::run(cmd, cfg.monitor.clone(), tx.clone()),
        Action::Capture(cmd) => crate::capture::run(cmd, cfg.capture.clone(), tx.clone()),
        Action::DrunLoad(px) => {
            // zoznam aplikácií a ikony (vrátane prvej rasterizácie SVG) sa pripravia na pozadí
            let (d, tx) = (cfg.drun.clone(), tx.clone());
            std::thread::spawn(move || {
                let items: Vec<(crate::desktop::App, String)> = crate::desktop::load_all(&d.desktop)
                    .into_iter()
                    .map(|a| {
                        let p = crate::desktop::icon_png(&a.icon, px, &d.icon_theme);
                        (a, p)
                    })
                    .collect();
                tx.send(Msg::Drun(items));
            });
        }
        Action::Bluetooth(cmd) => crate::bluetooth::run(cmd, cfg.bluetooth.clone(), tx.clone()),
        Action::WifiPaste => crate::wifi::paste(tx.clone()),
        Action::WallLoad => crate::wallpaper::load(cfg.wall.clone(), tx.clone()),
        Action::WallApply { path, background_only } => crate::wallpaper::apply(cfg.wall.clone(), path, background_only, tx.clone()),
        Action::WallTheme(Some((name, path))) => crate::wallpaper::set_theme(cfg.wall.clone(), name, path, tx.clone()),
        Action::WallTheme(None) => crate::wallpaper::clear_theme(&cfg.wall),
        Action::DiskEject(b) => {
            if let Some(t) = disk_tx {
                t.send(DiskCmd::Eject(b));
            }
        }
        Action::DiskEjectAll => {
            if let Some(t) = disk_tx {
                t.send(DiskCmd::EjectAll);
            }
        }
        Action::Run(cmd) => spawn_detached(&cmd),
        Action::RunTracked { group, name, cmd } => start_tracked(group, name, cmd, tx.clone()),
        Action::TrackStop(group) => stop_group(&group, false),
        Action::InhibitToggle => {
            let f = &cfg.inh_file;
            if std::path::Path::new(f).exists() {
                let _ = std::fs::remove_file(f);
            } else {
                let _ = std::fs::write(f, b"");
            }
        }
        Action::LayoutToggle => wayfire::toggle_layout(),
        Action::AudioMute => spawn_detached("wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle"),
    }
}

// ------------------------------------------------------------ okno v strede (spúšťač a panely zvonka)

/// Okno v strede obrazovky (`--drun`, `--action`, `--clip`): samostatný povrch layer-shell s vlastným Wayland spojením,
/// EGL kontextom a rendererom. Vzniká pri otvorení a zaniká po zatvorení; lišta hore je na ňom nezávislá, takže ostáva
/// stále viditeľná. Obsah okna kreslí vlastná inštancia `Ui` (`win_ui` v `run`).
struct Win {
    // poradie polí = poradie zničenia: najprv renderer, potom EGL povrch a nakoniec okno so spojením
    renderer: Renderer,
    egl: Egl,
    wl: Wl,
    scale: i32,
    dirty: bool,
    animating: bool,
    first_draw: bool,
    last_frame: Instant,
    last_input: Option<Option<(i32, i32, i32, i32)>>,
}

fn open_win(cfg: &Config) -> Result<Win, String> {
    let mut wl = Wl::new_with(cfg.surface_w, cfg.surface_h, true, "dynamic-island")?;
    let scale = wl.st.scale.max(1);
    let egl = wl.create_egl(cfg.surface_w as i32 * scale, cfg.surface_h as i32 * scale)?;
    let renderer = Renderer::new(&egl)?;
    Ok(Win { renderer, egl, wl, scale, dirty: true, animating: false, first_draw: true, last_frame: Instant::now(), last_input: None })
}

/// Správy, ktoré patria len lište (notifikácie, HUD jasu, skrývanie); okno v strede ich nedostane.
/// Všetky ostatné (stav modulov, výsledky načítania panelov) dostávajú obe inštancie `Ui`.
fn bar_only(m: &Msg) -> bool {
    matches!(m, Msg::Notif(_) | Msg::NotifClose(_) | Msg::Hide | Msg::Show | Msg::AutoHide(_) | Msg::Brightness(_))
}

/// Číslo otvorenia menu, od ktorého počíta okno (lišta počíta od 0) – pozri `Ui::set_menu_gen`.
const WIN_MENU_GEN: u64 = 1 << 48;

pub fn run(cfg: Config) -> Result<(), String> {
    let waker = Waker::new();
    SIG_FD.store(waker.fd(), Ordering::SeqCst);
    sys::install_signal(sys::SIGUSR1, on_sigusr1);
    sys::install_signal(sys::SIGUSR2, on_sigusr2);
    let (tx, rx) = sys::channel::<Msg>(waker.clone());

    let fonts = Fonts::load(&cfg.font_family, &cfg.icon_family)?;
    let mut text = Text::new(Box::new(fonts));

    let mut wl = Wl::new(cfg.surface_w, cfg.surface_h)?;
    let mut scale = wl.st.scale.max(1);
    let egl = wl.create_egl((cfg.surface_w as i32) * scale, (cfg.surface_h as i32) * scale)?;
    let mut renderer = Renderer::new(&egl)?;

    let mut theme = Theme::load(&cfg);
    let mut ui = Ui::new(cfg.clone(), theme, scale as f32);
    // okno v strede: vlastná `Ui` (stále existuje a dostáva stav modulov), povrch sa vytvára len kým je otvorené
    let mut win_ui = Ui::new(cfg.clone(), theme, scale as f32);
    win_ui.set_menu_gen(WIN_MENU_GEN);
    let mut win: Option<Win> = None;

    // sledovanie zmeny farieb (matugen) a konfigurácie
    let inotify = Inotify::new();
    let colors_path = crate::config::expand(&cfg.colors_file);
    let colors_name = std::path::Path::new(&colors_path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let cfg_name = cfg.path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    if let Some(i) = &inotify {
        let mask = sys::IN_CLOSE_WRITE | sys::IN_MOVED_TO | sys::IN_CREATE;
        if let Some(d) = std::path::Path::new(&colors_path).parent() {
            i.watch(&d.to_string_lossy(), mask);
        }
        if let Some(d) = cfg.path.parent() {
            i.watch(&d.to_string_lossy(), mask);
        }
    }

    let hud_dir = crate::config::hud_dir();
    let _ = std::fs::create_dir_all(&hud_dir);
    // povely z minulého behu sa nesmú vykonať; PID určuje adresáta pre `--drun` / `--action`
    if let Ok(rd) = std::fs::read_dir(&hud_dir) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with("action.") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let _ = std::fs::write(crate::config::island_pid_file(), std::process::id().to_string());
    if let Some(i) = &inotify {
        i.watch(&hud_dir.to_string_lossy(), sys::IN_CLOSE_WRITE | sys::IN_MOVED_TO);
    }

    providers::spawn_all(&cfg, &tx);
    let notif_tx = if cfg.notif_enabled {
        let nw = Waker::new();
        let (ntx, nrx) = sys::channel::<NotifCmd>(nw.clone());
        notify::spawn(tx.clone(), nrx, nw);
        Some(ntx)
    } else {
        None
    };

    let disk_tx = if cfg.modules.iter().any(|m| m == "disks") {
        let dw = Waker::new();
        let (dtx, drx) = sys::channel::<DiskCmd>(dw.clone());
        disks::spawn(disks::DiskCfg {
            automount: cfg.disks_automount,
            notify_mount: cfg.disks_notify,
            notify_unmount: cfg.disks_notify,
            notify_error: cfg.disks_notify,
            timeout_ms: cfg.disks_timeout,
        }, tx.clone(), drx, dw);
        Some(dtx)
    } else {
        None
    };
    if cfg.modules.iter().any(|m| m == "media") {
        crate::mpris::spawn(cfg.media_ignore.clone(), tx.clone());
    }
    let mut history = if cfg.history_max > 0 { Some(History::open(cfg.history_file.clone(), cfg.history_max)) } else { None };

    let mut dirty = true;
    let mut animating = false;
    let mut last_frame = Instant::now();
    let mut last_input: Option<Option<(i32, i32, i32, i32)>> = None;
    let mut clock_next = Instant::now();
    let mut last_clock = String::new();
    let mut first_draw = true;
    let mut kb_on = false;

    loop {
        let now = Instant::now();

        // hodiny
        if now >= clock_next {
            let tm = sys::local_tm(sys::now_unix());
            let s = sys::strftime_str(&cfg.clock_format, &tm);
            if s != last_clock {
                last_clock = s.clone();
                win_ui.set_clock(s.clone());
                ui.set_clock(s);
                dirty = true;
            }
            let per_sec = cfg.clock_format.contains("%S") || cfg.clock_format.contains("%T") || cfg.clock_format.contains("%X");
            let wait = if per_sec { 1.0 } else { (60 - tm.tm_sec.clamp(0, 59)) as f64 };
            clock_next = now + Duration::from_secs_f64(wait) + Duration::from_millis(30);
        }

        // časovače ostrovčeka
        if ui.tick_tip(now, &mut text) {
            dirty = true;
        }
        if ui.tick(now) {
            dirty = true;
        }
        // akcie vzniknuté časovačom (obnovenie položiek menu s `interval`)
        for a in ui.take_queued() {
            run_action(a, &cfg, &disk_tx, &tx);
        }

        // časovače okna v strede (obnovenie položiek menu s `interval`, poistka hesla Wi-Fi)
        if let Some(w) = win.as_mut() {
            if win_ui.tick(now) {
                w.dirty = true;
            }
            for a in win_ui.take_queued() {
                run_action(a, &cfg, &disk_tx, &tx);
            }
        }

        // koľko môžeme spať
        let win_can_draw = win.as_ref().is_some_and(|w| (w.dirty || w.animating) && !w.wl.st.frame_pending);
        let can_draw = ((dirty || animating) && !wl.st.frame_pending) || win_can_draw;
        let mut deadline = clock_next;
        if let Some(d) = ui.next_deadline() {
            deadline = deadline.min(d);
        }
        if win.is_some() {
            if let Some(d) = win_ui.next_deadline() {
                deadline = deadline.min(d);
            }
        }
        let timeout_ms = if can_draw {
            0
        } else {
            deadline.saturating_duration_since(now).as_millis().min(60_000) as i32 + 1
        };

        if let Some(w) = win.as_ref() {
            w.wl.flush();
        }
        let extra = [waker.fd(), inotify.as_ref().map(|i| i.fd()).unwrap_or(-1), win.as_ref().map(|w| w.wl.fd()).unwrap_or(-1)];
        let ready = wl.wait_events(&extra, timeout_ms)?;
        if wl.st.closed {
            return Ok(());
        }
        // udalosti okna v strede (vlastné spojenie); jeho strata (kompozitor povrch zrušil) lištu nezhodí
        let mut win_lost = false;
        if let Some(w) = win.as_mut() {
            if let Err(e) = w.wl.wait_events(&[], 0) {
                eprintln!("dynamic-island: okno v strede: {e}");
                win_lost = true;
            }
            win_lost |= w.wl.st.closed;
        }

        if ready[0] {
            waker.drain();
            while let Ok(m) = rx.try_recv() {
                if let (Msg::Notif(n), Some(h)) = (&m, history.as_mut()) {
                    h.append(n);
                }
                if !bar_only(&m) {
                    win_ui.apply(m.clone(), Instant::now());
                    if let Some(w) = win.as_mut() {
                        w.dirty = true;
                    }
                }
                ui.apply(m, Instant::now());
                dirty = true;
            }
            let f = SIG_FLAGS.swap(0, Ordering::SeqCst);
            if f & 1 != 0 {
                ui.apply(Msg::Hide, Instant::now());
                dirty = true;
            }
            if f & 2 != 0 {
                ui.apply(Msg::Show, Instant::now());
                dirty = true;
            }
        }
        if ready[1] {
            if let Some(i) = &inotify {
                let names = i.read_names();
                // povely od `dynamic-island --drun` / `--action NAZOV`: súbory `action.*` s názvom panela
                let mut cmds: Vec<&String> = names.iter().filter(|n| n.starts_with("action.") && !n.ends_with(".tmp")).collect();
                cmds.sort();
                for n in cmds {
                    let p = hud_dir.join(n);
                    if let Ok(s) = std::fs::read_to_string(&p) {
                        // povel patrí oknu v strede; lišta sa ho netýka (ostáva hore)
                        let name = s.trim();
                        if win.is_none() {
                            win_ui.set_scale(scale as f32);
                        }
                        win_ui.external_action(name);
                        if win.is_none() && win_ui.is_centered() {
                            match open_win(&cfg) {
                                Ok(w) => {
                                    win_ui.set_scale(w.scale as f32);
                                    win = Some(w);
                                }
                                Err(e) => {
                                    eprintln!("dynamic-island: okno v strede sa nepodarilo otvoriť: {e}");
                                    win_ui.close_window();
                                    win_ui.update(0.0, Instant::now(), &mut text);
                                }
                            }
                        }
                        if let Some(w) = win.as_mut() {
                            w.dirty = true;
                        }
                    }
                    let _ = std::fs::remove_file(&p);
                }
                if names.iter().any(|n| n == "hud") {
                    if let Ok(s) = std::fs::read_to_string(hud_dir.join("hud")) {
                        let mut it = s.split_whitespace();
                        if let (Some("brightness"), Some(v)) = (it.next(), it.next()) {
                            if let Ok(p) = v.trim_end_matches('%').parse::<f32>() {
                                ui.apply(Msg::Brightness(p / 100.0), Instant::now());
                                dirty = true;
                            }
                        }
                    }
                }
                if names.iter().any(|n| *n == cfg_name) && !cfg_name.is_empty() {
                    eprintln!("dynamic-island: konfigurácia sa zmenila, reštartujem");
                    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                    stop_tracked();
                    providers::stop_children();
                    let err = Command::new(exe).args(std::env::args().skip(1)).exec();
                    return Err(format!("exec: {err}"));
                }
                if names.iter().any(|n| *n == colors_name) && !colors_name.is_empty() {
                    theme = Theme::load(&cfg);
                    ui.theme = theme;
                    win_ui.theme = theme;
                    if let Some(w) = win.as_mut() {
                        w.dirty = true;
                    }
                    dirty = true;
                }
            }
        }

        // ukazovateľ
        let evs: Vec<PtrEvent> = std::mem::take(&mut wl.st.ptr_events);
        for e in evs {
            let now = Instant::now();
            match e {
                PtrEvent::Enter(x, y) | PtrEvent::Motion(x, y) => ui.pointer_move(x as f32, y as f32, now),
                PtrEvent::Leave => ui.pointer_leave(now),
                PtrEvent::Button(b, true) => {
                    for a in ui.click(b, now) {
                        run_action(a, &cfg, &disk_tx, &tx);
                    }
                }
                PtrEvent::Button(b, false) => ui.release(b, now),
                PtrEvent::Scroll(v) => ui.scroll(v as f32),
            }
            // akcie vzniknuté pohybom / kolieskom (posuvník hlasitosti)
            for a in ui.take_queued() {
                run_action(a, &cfg, &disk_tx, &tx);
            }
            dirty = true;
        }

        // klávesnica (len pri písaní hesla Wi-Fi; inak povrch klávesnicu nedostáva)
        for k in std::mem::take(&mut wl.st.key_events) {
            for a in ui.key(&k) {
                run_action(a, &cfg, &disk_tx, &tx);
            }
            dirty = true;
        }
        let mut want_kb = ui.wants_keyboard();
        if want_kb && !wl.keyboard_ready() {
            // bez mapy kláves by sme klávesnicu zobrali a nevedeli ju použiť – radšej písanie zrušiť
            ui.keyboard_unavailable();
            want_kb = false;
        }
        if want_kb != kb_on {
            wl.set_keyboard(want_kb);
            kb_on = want_kb;
            dirty = true;
        }

        // ukazovateľ a klávesnica okna v strede (okno drží klávesnicu výhradne, kým je otvorené)
        if let Some(w) = win.as_mut() {
            let evs: Vec<PtrEvent> = std::mem::take(&mut w.wl.st.ptr_events);
            for e in evs {
                let now = Instant::now();
                match e {
                    PtrEvent::Enter(x, y) | PtrEvent::Motion(x, y) => win_ui.pointer_move(x as f32, y as f32, now),
                    PtrEvent::Leave => win_ui.pointer_leave(now),
                    PtrEvent::Button(b, true) => {
                        for a in win_ui.click(b, now) {
                            run_action(a, &cfg, &disk_tx, &tx);
                        }
                    }
                    PtrEvent::Button(b, false) => win_ui.release(b, now),
                    PtrEvent::Scroll(v) => win_ui.scroll(v as f32),
                }
                for a in win_ui.take_queued() {
                    run_action(a, &cfg, &disk_tx, &tx);
                }
                w.dirty = true;
            }
            for k in std::mem::take(&mut w.wl.st.key_events) {
                for a in win_ui.key(&k) {
                    run_action(a, &cfg, &disk_tx, &tx);
                }
                w.dirty = true;
            }
            // akcie vzniknuté klávesou / otvorením panela (načítanie zoznamov)
            for a in win_ui.take_queued() {
                run_action(a, &cfg, &disk_tx, &tx);
            }
            if win_ui.wants_keyboard() && !w.wl.keyboard_ready() {
                win_ui.keyboard_unavailable();
            }
            if w.wl.st.scale_dirty {
                w.scale = w.wl.st.scale.max(1);
                w.wl.apply_scale(cfg.surface_w as i32 * w.scale, cfg.surface_h as i32 * w.scale);
                win_ui.set_scale(w.scale as f32);
                w.dirty = true;
            }
        }

        // udalosti notifikácií späť do D-Bus vlákna (vypršanie, zatvorenie, akcia)
        for e in ui.take_events() {
            if let Some(n) = &notif_tx {
                n.send(match e {
                    NotifEvent::Closed(id, r) => NotifCmd::Closed(id, r),
                    NotifEvent::Action(id, k) => NotifCmd::Action(id, k),
                });
            }
        }

        // zmena mierky
        if wl.st.scale_dirty {
            scale = wl.st.scale.max(1);
            wl.apply_scale(cfg.surface_w as i32 * scale, cfg.surface_h as i32 * scale);
            ui.set_scale(scale as f32);
            dirty = true;
        }

        // kreslenie
        if (dirty || animating) && !wl.st.frame_pending {
            let now = Instant::now();
            let dt = if first_draw { 0.0 } else { now.duration_since(last_frame).as_secs_f32().min(0.05) };
            first_draw = false;
            last_frame = now;
            animating = ui.update(dt, now, &mut text);
            let frame = ui.build(&mut text);
            let ir = ui.input_rect();
            if last_input != Some(ir) {
                wl.set_input_region(ir);
                last_input = Some(ir);
            }
            // lišta a okno v strede majú každé vlastný EGL kontext: pred kreslením sa vždy nastaví ten správny
            egl.make_current();
            renderer.draw(cfg.surface_w * scale as u32, cfg.surface_h * scale as u32, &frame, &mut text, &mut ui.icons);
            wl.request_frame();
            if !egl.swap() {
                return Err("eglSwapBuffers zlyhalo".into());
            }
            dirty = false;
        }

        // kreslenie okna v strede; po dobehnutí zatváracej animácie sa povrch zničí
        if let Some(w) = win.as_mut() {
            if !win_lost && (w.dirty || w.animating) && !w.wl.st.frame_pending {
                let now = Instant::now();
                let dt = if w.first_draw { 0.0 } else { now.duration_since(w.last_frame).as_secs_f32().min(0.05) };
                w.first_draw = false;
                w.last_frame = now;
                w.animating = win_ui.update(dt, now, &mut text);
                if win_ui.is_centered() {
                    let frame = win_ui.build(&mut text);
                    let ir = win_ui.input_rect();
                    if w.last_input != Some(ir) {
                        w.wl.set_input_region(ir);
                        w.last_input = Some(ir);
                    }
                    w.egl.make_current();
                    w.renderer.draw(cfg.surface_w * w.scale as u32, cfg.surface_h * w.scale as u32, &frame, &mut text, &mut win_ui.icons);
                    w.wl.request_frame();
                    if !w.egl.swap() {
                        eprintln!("dynamic-island: eglSwapBuffers okna v strede zlyhalo");
                        win_lost = true;
                    }
                    w.dirty = false;
                } else {
                    // zatvorené: `Ui` sa vrátila do pokoja, povrch netreba
                    win_lost = true;
                }
            }
        }
        if win_lost {
            if win.is_some() {
                // povrch zanikol zvonka alebo sa okno zatvorilo: `Ui` sa dostane do pokoja a povrch sa zničí
                if win_ui.is_centered() {
                    win_ui.close_window();
                    win_ui.update(0.0, Instant::now(), &mut text);
                }
                win = None;
            }
        }
    }
}
