//! Samostatný spúšťač aplikácií (`dynamic-island --drun`): vlastný proces a vlastný povrch v strede obrazovky,
//! nezávislý od bežiaceho ostrovčeka. Po spustení aplikácie, Esc alebo zatvorení povrchu skončí.
use crate::config::{Config, DrunCfg, Theme};
use crate::desktop::{self, App};
use crate::fonts::Fonts;
use crate::grid::{self, Grid, Nav};
use crate::icons::IconCache;
use crate::render::{Color, Frame, Item, Rect, Renderer};
use crate::text::Text;
use crate::wl::{KeyEvent, PtrEvent, Wl};
use std::collections::HashMap;

const BTN_LEFT: u32 = 272;

/// Čo sa stalo po stlačení klávesy.
#[derive(Debug, PartialEq)]
enum KeyOut {
    None,
    Redraw,
    Close,
    Launch(usize),
    /// spustiť napísaný text ako príkaz (aplikácia nie je v zozname, napr. s `NoDisplay=true`)
    Raw(String),
}

struct State {
    apps: Vec<App>,
    query: String,
    /// indexy do `apps` po filtrovaní
    hits: Vec<usize>,
    /// vybraná položka (index do `hits`)
    sel: Option<usize>,
    /// posun v líniách (riadkoch; pri vertikálnom usporiadaní v stĺpcoch)
    scroll: usize,
}

impl State {
    fn new(apps: Vec<App>) -> State {
        let hits = desktop::filter(&apps, "");
        State { apps, query: String::new(), hits, sel: None, scroll: 0 }
    }

    fn refilter(&mut self) {
        self.hits = desktop::filter(&self.apps, &self.query);
        self.sel = if self.hits.is_empty() || self.query.trim().is_empty() { None } else { Some(0) };
        self.scroll = 0;
    }

    /// Položka v bunke `slot` aktuálnej stránky.
    fn at_slot(&self, slot: usize, line_len: usize) -> Option<usize> {
        let i = self.scroll * line_len + slot;
        (i < self.hits.len()).then_some(i)
    }

    fn key(&mut self, k: &KeyEvent, g: &Grid) -> KeyOut {
        let nav = |s: &mut State, n: Nav| {
            s.sel = grid::nav(s.sel, n, g.line_len(), g.lines(), s.hits.len());
            if let Some(i) = s.sel {
                s.scroll = grid::scroll_for(s.scroll, i, g.line_len(), g.lines(), s.hits.len());
            }
            KeyOut::Redraw
        };
        match k.sym {
            0xff1b => KeyOut::Close,
            0xff0d | 0xff8d => {
                // Ctrl+Enter alebo žiadna zhoda: napísaný text sa spustí ako príkaz (ako v rofi)
                let raw = self.query.trim();
                if !raw.is_empty() && (k.ctrl || self.hits.is_empty()) {
                    return KeyOut::Raw(raw.to_string());
                }
                match self.sel.or(if self.hits.is_empty() { None } else { Some(0) }) {
                    Some(i) => KeyOut::Launch(i),
                    None => KeyOut::None,
                }
            }
            0xff08 => {
                if self.query.pop().is_some() {
                    self.refilter();
                }
                KeyOut::Redraw
            }
            // šípky sú fyzické: pri vertikálnom usporiadaní (poradie po stĺpcoch) sa vľavo/vpravo pohybuje po stĺpcoch
            0xff51 => nav(self, if g.vertical { Nav::Up } else { Nav::Left }),
            0xff53 => nav(self, if g.vertical { Nav::Down } else { Nav::Right }),
            0xff52 => nav(self, if g.vertical { Nav::Left } else { Nav::Up }),
            0xff54 => nav(self, if g.vertical { Nav::Right } else { Nav::Down }),
            0xff55 => nav(self, Nav::PageUp),
            0xff56 => nav(self, Nav::PageDown),
            0xff50 => nav(self, Nav::Home),
            0xff57 => nav(self, Nav::End),
            0xff09 => nav(self, Nav::Right),
            0xfe20 => nav(self, Nav::Left),
            0x75 if k.ctrl => {
                self.query.clear();
                self.refilter();
                KeyOut::Redraw
            }
            _ => {
                if !k.ctrl && !k.text.is_empty() && k.text.chars().all(|c| !c.is_control()) {
                    self.query.push_str(&k.text);
                    self.refilter();
                    KeyOut::Redraw
                } else {
                    KeyOut::None
                }
            }
        }
    }

