//! Minimálna podpora klávesnice: libxkbcommon načítaná za behu (dlopen), ako EGL.
//! Premieňa kódy kláves z `wl_keyboard` na symboly a UTF-8 text podľa rozloženia kompozitora.
use crate::sys::Lib;
use std::ffi::{c_char, c_int, c_void, CString};

type Ptr = *mut c_void;

struct Fns {
    context_new: unsafe extern "C" fn(c_int) -> Ptr,
    context_unref: unsafe extern "C" fn(Ptr),
    keymap_new_from_string: unsafe extern "C" fn(Ptr, *const c_char, c_int, c_int) -> Ptr,
    keymap_unref: unsafe extern "C" fn(Ptr),
    state_new: unsafe extern "C" fn(Ptr) -> Ptr,
    state_unref: unsafe extern "C" fn(Ptr),
    state_update_mask: unsafe extern "C" fn(Ptr, u32, u32, u32, u32, u32, u32) -> c_int,
    state_key_get_one_sym: unsafe extern "C" fn(Ptr, u32) -> u32,
    state_key_get_utf8: unsafe extern "C" fn(Ptr, u32, *mut c_char, usize) -> c_int,
    state_mod_name_is_active: unsafe extern "C" fn(Ptr, *const c_char, c_int) -> c_int,
}

fn load(lib: &Lib) -> Option<Fns> {
    macro_rules! f {
        ($n:literal) => {{
            let p = lib.sym($n);
            if p.is_null() {
                return None;
            }
            unsafe { std::mem::transmute(p) }
        }};
    }
    Some(Fns {
        context_new: f!("xkb_context_new"),
        context_unref: f!("xkb_context_unref"),
        keymap_new_from_string: f!("xkb_keymap_new_from_string"),
        keymap_unref: f!("xkb_keymap_unref"),
        state_new: f!("xkb_state_new"),
        state_unref: f!("xkb_state_unref"),
        state_update_mask: f!("xkb_state_update_mask"),
        state_key_get_one_sym: f!("xkb_state_key_get_one_sym"),
        state_key_get_utf8: f!("xkb_state_key_get_utf8"),
        state_mod_name_is_active: f!("xkb_state_mod_name_is_active"),
    })
}

const XKB_KEYMAP_FORMAT_TEXT_V1: c_int = 1;
const XKB_STATE_MODS_EFFECTIVE: c_int = 8;

pub struct Xkb {
    f: Fns,
    ctx: Ptr,
    keymap: Ptr,
    state: Ptr,
    _lib: Lib,
}

impl Xkb {
    /// `keymap` = text mapy kláves (XKB_V1) bez koncového NUL.
    pub fn new(keymap: &[u8]) -> Option<Xkb> {
        let lib = Lib::open(&["libxkbcommon.so.0", "libxkbcommon.so"])?;
        let f = load(&lib)?;
        let text = CString::new(keymap).ok()?;
        unsafe {
            let ctx = (f.context_new)(0);
            if ctx.is_null() {
                return None;
            }
            let km = (f.keymap_new_from_string)(ctx, text.as_ptr(), XKB_KEYMAP_FORMAT_TEXT_V1, 0);
            if km.is_null() {
                (f.context_unref)(ctx);
                return None;
            }
            let st = (f.state_new)(km);
            if st.is_null() {
                (f.keymap_unref)(km);
                (f.context_unref)(ctx);
                return None;
            }
            Some(Xkb { f, ctx, keymap: km, state: st, _lib: lib })
        }
    }

    pub fn update(&mut self, depressed: u32, latched: u32, locked: u32, group: u32) {
        unsafe {
            (self.f.state_update_mask)(self.state, depressed, latched, locked, 0, 0, group);
        }
    }

    /// `key` = evdev kód z `wl_keyboard.key` (pripočíta sa 8 ako vyžaduje xkb).
    pub fn sym(&self, key: u32) -> u32 {
        unsafe { (self.f.state_key_get_one_sym)(self.state, key + 8) }
    }

    pub fn utf8(&self, key: u32) -> String {
        let mut buf = [0 as c_char; 16];
        let n = unsafe { (self.f.state_key_get_utf8)(self.state, key + 8, buf.as_mut_ptr(), buf.len()) };
        if n <= 0 {
            return String::new();
        }
        let bytes: Vec<u8> = buf[..(n as usize).min(15)].iter().map(|c| *c as u8).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub fn ctrl(&self) -> bool {
        let name = CString::new("Control").unwrap_or_default();
        unsafe { (self.f.state_mod_name_is_active)(self.state, name.as_ptr(), XKB_STATE_MODS_EFFECTIVE) > 0 }
    }
}

impl Drop for Xkb {
    fn drop(&mut self) {
        unsafe {
            (self.f.state_unref)(self.state);
            (self.f.keymap_unref)(self.keymap);
            (self.f.context_unref)(self.ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ak je na stroji libxkbcommon a `xkbcli`/základné pravidlá, over, že mapa kláves funguje.
    #[test]
    fn loads_and_translates() {
        if Lib::open(&["libxkbcommon.so.0"]).is_none() {
            return;
        }
        // minimálna mapa: kláves <AC01> (evdev 30 = 'a') → symbol a
        let km = b"xkb_keymap {\n xkb_keycodes \"k\" { minimum = 8; maximum = 255; <AC01> = 38; };\n xkb_types \"t\" { type \"ONE_LEVEL\" { modifiers = none; map[none] = Level1; level_name[Level1] = \"Any\"; }; };\n xkb_compatibility \"c\" { };\n xkb_symbols \"s\" { key <AC01> { type = \"ONE_LEVEL\", symbols[Group1] = [ a ] }; };\n};\n";
        let x = Xkb::new(km).expect("keymap");
        assert_eq!(x.sym(30), 0x61);
        assert_eq!(x.utf8(30), "a");
        assert!(!x.ctrl());
    }
}
