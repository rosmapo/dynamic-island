//! Wayland vrstva: wlr-layer-shell povrch, ukazovateľ, frame callbacky, EGL okno.
use crate::gl::Egl;
use crate::sys::{self, PollFd, POLLIN};
use std::collections::HashMap;
use std::ffi::c_void;
use std::os::fd::{AsRawFd, RawFd};
use wayland_client::backend::{ObjectId, WaylandError};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_callback::{self, WlCallback},
    wl_compositor::{self, WlCompositor},
    wl_output::{self, WlOutput},
    wl_pointer::{self, WlPointer},
    wl_region::{self, WlRegion},
    wl_registry::{self, WlRegistry},
    wl_seat::{self, WlSeat},
    wl_surface::{self, WlSurface},
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::{self, ExtIdleNotifierV1},
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};

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
    outputs: HashMap<ObjectId, i32>,
    entered: Vec<ObjectId>,
    pub configured: bool,
    pub closed: bool,
    pub frame_pending: bool,
    pub scale: i32,
    pub scale_dirty: bool,
    pub ptr_events: Vec<PtrEvent>,
    /// (index pravidla [[idle]], true = nečinný / false = návrat)
    pub idle_events: Vec<(usize, bool)>,
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

pub struct Wl {
    conn: Connection,
    queue: EventQueue<WlState>,
    qh: QueueHandle<WlState>,
    pub st: WlState,
    egl_win: Option<wayland_egl::WlEglSurface>,
    _outputs: Vec<WlOutput>,
    _seat: Option<WlSeat>,
    _shell: ZwlrLayerShellV1,
    _idle: Vec<ExtIdleNotificationV1>,
    _notifier: Option<ExtIdleNotifierV1>,
}

impl Wl {
    pub fn new(width: u32, height: u32, idle_secs: &[u32]) -> Result<Wl, String> {
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

        // náhrada swayidle: ext-idle-notify-v1
        let notifier: Option<ExtIdleNotifierV1> = globals.bind(&qh, 1..=2, ()).ok();
        let mut idle = Vec::new();
        if !idle_secs.is_empty() {
            match (&notifier, &seat) {
                (Some(n), Some(s)) => {
                    for (i, secs) in idle_secs.iter().enumerate() {
                        idle.push(n.get_idle_notification(secs.saturating_mul(1000), s, &qh, i));
                    }
                }
                _ => eprintln!("dynamic-island: kompozitor nepodporuje ext-idle-notify-v1, [[idle]] sa ignoruje"),
            }
        }

        let surface = compositor.create_surface(&qh, ());
        let layer = shell.get_layer_surface(&surface, None, zwlr_layer_shell_v1::Layer::Overlay, "dynamic-island".to_string(), &qh, ());
        layer.set_size(width, height);
        layer.set_anchor(Anchor::Top);
        layer.set_exclusive_zone(0);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        surface.commit();

        let mut st = WlState {
            surface,
            compositor,
            layer,
            pointer: None,
            outputs: HashMap::new(),
            entered: Vec::new(),
            configured: false,
            closed: false,
            frame_pending: false,
            scale: 1,
            scale_dirty: false,
            ptr_events: Vec::new(),
            idle_events: Vec::new(),
        };
        queue.roundtrip(&mut st).map_err(|e| format!("roundtrip: {e}"))?;
        while !st.configured && !st.closed {
            queue.blocking_dispatch(&mut st).map_err(|e| format!("dispatch: {e}"))?;
        }
        queue.roundtrip(&mut st).map_err(|e| format!("roundtrip: {e}"))?;
        // surface.enter príde až po prvom bufferi; dovtedy odhadni mierku podľa výstupov
        if let Some(m) = st.outputs.values().max() {
            st.scale = (*m).max(1);
        }
        Ok(Wl { conn, queue, qh, st, egl_win: None, _outputs: outputs, _seat: seat, _shell: shell, _idle: idle, _notifier: notifier })
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

    pub fn request_frame(&mut self) {
        self.st.surface.frame(&self.qh, ());
        self.st.frame_pending = true;
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

impl Dispatch<ExtIdleNotifierV1, ()> for WlState {
    fn event(_: &mut Self, _: &ExtIdleNotifierV1, _: ext_idle_notifier_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ExtIdleNotificationV1, usize> for WlState {
    fn event(st: &mut Self, _: &ExtIdleNotificationV1, ev: ext_idle_notification_v1::Event, idx: &usize, _: &Connection, _: &QueueHandle<Self>) {
        match ev {
            ext_idle_notification_v1::Event::Idled => st.idle_events.push((*idx, true)),
            ext_idle_notification_v1::Event::Resumed => st.idle_events.push((*idx, false)),
            _ => {}
        }
    }
}

// zachovaj `layer` v štruktúre (zničí sa spolu s Wl)
impl Drop for Wl {
    fn drop(&mut self) {
        self.st.layer.destroy();
        self.st.surface.destroy();
    }
}
