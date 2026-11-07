//! `--snapshot DIR`: vykreslí ukážkové stavy bez kompozitora (EGL pbuffer) do surových RGBA súborov.
use crate::config::{Config, Theme};
use crate::data::*;
use crate::fonts::Fonts;
use crate::gl::Egl;
use crate::render::Renderer;
use crate::text::Text;
use crate::ui::Ui;
use std::time::Instant;

pub fn run(cfg: Config, dir: &str) -> Result<(), String> {
    let scale = 1.0f32;
    let (w, h) = (cfg.surface_w, cfg.surface_h);
    let egl = Egl::headless(w as i32, h as i32)?;
    let mut renderer = Renderer::new(&egl)?;
    let mut text = Text::new(Box::new(Fonts::load(&cfg.font_family, &cfg.icon_family)?));
    let theme = Theme::load(&cfg);
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;

    let mut ui = Ui::new(cfg.clone(), theme, scale);
    let now = Instant::now();
    let tm = crate::sys::local_tm(crate::sys::now_unix());
    ui.set_clock(crate::sys::strftime_str(&cfg.clock_format, &tm));
    ui.apply(Msg::Net(NetInfo { ifname: "wlan0".into(), wifi: true, up: true, ssid: "Doma 5G".into(), signal_pct: 80, dbm: -60, freq_ghz: "5.18".into(), ip: "192.168.1.23".into(), gateway: "192.168.1.1".into() }), now);
    ui.apply(Msg::Layout("SK".into()), now);
    ui.apply(Msg::Audio { muted: false, volume: 0.45 }, now);

    let shot = |ui: &mut Ui, name: &str, text: &mut Text, renderer: &mut Renderer| -> Result<(), String> {
        let mut n = 0;
        while ui.update(1.0 / 60.0, Instant::now(), text) && n < 900 {
            n += 1;
        }
        let frame = ui.build(text);
        renderer.draw(w, h, &frame, text, &mut ui.icons);
        let px = renderer.read_pixels(w, h);
        std::fs::write(format!("{dir}/{name}_{w}x{h}.rgba"), px).map_err(|e| e.to_string())
    };

    shot(&mut ui, "1_compact", &mut text, &mut renderer)?;
    ui.apply(Msg::Vpn(Some(VpnInfo { ifname: "tun0".into(), ip: "10.8.0.2".into() })), now);
    ui.apply(Msg::Caps(true), now);
    ui.apply(Msg::Inhibit(true), now);
    ui.apply(Msg::Rec("REC".into()), now);
    ui.apply(Msg::Audio { muted: true, volume: 0.45 }, now);
    ui.preview("");
    shot(&mut ui, "2_badges", &mut text, &mut renderer)?;
    ui.preview("hud");
    ui.apply(Msg::Audio { muted: false, volume: 0.72 }, Instant::now());
    shot(&mut ui, "3_hud", &mut text, &mut renderer)?;
    ui.preview("clear");
    ui.preview("tip");
    ui.preview_tip(&mut text);
    shot(&mut ui, "4_tooltip", &mut text, &mut renderer)?;
    ui.preview("calendar");
    shot(&mut ui, "5_calendar", &mut text, &mut renderer)?;
    ui.preview("disks");
    shot(&mut ui, "5b_disks", &mut text, &mut renderer)?;
    ui.preview("hide");
    shot(&mut ui, "6_hidden", &mut text, &mut renderer)?;
    ui.apply(Msg::Show, now);
    let mk = |id: u32, app: &str, sum: &str, body: &str, urg: u8| Notification { id, app: app.into(), summary: sum.into(), body: body.into(), actions: vec![("default".into(), "Open".into())], urgency: urg, timeout_ms: 0, icon: String::new() };
    ui.apply(Msg::Notif(mk(1, "mail", "Nová správa od Janka", "Ahoj, posielam podklady k zajtrajšej schôdzi. Pozri si ich prosím pred obedom a daj mi vedieť, či je to v poriadku alebo treba niečo doplniť.", 1)), now);
    ui.apply(Msg::Notif(mk(2, "system", "Batéria takmer vybitá", "Zostáva 5 %. Pripoj nabíjačku.", 2)), now);
    shot(&mut ui, "7_notif", &mut text, &mut renderer)?;
    eprintln!("snímky uložené do {dir}");
    Ok(())
}
