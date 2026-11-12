//! dynamic-island – „dynamic island“ lišta pre Wayland (Wayfire/wlroots).
//! Rust + EGL/OpenGL ES 2.0, bez UI frameworku.
mod anim;
mod app;
mod audio;
mod bluetooth;
mod capture;
mod config;
mod data;
mod dbus;
mod desktop;
mod disks;
mod drun;
mod fonts;
mod gl;
mod grid;
mod quickset;
mod history;
mod icons;
mod json;
mod monitor;
mod mpris;
mod notify;
mod nicons;
mod providers;
mod render;
mod snapshot;
mod sysinfo;
mod sys;
mod text;
mod trash;
mod toml;
mod ui;
mod wallpaper;
mod wayfire;
mod wifi;
mod wl;
mod xkb;

use std::path::PathBuf;

const HELP: &str = "dynamic-island [--config CESTA] [--check] [--snapshot ADRESÁR] [--volume up|down|mute|get|N] [--brightness up|down|get|N] [--hud brightness %] [--action NAZOV] [--history [N]] [--drun] [--drun-list] [--clip]

  --config CESTA      konfigurácia (predvolene ~/.config/dynamic-island/config.toml)
  --check             skontroluje konfiguráciu a skončí
  --drun              spúšťač aplikácií (ako rofi -show drun): bežiaci ostrovček otvorí samostatné okno v strede obrazovky,
                      lišta hore ostáva viditeľná; ak je okno otvorené, povel ho zatvorí
  --action NAZOV      to isté pre iný panel alebo vlastné menu (sysmenu, trash, quickset, quickset:wifi, wallpaper, …)
  --clip              to isté pre históriu schránky (cliphist)
  --drun-list         vypíše všetky nájdené .desktop súbory a prečo sa niektoré nezobrazia
  (--drun / --action / --clip vyžadujú bežiaci dynamic-island)
  --history [N]       vypíše posledných N (predvolene 20) notifikácií z histórie
  --snapshot ADRESÁR  vykreslí ukážkové stavy bez kompozitora (surové RGBA súbory)

Signály: SIGUSR1 = skryť, SIGUSR2 = zobraziť.";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut path: Option<PathBuf> = None;
    let mut snap: Option<String> = None;
    let mut check = false;
    let mut bright: Option<String> = None;
    let mut volume: Option<String> = None;
    let mut history: Option<usize> = None;
    let mut drun_list = false;
    let mut action: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--config" => {
                i += 1;
                path = args.get(i).map(PathBuf::from);
            }
            "--snapshot" => {
                i += 1;
                snap = args.get(i).cloned();
            }
            "--check" => check = true,
            "--drun" => action = Some("drun".into()),
            "--drun-list" => drun_list = true,
            "--clip" => action = Some("clipboard".into()),
            "--history" => {
                let n = args.get(i + 1).and_then(|s| s.parse::<usize>().ok());
                if n.is_some() {
                    i += 1;
                }
                history = Some(n.unwrap_or(20));
            }
            "--volume" => {
                i += 1;
                volume = args.get(i).cloned();
                if volume.is_none() {
                    eprintln!("použitie: dynamic-island --volume up|down|mute|get|PERCENTO");
                    std::process::exit(2);
                }
            }
            "--brightness" => {
                i += 1;
                bright = args.get(i).cloned();
                if bright.is_none() {
                    eprintln!("použitie: dynamic-island --brightness up|down|get|PERCENTO");
                    std::process::exit(2);
                }
            }
            "--hud" => {
                // dynamic-island --hud brightness 45  (pošle bežiacemu ostrovčeku HUD)
                let (kind, val) = (args.get(i + 1), args.get(i + 2));
                match (kind, val) {
                    (Some(k), Some(v)) if k == "brightness" => {
                        let dir = config::hud_dir();
                        let _ = std::fs::create_dir_all(&dir);
                        let tmp = dir.join(".hud.tmp");
                        if std::fs::write(&tmp, format!("{k} {v}\n")).is_ok() {
                            let _ = std::fs::rename(&tmp, dir.join("hud"));
                        }
                        return;
                    }
                    _ => {
                        eprintln!("použitie: dynamic-island --hud brightness PERCENTO");
                        std::process::exit(2);
                    }
                }
            }
            "--action" => {
                // dynamic-island --action sysmenu  (panel / menu v strede obrazovky, prepínač)
                i += 1;
                action = args.get(i).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                if action.is_none() {
                    eprintln!("použitie: dynamic-island --action NAZOV");
                    std::process::exit(2);
                }
            }
            "-h" | "--help" => {
                println!("{HELP}");
                return;
            }
            other => {
                eprintln!("neznámy argument: {other}\n\n{HELP}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    let cfg = match config::Config::load(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("chyba konfigurácie: {e}");
            std::process::exit(1);
        }
    };
    if check {
        println!("konfigurácia v poriadku ({} modulov)", cfg.modules.len());
        return;
    }
    if let Some(n) = history {
        std::process::exit(history::print(&cfg.history_file, n));
    }
    if let Some(arg) = volume {
        std::process::exit(volume_cmd(&cfg, &arg));
    }
    if let Some(arg) = bright {
        std::process::exit(brightness(&cfg, &arg));
    }
    if let Some(name) = action {
        // povel bežiacemu ostrovčeku: otvorí panel v strede obrazovky (alebo ho zatvorí)
        if let Err(e) = config::send_action(&name) {
            eprintln!("dynamic-island: {e}");
            std::process::exit(1);
        }
        return;
    }
    if drun_list {
        desktop::print_list(&cfg.drun.desktop);
        return;
    }
    sys::set_time_locale(&cfg.clock_locale);
    let res = match snap {
        Some(dir) => snapshot::run(cfg, &dir),
        None => app::run(cfg),
    };
    if let Err(e) = res {
        eprintln!("dynamic-island: {e}");
        std::process::exit(1);
    }
}

