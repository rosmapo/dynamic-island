//! OpenGL ES 2.0 renderer: SDF zaoblené obdĺžniky (tiene, AA) + textové quady z atlasu + RGBA ikony.
use crate::gl::*;
use crate::icons::{IconCache, IconRgba};
use crate::text::{GlyphQuad, Text, ATLAS};
use std::collections::HashMap;
use std::ffi::CString;

pub type Color = [f32; 4];

#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}
impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

#[derive(Clone, Debug)]
pub enum Item {
    /// `soft` = šírka okraja v px (0.8 = ostrá hrana, väčšie = rozmazaný tieň); farba nie je premultiplikovaná
    Rect { r: Rect, radius: f32, soft: f32, color: Color },
    Glyph(GlyphQuad),
    /// RGBA textúra; `path` je kľúč v IconCache (už expandovaný / resolved).
    /// `radius` > 0 zaoblí rohy obrázka (v shadri, s vyhladením hrany)
    Image { r: Rect, path: String, color: Color, radius: f32 },
}

impl Item {
    /// Vynásobí krytie položky číslom `f` (0..1); používa fade-out okna spúšťača pri prechode do panela.
    pub fn fade(&mut self, f: f32) {
        let c = match self {
            Item::Rect { color, .. } => color,
            Item::Glyph(q) => &mut q.color,
            Item::Image { color, .. } => color,
        };
        c[3] *= f.clamp(0.0, 1.0);
    }
}

#[derive(Default)]
pub struct Frame {
    /// pozadie, tieň (bez orezania)
    pub back: Vec<Item>,
    /// obsah (orezaný na `clip`)
    pub content: Vec<Item>,
    pub clip: Option<Rect>,
}

impl Frame {
    /// Vynásobí krytie všetkých položiek (pozadia aj obsahu) číslom `f` (0..1).
    pub fn fade(&mut self, f: f32) {
        for it in self.back.iter_mut().chain(self.content.iter_mut()) {
            it.fade(f);
        }
    }
}

const RECT_VS: &str = "
attribute vec2 a_pos; attribute vec2 a_local; attribute vec4 a_shape; attribute vec4 a_color;
uniform vec2 u_res;
varying vec2 v_local; varying vec4 v_shape; varying vec4 v_color;
void main() {
  v_local = a_local; v_shape = a_shape; v_color = a_color;
  gl_Position = vec4(a_pos.x / u_res.x * 2.0 - 1.0, 1.0 - a_pos.y / u_res.y * 2.0, 0.0, 1.0);
}";
const RECT_FS: &str = "
#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
varying vec2 v_local; varying vec4 v_shape; varying vec4 v_color;
float sdbox(vec2 p, vec2 b, float r) {
  vec2 q = abs(p) - b + r;
  return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}
void main() {
  float d = sdbox(v_local, v_shape.xy, v_shape.z);
  float s = max(v_shape.w, 0.5);
  float a = 1.0 - smoothstep(-s, s, d);
  gl_FragColor = v_color * a;
}";
const TEXT_VS: &str = "
attribute vec2 a_pos; attribute vec2 a_uv; attribute vec4 a_color;
uniform vec2 u_res;
varying vec2 v_uv; varying vec4 v_color;
void main() {
  v_uv = a_uv; v_color = a_color;
  gl_Position = vec4(a_pos.x / u_res.x * 2.0 - 1.0, 1.0 - a_pos.y / u_res.y * 2.0, 0.0, 1.0);
}";
const TEXT_FS: &str = "
precision mediump float;
varying vec2 v_uv; varying vec4 v_color;
uniform sampler2D u_tex;
void main() { gl_FragColor = v_color * texture2D(u_tex, v_uv).a; }";
// RGBA ikony (premultiplied alpha výstup)
const IMAGE_FS: &str = "
precision mediump float;
varying vec2 v_uv; varying vec4 v_color;
uniform sampler2D u_tex;
uniform vec2 u_size;
uniform vec2 u_radius;
void main() {
  vec4 t = texture2D(u_tex, v_uv);
  float k = 1.0;
  if (u_radius.x > 0.0) {
    vec2 p = (v_uv - 0.5) * u_size;
    vec2 q = abs(p) - (u_size * 0.5 - vec2(u_radius.x));
    float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - u_radius.x;
    k = clamp(0.5 - d, 0.0, 1.0);
  }
  gl_FragColor = vec4(t.rgb * t.a, t.a) * v_color * k;
}";

