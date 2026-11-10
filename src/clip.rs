//! Samostatné okno histórie schránky (`dynamic-island --clip`), nezávislé od ostrovčeka: vhodné na klávesovú skratku
//! (ako `cliphist list | rofi -dmenu | cliphist decode | wl-copy`). Druhé spustenie okno zatvorí (prepínač).
//! Používa voľby `[clipboard]` (`list_command`, `copy_command`, `max_items`, `rows`, `width`, `font_size`) a vzhľad ostrovčeka.
use crate::config::{Config, Theme};
use crate::data::{clip_filter, parse_clip_list, ClipEntry};
use crate::drun::{close_running, fade, fit, PidGuard};
use crate::fonts::Fonts;
use crate::icons::IconCache;
use crate::render::{Frame, Item, Rect, Renderer};
use crate::text::Text;
use crate::wl::{KeyEvent, PtrEvent, Wl};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

const BTN_LEFT: u32 = 272;
const PAD: f32 = 12.0;
const ROW_EXTRA: f32 = 16.0;
const SCROLLBAR: f32 = 8.0;
const GAP: f32 = 8.0;

#[derive(Debug, PartialEq)]
enum KeyOut {
    None,
    Redraw,
    Close,
    Copy(usize),
}

/// Rozmery okna (logické px).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Geo {
    width: f32,
    rows: usize,
    row_h: f32,
    /// výška riadka hľadania (0 = skrytý)
    search_h: f32,
}

impl Geo {
    fn new(font: f32, rows: usize, width: f32, search: bool) -> Geo {
        Geo { width, rows, row_h: font + ROW_EXTRA, search_h: if search { (font * 2.0).round() } else { 0.0 } }
    }
    fn list_y(&self) -> f32 {
        if self.search_h > 0.0 { PAD + self.search_h + GAP } else { PAD }
    }
    fn size(&self) -> (f32, f32) {
        (self.width, self.list_y() + self.rows as f32 * self.row_h + PAD)
    }
    /// Riadok (0 .. rows) pod bodom (x, y) logických px.
    fn row_at(&self, x: f32, y: f32) -> Option<usize> {
        let gy = y - self.list_y();
        if gy < 0.0 || x < PAD || x > self.width - PAD {
            return None;
        }
        let r = (gy / self.row_h) as usize;
        (r < self.rows).then_some(r)
    }
}

struct State {
    entries: Vec<ClipEntry>,
    query: String,
    hits: Vec<usize>,
    sel: Option<usize>,
    scroll: usize,
    rows: usize,
}

impl State {
    fn new(entries: Vec<ClipEntry>, rows: usize) -> State {
        let hits = clip_filter(&entries, "");
        State { entries, query: String::new(), hits, sel: None, scroll: 0, rows }
    }

    fn max_scroll(&self) -> usize {
        self.hits.len().saturating_sub(self.rows)
    }

    fn refilter(&mut self) {
        self.hits = clip_filter(&self.entries, &self.query);
        self.scroll = 0;
        // po písaní je vybraná najlepšia (prvá) zhoda, Enter ju skopíruje
        self.sel = if self.hits.is_empty() || self.query.trim().is_empty() { None } else { Some(0) };
    }

    fn move_sel(&mut self, delta: isize) {
        let n = self.hits.len();
        if n == 0 {
            return;
        }
        // bez výberu sa ráta od „riadku pred prvým“, takže ↓ vyberie prvú položku
        let base = match self.sel {
            Some(p) => p as isize,
            None if delta > 0 => -1,
            None => 0,
        };
        let p = (base + delta).clamp(0, n as isize - 1) as usize;
        self.sel = Some(p);
        if p < self.scroll {
            self.scroll = p;
        } else if p >= self.scroll + self.rows {
            self.scroll = p + 1 - self.rows;
        }
        self.scroll = self.scroll.min(self.max_scroll());
    }

    fn scroll_by(&mut self, delta: i32) -> bool {
        let n = (self.scroll as i32 + delta).clamp(0, self.max_scroll() as i32) as usize;
        let changed = n != self.scroll;
        self.scroll = n;
        changed
    }