    fn scroll_by(&mut self, delta: i32, g: &Grid) -> bool {
        let max = grid::max_scroll(self.hits.len(), g.line_len(), g.lines()) as i32;
        let n = (self.scroll as i32 + delta).clamp(0, max) as usize;
        let changed = n != self.scroll;
        self.scroll = n;
        changed
    }

    fn launch_cmd(&self, i: usize, d: &DrunCfg) -> Option<String> {
        let app = self.apps.get(*self.hits.get(i)?)?;
        Some(desktop::launch_cmd(app, &d.terminal))
    }
}

pub(crate) fn fade(c: Color, a: f32) -> Color {
    [c[0], c[1], c[2], c[3] * a.clamp(0.0, 1.0)]
}

/// Skráti text s `…`, aby sa zmestil do `maxw`.
pub(crate) fn fit(text: &mut Text, s: &str, px: f32, maxw: f32) -> String {
    if text.measure(s, px) <= maxw {
        return s.to_string();
    }
    let mut t = s.to_string();
    while !t.is_empty() {
        t.pop();
        let c = format!("{}…", t.trim_end());
        if text.measure(&c, px) <= maxw {
            return c;
        }
    }
    String::new()
}

/// Text pri prázdnom výsledku: s dotazom ponúkne jeho spustenie ako príkazu.
pub fn empty_message(query: &str) -> String {
    let q = query.trim();
    if q.is_empty() {
        "Nenašli sa žiadne aplikácie".to_string()
    } else {
        format!("Nič nenájdené · Enter spustí „{q}“")
    }
}

/// Bunka mriežky na vykreslenie: aplikácia, cesta k PNG ikone (prázdna = náhradná) a či je vybraná.
pub struct CellView<'a> {
    pub app: &'a App,
    pub icon: String,
    pub selected: bool,
}

/// Riadok hľadania (zaoblený obdĺžnik s dotazom a kurzorom); `r` je v pixeloch.
pub fn draw_search(out: &mut Vec<Item>, d: &DrunCfg, theme: &Theme, s: f32, text: &mut Text, r: Rect, query: &str) {
    let npx = d.name_size * s;
    out.push(Item::Rect { r, radius: r.h * 0.5, soft: 0.8, color: fade(theme.fg, 0.08) });
    let (a, dd) = text.line_metrics(npx);
    let shown = if query.is_empty() { String::new() } else { format!("{query}▏") };
    let shown = fit(text, &shown, npx, r.w - r.h);
    text.layout_to(out, &shown, npx, r.x + r.h * 0.5, r.y + r.h * 0.5 + (a - dd) * 0.5, theme.fg);
}

