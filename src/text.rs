//! Text: rasterizované glyfy v jednom A8 atlase + rozloženie reťazca do quadov.
use std::collections::HashMap;

pub type Color = [f32; 4];

/// Surový glyf od poskytovateľa písma (bitmapa pokrytia 0..255).
pub struct RawGlyph {
    pub w: u32,
    pub h: u32,
    pub xmin: i32,
    pub ymin: i32, // spodok bitmapy nad základnou čiarou (môže byť záporný)
    pub advance: f32,
    pub bitmap: Vec<u8>,
}

pub trait GlyphSource {
    fn rasterize(&mut self, ch: char, px: f32, icon: bool) -> Option<RawGlyph>;
    /// (ascent, descent) v pixeloch, obe kladné.
    fn line_metrics(&mut self, px: f32) -> (f32, f32);
}

/// Znaky z oblastí súkromného použitia (Nerd Font ikony) idú cez ikonové písmo.
pub fn is_icon(ch: char) -> bool {
    let c = ch as u32;
    (0xE000..=0xF8FF).contains(&c) || c >= 0xF0000
}

#[derive(Clone, Copy, Debug)]
pub struct GlyphInfo {
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
    pub w: f32,
    pub h: f32,
    pub left: f32,
    pub top: f32, // vzdialenosť horného okraja bitmapy nad základnou čiarou
    pub advance: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct GlyphQuad {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
    pub color: Color,
}

pub const ATLAS: u32 = 1024;

pub struct Text {
    src: Box<dyn GlyphSource>,
    pub pixels: Vec<u8>,
    pub dirty: bool,
    /// zvýši sa pri každom vyčistení atlasu (renderer podľa toho vie, že musí nahrať celý)
    pub generation: u32,
    shelf_x: u32,
    shelf_y: u32,
    shelf_h: u32,
    cache: HashMap<(char, u32), Option<GlyphInfo>>,
    metrics: HashMap<u32, (f32, f32)>,
}

impl Text {
    pub fn new(src: Box<dyn GlyphSource>) -> Text {
        Text {
            src,
            pixels: vec![0; (ATLAS * ATLAS) as usize],
            dirty: true,
            generation: 0,
            shelf_x: 1,
            shelf_y: 1,
            shelf_h: 0,
            cache: HashMap::new(),
            metrics: HashMap::new(),
        }
    }

    fn key_px(px: f32) -> u32 {
        (px * 16.0).round() as u32
    }

    fn reset(&mut self) {
        self.pixels.iter_mut().for_each(|p| *p = 0);
        self.cache.clear();
        self.shelf_x = 1;
        self.shelf_y = 1;
        self.shelf_h = 0;
        self.generation += 1;
        self.dirty = true;
    }

    fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w + 2 > ATLAS || h + 2 > ATLAS {
            return None;
        }
        if self.shelf_x + w + 1 > ATLAS {
            self.shelf_y += self.shelf_h + 1;
            self.shelf_x = 1;
            self.shelf_h = 0;
        }
        if self.shelf_y + h + 1 > ATLAS {
            return None;
        }
        let p = (self.shelf_x, self.shelf_y);
        self.shelf_x += w + 1;
        self.shelf_h = self.shelf_h.max(h);
        Some(p)
    }

