//! Načítanie a cache PNG ikon (RGBA) pre custom menu.
//! Cesta: len názov → `{config_dir}/icons/názov`; inak expandovaná cesta (`~/…` alebo absolútna).
use crate::config::expand;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct IconRgba {
    pub w: u32,
    pub h: u32,
    pub pixels: Vec<u8>, // RGBA8
}

#[derive(Default)]
pub struct IconCache {
    /// Adresár konfigurácie (rodič `config.toml`) – predvolené miesto pre `icons/`.
    config_dir: PathBuf,
    map: HashMap<String, Option<IconRgba>>,
}

impl IconCache {
    pub fn new(config_dir: PathBuf) -> IconCache {
        IconCache { config_dir, map: HashMap::new() }
    }

    /// `icon.png` → `{config_dir}/icons/icon.png`
    /// `~/foo/bar.png` / `/abs/path.png` → expandovaná cesta
    pub fn resolve(&self, path: &str) -> PathBuf {
        let p = path.trim();
        if p.is_empty() {
            return PathBuf::new();
        }
        // absolútna alebo s ~/ alebo s ./ alebo obsahuje /
        if p.starts_with('/') || p.starts_with("~/") || p.starts_with("./") || p.contains('/') {
            return PathBuf::from(expand(p));
        }
        self.config_dir.join("icons").join(p)
    }

    /// Vráti ikonu. Pri chybe `None` (stderr raz pri prvom pokuse).
    pub fn get(&mut self, path: &str) -> Option<&IconRgba> {
        if path.trim().is_empty() {
            return None;
        }
        let key = self.resolve(path).to_string_lossy().into_owned();
        if key.is_empty() {
            return None;
        }
        if !self.map.contains_key(&key) {
            let loaded = load_png(&key);
            if loaded.is_none() {
                eprintln!("dynamic-island: nepodarilo sa načítať ikonu '{key}'");
            }
            self.map.insert(key.clone(), loaded);
        }
        self.map.get(&key).and_then(|v| v.as_ref())
    }

    pub fn preload<'a>(&mut self, paths: impl IntoIterator<Item = &'a str>) {
        for p in paths {
            let _ = self.get(p);
        }
    }
}

fn load_png(path: &str) -> Option<IconRgba> {
    let p = Path::new(path);
    if !p.is_file() {
        return None;
    }
    let file = std::fs::File::open(p).ok()?;
    let mut decoder = png::Decoder::new(file);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width, info.height);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => {
            let rgb = &buf[..info.buffer_size()];
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for chunk in rgb.chunks_exact(3) {
                out.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
            }
            out
        }
        png::ColorType::Grayscale => {
            let g = &buf[..info.buffer_size()];
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for &v in g {
                out.extend_from_slice(&[v, v, v, 255]);
            }
            out
        }
        png::ColorType::GrayscaleAlpha => {
            let ga = &buf[..info.buffer_size()];
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for chunk in ga.chunks_exact(2) {
                out.extend_from_slice(&[chunk[0], chunk[0], chunk[0], chunk[1]]);
            }
            out
        }
        png::ColorType::Indexed => return None,
    };
    Some(IconRgba { w, h, pixels: rgba })
}