    fn key(&mut self, k: &KeyEvent) -> KeyOut {
        let rows = self.rows as isize;
        match k.sym {
            0xff1b => KeyOut::Close,
            0xff0d | 0xff8d => match self.sel.or(if self.hits.is_empty() { None } else { Some(0) }) {
                Some(p) => KeyOut::Copy(p),
                None => KeyOut::None,
            },
            0xff08 => {
                if self.query.pop().is_some() {
                    self.refilter();
                }
                KeyOut::Redraw
            }
            0xff52 => self.nav(-1),
            0xff54 | 0xff09 => self.nav(1),
            0xfe20 => self.nav(-1),
            0xff55 => self.nav(-rows),
            0xff56 => self.nav(rows),
            0xff50 => self.nav(isize::MIN / 2),
            0xff57 => self.nav(isize::MAX / 2),
            0x75 | 0x55 if k.ctrl => {
                self.query.clear();
                self.refilter();
                KeyOut::Redraw
            }
            _ if !k.ctrl && !k.text.is_empty() && k.text.chars().all(|c| !c.is_control()) => {
                self.query.push_str(&k.text);
                self.refilter();
                KeyOut::Redraw
            }
            _ => KeyOut::None,
        }
    }

    fn nav(&mut self, d: isize) -> KeyOut {
        self.move_sel(d);
        KeyOut::Redraw
    }
}

/// Vykreslí okno (výstup v pixeloch = logické × `s`).
fn build(st: &State, geo: &Geo, cfg: &Config, theme: &Theme, s: f32, text: &mut Text) -> Frame {
    let mut frame = Frame::default();
    let (w, h) = geo.size();
    let mut bg = theme.bg;
    bg[3] *= cfg.opacity;
    frame.back.push(Item::Rect { r: Rect::new(0.0, 0.0, w * s, h * s), radius: cfg.clip_window_radius * s, soft: 0.8, color: bg });
    let px = cfg.clip_font * s;
    let (asc, desc) = text.line_metrics(px);

    // riadok hľadania (filtrovanie písaním funguje aj bez neho)
    if geo.search_h > 0.0 {
    let sr = Rect::new(PAD * s, PAD * s, (w - 2.0 * PAD) * s, geo.search_h * s);
    frame.content.push(Item::Rect { r: sr, radius: sr.h * 0.5, soft: 0.8, color: fade(theme.fg, 0.08) });
    let shown = if st.query.is_empty() { String::new() } else { format!("{}▏", st.query) };
    let shown = fit(text, &shown, px, sr.w - sr.h);
    text.layout_to(&mut frame.content, &shown, px, sr.x + sr.h * 0.5, sr.y + sr.h * 0.5 + (asc - desc) * 0.5, theme.fg);
    }

    let n = st.hits.len();
    let (list_y, row_h) = (geo.list_y() * s, geo.row_h * s);
    if n == 0 {
        let msg = if st.entries.is_empty() { "Schránka je prázdna".to_string() } else { format!("Nič nenájdené pre „{}“", st.query.trim()) };
        let msg = fit(text, &msg, px, (w - 2.0 * PAD) * s);
        let tw = text.measure(&msg, px);
        let cy = list_y + geo.rows as f32 * row_h * 0.5;
        text.layout_to(&mut frame.content, &msg, px, (w * s - tw) * 0.5, cy + (asc - desc) * 0.5, fade(theme.fg, 0.7));
        return frame;
    }
    let scrollable = n > geo.rows;
    let sb = if scrollable { SCROLLBAR * s } else { 0.0 };
    let inset = 10.0 * s;
    let cell_w = (w - 2.0 * PAD) * s - sb;
    for k in 0..geo.rows {
        let pos = st.scroll + k;
        let Some(&i) = st.hits.get(pos) else { break };
        let y = list_y + k as f32 * row_h;
        let cell = Rect::new(PAD * s, y, cell_w, row_h);
        let hov = st.sel == Some(pos);
        let fg = if hov {
            frame.content.push(Item::Rect { r: cell, radius: 10.0 * s, soft: 0.8, color: theme.menu_selected });
            theme.menu_selected_text
        } else {
            theme.fg
        };
        let label = fit(text, &st.entries[i].text, px, cell_w - 2.0 * inset);
        text.layout_to(&mut frame.content, &label, px, cell.x + inset, y + row_h * 0.5 + (asc - desc) * 0.5, fg);
    }
    if scrollable {
        let track_h = geo.rows as f32 * row_h;
        let thumb_h = (track_h * geo.rows as f32 / n as f32).max(16.0 * s).min(track_h);
        let frac = st.scroll as f32 / (n - geo.rows) as f32;
        let tw = 4.0 * s;
        let x = (w - PAD) * s - sb + (sb - tw) * 0.5;
        frame.content.push(Item::Rect { r: Rect::new(x, list_y + (track_h - thumb_h) * frac, tw, thumb_h), radius: tw * 0.5, soft: 0.8, color: fade(theme.fg, 0.35) });
    }
    frame
}

