//! Minimálny D-Bus klient (std): autentifikácia EXTERNAL, marshalling, správy.
//! Len to, čo potrebuje server org.freedesktop.Notifications.
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;

extern "C" {
    fn getuid() -> u32;
}

#[derive(Clone, Debug, PartialEq)]
pub enum V {
    Byte(u8),
    Bool(bool),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    F64(f64),
    Str(String),
    Arr(Vec<V>),
    Struct(Vec<V>),
    Var(Box<V>),
    Dict(Box<V>, Box<V>),
}

impl V {
    pub fn as_str(&self) -> Option<&str> {
        if let V::Str(s) = self { Some(s) } else { None }
    }
    pub fn as_u32(&self) -> Option<u32> {
        if let V::U32(s) = self { Some(*s) } else { None }
    }
    pub fn as_i32(&self) -> Option<i32> {
        if let V::I32(s) = self { Some(*s) } else { None }
    }
}

// --------------------------------------------------------------- čítanie

pub struct Reader<'a> {
    pub b: &'a [u8],
    pub p: usize,
}

fn align_of(c: u8) -> usize {
    match c {
        b'n' | b'q' => 2,
        b'b' | b'i' | b'u' | b's' | b'o' | b'a' => 4,
        b'x' | b't' | b'd' | b'(' | b'{' => 8,
        _ => 1,
    }
}

/// Dĺžka prvého kompletného typu v signatúre.
pub fn type_len(sig: &[u8]) -> usize {
    match sig.first() {
        Some(b'a') => 1 + type_len(&sig[1..]),
        Some(b'(') | Some(b'{') => {
            let (open, close) = if sig[0] == b'(' { (b'(', b')') } else { (b'{', b'}') };
            let mut d = 0;
            for (i, &c) in sig.iter().enumerate() {
                if c == open {
                    d += 1;
                } else if c == close {
                    d -= 1;
                    if d == 0 {
                        return i + 1;
                    }
                }
            }
            sig.len()
        }
        Some(_) => 1,
        None => 0,
    }
}