pub struct Renderer {
    gl: Gl,
    rect_prog: u32,
    text_prog: u32,
    image_prog: u32,
    rect_res: i32,
    text_res: i32,
    text_tex_u: i32,
    image_res: i32,
    image_tex_u: i32,
    image_size: i32,
    image_radius: i32,
    vbo: u32,
    tex: u32,
    tex_gen: u32,
    /// path → GL texture id
    img_tex: HashMap<String, u32>,
    verts: Vec<f32>,
}

fn pm(c: Color) -> [f32; 4] {
    [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]]
}

impl Renderer {
    pub fn new(egl: &Egl) -> Result<Renderer, String> {
        let gl = Gl::load(&|n| egl.get_proc(n))?;
        unsafe {
            let rect_prog = Self::program(&gl, RECT_VS, RECT_FS, &["a_pos", "a_local", "a_shape", "a_color"])?;
            let text_prog = Self::program(&gl, TEXT_VS, TEXT_FS, &["a_pos", "a_uv", "a_color"])?;
            let image_prog = Self::program(&gl, TEXT_VS, IMAGE_FS, &["a_pos", "a_uv", "a_color"])?;
            let u = |p: u32, n: &str| {
                let c = CString::new(n).unwrap();
                (gl.get_uniform_location)(p, c.as_ptr())
            };
            let rect_res = u(rect_prog, "u_res");
            let text_res = u(text_prog, "u_res");
            let text_tex_u = u(text_prog, "u_tex");
            let image_res = u(image_prog, "u_res");
            let image_tex_u = u(image_prog, "u_tex");
            let image_size = u(image_prog, "u_size");
            let image_radius = u(image_prog, "u_radius");
            let mut vbo = 0u32;
            (gl.gen_buffers)(1, &mut vbo);
            let mut tex = 0u32;
            (gl.gen_textures)(1, &mut tex);
            (gl.bind_texture)(TEXTURE_2D, tex);
            (gl.tex_parameteri)(TEXTURE_2D, TEXTURE_MIN_FILTER, LINEAR);
            (gl.tex_parameteri)(TEXTURE_2D, TEXTURE_MAG_FILTER, LINEAR);
            (gl.tex_parameteri)(TEXTURE_2D, TEXTURE_WRAP_S, CLAMP_TO_EDGE);
            (gl.tex_parameteri)(TEXTURE_2D, TEXTURE_WRAP_T, CLAMP_TO_EDGE);
            Ok(Renderer {
                gl,
                rect_prog,
                text_prog,
                image_prog,
                rect_res,
                text_res,
                text_tex_u,
                image_res,
                image_tex_u,
                image_size,
                image_radius,
                vbo,
                tex,
                tex_gen: u32::MAX,
                img_tex: HashMap::new(),
                verts: Vec::new(),
            })
        }
    }

    unsafe fn shader(gl: &Gl, kind: u32, src: &str) -> Result<u32, String> {
        let s = (gl.create_shader)(kind);
        let c = CString::new(src).unwrap();
        let ptr = c.as_ptr();
        (gl.shader_source)(s, 1, &ptr, std::ptr::null());
        (gl.compile_shader)(s);
        let mut ok = 0;
        (gl.get_shader_iv)(s, COMPILE_STATUS, &mut ok);
        if ok == 0 {
            let mut len = 0;
            (gl.get_shader_iv)(s, INFO_LOG_LENGTH, &mut len);
            let mut buf = vec![0u8; len.max(1) as usize];
            (gl.get_shader_info_log)(s, len, std::ptr::null_mut(), buf.as_mut_ptr() as *mut _);
            return Err(format!("shader: {}", String::from_utf8_lossy(&buf).trim_end_matches('\0')));
        }
        Ok(s)
    }