/// Vykreslí bunky viditeľnej stránky: `cells[slot]` patrí do bunky `slot` mriežky `g`. Mriežka sa kreslí s ľavým horným
/// rohom v (`ox`, `oy`) pixeloch; geometria mriežky je logická (× `s`). Spoločné pre samostatný spúšťač aj panel v ostrovčeku.
#[allow(clippy::too_many_arguments)]
pub fn draw_cells(out: &mut Vec<Item>, g: &Grid, d: &DrunCfg, theme: &Theme, s: f32, text: &mut Text, ox: f32, oy: f32, cells: &[CellView]) {
    let show = d.show.as_str();
    let has_text = show != "icon";
    let (isz, npx) = (d.icon_size * s, d.name_size * s);
    for (slot, cell) in cells.iter().enumerate().take(g.cols * g.rows) {
        let app = cell.app;
        let (cx, cy, cw, ch) = g.cell(slot);
        let (x, y, cw, ch) = (ox + cx * s, oy + cy * s, cw * s, ch * s);
        let selected = cell.selected;
        if selected {
            let c = if has_text { theme.menu_selected } else { fade(theme.menu_selected, 0.3) };
            out.push(Item::Rect { r: Rect::new(x, y, cw, ch), radius: d.cell_radius * s, soft: 0.8, color: c });
        }
        let fg = if selected && has_text { theme.menu_selected_text } else { theme.fg };
        let path = cell.icon.clone();
        let (a, dd) = text.line_metrics(npx);
        let line_h = a - dd;

        // poloha ikony a názvu podľa režimu
        let (ix, iy, name_pos): (f32, f32, Option<(f32, f32, f32, bool)>) = match show {
            "name" => (0.0, 0.0, Some((x + 8.0 * s, y + ch * 0.5, cw - 16.0 * s, true))),
            "icon+name-right" => {
                let ix = x + 12.0 * s;
                (ix, y + (ch - isz) * 0.5, Some((ix + isz + 12.0 * s, y + ch * 0.5, cw - isz - 36.0 * s, false)))
            }
            "icon+name-below" => {
                let block = isz + 6.0 * s + line_h;
                let top = y + (ch - block) * 0.5;
                (x + (cw - isz) * 0.5, top, Some((x + 4.0 * s, top + isz + 6.0 * s + line_h * 0.5, cw - 8.0 * s, true)))
            }
            _ => (x + (cw - isz) * 0.5, y + (ch - isz) * 0.5, None),
        };

        if show != "name" {
            if path.is_empty() {
                // bez ikony: farebný štvorec s prvým písmenom
                let r = Rect::new(ix, iy, isz, isz);
                out.push(Item::Rect { r, radius: isz * 0.22, soft: 0.8, color: fade(theme.accent, 0.35) });
                let letter: String = app.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                let lp = isz * 0.5;
                let lw = text.measure(&letter, lp);
                let (la, ld) = text.line_metrics(lp);
                text.layout_to(out, &letter, lp, ix + (isz - lw) * 0.5, iy + isz * 0.5 + (la - ld) * 0.5, theme.fg);
            } else {
                out.push(Item::Image { r: Rect::new(ix, iy, isz, isz), path, color: [1.0, 1.0, 1.0, 1.0], radius: 0.0 });
            }
        }
        if let Some((tx, tcy, maxw, center)) = name_pos {
            let label = fit(text, &app.name, npx, maxw);
            let lw = text.measure(&label, npx);
            let px = if center { tx + (maxw - lw) * 0.5 } else { tx };
            text.layout_to(out, &label, npx, px, tcy + line_h * 0.5, fg);
        }
    }
}

