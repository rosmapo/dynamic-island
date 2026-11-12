//! EGL + OpenGL ES 2.0 načítané za behu cez dlopen (žiadne linkovanie na -lEGL/-lGLESv2).
use crate::sys::Lib;
use std::ffi::{c_char, c_void, CString};

pub type Ptr = *mut c_void;

// ------------------------------------------------------------------ EGL
macro_rules! fn_table {
    ($sname:ident { $( $name:ident = $sym:literal : ($($at:ty),*) $(-> $r:ty)? ; )* }) => {
        #[allow(dead_code)]
        pub struct $sname { $( pub $name: unsafe extern "C" fn($($at),*) $(-> $r)? ),* }
        impl $sname {
            pub fn load(get: &dyn Fn(&str) -> *const c_void) -> Result<$sname, String> {
                Ok($sname { $( $name: {
                    let p = get($sym);
                    if p.is_null() { return Err(format!("chýba funkcia {}", $sym)); }
                    unsafe { std::mem::transmute::<*const c_void, unsafe extern "C" fn($($at),*) $(-> $r)?>(p) }
                } ),* })
            }
        }
    };
}

fn_table!(EglFns {
    get_display = "eglGetDisplay": (Ptr) -> Ptr;
    initialize = "eglInitialize": (Ptr, *mut i32, *mut i32) -> u32;
    bind_api = "eglBindAPI": (u32) -> u32;
    choose_config = "eglChooseConfig": (Ptr, *const i32, *mut Ptr, i32, *mut i32) -> u32;
    create_context = "eglCreateContext": (Ptr, Ptr, Ptr, *const i32) -> Ptr;
    create_window_surface = "eglCreateWindowSurface": (Ptr, Ptr, Ptr, *const i32) -> Ptr;
    create_pbuffer_surface = "eglCreatePbufferSurface": (Ptr, Ptr, *const i32) -> Ptr;
    make_current = "eglMakeCurrent": (Ptr, Ptr, Ptr, Ptr) -> u32;
    swap_buffers = "eglSwapBuffers": (Ptr, Ptr) -> u32;
    swap_interval = "eglSwapInterval": (Ptr, i32) -> u32;
    get_proc_address = "eglGetProcAddress": (*const c_char) -> *const c_void;
    get_error = "eglGetError": () -> i32;
    destroy_surface = "eglDestroySurface": (Ptr, Ptr) -> u32;
    destroy_context = "eglDestroyContext": (Ptr, Ptr) -> u32;
    terminate = "eglTerminate": (Ptr) -> u32;
});

const EGL_SURFACE_TYPE: i32 = 0x3033;
const EGL_WINDOW_BIT: i32 = 0x4;
const EGL_PBUFFER_BIT: i32 = 0x1;
const EGL_RENDERABLE_TYPE: i32 = 0x3040;
const EGL_OPENGL_ES2_BIT: i32 = 0x4;
const EGL_RED: i32 = 0x3024;
const EGL_GREEN: i32 = 0x3023;
const EGL_BLUE: i32 = 0x3022;
const EGL_ALPHA: i32 = 0x3021;
const EGL_NONE: i32 = 0x3038;
const EGL_CONTEXT_CLIENT_VERSION: i32 = 0x3098;
const EGL_OPENGL_ES_API: u32 = 0x30A0;
const EGL_WIDTH: i32 = 0x3057;
const EGL_HEIGHT: i32 = 0x3056;

pub struct Egl {
    pub fns: EglFns,
    pub dpy: Ptr,
    pub ctx: Ptr,
    pub surf: Ptr,
    cfg: Ptr,
    _lib: Lib,
    gles: Option<Lib>,
}

impl Egl {
    fn open() -> Result<(Lib, EglFns), String> {
        let lib = Lib::open(&["libEGL.so.1", "libEGL.so"]).ok_or("nenašiel som libEGL.so.1 (nainštaluj libegl1)")?;
        let fns = EglFns::load(&|n| lib.sym(n))?;
        Ok((lib, fns))
    }

