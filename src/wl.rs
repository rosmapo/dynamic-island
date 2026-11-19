//! Wayland vrstva: wlr-layer-shell povrch, ukazovateľ, frame callbacky, EGL okno.
use crate::gl::Egl;
use crate::sys::{self, PollFd, POLLIN};
use crate::wfshell::proto::{
    zwf_hotspot_v2::{self, ZwfHotspotV2},
    zwf_output_v2::{self, ZwfOutputV2},
    zwf_shell_manager_v2::{self, ZwfShellManagerV2},
};
use crate::xkb::Xkb;
use std::collections::HashMap;
use std::ffi::c_void;
use std::os::fd::{AsRawFd, RawFd};
use wayland_client::backend::{ObjectId, WaylandError};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_callback::{self, WlCallback},
    wl_compositor::{self, WlCompositor},
    wl_keyboard::{self, WlKeyboard},
    wl_output::{self, WlOutput},
    wl_pointer::{self, WlPointer},
    wl_region::{self, WlRegion},
    wl_registry::{self, WlRegistry},
    wl_seat::{self, WlSeat},
    wl_surface::{self, WlSurface},
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::xdg_output::zv1::client::{
    zxdg_output_manager_v1::{self, ZxdgOutputManagerV1},
    zxdg_output_v1::{self, ZxdgOutputV1},
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};

/// Stlačená klávesa (po preklade cez xkb).
#[derive(Debug, Clone)]
pub struct KeyEvent {
    pub sym: u32,
    pub text: String,
    pub ctrl: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum PtrEvent {
    Enter(f64, f64),
    Leave,
    Motion(f64, f64),
    Button(u32, bool),
    Scroll(f64),
}

pub struct WlState {
    surface: WlSurface,
    compositor: WlCompositor,
    layer: ZwlrLayerSurfaceV1,
    pointer: Option<WlPointer>,
    keyboard: Option<WlKeyboard>,
    xkb: Option<Xkb>,
    pub key_events: Vec<KeyEvent>,
    outputs: HashMap<ObjectId, i32>,
    entered: Vec<ObjectId>,
    pub configured: bool,
    pub closed: bool,
    pub frame_pending: bool,
    pub scale: i32,
    pub scale_dirty: bool,
    pub ptr_events: Vec<PtrEvent>,
    /// logická veľkosť výstupov (xdg-output)
    logical: HashMap<ObjectId, (i32, i32)>,
    /// vstup (`true`) a odchod (`false`) kurzora z hotspotu na hornom okraji
    pub hot_events: Vec<bool>,
    hot_gen: u32,
}

impl WlState {
    fn recalc_scale(&mut self) {
        let new = self.entered.iter().filter_map(|o| self.outputs.get(o)).copied().max().unwrap_or(self.scale).max(1);
        if new != self.scale {
            self.scale = new;
            self.scale_dirty = true;
        }
    }
}

/// Hotspot na hornom okraji výstupu. Protokol nemá žiadosť na zrušenie, preto sa nový vytvára len pri zmene geometrie.
struct Hot {
    key: (ObjectId, i32, i32, i32),
    _out: ZwfOutputV2,
    _hs: ZwfHotspotV2,
}

pub struct Wl {
    conn: Connection,
    queue: EventQueue<WlState>,
    qh: QueueHandle<WlState>,
    pub st: WlState,
    egl_win: Option<wayland_egl::WlEglSurface>,
    _outputs: Vec<WlOutput>,
    _seat: Option<WlSeat>,
    _shell: ZwlrLayerShellV1,
    wf_mgr: Option<ZwfShellManagerV2>,
    _xdg_outs: Vec<ZxdgOutputV1>,
    hot: Option<Hot>,
}

impl Wl {
    pub fn new(width: u32, height: u32) -> Result<Wl, String> {
        Wl::new_with(width, height, false, "dynamic-island")
    }