/// Vykreslí kartu so mriežkou aplikácií (samostatný spúšťač). Súradnice mriežky sú logické, výstup je v pixeloch (× `s`).
fn build(st: &State, g: &Grid, d: &DrunCfg, theme: &Theme, opacity: f32, s: f32, text: &mut Text, icon_paths: &mut HashMap<usize, String>) -> Frame {
    let mut frame = Frame::default();
    let (w, h) = g.size();
    // pozadie má rovnaké krytie ako ostrovček (`[island] opacity`), ak `[drun] opacity` nie je nastavené
    let mut bg = theme.bg;
    bg[3] *= if d.opacity >= 0.0 { d.opacity.min(1.0) } else { opacity };
    frame.back.push(Item::Rect { r: Rect::new(0.0, 0.0, w * s, h * s), radius: d.radius * s, soft: 0.8, color: bg });
    let isz = d.icon_size * s;

    if d.search {
        let r = Rect::new(g.pad * s, g.pad * s * 0.6, (w - 2.0 * g.pad) * s, (g.top - 6.0) * s);
        draw_search(&mut frame.content, d, theme, s, text, r, &st.query);
    }

    let mut cells = Vec::new();
    for slot in 0..g.cols * g.rows {
        let Some(i) = st.at_slot(slot, g.line_len()) else { break };
        let app_idx = st.hits[i];
        let app = &st.apps[app_idx];
        let icon = icon_paths.entry(app_idx).or_insert_with(|| desktop::icon_png(&app.icon, isz.round() as u32, &d.icon_theme)).clone();
        cells.push(CellView { app, icon, selected: st.sel == Some(i) });
    }
    draw_cells(&mut frame.content, g, d, theme, s, text, 0.0, 0.0, &cells);
    if cells.is_empty() {
        let npx = d.name_size * s;
        let (a, dd) = text.line_metrics(npx);
        let msg = fit(text, &empty_message(&st.query), npx, (w - 2.0 * g.pad) * s);
        let tw = text.measure(&msg, npx);
        text.layout_to(&mut frame.content, &msg, npx, (w * s - tw) * 0.5, (g.top + g.pad) * s + (h * s - (g.top + g.pad) * s) * 0.5 + (a - dd) * 0.5, fade(theme.fg, 0.7));
    }
    frame
}

/// Súbor s PID bežiaceho samostatného okna (`$XDG_RUNTIME_DIR/dynamic-island-<názov>.pid`).
fn pid_file(name: &str) -> std::path::PathBuf {
    let dir = std::env::var("XDG_RUNTIME_DIR").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/tmp".into());
    std::path::PathBuf::from(dir).join(format!("dynamic-island-{name}.pid"))
}

/// Beží proces `pid` ako `dynamic-island <flag>` (a nie napr. náhodný proces so starým PID)?
fn is_window_process(pid: i32, flag: &str) -> bool {
    std::fs::read(format!("/proc/{pid}/cmdline")).map(|b| b.split(|c| *c == 0).any(|a| a == flag.as_bytes())).unwrap_or(false)
}

/// Prepínač (ako `pkill rofi || rofi`): ak už okno `name` (spustené s `flag`) beží, zatvorí ho a vráti `true`
/// (tento proces má skončiť).
pub(crate) fn close_running(name: &str, flag: &str) -> bool {
    let me = std::process::id() as i32;
    let pid = std::fs::read_to_string(pid_file(name)).ok().and_then(|s| s.trim().parse::<i32>().ok());
    match pid {
        Some(p) if p != me && is_window_process(p, flag) => {
            crate::sys::terminate(p);
            let _ = std::fs::remove_file(pid_file(name));
            true
        }
        _ => false,
    }
}

/// Zapíše PID tohto procesu a pri skončení ho zmaže.
pub(crate) struct PidGuard(String);

impl PidGuard {
    pub(crate) fn new(name: &str) -> PidGuard {
        let _ = std::fs::write(pid_file(name), std::process::id().to_string());
        PidGuard(name.to_string())
    }
}

impl Drop for PidGuard {
    fn drop(&mut self) {
        let mine = std::process::id().to_string();
        if std::fs::read_to_string(pid_file(&self.0)).map(|s| s.trim() == mine).unwrap_or(false) {
            let _ = std::fs::remove_file(pid_file(&self.0));
        }
    }
}