/// Prvé `NN%` z výstupu príkazu.
fn first_percent(s: &str) -> Option<u32> {
    let b = s.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        if c == b'%' && i > 0 && b[i - 1].is_ascii_digit() {
            let mut st = i;
            while st > 0 && b[st - 1].is_ascii_digit() {
                st -= 1;
            }
            return s[st..i].parse().ok();
        }
    }
    None
}

/// `--brightness up|down|get|N`: spustí príkaz z [brightness], pošle HUD bežiacemu ostrovčeku.
fn brightness(cfg: &config::Config, arg: &str) -> i32 {
    let step = cfg.bright_step;
    let cmd = match arg {
        "get" => cfg.bright_get.clone(),
        "up" | "+" => cfg.bright_set.replace("{value}", &format!("{step}+")),
        "down" | "-" => cfg.bright_set.replace("{value}", &format!("{step}-")),
        n if n.parse::<u32>().is_ok() => cfg.bright_set.replace("{value}", n),
        _ => {
            eprintln!("neznámy argument '{arg}' (up|down|get|PERCENTO)");
            return 2;
        }
    };
    let out = std::process::Command::new("sh").arg("-c").arg(&cmd).stdin(std::process::Stdio::null()).output();
    let text = match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr),
        Err(e) => {
            eprintln!("príkaz '{cmd}' sa nespustil: {e}");
            return 1;
        }
    };
    match first_percent(&text) {
        Some(p) => {
            let dir = config::hud_dir();
            let _ = std::fs::create_dir_all(&dir);
            let tmp = dir.join(".hud.tmp");
            if std::fs::write(&tmp, format!("brightness {p}\n")).is_ok() {
                let _ = std::fs::rename(&tmp, dir.join("hud"));
            }
            println!("{p}%");
            0
        }
        None => {
            eprintln!("vo výstupe '{cmd}' som nenašiel percento: {}", text.trim());
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::first_percent;
    #[test]
    fn percent() {
        assert_eq!(first_percent("Jas nastavený [####] 70%\n"), Some(70));
        assert_eq!(first_percent("bez cisla %"), None);
    }
}

/// `--volume ...`: nastaví hlasitosť cez wpctl; bežiaci ostrovček zmenu zachytí sám (HUD).
fn volume_cmd(cfg: &config::Config, arg: &str) -> i32 {
    const SINK: &str = "@DEFAULT_AUDIO_SINK@";
    let step = format!("{}%", cfg.vol_step);
    let limit = format!("{:.2}", cfg.vol_limit);
    let run = |args: &[&str]| -> bool {
        std::process::Command::new("wpctl").args(args).status().map(|s| s.success()).unwrap_or(false)
    };
    let ok = match arg {
        "up" | "+" => run(&["set-mute", SINK, "0"]) && run(&["set-volume", "-l", &limit, SINK, &format!("{step}+")]),
        "down" | "-" => run(&["set-mute", SINK, "0"]) && run(&["set-volume", SINK, &format!("{step}-")]),
        "mute" | "toggle" => run(&["set-mute", SINK, "toggle"]),
        "get" => run(&["get-volume", SINK]),
        n if n.parse::<u32>().is_ok() => run(&["set-volume", "-l", &limit, SINK, &format!("{n}%")]),
        _ => {
            eprintln!("neznámy argument '{arg}' (up|down|mute|get|PERCENTO)");
            return 2;
        }
    };
    if ok {
        0
    } else {
        eprintln!("wpctl zlyhal (je nainštalovaný wireplumber / pipewire?)");
        1
    }
}