    /// `centered`: povrch bez kotvy (kompozitor ho vycentruje) s výhradnou klávesnicou.
    pub fn new_with(width: u32, height: u32, centered: bool, namespace: &str) -> Result<Wl, String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("nepodarilo sa pripojiť k Waylandu: {e}"))?;
        let (globals, mut queue) = registry_queue_init::<WlState>(&conn).map_err(|e| format!("wl_registry: {e}"))?;
        let qh = queue.handle();
        let compositor: WlCompositor = globals.bind(&qh, 3..=6, ()).map_err(|e| format!("wl_compositor: {e}"))?;
        let shell: ZwlrLayerShellV1 = globals
            .bind(&qh, 1..=4, ())
            .map_err(|_| "kompozitor nepodporuje zwlr_layer_shell_v1".to_string())?;
        let seat: Option<WlSeat> = globals.bind(&qh, 1..=7, ()).ok();

        let mut want = Vec::new();
        globals.contents().with_list(|list| {
            for g in list {
                if g.interface == "wl_output" {
                    want.push((g.name, g.version));
                }
            }
        });
        let outputs: Vec<WlOutput> =
            want.into_iter().map(|(name, ver)| globals.registry().bind::<WlOutput, _, _>(name, ver.min(3), &qh, ())).collect();

        // hotspot (Wayfire 0.11+): len pre lištu; bez protokolu autohide ostáva na pruhu pri hornom okraji
        let wf_mgr: Option<ZwfShellManagerV2> = if centered { None } else { globals.bind(&qh, 3..=3, ()).ok() };
        let xdg_mgr: Option<ZxdgOutputManagerV1> = if wf_mgr.is_some() { globals.bind(&qh, 1..=3, ()).ok() } else { None };
        let xdg_outs: Vec<ZxdgOutputV1> = match &xdg_mgr {
            Some(m) => outputs.iter().map(|o| m.get_xdg_output(o, &qh, o.id())).collect(),
            None => Vec::new(),
        };
        let wf_mgr = if xdg_mgr.is_some() { wf_mgr } else { None };

        let surface = compositor.create_surface(&qh, ());
        let layer = shell.get_layer_surface(&surface, None, zwlr_layer_shell_v1::Layer::Overlay, namespace.to_string(), &qh, ());
        layer.set_size(width, height);
        layer.set_anchor(if centered { Anchor::empty() } else { Anchor::Top });
        layer.set_exclusive_zone(if centered { -1 } else { 0 });
        layer.set_keyboard_interactivity(if centered { KeyboardInteractivity::Exclusive } else { KeyboardInteractivity::None });
        surface.commit();

