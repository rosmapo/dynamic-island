//! Načítanie písiem (fontdb) a rasterizácia (fontdue). Jediný modul, ktorý ich pozná.
use crate::text::{GlyphSource, RawGlyph};

pub struct Fonts {
    faces: Vec<fontdue::Font>,
    text: Vec<usize>,
    icon: Vec<usize>,
}

impl Fonts {
    pub fn load(text_families: &[String], icon_families: &[String]) -> Result<Fonts, String> {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let mut faces: Vec<fontdue::Font> = Vec::new();
        let mut seen: Vec<fontdb::ID> = Vec::new();
        let mut pick = |fams: &[String]| -> Vec<usize> {
            let mut idx = Vec::new();
            for name in fams {
                let q = fontdb::Query { families: &[fontdb::Family::Name(name.as_str())], ..Default::default() };
                let found = db.query(&q);
                if found.is_none() {
                    eprintln!("dynamic-island: písmo '{name}' nie je nainštalované");
                }
                if let Some(id) = found {
                    if seen.contains(&id) {
                        continue;
                    }
                    let font = db.with_face_data(id, |data, index| {
                        fontdue::Font::from_bytes(
                            data,
                            fontdue::FontSettings { collection_index: index, ..Default::default() },
                        )
                    });
                    if let Some(Ok(f)) = font {
                        seen.push(id);
                        faces.push(f);
                        idx.push(faces.len() - 1);
                    }
                }
            }
            idx
        };
        let text = pick(text_families);
        let icon = pick(icon_families);
        if text.is_empty() && icon.is_empty() {
            return Err("nenašiel som žiadne použiteľné písmo z font_family / icon_family v konfigurácii".into());
        }
        Ok(Fonts { faces, text, icon })
    }

    fn order(&self, icon: bool) -> impl Iterator<Item = usize> + '_ {
        let (a, b) = if icon { (&self.icon, &self.text) } else { (&self.text, &self.icon) };
        a.iter().chain(b.iter()).copied()
    }
}

impl GlyphSource for Fonts {
    fn rasterize(&mut self, ch: char, px: f32, icon: bool) -> Option<RawGlyph> {
        let found = self.order(icon).find(|&i| self.faces[i].lookup_glyph_index(ch) != 0);
        let fi = match found {
            Some(i) => i,
            None if ch == ' ' => self.order(icon).next()?,
            None => self.order(icon).next()?, // zobrazí sa „tofu“ / .notdef prvého písma
        };
        let (m, bitmap) = self.faces[fi].rasterize(ch, px);
        Some(RawGlyph { w: m.width as u32, h: m.height as u32, xmin: m.xmin, ymin: m.ymin, advance: m.advance_width, bitmap })
    }

    fn line_metrics(&mut self, px: f32) -> (f32, f32) {
        let fi = self.text.first().or(self.icon.first()).copied().unwrap_or(0);
        match self.faces[fi].horizontal_line_metrics(px) {
            Some(l) => (l.ascent, -l.descent),
            None => (px * 0.8, px * 0.2),
        }
    }
}
