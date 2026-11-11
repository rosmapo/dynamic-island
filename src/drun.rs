//! Spoločné kreslenie mriežky spúšťača aplikácií (panel `drun` v ostrovčeku): bunky a pole hľadania.
use crate::config::{DrunCfg, Theme};
use crate::desktop::App;
use crate::grid::Grid;
use crate::render::{Color, Item, Rect};
use crate::text::Text;

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