/// Načíta históriu príkazom `list_command`.
fn load(cfg: &Config) -> Vec<ClipEntry> {
    match Command::new("sh").arg("-c").arg(&cfg.clip_list).stdin(Stdio::null()).stderr(Stdio::null()).output() {
        Ok(o) => parse_clip_list(&String::from_utf8_lossy(&o.stdout), cfg.clip_max),
        Err(_) => Vec::new(),
    }
}

/// Pošle položku príkazu `copy_command` na stdin (zápis dokončí hneď, aby sa nestratil po skončení okna).
fn copy(cmd: &str, raw: &str) {
    let mut c = Command::new("sh");
    c.arg("-c").arg(cmd).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe {
        c.pre_exec(|| {
            crate::sys::new_session();
            Ok(())
        });
    }
    if let Ok(mut child) = c.spawn() {
        if let Some(mut si) = child.stdin.take() {
            use std::io::Write;
            let _ = si.write_all(raw.as_bytes());
            let _ = si.write_all(b"\n");
        }
        // `wl-copy` sa odpojí sám; na dokončenie nečakáme dlhšie než chvíľu
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        std::thread::sleep(std::time::Duration::from_millis(80));
    }
}

pub fn run(cfg: Config) -> Result<(), String> {
    // druhé spustenie zatvorí prvé (prepínač)
    if close_running("action", "--action") || close_running("clip", "--clip") {
        return Ok(());
    }
    let _guard = PidGuard::new("clip");
    let entries = load(&cfg);
    let fonts = Fonts::load(&cfg.font_family, &cfg.icon_family)?;
    let mut text = Text::new(Box::new(fonts));

    let geo = Geo::new(cfg.clip_font, cfg.clip_rows, cfg.clip_width, cfg.clip_search);
    let (w, h) = geo.size();
    let (wu, hu) = (w.ceil() as u32, h.ceil() as u32);
    let mut wl = Wl::new_with(wu, hu, true, "dynamic-island-clip")?;
    let mut scale = wl.st.scale.max(1);
    let egl = wl.create_egl(wu as i32 * scale, hu as i32 * scale)?;
    let mut renderer = Renderer::new(&egl)?;
    let theme = Theme::load(&cfg);
    let dir = cfg.path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| std::path::PathBuf::from("."));
    let mut icons = IconCache::new(dir);
    let mut st = State::new(entries, cfg.clip_rows);
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
                    if let Some(r) = geo.row_at(ptr.0, ptr.1) {
                        let pos = st.scroll + r;
                        if pos < st.hits.len() && st.sel != Some(pos) {
                            st.sel = Some(pos);
                            dirty = true;
                        }
                    }
                }
                PtrEvent::Button(BTN_LEFT, true) => {
                    if let Some(r) = geo.row_at(ptr.0, ptr.1) {
                        let pos = st.scroll + r;
                        if let Some(&i) = st.hits.get(pos) {
                            copy(&cfg.clip_copy, &st.entries[i].raw);
                            return Ok(());
                        }
                    }
                }
                PtrEvent::Scroll(v) => {
                    if st.scroll_by(if v > 0.0 { 1 } else { -1 }) {
                        dirty = true;
                    }
                }
                _ => {}
            }
        }

        for k in std::mem::take(&mut wl.st.key_events) {
            match st.key(&k) {
                KeyOut::Close => return Ok(()),
                KeyOut::Copy(pos) => {
                    if let Some(&i) = st.hits.get(pos) {
                        copy(&cfg.clip_copy, &st.entries[i].raw);
                        return Ok(());
                    }
                }
                KeyOut::Redraw => dirty = true,
                KeyOut::None => {}
            }
        }

        if wl.st.scale_dirty {
            scale = wl.st.scale.max(1);
            wl.apply_scale(wu as i32 * scale, hu as i32 * scale);
            dirty = true;
        }

        if dirty && !wl.st.frame_pending {
            let frame = build(&st, &geo, &cfg, &theme, scale as f32, &mut text);
            renderer.draw(wu * scale as u32, hu * scale as u32, &frame, &mut text, &mut icons);
            wl.request_frame();
            if !egl.swap() {
                return Err("eglSwapBuffers zlyhalo".into());
            }
            dirty = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(n: usize) -> Vec<ClipEntry> {
        (0..n).map(|i| ClipEntry { raw: format!("{i}\tx"), text: format!("položka {i}") }).collect()
    }

    fn key(sym: u32, text: &str, ctrl: bool) -> KeyEvent {
        KeyEvent { sym, text: text.into(), ctrl }
    }

    #[test]
    fn geometry() {
        let g = Geo::new(15.0, 8, 420.0, true);
        assert_eq!(g.row_h, 31.0);
        let (w, h) = g.size();
        assert_eq!((w, h), (420.0, 12.0 + 30.0 + 8.0 + 8.0 * 31.0 + 12.0));
        assert_eq!(g.row_at(20.0, g.list_y() + 1.0), Some(0));
        assert_eq!(g.row_at(20.0, g.list_y() + 31.0 * 2.0 + 1.0), Some(2));
        assert_eq!(g.row_at(20.0, g.list_y() - 1.0), None);
        assert_eq!(g.row_at(5.0, g.list_y() + 1.0), None);
        assert_eq!(g.row_at(20.0, g.list_y() + 31.0 * 8.0 + 1.0), None);
        let n = Geo::new(15.0, 8, 420.0, false);
        assert_eq!((n.list_y(), n.size().1), (12.0, 12.0 + 8.0 * 31.0 + 12.0));
    }

    #[test]
    fn selection_and_scroll() {
        let mut st = State::new(entries(20), 8);
        assert_eq!(st.key(&key(0xff54, "", false)), KeyOut::Redraw); // ↓ vyberie prvú
        assert_eq!(st.sel, Some(0));
        for _ in 0..9 {
            st.key(&key(0xff54, "", false));
        }
        assert_eq!((st.sel, st.scroll), (Some(9), 2)); // výber zostane viditeľný
        st.key(&key(0xff57, "", false)); // End
        assert_eq!((st.sel, st.scroll), (Some(19), 12));
        st.key(&key(0xff50, "", false)); // Home
        assert_eq!((st.sel, st.scroll), (Some(0), 0));
        assert!(st.scroll_by(1) && st.scroll == 1);
        assert!(!st.scroll_by(-5) || st.scroll == 0);
    }

    #[test]
    fn typing_filters_and_enter_copies_best_match() {
        let mut st = State::new(entries(20), 8);
        for c in "položka 1".chars() {
            st.key(&key(0x20, &c.to_string(), false));
        }
        assert_eq!(st.hits.len(), 11); // 1, 10..19
        assert_eq!(st.sel, Some(0));
        assert_eq!(st.key(&key(0xff0d, "", false)), KeyOut::Copy(0));
        st.key(&key(0x75, "u", true)); // Ctrl+U
        assert_eq!(st.query, "");
        assert_eq!(st.hits.len(), 20);
        for c in "zzz".chars() {
            st.key(&key(0x7a, &c.to_string(), false));
        }
        assert_eq!(st.key(&key(0xff0d, "", false)), KeyOut::None);
        assert_eq!(st.key(&key(0xff1b, "", false)), KeyOut::Close);
    }
}