    unsafe fn program(gl: &Gl, vs: &str, fs: &str, attribs: &[&str]) -> Result<u32, String> {
        let v = Self::shader(gl, VERTEX_SHADER, vs)?;
        let f = Self::shader(gl, FRAGMENT_SHADER, fs)?;
        let p = (gl.create_program)();
        (gl.attach_shader)(p, v);
        (gl.attach_shader)(p, f);
        for (i, a) in attribs.iter().enumerate() {
            let c = CString::new(*a).unwrap();
            (gl.bind_attrib_location)(p, i as u32, c.as_ptr());
        }
        (gl.link_program)(p);
        let mut ok = 0;
        (gl.get_program_iv)(p, LINK_STATUS, &mut ok);
        if ok == 0 {
            let mut buf = vec![0u8; 1024];
            (gl.get_program_info_log)(p, 1024, std::ptr::null_mut(), buf.as_mut_ptr() as *mut _);
            return Err(format!("link: {}", String::from_utf8_lossy(&buf).trim_end_matches('\0')));
        }
        (gl.delete_shader)(v);
        (gl.delete_shader)(f);
        Ok(p)
    }

    fn upload_atlas(&mut self, text: &mut Text) {
        if !text.dirty {
            return;
        }
        unsafe {
            let gl = &self.gl;
            (gl.active_texture)(TEXTURE0);
            (gl.bind_texture)(TEXTURE_2D, self.tex);
            (gl.pixel_storei)(UNPACK_ALIGNMENT, 1);
            (gl.tex_image_2d)(
                TEXTURE_2D,
                0,
                ALPHA as i32,
                ATLAS as i32,
                ATLAS as i32,
                0,
                ALPHA,
                UNSIGNED_BYTE,
                text.pixels.as_ptr() as *const _,
            );
        }
        self.tex_gen = text.generation;
        text.dirty = false;
    }

    fn ensure_image_tex(&mut self, path: &str, icons: &mut IconCache) -> Option<u32> {
        if let Some(&id) = self.img_tex.get(path) {
            return Some(id);
        }
        let icon = icons.get(path)?;
        let id = self.upload_rgba(icon);
        self.img_tex.insert(path.to_string(), id);
        Some(id)
    }

    fn upload_rgba(&self, icon: &IconRgba) -> u32 {
        unsafe {
            let gl = &self.gl;
            let mut id = 0u32;
            (gl.gen_textures)(1, &mut id);
            (gl.bind_texture)(TEXTURE_2D, id);
            (gl.tex_parameteri)(TEXTURE_2D, TEXTURE_MIN_FILTER, LINEAR);
            (gl.tex_parameteri)(TEXTURE_2D, TEXTURE_MAG_FILTER, LINEAR);
            (gl.tex_parameteri)(TEXTURE_2D, TEXTURE_WRAP_S, CLAMP_TO_EDGE);
            (gl.tex_parameteri)(TEXTURE_2D, TEXTURE_WRAP_T, CLAMP_TO_EDGE);
            (gl.pixel_storei)(UNPACK_ALIGNMENT, 4);
            (gl.tex_image_2d)(
                TEXTURE_2D,
                0,
                RGBA as i32,
                icon.w as i32,
                icon.h as i32,
                0,
                RGBA,
                UNSIGNED_BYTE,
                icon.pixels.as_ptr() as *const _,
            );
            id
        }
    }

    pub fn draw(&mut self, w: u32, h: u32, frame: &Frame, text: &mut Text, icons: &mut IconCache) {
        self.upload_atlas(text);
        unsafe {
            let gl = &self.gl;
            (gl.viewport)(0, 0, w as i32, h as i32);
            (gl.disable)(SCISSOR_TEST);
            (gl.clear_color)(0.0, 0.0, 0.0, 0.0);
            (gl.clear)(COLOR_BUFFER_BIT);
            (gl.enable)(BLEND);
            (gl.blend_func)(ONE, ONE_MINUS_SRC_ALPHA);
        }
        self.draw_items(&frame.back, w, h, icons);
        if let Some(c) = frame.clip {
            unsafe {
                let x = c.x.floor().max(0.0) as i32;
                let y_top = c.y.floor().max(0.0) as i32;
                let cw = (c.w.ceil() as i32 + 1).max(0);
                let ch = (c.h.ceil() as i32 + 1).max(0);
                (self.gl.enable)(SCISSOR_TEST);
                (self.gl.scissor)(x, h as i32 - (y_top + ch), cw, ch);
            }
        }
        self.draw_items(&frame.content, w, h, icons);
        unsafe { (self.gl.disable)(SCISSOR_TEST) };
    }