        let mut st = WlState {
            surface,
            compositor,
            layer,
            pointer: None,
            keyboard: None,
            xkb: None,
            key_events: Vec::new(),
            outputs: HashMap::new(),
            entered: Vec::new(),
            configured: false,
            closed: false,
            frame_pending: false,
            scale: 1,
            scale_dirty: false,
            ptr_events: Vec::new(),
            logical: HashMap::new(),
            hot_events: Vec::new(),
            hot_gen: 0,
        };
        queue.roundtrip(&mut st).map_err(|e| format!("roundtrip: {e}"))?;
        while !st.configured && !st.closed {
            queue.blocking_dispatch(&mut st).map_err(|e| format!("dispatch: {e}"))?;
        }
        queue.roundtrip(&mut st).map_err(|e| format!("roundtrip: {e}"))?;
        if st.closed {
            return Err("kompozitor zatvoril povrch".to_string());
        }
        // surface.enter príde až po prvom bufferi; dovtedy odhadni mierku podľa výstupov
        if let Some(m) = st.outputs.values().max() {
            st.scale = (*m).max(1);
        }
        Ok(Wl { conn, queue, qh, st, egl_win: None, _outputs: outputs, _seat: seat, _shell: shell, wf_mgr, _xdg_outs: xdg_outs, hot: None })
    }

    pub fn create_egl(&mut self, pw: i32, ph: i32) -> Result<Egl, String> {
        let win = wayland_egl::WlEglSurface::new(self.st.surface.id(), pw, ph).map_err(|e| format!("wl_egl_window: {e}"))?;
        let dpy = self.conn.backend().display_ptr() as *mut c_void;
        let egl = Egl::for_wayland(dpy, win.ptr() as *mut c_void)?;
        self.egl_win = Some(win);
        self.st.surface.set_buffer_scale(self.st.scale);
        Ok(egl)
    }

    pub fn apply_scale(&mut self, pw: i32, ph: i32) {
        self.st.surface.set_buffer_scale(self.st.scale);
        if let Some(w) = &self.egl_win {
            w.resize(pw, ph, 0, 0);
        }
        self.st.scale_dirty = false;
    }

    /// Odošle čakajúce požiadavky (okno v strede sa inak vyprázdni až pri ďalšom `wait_events`, ktorý by mohol prísť neskoro).
    pub fn flush(&self) {
        let _ = self.conn.flush();
    }

    /// Deskriptor Wayland spojenia (na `poll` v hlavnej slučke, keď má proces viac spojení).
    pub fn fd(&self) -> RawFd {
        self.conn.backend().poll_fd().as_raw_fd()
    }

    /// Výhradné zachytávanie klávesnice len počas písania (heslo); inak ostrovček klávesnicu nechá.
    pub fn set_keyboard(&mut self, on: bool) {
        self.st.layer.set_keyboard_interactivity(if on { KeyboardInteractivity::Exclusive } else { KeyboardInteractivity::None });
        self.st.surface.commit();
    }

    /// Máme mapu kláves? Bez nej by zachytená klávesnica nebola použiteľná, preto ju vtedy nikdy nezachytávame.
    pub fn keyboard_ready(&self) -> bool {
        self.st.xkb.is_some()
    }

    pub fn request_frame(&mut self) {
        self.st.surface.frame(&self.qh, ());
        self.st.frame_pending = true;
    }

    /// Je aktívny hotspot na hornom okraji (kompozitor ho podporuje a je vytvorený)?
    pub fn has_hotspot(&self) -> bool {
        self.hot.is_some()
    }

    /// Vytvorí (alebo pri zmene geometrie obnoví) hotspot: obdĺžnik pri hornom okraji výstupu, vodorovne stredných
    /// `width_pct` % šírky, hrubý `edge_px`. Volá sa opakovane; bez zmeny nerobí nič.
    pub fn update_hotspot(&mut self, width_pct: f64, edge_px: f64) {
        let Some(mgr) = &self.wf_mgr else { return };
        let Some(oid) = self.st.entered.first().cloned() else { return };
        let Some(&(w, _)) = self.st.logical.get(&oid) else { return };
        let Some(out) = self._outputs.iter().find(|o| o.id() == oid) else { return };
        let hw = (((w as f64) * width_pct.clamp(0.0, 100.0) / 100.0).round() as i32).max(1);
        let x = ((w - hw) / 2).max(0);
        let thr = edge_px.max(1.0).ceil() as i32;
        let key = (oid, x, hw, thr);
        if self.hot.as_ref().is_some_and(|h| h.key == key) {
            return;
        }
        if self.hot.is_some() {
            self.st.hot_events.push(false); // starý hotspot sa už nesleduje
        }
        self.st.hot_gen += 1;
        let wo = mgr.get_wf_output(out, &self.qh, ());
        let hs = wo.create_custom_hotspot(x as u32, 0, hw as u32, 1, thr as u32, 0, &self.qh, self.st.hot_gen);
        self.hot = Some(Hot { key, _out: wo, _hs: hs });
    }

    /// Vstupná oblasť (logické súradnice); `None` = povrch je pre myš priehľadný.
    pub fn set_input_region(&self, r: Option<(i32, i32, i32, i32)>) {
        let region = self.st.compositor.create_region(&self.qh, ());
        if let Some((x, y, w, h)) = r {
            region.add(x, y, w, h);
        }
        self.st.surface.set_input_region(Some(&region));
        region.destroy();
    }

    /// Spracuje Wayland udalosti a čaká na `extra` deskriptory; vráti ich čitateľnosť.
    pub fn wait_events(&mut self, extra: &[RawFd], timeout_ms: i32) -> Result<Vec<bool>, String> {
        self.queue.dispatch_pending(&mut self.st).map_err(|e| format!("dispatch: {e}"))?;
        if let Err(e) = self.queue.flush() {
            if !matches!(&e, WaylandError::Io(io) if io.kind() == std::io::ErrorKind::WouldBlock) {
                return Err(format!("flush: {e}"));
            }
        }
        let guard = loop {
            match self.queue.prepare_read() {
                Some(g) => break g,
                None => {
                    self.queue.dispatch_pending(&mut self.st).map_err(|e| format!("dispatch: {e}"))?;
                }
            }
        };
        let mut fds = vec![PollFd { fd: guard.connection_fd().as_raw_fd(), events: POLLIN, revents: 0 }];
        for fd in extra {
            fds.push(PollFd { fd: *fd, events: POLLIN, revents: 0 });
        }
        sys::poll_in(&mut fds, timeout_ms);
        if fds[0].revents & POLLIN != 0 {
            if let Err(e) = guard.read() {
                if !matches!(&e, WaylandError::Io(io) if io.kind() == std::io::ErrorKind::WouldBlock) {
                    return Err(format!("spojenie s kompozitorom padlo: {e}"));
                }
            }
        } else {
            drop(guard);
        }
        self.queue.dispatch_pending(&mut self.st).map_err(|e| format!("dispatch: {e}"))?;
        Ok(fds[1..].iter().map(|f| f.revents & POLLIN != 0).collect())
    }
}

