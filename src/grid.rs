//! Čistá geometria a navigácia mriežky spúšťača (`--drun`): rozmery buniek, zásah myšou, pohyb výberu a posun.

/// Veľkosť bunky podľa režimu; nenulové `cw`/`ch` z konfigurácie majú prednosť.
pub fn cell_size(show: &str, icon: f32, name: f32, cw: f32, ch: f32) -> (f32, f32) {
    let (dw, dh) = match show {
        "name" => (name * 14.0, name * 2.6),
        "icon+name-right" => (icon + 24.0 + name * 10.0, icon * 1.4),
        "icon+name-below" => ((icon * 1.85).max(name * 8.5), icon + name * 1.6 + icon * 0.45),
        _ => (icon * 1.85, icon * 1.45),
    };
    (if cw > 0.0 { cw } else { dw }.round(), if ch > 0.0 { ch } else { dh }.round())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    pub cw: f32,
    pub ch: f32,
    pub pad: f32,
    /// výška riadka hľadania nad mriežkou (0 = žiadny)
    pub top: f32,
    /// zoraďovanie: ďalšia položka je pod predošlou (plní sa stĺpec zhora nadol, posun po stĺpcoch); inak vedľa nej
    pub vertical: bool,
}

impl Grid {
    /// Celkový rozmer okna (logické px).
    pub fn size(&self) -> (f32, f32) {
        (self.cols as f32 * self.cw + 2.0 * self.pad, self.rows as f32 * self.ch + 2.0 * self.pad + self.top)
    }

    /// Položiek v jednej „línii“ (riadok pri horizontálnom, stĺpec pri vertikálnom usporiadaní).
    pub fn line_len(&self) -> usize {
        if self.vertical { self.rows } else { self.cols }
    }

    /// Počet viditeľných línií (riadkov / stĺpcov).
    pub fn lines(&self) -> usize {
        if self.vertical { self.cols } else { self.rows }
    }

    /// Obdĺžnik bunky `slot` (0 .. cols*rows) viditeľnej stránky: x, y, w, h.
    /// Poradie slotov: horizontálne po riadkoch, vertikálne po stĺpcoch.
    pub fn cell(&self, slot: usize) -> (f32, f32, f32, f32) {
        let (c, r) = if self.vertical { (slot / self.rows, slot % self.rows) } else { (slot % self.cols, slot / self.cols) };
        (self.pad + c as f32 * self.cw, self.pad + self.top + r as f32 * self.ch, self.cw, self.ch)
    }