pub fn run(cfg: Config) -> Result<(), String> {
    // druhé spustenie zatvorí prvé (prepínač), takže jedna klávesová skratka otvára aj zatvára
    if close_running("action", "--action") || close_running("drun", "--drun") {
        return Ok(());
    }
    let _guard = PidGuard::new("drun");
    let d = cfg.drun.clone();
    let apps = desktop::load_all(&d.desktop);
    if apps.is_empty() {
        return Err("nenašli sa žiadne aplikácie (.desktop súbory v XDG_DATA_DIRS)".into());
    }
    let fonts = Fonts::load(&cfg.font_family, &cfg.icon_family)?;
    let mut text = Text::new(Box::new(fonts));

    let (cw, ch) = grid::cell_size(&d.show, d.icon_size, d.name_size, d.cell_w, d.cell_h);
    let top = if d.search { d.name_size * 2.0 + 12.0 } else { 0.0 };
    let g = Grid { cols: d.columns, rows: d.rows, cw, ch, pad: d.padding, top, vertical: d.vertical };
    let (w, h) = g.size();
    let (wu, hu) = (w.ceil() as u32, h.ceil() as u32);

    let mut wl = Wl::new_with(wu, hu, true, "dynamic-island-drun")?;
    let mut scale = wl.st.scale.max(1);
    let egl = wl.create_egl(wu as i32 * scale, hu as i32 * scale)?;
    let mut renderer = Renderer::new(&egl)?;
    let theme = Theme::load(&cfg);
    let dir = cfg.path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| std::path::PathBuf::from("."));
    let mut icons = IconCache::new(dir);
    let mut icon_paths: HashMap<usize, String> = HashMap::new();
    let mut st = State::new(apps);
    let mut dirty = true;
    let mut ptr = (-1.0f32, -1.0f32);

    loop {
        let can_draw = dirty && !wl.st.frame_pending;
        wl.wait_events(&[], if can_draw { 0 } else { 1000 })?;
        if wl.st.closed {
            return Ok(());
        }

        for e in std::mem::take(&mut wl.st.ptr_events) {
            match e {
                PtrEvent::Enter(x, y) | PtrEvent::Motion(x, y) => {
                    ptr = (x as f32, y as f32);
                    if d.hover {
                        if let Some(i) = g.hit(ptr.0, ptr.1).and_then(|s| st.at_slot(s, g.line_len())) {
                            if st.sel != Some(i) {
                                st.sel = Some(i);
                                dirty = true;
                            }
                        }
                    }
                }
                PtrEvent::Button(BTN_LEFT, true) => {
                    // klik spustí bunku pod ukazovateľom (aj pri vypnutom hover)
                    if let Some(i) = g.hit(ptr.0, ptr.1).and_then(|s| st.at_slot(s, g.line_len())) {
                        if let Some(cmd) = st.launch_cmd(i, &d) {
                            crate::app::spawn_detached(&cmd);
                            return Ok(());
                        }
                    }
                }
                PtrEvent::Scroll(v) => {
                    if st.scroll_by(if v > 0.0 { 1 } else { -1 }, &g) {
                        dirty = true;
                    }
                }
                _ => {}
            }
        }

        for k in std::mem::take(&mut wl.st.key_events) {
            match st.key(&k, &g) {
                KeyOut::Close => return Ok(()),
                KeyOut::Launch(i) => {
                    if let Some(cmd) = st.launch_cmd(i, &d) {
                        crate::app::spawn_detached(&cmd);
                        return Ok(());
                    }
                }
                KeyOut::Raw(cmd) => {
                    crate::app::spawn_detached(&cmd);
                    return Ok(());
                }
                KeyOut::Redraw => dirty = true,
                KeyOut::None => {}
            }
        }

        if wl.st.scale_dirty {
            scale = wl.st.scale.max(1);
            wl.apply_scale(wu as i32 * scale, hu as i32 * scale);
            icon_paths.clear();
            dirty = true;
        }

        if dirty && !wl.st.frame_pending {
            let frame = build(&st, &g, &d, &theme, cfg.opacity, scale as f32, &mut text, &mut icon_paths);
            renderer.draw(wu * scale as u32, hu * scale as u32, &frame, &mut text, &mut icons);
            wl.request_frame();
            if !egl.swap() {
                return Err("eglSwapBuffers zlyhalo".into());
            }
            dirty = false;
        }
    }
}