// ------------------------------------------------------------ Dispatch

impl Dispatch<WlRegistry, GlobalListContents> for WlState {
    fn event(_: &mut Self, _: &WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ZwfShellManagerV2, ()> for WlState {
    fn event(_: &mut Self, _: &ZwfShellManagerV2, _: zwf_shell_manager_v2::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ZwfOutputV2, ()> for WlState {
    fn event(_: &mut Self, _: &ZwfOutputV2, _: zwf_output_v2::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ZwfHotspotV2, u32> for WlState {
    fn event(st: &mut Self, _: &ZwfHotspotV2, ev: zwf_hotspot_v2::Event, generation: &u32, _: &Connection, _: &QueueHandle<Self>) {
        if *generation != st.hot_gen {
            return; // udalosť zo zastaraného hotspotu
        }
        match ev {
            zwf_hotspot_v2::Event::Enter => st.hot_events.push(true),
            zwf_hotspot_v2::Event::Leave => st.hot_events.push(false),
            _ => {}
        }
    }
}
impl Dispatch<ZxdgOutputManagerV1, ()> for WlState {
    fn event(_: &mut Self, _: &ZxdgOutputManagerV1, _: zxdg_output_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ZxdgOutputV1, ObjectId> for WlState {
    fn event(st: &mut Self, _: &ZxdgOutputV1, ev: zxdg_output_v1::Event, id: &ObjectId, _: &Connection, _: &QueueHandle<Self>) {
        if let zxdg_output_v1::Event::LogicalSize { width, height } = ev {
            st.logical.insert(id.clone(), (width, height));
        }
    }
}
impl Dispatch<WlCompositor, ()> for WlState {
    fn event(_: &mut Self, _: &WlCompositor, _: wl_compositor::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlRegion, ()> for WlState {
    fn event(_: &mut Self, _: &WlRegion, _: wl_region::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ZwlrLayerShellV1, ()> for WlState {
    fn event(_: &mut Self, _: &ZwlrLayerShellV1, _: zwlr_layer_shell_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlCallback, ()> for WlState {
    fn event(st: &mut Self, _: &WlCallback, ev: wl_callback::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_callback::Event::Done { .. } = ev {
            st.frame_pending = false;
        }
    }
}

impl Dispatch<WlSurface, ()> for WlState {
    fn event(st: &mut Self, _: &WlSurface, ev: wl_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match ev {
            wl_surface::Event::Enter { output } => {
                st.entered.push(output.id());
                st.recalc_scale();
            }
            wl_surface::Event::Leave { output } => {
                let id = output.id();
                st.entered.retain(|o| *o != id);
                st.recalc_scale();
            }
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, ()> for WlState {
    fn event(st: &mut Self, out: &WlOutput, ev: wl_output::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_output::Event::Scale { factor } = ev {
            st.outputs.insert(out.id(), factor);
            st.recalc_scale();
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for WlState {
    fn event(st: &mut Self, layer: &ZwlrLayerSurfaceV1, ev: zwlr_layer_surface_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match ev {
            zwlr_layer_surface_v1::Event::Configure { serial, .. } => {
                layer.ack_configure(serial);
                st.configured = true;
            }
            zwlr_layer_surface_v1::Event::Closed => st.closed = true,
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for WlState {
    fn event(st: &mut Self, seat: &WlSeat, ev: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = ev {
            if caps.contains(wl_seat::Capability::Pointer) {
                if st.pointer.is_none() {
                    st.pointer = Some(seat.get_pointer(qh, ()));
                }
            } else if let Some(p) = st.pointer.take() {
                p.release();
            }
            if caps.contains(wl_seat::Capability::Keyboard) {
                if st.keyboard.is_none() {
                    st.keyboard = Some(seat.get_keyboard(qh, ()));
                }
            } else if let Some(k) = st.keyboard.take() {
                k.release();
                st.xkb = None;
            }
        }
    }
}

impl Dispatch<WlPointer, ()> for WlState {
    fn event(st: &mut Self, _: &WlPointer, ev: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match ev {
            wl_pointer::Event::Enter { surface_x, surface_y, .. } => st.ptr_events.push(PtrEvent::Enter(surface_x, surface_y)),
            wl_pointer::Event::Leave { .. } => st.ptr_events.push(PtrEvent::Leave),
            wl_pointer::Event::Motion { surface_x, surface_y, .. } => st.ptr_events.push(PtrEvent::Motion(surface_x, surface_y)),
            wl_pointer::Event::Button { button, state, .. } => {
                st.ptr_events.push(PtrEvent::Button(button, matches!(state, WEnum::Value(wl_pointer::ButtonState::Pressed))))
            }
            wl_pointer::Event::Axis { axis, value, .. } => {
                if matches!(axis, WEnum::Value(wl_pointer::Axis::VerticalScroll)) {
                    st.ptr_events.push(PtrEvent::Scroll(value));
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlKeyboard, ()> for WlState {
    fn event(st: &mut Self, _: &WlKeyboard, ev: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match ev {
            wl_keyboard::Event::Keymap { format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1), fd, size } => {
                // čítame od pozície 0 bez ohľadu na offset: deskriptor zdieľa offset s kompozitorom,
                // ktorý do súboru práve zapisoval (obyčajné read() by vrátilo EOF)
                use std::os::unix::fs::FileExt;
                let f = std::fs::File::from(fd);
                let mut buf = vec![0u8; size as usize];
                if f.read_exact_at(&mut buf, 0).is_ok() {
                    if let Some(end) = buf.iter().position(|b| *b == 0) {
                        buf.truncate(end);
                    }
                    st.xkb = Xkb::new(&buf);
                    if st.xkb.is_none() {
                        eprintln!("dynamic-island: mapu kláves sa nepodarilo načítať (libxkbcommon?)");
                    }
                } else {
                    eprintln!("dynamic-island: mapu kláves sa nepodarilo prečítať");
                }
            }
            wl_keyboard::Event::Key { key, state: WEnum::Value(wl_keyboard::KeyState::Pressed), .. } => {
                if let Some(x) = &st.xkb {
                    st.key_events.push(KeyEvent { sym: x.sym(key), text: x.utf8(key), ctrl: x.ctrl() });
                }
            }
            wl_keyboard::Event::Modifiers { mods_depressed, mods_latched, mods_locked, group, .. } => {
                if let Some(x) = &mut st.xkb {
                    x.update(mods_depressed, mods_latched, mods_locked, group);
                }
            }
            _ => {}
        }
    }
}


// zachovaj `layer` v štruktúre (zničí sa spolu s Wl)
impl Drop for Wl {
    fn drop(&mut self) {
        // najprv wl_egl_window, potom povrch a vrstva; požiadavky sa ešte vyprázdnia, aby okno hneď zmizlo
        self.egl_win = None;
        self.st.layer.destroy();
        self.st.surface.destroy();
        let _ = self.conn.flush();
    }
}
