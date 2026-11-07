//! Server org.freedesktop.Notifications (nahrádza mako) nad vlastným D-Bus klientom.
use crate::data::{Msg, Notification};
use crate::dbus::*;
use crate::sys::{self, PollFd, Tx, Waker, POLLIN};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

/// Príkazy z hlavnej slučky do D-Bus vlákna.
#[derive(Debug)]
pub enum NotifCmd {
    /// (id, dôvod: 1 vypršalo, 2 zavreté používateľom, 3 CloseNotification)
    Closed(u32, u32),
    Action(u32, String),
}

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

pub fn spawn(tx: Tx<Msg>, rx: Receiver<NotifCmd>, waker: Arc<Waker>) {
    std::thread::spawn(move || {
        crate::nicons::cleanup(90);
        let mut next_id = 1u32;
        loop {
            if let Err(e) = session(&tx, &rx, &waker, &mut next_id) {
                eprintln!("dynamic-island: notifikácie nebežia: {e} (opakujem o 5 s)");
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    });
}

fn session(tx: &Tx<Msg>, rx: &Receiver<NotifCmd>, waker: &Waker, next_id: &mut u32) -> std::io::Result<()> {
    let mut c = Conn::connect()?;
    let mut b = W::default();
    b.string(NAME);
    b.u32(4); // DBUS_NAME_FLAG_DO_NOT_QUEUE
    let r = c.call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "RequestName", "su", b.b)?;
    let code = Reader::new(&r.body, 0).u32().unwrap_or(0);
    if code != 1 {
        return Err(std::io::Error::other("meno org.freedesktop.Notifications je obsadené (beží mako/dunst?)"));
    }
    loop {
        let mut fds = [PollFd { fd: c.fd(), events: POLLIN, revents: 0 }, PollFd { fd: waker.fd(), events: POLLIN, revents: 0 }];
        sys::poll_in(&mut fds, -1);
        if fds[1].revents & POLLIN != 0 {
            waker.drain();
            while let Ok(cmd) = rx.try_recv() {
                match cmd {
                    NotifCmd::Closed(id, reason) => {
                        let mut w = W::default();
                        w.u32(id);
                        w.u32(reason);
                        c.send(signal("NotificationClosed", "uu", w.b))?;
                    }
                    NotifCmd::Action(id, key) => {
                        let mut w = W::default();
                        w.u32(id);
                        w.string(&key);
                        c.send(signal("ActionInvoked", "us", w.b))?;
                    }
                }
            }
        }
        if fds[0].revents & POLLIN != 0 {
            let m = c.recv()?;
            if m.typ == METHOD_CALL {
                handle(&mut c, &m, tx, next_id)?;
            }
        }
    }
}

fn signal(member: &str, sig: &str, body: Vec<u8>) -> Message {
    Message {
        typ: SIGNAL,
        path: Some(PATH.into()),
        iface: Some(NAME.into()),
        member: Some(member.into()),
        sig: sig.into(),
        body,
        ..Default::default()
    }
}

fn reply(call: &Message, sig: &str, body: Vec<u8>) -> Message {
    Message { typ: METHOD_RETURN, reply_serial: Some(call.serial), dest: call.sender.clone(), sig: sig.into(), body, ..Default::default() }
}

fn error(call: &Message, name: &str) -> Message {
    let mut w = W::default();
    w.string(name);
    Message { typ: ERROR, error: Some(name.into()), reply_serial: Some(call.serial), dest: call.sender.clone(), sig: "s".into(), body: w.b, ..Default::default() }
}

/// Rozloží telo správy na hodnoty podľa signatúry.
fn values(m: &Message) -> Vec<V> {
    let mut r = Reader::new(&m.body, 0);
    let mut sig = m.sig.as_bytes();
    let mut out = Vec::new();
    while !sig.is_empty() {
        let n = type_len(sig);
        match r.value(&sig[..n]) {
            Some(v) => out.push(v),
            None => break,
        }
        sig = &sig[n..];
    }
    out
}

fn handle(c: &mut Conn, m: &Message, tx: &Tx<Msg>, next_id: &mut u32) -> std::io::Result<()> {
    let iface = m.iface.as_deref().unwrap_or("");
    let member = m.member.as_deref().unwrap_or("");
    let no_reply = m.flags & 1 != 0;
    let out: Message = match (iface, member) {
        (NAME, "Notify") => {
            let v = values(m);
            if v.len() < 8 {
                error(m, "org.freedesktop.DBus.Error.InvalidArgs")
            } else {
                let replaces = v[1].as_u32().unwrap_or(0);
                let id = if replaces != 0 {
                    replaces
                } else {
                    let i = *next_id;
                    *next_id = next_id.wrapping_add(1).max(1);
                    i
                };
                let mut actions = Vec::new();
                if let V::Arr(a) = &v[5] {
                    for p in a.chunks(2) {
                        if let [k, l] = p {
                            actions.push((k.as_str().unwrap_or("").to_string(), l.as_str().unwrap_or("").to_string()));
                        }
                    }
                }
                let mut urgency = 1u8;
                let mut image_path = String::new();
                let mut image_data: Option<&V> = None;
                if let V::Arr(h) = &v[6] {
                    for e in h {
                        if let V::Dict(k, val) = e {
                            let V::Var(inner) = &**val else { continue };
                            match k.as_str() {
                                Some("urgency") => {
                                    if let V::Byte(b) = **inner {
                                        urgency = b.min(2);
                                    }
                                }
                                Some("image-path") | Some("image_path") => image_path = inner.as_str().unwrap_or("").to_string(),
                                // `image-data` (1.2), `image_data` (1.1), `icon_data` (1.0)
                                Some("image-data") | Some("image_data") | Some("icon_data") => {
                                    if image_data.is_none() {
                                        image_data = Some(&**inner);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                let icon = crate::nicons::resolve(v[2].as_str().unwrap_or(""), &image_path, image_data);
                tx.send(Msg::Notif(Notification {
                    id,
                    app: v[0].as_str().unwrap_or("").to_string(),
                    summary: v[3].as_str().unwrap_or("").to_string(),
                    body: v[4].as_str().unwrap_or("").to_string(),
                    actions,
                    urgency,
                    timeout_ms: v[7].as_i32().unwrap_or(-1),
                    icon,
                }));
                let mut w = W::default();
                w.u32(id);
                reply(m, "u", w.b)
            }
        }
        (NAME, "GetCapabilities") => {
            let mut w = W::default();
            w.array(4, |w| {
                w.string("body");
                w.string("actions");
                w.string("icon-static");
            });
            reply(m, "as", w.b)
        }
        (NAME, "GetServerInformation") => {
            let mut w = W::default();
            for s in ["dynamic-island", "dynamic-island", env!("CARGO_PKG_VERSION"), "1.2"] {
                w.string(s);
            }
            reply(m, "ssss", w.b)
        }
        (NAME, "CloseNotification") => {
            let id = values(m).first().and_then(|v| v.as_u32()).unwrap_or(0);
            tx.send(Msg::NotifClose(id));
            let mut w = W::default();
            w.u32(id);
            w.u32(3);
            c.send(signal("NotificationClosed", "uu", w.b))?;
            reply(m, "", Vec::new())
        }
        ("org.freedesktop.DBus.Peer", "Ping") => reply(m, "", Vec::new()),
        ("org.freedesktop.DBus.Introspectable", "Introspect") => {
            let mut w = W::default();
            w.string("<node><interface name=\"org.freedesktop.Notifications\"/></node>");
            reply(m, "s", w.b)
        }
        _ => error(m, "org.freedesktop.DBus.Error.UnknownMethod"),
    };
    if !no_reply {
        c.send(out)?;
    }
    Ok(())
}