    unsafe fn init(fns: &EglFns, dpy: Ptr, window: bool) -> Result<(Ptr, Ptr), String> {
        if dpy.is_null() {
            return Err("eglGetDisplay zlyhalo".into());
        }
        let (mut maj, mut min) = (0, 0);
        if (fns.initialize)(dpy, &mut maj, &mut min) == 0 {
            return Err(format!("eglInitialize zlyhalo ({:#x})", (fns.get_error)()));
        }
        (fns.bind_api)(EGL_OPENGL_ES_API);
        let attribs = [
            EGL_SURFACE_TYPE,
            if window { EGL_WINDOW_BIT } else { EGL_PBUFFER_BIT },
            EGL_RENDERABLE_TYPE,
            EGL_OPENGL_ES2_BIT,
            EGL_RED,
            8,
            EGL_GREEN,
            8,
            EGL_BLUE,
            8,
            EGL_ALPHA,
            8,
            EGL_NONE,
        ];
        let mut cfg: Ptr = std::ptr::null_mut();
        let mut n = 0;
        if (fns.choose_config)(dpy, attribs.as_ptr(), &mut cfg, 1, &mut n) == 0 || n < 1 {
            return Err("EGL: nenašla sa konfigurácia s alfa kanálom".into());
        }
        let ctx_attr = [EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE];
        let ctx = (fns.create_context)(dpy, cfg, std::ptr::null_mut(), ctx_attr.as_ptr());
        if ctx.is_null() {
            return Err(format!("eglCreateContext zlyhalo ({:#x})", (fns.get_error)()));
        }
        Ok((cfg, ctx))
    }

    /// Okno z `wl_egl_window` (Wayland).
    pub fn for_wayland(wl_display: Ptr, wl_egl_window: Ptr) -> Result<Egl, String> {
        let (lib, fns) = Self::open()?;
        unsafe {
            let dpy = (fns.get_display)(wl_display);
            let (cfg, ctx) = Self::init(&fns, dpy, true)?;
            let surf = (fns.create_window_surface)(dpy, cfg, wl_egl_window, std::ptr::null());
            if surf.is_null() {
                return Err(format!("eglCreateWindowSurface zlyhalo ({:#x})", (fns.get_error)()));
            }
            if (fns.make_current)(dpy, surf, surf, ctx) == 0 {
                return Err("eglMakeCurrent zlyhalo".into());
            }
            // plynulosť riadi frame callback kompozitora, nie swap interval
            (fns.swap_interval)(dpy, 0);
            Ok(Egl { fns, dpy, ctx, surf, cfg, _lib: lib, gles: Lib::open(&["libGLESv2.so.2", "libGLESv2.so"]) })
        }
    }

    /// Pbuffer bez okna (snímky, testy) – Mesa surfaceless platforma.
    pub fn headless(w: i32, h: i32) -> Result<Egl, String> {
        let (lib, fns) = Self::open()?;
        unsafe {
            // eglGetPlatformDisplay(EGL_PLATFORM_SURFACELESS_MESA) ak je dostupné
            let getp = (fns.get_proc_address)(CString::new("eglGetPlatformDisplay").unwrap().as_ptr());
            let dpy = if !getp.is_null() {
                let f: unsafe extern "C" fn(u32, Ptr, *const isize) -> Ptr = std::mem::transmute(getp);
                f(0x31DD, std::ptr::null_mut(), std::ptr::null())
            } else {
                (fns.get_display)(std::ptr::null_mut())
            };
            let (cfg, ctx) = Self::init(&fns, dpy, false)?;
            let attr = [EGL_WIDTH, w, EGL_HEIGHT, h, EGL_NONE];
            let surf = (fns.create_pbuffer_surface)(dpy, cfg, attr.as_ptr());
            if surf.is_null() {
                return Err(format!("eglCreatePbufferSurface zlyhalo ({:#x})", (fns.get_error)()));
            }
            if (fns.make_current)(dpy, surf, surf, ctx) == 0 {
                return Err("eglMakeCurrent zlyhalo".into());
            }
            Ok(Egl { fns, dpy, ctx, surf, cfg, _lib: lib, gles: Lib::open(&["libGLESv2.so.2", "libGLESv2.so"]) })
        }
    }

    /// Urobí kontext tohto okna aktuálnym vo vlákne (lišta a okno v strede majú každé vlastný kontext).
    pub fn make_current(&self) -> bool {
        unsafe { (self.fns.make_current)(self.dpy, self.surf, self.surf, self.ctx) != 0 }
    }

    pub fn swap(&self) -> bool {
        unsafe { (self.fns.swap_buffers)(self.dpy, self.surf) != 0 }
    }

    pub fn get_proc(&self, name: &str) -> *const c_void {
        if let Some(l) = &self.gles {
            let p = l.sym(name);
            if !p.is_null() {
                return p;
            }
        }
        match CString::new(name) {
            Ok(c) => unsafe { (self.fns.get_proc_address)(c.as_ptr()) },
            Err(_) => std::ptr::null(),
        }
    }

    #[allow(dead_code)]
    pub fn config(&self) -> Ptr {
        self.cfg
    }
}