    /// Slot pod bodom (x, y), ak je v mriežke.
    pub fn hit(&self, x: f32, y: f32) -> Option<usize> {
        let (gx, gy) = (x - self.pad, y - self.pad - self.top);
        if gx < 0.0 || gy < 0.0 {
            return None;
        }
        let (c, r) = ((gx / self.cw) as usize, (gy / self.ch) as usize);
        (c < self.cols && r < self.rows).then_some(if self.vertical { c * self.rows + r } else { r * self.cols + c })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Nav {
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
}

/// Nový výber po stlačení klávesy; výber cyklí (ako `cycle: true` v rofi). Bez výberu prvá klávesa vyberie prvú položku.
pub fn nav(sel: Option<usize>, key: Nav, cols: usize, rows: usize, total: usize) -> Option<usize> {
    if total == 0 {
        return None;
    }
    let Some(i) = sel.filter(|i| *i < total) else { return Some(0) };
    let last_row = (total - 1) / cols;
    let (r, c) = (i / cols, i % cols);
    Some(match key {
        Nav::Left => if i == 0 { total - 1 } else { i - 1 },
        Nav::Right => if i + 1 == total { 0 } else { i + 1 },
        Nav::Up => {
            if r > 0 {
                i - cols
            } else {
                // z prvého riadka na koniec toho istého stĺpca (alebo posledná položka)
                let t = last_row * cols + c;
                if t < total { t } else { t - cols }
            }
        }
        Nav::Down => {
            if i + cols < total {
                i + cols
            } else if r < last_row {
                total - 1
            } else {
                c
            }
        }
        Nav::PageUp => i.saturating_sub(cols * rows),
        Nav::PageDown => (i + cols * rows).min(total - 1),
        Nav::Home => 0,
        Nav::End => total - 1,
    })
}

/// Najväčší posun (v riadkoch), pri ktorom je ešte čo ukázať.
pub fn max_scroll(total: usize, cols: usize, rows: usize) -> usize {
    total.div_ceil(cols).saturating_sub(rows)
}

/// Upraví posun tak, aby bol riadok výberu viditeľný.
pub fn scroll_for(scroll: usize, sel: usize, cols: usize, rows: usize, total: usize) -> usize {
    let r = sel / cols;
    let s = if r < scroll { r } else if r >= scroll + rows { r + 1 - rows } else { scroll };
    s.min(max_scroll(total, cols, rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g() -> Grid {
        Grid { cols: 5, rows: 4, cw: 118.0, ch: 94.0, pad: 15.0, top: 0.0, vertical: false }
    }

    #[test]
    fn rofi_like_size() {
        assert_eq!(cell_size("icon", 64.0, 14.0, 0.0, 0.0), (118.0, 93.0));
        assert_eq!(cell_size("icon", 64.0, 14.0, 100.0, 80.0), (100.0, 80.0));
        let (w, h) = g().size();
        assert_eq!((w, h), (620.0, 406.0));
    }

    #[test]
    fn cells_and_hits() {
        let g = g();
        assert_eq!(g.cell(0), (15.0, 15.0, 118.0, 94.0));
        assert_eq!(g.cell(6), (133.0, 109.0, 118.0, 94.0));
        assert_eq!(g.hit(16.0, 16.0), Some(0));
        assert_eq!(g.hit(140.0, 120.0), Some(6));
        assert_eq!(g.hit(10.0, 50.0), None);
        assert_eq!(g.hit(15.0 + 5.0 * 118.0 + 1.0, 50.0), None);
        assert_eq!(g.hit(50.0, 15.0 + 4.0 * 94.0 + 1.0), None);
        let s = Grid { top: 40.0, ..g };
        assert_eq!(s.hit(16.0, 16.0), None);
        assert_eq!(s.hit(16.0, 56.0), Some(0));
    }

    #[test]
    fn vertical_cells_and_hits() {
        // 5 stĺpcov × 4 riadky: sloty idú po stĺpcoch (0..3 = prvý stĺpec)
        let v = Grid { vertical: true, ..g() };
        assert_eq!((v.line_len(), v.lines()), (4, 5));
        assert_eq!(v.cell(0), (15.0, 15.0, 118.0, 94.0));
        assert_eq!(v.cell(1), (15.0, 109.0, 118.0, 94.0)); // druhý riadok prvého stĺpca
        assert_eq!(v.cell(4), (133.0, 15.0, 118.0, 94.0)); // začiatok druhého stĺpca
        assert_eq!(v.hit(16.0, 110.0), Some(1));
        assert_eq!(v.hit(140.0, 16.0), Some(4));
        for slot in 0..20 {
            let (x, y, _, _) = v.cell(slot);
            assert_eq!(v.hit(x + 1.0, y + 1.0), Some(slot));
        }
        assert_eq!((g().line_len(), g().lines()), (5, 4));
    }

    #[test]
    fn navigation_wraps() {
        // 12 položiek v 5 stĺpcoch: riadky 0-4, 5-9, 10-11
        assert_eq!(nav(None, Nav::Down, 5, 4, 12), Some(0));
        assert_eq!(nav(Some(0), Nav::Left, 5, 4, 12), Some(11));
        assert_eq!(nav(Some(11), Nav::Right, 5, 4, 12), Some(0));
        assert_eq!(nav(Some(2), Nav::Down, 5, 4, 12), Some(7));
        assert_eq!(nav(Some(7), Nav::Down, 5, 4, 12), Some(11)); // pod ním nič nie je: posledná položka
        assert_eq!(nav(Some(11), Nav::Down, 5, 4, 12), Some(1)); // z posledného riadka na začiatok stĺpca
        assert_eq!(nav(Some(1), Nav::Up, 5, 4, 12), Some(11));
        assert_eq!(nav(Some(4), Nav::Up, 5, 4, 12), Some(9)); // stĺpec 4 má posledný riadok neúplný
        assert_eq!(nav(Some(8), Nav::Up, 5, 4, 12), Some(3));
        assert_eq!(nav(Some(0), Nav::Home, 5, 4, 12), Some(0));
        assert_eq!(nav(Some(0), Nav::End, 5, 4, 12), Some(11));
        assert_eq!(nav(Some(3), Nav::PageDown, 5, 4, 12), Some(11));
        assert_eq!(nav(Some(11), Nav::PageUp, 5, 4, 12), Some(0));
        assert_eq!(nav(Some(3), Nav::Left, 5, 4, 0), None);
    }

    #[test]
    fn scrolling_follows_selection() {
        assert_eq!(max_scroll(12, 5, 4), 0);
        assert_eq!(max_scroll(40, 5, 4), 4);
        assert_eq!(scroll_for(0, 5, 5, 4, 40), 0);
        assert_eq!(scroll_for(0, 20, 5, 4, 40), 1);
        assert_eq!(scroll_for(3, 2, 5, 4, 40), 0);
        assert_eq!(scroll_for(0, 39, 5, 4, 40), 4);
        assert_eq!(scroll_for(2, 12, 5, 4, 40), 2);
    }
}
