//! Hlavná slučka: poll nad Waylandom + budík vlákien + inotify + časovače.
use crate::config::{Config, Theme};
use crate::data::Msg;
use crate::fonts::Fonts;
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

/// Spustí príkaz oddelene od ostrovčeka (vlastná session, bez čakania).
pub fn spawn_detached(cmd: &str) {
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
    if let Ok(mut child) = c.spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
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
            std::thread::spawn(move || {
                let raw = run_capture(&cmd, MENU_EXEC_TIMEOUT);
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

pub fn run(cfg: Config) -> Result<(), String> {
    let waker = Waker::new();
    SIG_FD.store(waker.fd(), Ordering::SeqCst);
    sys::install_signal(sys::SIGUSR1, on_sigusr1);
    sys::install_signal(sys::SIGUSR2, on_sigusr2);
    let (tx, rx) = sys::channel::<Msg>(waker.clone());

    let fonts = Fonts::load(&cfg.font_family, &cfg.icon_family)?;
    let mut text = Text::new(Box::new(fonts));

    let idle_secs: Vec<u32> = cfg.idle.iter().map(|i| i.timeout).collect();
    let mut wl = Wl::new(cfg.surface_w, cfg.surface_h, &idle_secs)?;
    let mut scale = wl.st.scale.max(1);
    let egl = wl.create_egl((cfg.surface_w as i32) * scale, (cfg.surface_h as i32) * scale)?;
    let mut renderer = Renderer::new(&egl)?;

    let mut theme = Theme::load(&cfg);
    let mut ui = Ui::new(cfg.clone(), theme, scale as f32);

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

        // koľko môžeme spať
        let can_draw = (dirty || animating) && !wl.st.frame_pending;
        let mut deadline = clock_next;
        if let Some(d) = ui.next_deadline() {
            deadline = deadline.min(d);
        }
        let timeout_ms = if can_draw {
            0
        } else {
            deadline.saturating_duration_since(now).as_millis().min(60_000) as i32 + 1
        };

        let extra = [waker.fd(), inotify.as_ref().map(|i| i.fd()).unwrap_or(-1)];
        let ready = wl.wait_events(&extra, timeout_ms)?;
        if wl.st.closed {
            return Ok(());
        }

        if ready[0] {
            waker.drain();
            while let Ok(m) = rx.try_recv() {
                if let (Msg::Notif(n), Some(h)) = (&m, history.as_mut()) {
                    h.append(n);
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
                    let err = Command::new(exe).args(std::env::args().skip(1)).exec();
                    return Err(format!("exec: {err}"));
                }
                if names.iter().any(|n| *n == colors_name) && !colors_name.is_empty() {
                    theme = Theme::load(&cfg);
                    ui.theme = theme;
                    dirty = true;
                }
            }
        }

        // nečinnosť (náhrada swayidle)
        for (i, idle) in std::mem::take(&mut wl.st.idle_events) {
            if let Some(rule) = cfg.idle.get(i) {
                let cmd = if idle { &rule.on_idle } else { &rule.on_resume };
                if !cmd.is_empty() {
                    spawn_detached(cmd);
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
            renderer.draw(cfg.surface_w * scale as u32, cfg.surface_h * scale as u32, &frame, &mut text, &mut ui.icons);
            wl.request_frame();
            if !egl.swap() {
                return Err("eglSwapBuffers zlyhalo".into());
            }
            dirty = false;
        }
    }
}