    fn draw_items(&mut self, items: &[Item], w: u32, h: u32, icons: &mut IconCache) {
        let mut i = 0;
        while i < items.len() {
            match &items[i] {
                Item::Image { r, path, color, radius } => {
                    if let Some(tex) = self.ensure_image_tex(path, icons) {
                        self.draw_image(*r, tex, *color, *radius, w, h);
                    }
                    i += 1;
                }
                Item::Rect { .. } => {
                    self.verts.clear();
                    let mut j = i;
                    while j < items.len() {
                        if let Item::Rect { r, radius, soft, color } = items[j] {
                            self.push_rect(r, radius, soft, color);
                            j += 1;
                        } else {
                            break;
                        }
                    }
                    self.flush_rect(w, h);
                    i = j;
                }
                Item::Glyph(_) => {
                    self.verts.clear();
                    let mut j = i;
                    while j < items.len() {
                        if let Item::Glyph(q) = &items[j] {
                            self.push_glyph(*q);
                            j += 1;
                        } else {
                            break;
                        }
                    }
                    self.flush_glyph(w, h);
                    i = j;
                }
            }
        }
    }

    fn push_rect(&mut self, r: Rect, radius: f32, soft: f32, color: Color) {
        let c = pm(color);
        let pad = soft.max(0.8) + 1.0;
        let (hw, hh) = (r.w * 0.5, r.h * 0.5);
        let (cx, cy) = (r.x + hw, r.y + hh);
        let rad = radius.min(hw).min(hh).max(0.0);
        let (x0, y0, x1, y1) = (r.x - pad, r.y - pad, r.x + r.w + pad, r.y + r.h + pad);
        let corners = [(x0, y0), (x1, y0), (x0, y1), (x1, y0), (x1, y1), (x0, y1)];
        for (x, y) in corners {
            self.verts.extend_from_slice(&[x, y, x - cx, y - cy, hw, hh, rad, soft, c[0], c[1], c[2], c[3]]);
        }
    }

    fn push_glyph(&mut self, q: GlyphQuad) {
        let c = pm(q.color);
        let (x0, y0, x1, y1) = (q.x, q.y, q.x + q.w, q.y + q.h);
        let v = [
            (x0, y0, q.u0, q.v0),
            (x1, y0, q.u1, q.v0),
            (x0, y1, q.u0, q.v1),
            (x1, y0, q.u1, q.v0),
            (x1, y1, q.u1, q.v1),
            (x0, y1, q.u0, q.v1),
        ];
        for (x, y, u, vv) in v {
            self.verts.extend_from_slice(&[x, y, u, vv, c[0], c[1], c[2], c[3]]);
        }
    }

    fn flush_rect(&mut self, w: u32, h: u32) {
        if self.verts.is_empty() {
            return;
        }
        let count = (self.verts.len() / 12) as i32;
        unsafe {
            let gl = &self.gl;
            (gl.bind_buffer)(ARRAY_BUFFER, self.vbo);
            (gl.buffer_data)(ARRAY_BUFFER, (self.verts.len() * 4) as isize, self.verts.as_ptr() as *const _, STREAM_DRAW);
            let stride = 48i32;
            let attr = |idx: u32, n: i32, off: usize| {
                (gl.enable_vertex_attrib_array)(idx);
                (gl.vertex_attrib_pointer)(idx, n, FLOAT, 0, stride, (off * 4) as *const _);
            };
            (gl.use_program)(self.rect_prog);
            (gl.uniform2f)(self.rect_res, w as f32, h as f32);
            attr(0, 2, 0);
            attr(1, 2, 2);
            attr(2, 4, 4);
            attr(3, 4, 8);
            (gl.draw_arrays)(TRIANGLES, 0, count);
        }
    }