    pub fn glyph(&mut self, ch: char, px: f32) -> Option<GlyphInfo> {
        let key = (ch, Self::key_px(px));
        if let Some(g) = self.cache.get(&key) {
            return *g;
        }
        let raw = self.src.rasterize(ch, px, is_icon(ch));
        let info = match raw {
            None => None,
            Some(g) => {
                if g.w == 0 || g.h == 0 {
                    // medzera: len posun
                    Some(GlyphInfo { u0: 0.0, v0: 0.0, u1: 0.0, v1: 0.0, w: 0.0, h: 0.0, left: 0.0, top: 0.0, advance: g.advance })
                } else {
                    let pos = match self.alloc(g.w, g.h) {
                        Some(p) => p,
                        None => {
                            self.reset();
                            self.alloc(g.w, g.h)?
                        }
                    };
                    for row in 0..g.h {
                        let dst = ((pos.1 + row) * ATLAS + pos.0) as usize;
                        let srcrow = (row * g.w) as usize;
                        self.pixels[dst..dst + g.w as usize].copy_from_slice(&g.bitmap[srcrow..srcrow + g.w as usize]);
                    }
                    self.dirty = true;
                    let a = ATLAS as f32;
                    Some(GlyphInfo {
                        u0: pos.0 as f32 / a,
                        v0: pos.1 as f32 / a,
                        u1: (pos.0 + g.w) as f32 / a,
                        v1: (pos.1 + g.h) as f32 / a,
                        w: g.w as f32,
                        h: g.h as f32,
                        left: g.xmin as f32,
                        top: (g.ymin + g.h as i32) as f32,
                        advance: g.advance,
                    })
                }
            }
        };
        self.cache.insert(key, info);
        info
    }

    pub fn measure(&mut self, s: &str, px: f32) -> f32 {
        let mut w = 0.0;
        for ch in s.chars() {
            if let Some(g) = self.glyph(ch, px) {
                w += g.advance;
            }
        }
        w
    }

    pub fn line_metrics(&mut self, px: f32) -> (f32, f32) {
        let k = Self::key_px(px);
        if let Some(m) = self.metrics.get(&k) {
            return *m;
        }
        let m = self.src.line_metrics(px);
        self.metrics.insert(k, m);
        m
    }

    /// Rozloží reťazec od (x, baseline); výstup quady v pixeloch (zaokrúhlené na celé pixely).
    pub fn layout(&mut self, s: &str, px: f32, x: f32, baseline: f32, color: Color, out: &mut Vec<GlyphQuad>) {
        let mut pen = x;
        for ch in s.chars() {
            if let Some(g) = self.glyph(ch, px) {
                if g.w > 0.0 {
                    out.push(GlyphQuad {
                        x: (pen + g.left).round(),
                        y: (baseline - g.top).round(),
                        w: g.w,
                        h: g.h,
                        u0: g.u0,
                        v0: g.v0,
                        u1: g.u1,
                        v1: g.v1,
                        color,
                    });
                }
                pen += g.advance;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Box8;
    impl GlyphSource for Box8 {
        fn rasterize(&mut self, ch: char, _px: f32, _icon: bool) -> Option<RawGlyph> {
            if ch == ' ' {
                return Some(RawGlyph { w: 0, h: 0, xmin: 0, ymin: 0, advance: 4.0, bitmap: vec![] });
            }
            Some(RawGlyph { w: 6, h: 8, xmin: 1, ymin: 0, advance: 8.0, bitmap: vec![255; 48] })
        }
        fn line_metrics(&mut self, px: f32) -> (f32, f32) {
            (px * 0.8, px * 0.2)
        }
    }
    #[test]
    fn layout_and_cache() {
        let mut t = Text::new(Box::new(Box8));
        assert_eq!(t.measure("ab c", 16.0), 8.0 + 8.0 + 4.0 + 8.0);
        let mut q = Vec::new();
        t.layout("ab", 16.0, 10.0, 20.0, [1.0; 4], &mut q);
        assert_eq!(q.len(), 2);
        assert_eq!(q[0].x, 11.0);
        assert_eq!(q[0].y, 12.0); // 20 - 8
        assert_eq!(q[1].x, 19.0);
        assert!(is_icon('\u{f287}') && !is_icon('a'));
    }
    #[test]
    fn atlas_overflow_resets() {
        let mut t = Text::new(Box::new(Box8));
        for i in 0..20000u32 {
            let ch = char::from_u32(0x4e00 + i).unwrap();
            assert!(t.glyph(ch, 16.0).is_some());
        }
        assert!(t.generation >= 1);
    }
}