impl<'a> Reader<'a> {
    pub fn new(b: &'a [u8], p: usize) -> Reader<'a> {
        Reader { b, p }
    }
    fn align(&mut self, n: usize) {
        while self.p % n != 0 {
            self.p += 1;
        }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.p..self.p.checked_add(n)?)?;
        self.p += n;
        Some(s)
    }
    pub fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    pub fn u32(&mut self) -> Option<u32> {
        self.align(4);
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    pub fn string(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        let s = self.take(n)?;
        self.take(1)?;
        Some(String::from_utf8_lossy(s).into_owned())
    }
    pub fn signature(&mut self) -> Option<String> {
        let n = self.u8()? as usize;
        let s = self.take(n)?;
        self.take(1)?;
        Some(String::from_utf8_lossy(s).into_owned())
    }

    /// Prečíta jednu hodnotu typu `sig` (jeden kompletný typ).
    pub fn value(&mut self, sig: &[u8]) -> Option<V> {
        let c = *sig.first()?;
        match c {
            b'y' => Some(V::Byte(self.u8()?)),
            b'b' => Some(V::Bool(self.u32()? != 0)),
            b'n' | b'q' => {
                self.align(2);
                self.take(2)?;
                Some(V::U32(0))
            }
            b'i' => Some(V::I32(self.u32()? as i32)),
            b'u' => Some(V::U32(self.u32()?)),
            b'x' | b't' | b'd' => {
                self.align(8);
                let raw = u64::from_le_bytes(self.take(8)?.try_into().ok()?);
                Some(match c {
                    b'x' => V::I64(raw as i64),
                    b't' => V::U64(raw),
                    _ => V::F64(f64::from_bits(raw)),
                })
            }
            b's' | b'o' => Some(V::Str(self.string()?)),
            b'g' => Some(V::Str(self.signature()?)),
            b'v' => {
                let s = self.signature()?;
                Some(V::Var(Box::new(self.value(s.as_bytes())?)))
            }
            b'a' => {
                let len = self.u32()? as usize;
                let elem = &sig[1..];
                self.align(align_of(*elem.first()?));
                let end = self.p.checked_add(len)?;
                if end > self.b.len() {
                    return None;
                }
                let mut v = Vec::new();
                while self.p < end {
                    v.push(self.value(&elem[..type_len(elem)])?);
                }
                Some(V::Arr(v))
            }
            b'(' => {
                self.align(8);
                let mut inner = &sig[1..type_len(sig) - 1];
                let mut v = Vec::new();
                while !inner.is_empty() {
                    let n = type_len(inner);
                    v.push(self.value(&inner[..n])?);
                    inner = &inner[n..];
                }
                Some(V::Struct(v))
            }
            b'{' => {
                self.align(8);
                let inner = &sig[1..type_len(sig) - 1];
                let kn = type_len(inner);
                let k = self.value(&inner[..kn])?;
                let v = self.value(&inner[kn..])?;
                Some(V::Dict(Box::new(k), Box::new(v)))
            }
            _ => None,
        }
    }
}

// -------------------------------------------------------------- zápis

#[derive(Default)]
pub struct W {
    pub b: Vec<u8>,
}

impl W {
    pub fn align(&mut self, n: usize) {
        while self.b.len() % n != 0 {
            self.b.push(0);
        }
    }
    pub fn u8(&mut self, v: u8) {
        self.b.push(v);
    }
    pub fn u32(&mut self, v: u32) {
        self.align(4);
        self.b.extend_from_slice(&v.to_le_bytes());
    }
    pub fn string(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.b.extend_from_slice(s.as_bytes());
        self.b.push(0);
    }
    pub fn signature(&mut self, s: &str) {
        self.b.push(s.len() as u8);
        self.b.extend_from_slice(s.as_bytes());
        self.b.push(0);
    }
    pub fn array(&mut self, elem_align: usize, f: impl FnOnce(&mut W)) {
        self.align(4);
        let at = self.b.len();
        self.b.extend_from_slice(&[0; 4]);
        self.align(elem_align);
        let start = self.b.len();
        f(self);
        let len = (self.b.len() - start) as u32;
        self.b[at..at + 4].copy_from_slice(&len.to_le_bytes());
    }
}

// ------------------------------------------------------------- správy

pub const METHOD_CALL: u8 = 1;
pub const METHOD_RETURN: u8 = 2;
pub const ERROR: u8 = 3;
pub const SIGNAL: u8 = 4;

#[derive(Clone, Debug, Default)]
pub struct Message {
    pub typ: u8,
    pub flags: u8,
    pub serial: u32,
    pub path: Option<String>,
    pub iface: Option<String>,
    pub member: Option<String>,
    pub error: Option<String>,
    pub reply_serial: Option<u32>,
    pub dest: Option<String>,
    pub sender: Option<String>,
    pub sig: String,
    pub body: Vec<u8>,
}

impl Message {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::default();
        w.b.extend_from_slice(&[b'l', self.typ, self.flags, 1]);
        w.u32(self.body.len() as u32);
        w.u32(self.serial);
        w.array(8, |w| {
            let s_field = |w: &mut W, code: u8, ty: &str, val: &str| {
                w.align(8);
                w.u8(code);
                w.signature(ty);
                if ty == "g" {
                    w.signature(val);
                } else {
                    w.string(val);
                }
            };
            if let Some(v) = &self.path {
                s_field(w, 1, "o", v);
            }
            if let Some(v) = &self.iface {
                s_field(w, 2, "s", v);
            }
            if let Some(v) = &self.member {
                s_field(w, 3, "s", v);
            }
            if let Some(v) = &self.error {
                s_field(w, 4, "s", v);
            }
            if let Some(v) = self.reply_serial {
                w.align(8);
                w.u8(5);
                w.signature("u");
                w.u32(v);
            }
            if let Some(v) = &self.dest {
                s_field(w, 6, "s", v);
            }
            if let Some(v) = &self.sender {
                s_field(w, 7, "s", v);
            }
            if !self.sig.is_empty() {
                s_field(w, 8, "g", &self.sig);
            }
        });
        w.align(8);
        w.b.extend_from_slice(&self.body);
        w.b
    }