/// Okno v strede vzniká a zaniká za behu, preto sa EGL povrch, kontext aj displej uvoľnia (inak by unikali
/// a displej by ostal viazaný na už zatvorené Wayland spojenie).
impl Drop for Egl {
    fn drop(&mut self) {
        unsafe {
            (self.fns.make_current)(self.dpy, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
            if !self.surf.is_null() {
                (self.fns.destroy_surface)(self.dpy, self.surf);
            }
            (self.fns.destroy_context)(self.dpy, self.ctx);
            (self.fns.terminate)(self.dpy);
        }
    }
}

// ------------------------------------------------------------------- GL
pub const COLOR_BUFFER_BIT: u32 = 0x4000;
pub const BLEND: u32 = 0x0BE2;
pub const SCISSOR_TEST: u32 = 0x0C11;
pub const ONE: u32 = 1;
pub const ONE_MINUS_SRC_ALPHA: u32 = 0x0303;
pub const TRIANGLES: u32 = 4;
pub const ARRAY_BUFFER: u32 = 0x8892;
pub const STREAM_DRAW: u32 = 0x88E0;
pub const FLOAT: u32 = 0x1406;
pub const TEXTURE_2D: u32 = 0x0DE1;
pub const TEXTURE0: u32 = 0x84C0;
pub const ALPHA: u32 = 0x1906;
pub const RGBA: u32 = 0x1908;
pub const UNSIGNED_BYTE: u32 = 0x1401;
pub const TEXTURE_MIN_FILTER: u32 = 0x2801;
pub const TEXTURE_MAG_FILTER: u32 = 0x2800;
pub const TEXTURE_WRAP_S: u32 = 0x2802;
pub const TEXTURE_WRAP_T: u32 = 0x2803;
pub const LINEAR: i32 = 0x2601;
pub const CLAMP_TO_EDGE: i32 = 0x812F;
pub const VERTEX_SHADER: u32 = 0x8B31;
pub const FRAGMENT_SHADER: u32 = 0x8B30;
pub const COMPILE_STATUS: u32 = 0x8B81;
pub const LINK_STATUS: u32 = 0x8B82;
pub const INFO_LOG_LENGTH: u32 = 0x8B84;
pub const UNPACK_ALIGNMENT: u32 = 0x0CF5;

fn_table!(Gl {
    clear_color = "glClearColor": (f32, f32, f32, f32);
    clear = "glClear": (u32);
    viewport = "glViewport": (i32, i32, i32, i32);
    scissor = "glScissor": (i32, i32, i32, i32);
    enable = "glEnable": (u32);
    disable = "glDisable": (u32);
    blend_func = "glBlendFunc": (u32, u32);
    create_shader = "glCreateShader": (u32) -> u32;
    shader_source = "glShaderSource": (u32, i32, *const *const c_char, *const i32);
    compile_shader = "glCompileShader": (u32);
    get_shader_iv = "glGetShaderiv": (u32, u32, *mut i32);
    get_shader_info_log = "glGetShaderInfoLog": (u32, i32, *mut i32, *mut c_char);
    delete_shader = "glDeleteShader": (u32);
    create_program = "glCreateProgram": () -> u32;
    attach_shader = "glAttachShader": (u32, u32);
    bind_attrib_location = "glBindAttribLocation": (u32, u32, *const c_char);
    link_program = "glLinkProgram": (u32);
    get_program_iv = "glGetProgramiv": (u32, u32, *mut i32);
    get_program_info_log = "glGetProgramInfoLog": (u32, i32, *mut i32, *mut c_char);
    use_program = "glUseProgram": (u32);
    get_uniform_location = "glGetUniformLocation": (u32, *const c_char) -> i32;
    uniform2f = "glUniform2f": (i32, f32, f32);
    uniform1i = "glUniform1i": (i32, i32);
    enable_vertex_attrib_array = "glEnableVertexAttribArray": (u32);
    vertex_attrib_pointer = "glVertexAttribPointer": (u32, i32, u32, u8, i32, *const c_void);
    draw_arrays = "glDrawArrays": (u32, i32, i32);
    gen_buffers = "glGenBuffers": (i32, *mut u32);
    bind_buffer = "glBindBuffer": (u32, u32);
    buffer_data = "glBufferData": (u32, isize, *const c_void, u32);
    gen_textures = "glGenTextures": (i32, *mut u32);
    bind_texture = "glBindTexture": (u32, u32);
    tex_image_2d = "glTexImage2D": (u32, i32, i32, i32, i32, i32, u32, u32, *const c_void);
    tex_parameteri = "glTexParameteri": (u32, u32, i32);
    active_texture = "glActiveTexture": (u32);
    pixel_storei = "glPixelStorei": (u32, i32);
    read_pixels = "glReadPixels": (i32, i32, i32, i32, u32, u32, *mut c_void);
});