    fn flush_glyph(&mut self, w: u32, h: u32) {
        if self.verts.is_empty() {
            return;
        }
        let count = (self.verts.len() / 8) as i32;
        unsafe {
            let gl = &self.gl;
            (gl.bind_buffer)(ARRAY_BUFFER, self.vbo);
            (gl.buffer_data)(ARRAY_BUFFER, (self.verts.len() * 4) as isize, self.verts.as_ptr() as *const _, STREAM_DRAW);
            let stride = 32i32;
            let attr = |idx: u32, n: i32, off: usize| {
                (gl.enable_vertex_attrib_array)(idx);
                (gl.vertex_attrib_pointer)(idx, n, FLOAT, 0, stride, (off * 4) as *const _);
            };
            (gl.use_program)(self.text_prog);
            (gl.uniform2f)(self.text_res, w as f32, h as f32);
            (gl.active_texture)(TEXTURE0);
            (gl.bind_texture)(TEXTURE_2D, self.tex);
            (gl.uniform1i)(self.text_tex_u, 0);
            attr(0, 2, 0);
            attr(1, 2, 2);
            attr(2, 4, 4);
            (gl.draw_arrays)(TRIANGLES, 0, count);
        }
    }

    fn draw_image(&mut self, r: Rect, tex: u32, color: Color, radius: f32, w: u32, h: u32) {
        let c = [color[0], color[1], color[2], color[3]]; // alpha multiply v shadri cez v_color
        let (x0, y0, x1, y1) = (r.x, r.y, r.x + r.w, r.y + r.h);
        let verts = [
            // PNG zhora nadol → prvý riadok na v=0; y0 (hore) = v=0
            x0, y0, 0.0, 0.0, c[0], c[1], c[2], c[3],
            x1, y0, 1.0, 0.0, c[0], c[1], c[2], c[3],
            x0, y1, 0.0, 1.0, c[0], c[1], c[2], c[3],
            x1, y0, 1.0, 0.0, c[0], c[1], c[2], c[3],
            x1, y1, 1.0, 1.0, c[0], c[1], c[2], c[3],
            x0, y1, 0.0, 1.0, c[0], c[1], c[2], c[3],
        ];
        unsafe {
            let gl = &self.gl;
            (gl.bind_buffer)(ARRAY_BUFFER, self.vbo);
            (gl.buffer_data)(ARRAY_BUFFER, (verts.len() * 4) as isize, verts.as_ptr() as *const _, STREAM_DRAW);
            let stride = 32i32;
            let attr = |idx: u32, n: i32, off: usize| {
                (gl.enable_vertex_attrib_array)(idx);
                (gl.vertex_attrib_pointer)(idx, n, FLOAT, 0, stride, (off * 4) as *const _);
            };
            (gl.use_program)(self.image_prog);
            (gl.uniform2f)(self.image_res, w as f32, h as f32);
            (gl.uniform2f)(self.image_size, r.w, r.h);
            (gl.uniform2f)(self.image_radius, radius.min(r.w.min(r.h) * 0.5).max(0.0), 0.0);
            (gl.active_texture)(TEXTURE0);
            (gl.bind_texture)(TEXTURE_2D, tex);
            (gl.uniform1i)(self.image_tex_u, 0);
            attr(0, 2, 0);
            attr(1, 2, 2);
            attr(2, 4, 4);
            (gl.draw_arrays)(TRIANGLES, 0, 6);
        }
    }

    /// Prečíta hotový rámec (RGBA, riadky zhora nadol). Len pre snímky/testy.
    pub fn read_pixels(&self, w: u32, h: u32) -> Vec<u8> {
        let mut buf = vec![0u8; (w * h * 4) as usize];
        unsafe { (self.gl.read_pixels)(0, 0, w as i32, h as i32, RGBA, UNSIGNED_BYTE, buf.as_mut_ptr() as *mut _) };
        let row = (w * 4) as usize;
        let mut out = vec![0u8; buf.len()];
        for y in 0..h as usize {
            let src = (h as usize - 1 - y) * row;
            out[y * row..(y + 1) * row].copy_from_slice(&buf[src..src + row]);
        }
        out
    }
}