    /// Dĺžka celej správy z prvých 16 bajtov.
    pub fn total_len(head: &[u8; 16]) -> usize {
        let body = u32::from_le_bytes(head[4..8].try_into().unwrap()) as usize;
        let fields = u32::from_le_bytes(head[12..16].try_into().unwrap()) as usize;
        16 + fields.div_ceil(8) * 8 + body
    }

    pub fn decode(b: &[u8]) -> Option<Message> {
        if b.len() < 16 || b[0] != b'l' {
            return None;
        }
        let mut m = Message { typ: b[1], flags: b[2], ..Default::default() };
        let body_len = u32::from_le_bytes(b[4..8].try_into().ok()?) as usize;
        m.serial = u32::from_le_bytes(b[8..12].try_into().ok()?);
        let mut r = Reader::new(b, 12);
        let flen = r.u32()? as usize;
        let end = r.p + flen;
        while r.p < end {
            r.align(8);
            if r.p >= end {
                break;
            }
            let code = r.u8()?;
            let sig = r.signature()?;
            let v = r.value(sig.as_bytes())?;
            match (code, v) {
                (1, V::Str(s)) => m.path = Some(s),
                (2, V::Str(s)) => m.iface = Some(s),
                (3, V::Str(s)) => m.member = Some(s),
                (4, V::Str(s)) => m.error = Some(s),
                (5, V::U32(n)) => m.reply_serial = Some(n),
                (6, V::Str(s)) => m.dest = Some(s),
                (7, V::Str(s)) => m.sender = Some(s),
                (8, V::Str(s)) => m.sig = s,
                _ => {}
            }
        }
        let start = end.div_ceil(8) * 8;
        m.body = b.get(start..start + body_len)?.to_vec();
        Some(m)
    }
}

// -------------------------------------------------------------- spojenie

pub struct Conn {
    s: UnixStream,
    serial: u32,
    pub unique: String,
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

fn open_bus(system: bool) -> std::io::Result<UnixStream> {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::SocketAddr;
    if system {
        let addr = std::env::var("DBUS_SYSTEM_BUS_ADDRESS").unwrap_or_default();
        if let Some(p) = addr.strip_prefix("unix:path=") {
            return UnixStream::connect(p.split(',').next().unwrap_or(p));
        }
        return UnixStream::connect("/run/dbus/system_bus_socket");
    }
    let addr = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
    for part in addr.split(';') {
        let Some(rest) = part.strip_prefix("unix:") else { continue };
        for kv in rest.split(',') {
            if let Some(p) = kv.strip_prefix("path=") {
                return UnixStream::connect(p);
            }
            if let Some(a) = kv.strip_prefix("abstract=") {
                let sa = SocketAddr::from_abstract_name(a.as_bytes())?;
                return UnixStream::connect_addr(&sa);
            }
        }
    }
    let uid = unsafe { getuid() };
    UnixStream::connect(format!("/run/user/{uid}/bus"))
}

impl Conn {
    pub fn connect() -> std::io::Result<Conn> {
        Conn::open(false)
    }

    /// Systémová zbernica (UDisks2).
    pub fn connect_system() -> std::io::Result<Conn> {
        Conn::open(true)
    }

    pub fn set_read_timeout(&self, d: Option<std::time::Duration>) {
        let _ = self.s.set_read_timeout(d);
    }

    fn open(system: bool) -> std::io::Result<Conn> {
        let mut s = open_bus(system)?;
        let uid = unsafe { getuid() }.to_string();
        s.write_all(b"\0")?;
        s.write_all(format!("AUTH EXTERNAL {}\r\n", hex(&uid)).as_bytes())?;
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        while !line.ends_with(b"\r\n") {
            s.read_exact(&mut byte)?;
            line.push(byte[0]);
            if line.len() > 512 {
                break;
            }
        }
        if !line.starts_with(b"OK") {
            return Err(std::io::Error::other(format!("D-Bus autentifikácia zlyhala: {}", String::from_utf8_lossy(&line).trim())));
        }
        s.write_all(b"BEGIN\r\n")?;
        let mut c = Conn { s, serial: 0, unique: String::new() };
        let reply = c.call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "Hello", "", Vec::new())?;
        c.unique = Reader::new(&reply.body, 0).string().unwrap_or_default();
        Ok(c)
    }

    pub fn fd(&self) -> RawFd {
        self.s.as_raw_fd()
    }

    pub fn send(&mut self, mut m: Message) -> std::io::Result<u32> {
        self.serial += 1;
        m.serial = self.serial;
        self.s.write_all(&m.encode())?;
        Ok(m.serial)
    }

    pub fn recv(&mut self) -> std::io::Result<Message> {
        let mut head = [0u8; 16];
        self.s.read_exact(&mut head)?;
        let total = Message::total_len(&head);
        if total > 64 * 1024 * 1024 {
            return Err(std::io::Error::other("príliš veľká D-Bus správa"));
        }
        let mut buf = vec![0u8; total];
        buf[..16].copy_from_slice(&head);
        self.s.read_exact(&mut buf[16..])?;
        Message::decode(&buf).ok_or_else(|| std::io::Error::other("neplatná D-Bus správa"))
    }

    /// Volanie metódy a čakanie na odpoveď (ostatné správy sa zahodia – iba pre štart).
    pub fn call(&mut self, dest: &str, path: &str, iface: &str, member: &str, sig: &str, body: Vec<u8>) -> std::io::Result<Message> {
        let serial = self.send(Message {
            typ: METHOD_CALL,
            path: Some(path.into()),
            iface: Some(iface.into()),
            member: Some(member.into()),
            dest: Some(dest.into()),
            sig: sig.into(),
            body,
            ..Default::default()
        })?;
        loop {
            let m = self.recv()?;
            if m.reply_serial == Some(serial) {
                if m.typ == ERROR {
                    let mut e = m.error.clone().unwrap_or_default();
                    if m.sig.starts_with('s') {
                        if let Some(t) = Reader::new(&m.body, 0).string() {
                            e = format!("{e}: {t}");
                        }
                    }
                    return Err(std::io::Error::other(e));
                }
                return Ok(m);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_notify_like() {
        let mut body = W::default();
        body.string("app");
        body.u32(0);
        body.string("");
        body.string("Súhrn");
        body.string("Telo správy");
        body.array(4, |w| {
            w.string("default");
            w.string("Open");
        });
        body.array(8, |w| {
            w.align(8);
            w.string("urgency");
            w.signature("y");
            w.u8(2);
        });
        body.u32((-1i32) as u32);
        let m = Message {
            typ: METHOD_CALL,
            serial: 7,
            path: Some("/org/freedesktop/Notifications".into()),
            iface: Some("org.freedesktop.Notifications".into()),
            member: Some("Notify".into()),
            dest: Some("org.freedesktop.Notifications".into()),
            sig: "susssasa{sv}i".into(),
            body: body.b,
            ..Default::default()
        };
        let bytes = m.encode();
        let mut head = [0u8; 16];
        head.copy_from_slice(&bytes[..16]);
        assert_eq!(Message::total_len(&head), bytes.len());
        let d = Message::decode(&bytes).unwrap();
        assert_eq!(d.member.as_deref(), Some("Notify"));
        assert_eq!(d.sig, "susssasa{sv}i");
        let mut r = Reader::new(&d.body, 0);
        let mut vals = Vec::new();
        let mut sig = d.sig.as_bytes();
        while !sig.is_empty() {
            let n = type_len(sig);
            vals.push(r.value(&sig[..n]).unwrap());
            sig = &sig[n..];
        }
        assert_eq!(vals[3].as_str(), Some("Súhrn"));
        assert_eq!(vals[5], V::Arr(vec![V::Str("default".into()), V::Str("Open".into())]));
        match &vals[6] {
            V::Arr(a) => match &a[0] {
                V::Dict(k, v) => {
                    assert_eq!(k.as_str(), Some("urgency"));
                    assert_eq!(**v, V::Var(Box::new(V::Byte(2))));
                }
                _ => panic!(),
            },
            _ => panic!(),
        }
        assert_eq!(vals[7].as_i32(), Some(-1));
    }

    #[test]
    fn type_lengths() {
        assert_eq!(type_len(b"a{sv}i"), 5);
        assert_eq!(type_len(b"(ii)s"), 4);
        assert_eq!(type_len(b"as"), 2);
    }
}
