//! Tenká vrstva nad libc (bez crate `libc`): poll, eventfd, inotify, dlopen, čas.
use std::ffi::{c_char, c_int, c_long, c_void, CString};
use std::os::fd::RawFd;
use std::sync::mpsc;
use std::sync::Arc;

#[repr(C)]
pub struct PollFd {
    pub fd: c_int,
    pub events: i16,
    pub revents: i16,
}
pub const POLLIN: i16 = 1;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Tm {
    pub tm_sec: c_int,
    pub tm_min: c_int,
    pub tm_hour: c_int,
    pub tm_mday: c_int,
    pub tm_mon: c_int,
    pub tm_year: c_int,
    pub tm_wday: c_int,
    pub tm_yday: c_int,
    pub tm_isdst: c_int,
    pub tm_gmtoff: c_long,
    pub tm_zone: *const c_char,
}

extern "C" {
    fn poll(fds: *mut PollFd, nfds: std::ffi::c_ulong, timeout: c_int) -> c_int;
    fn eventfd(initval: u32, flags: c_int) -> c_int;
    fn inotify_init1(flags: c_int) -> c_int;
    fn inotify_add_watch(fd: c_int, path: *const c_char, mask: u32) -> c_int;
    fn read(fd: c_int, buf: *mut c_void, n: usize) -> isize;
    fn write(fd: c_int, buf: *const c_void, n: usize) -> isize;
    fn close(fd: c_int) -> c_int;
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn setlocale(cat: c_int, loc: *const c_char) -> *mut c_char;
    fn time(t: *mut i64) -> i64;
    fn localtime_r(t: *const i64, out: *mut Tm) -> *mut Tm;
    fn mktime(tm: *mut Tm) -> i64;
    fn strftime(s: *mut c_char, max: usize, fmt: *const c_char, tm: *const Tm) -> usize;
    fn signal(sig: c_int, handler: usize) -> usize;
    fn prctl(option: c_int, a2: std::ffi::c_ulong, a3: std::ffi::c_ulong, a4: std::ffi::c_ulong, a5: std::ffi::c_ulong) -> c_int;
    fn setsid() -> c_int;
    fn kill(pid: c_int, sig: c_int) -> c_int;
}

const LC_TIME: c_int = 2;
const RTLD_NOW: c_int = 2;
const EFD_NONBLOCK: c_int = 0o4000;
const EFD_CLOEXEC: c_int = 0o2000000;
const IN_NONBLOCK: c_int = 0o4000;
const IN_CLOEXEC: c_int = 0o2000000;
pub const IN_MODIFY: u32 = 0x2;
pub const IN_CLOSE_WRITE: u32 = 0x8;
pub const IN_MOVED_TO: u32 = 0x80;
pub const IN_CREATE: u32 = 0x100;
pub const IN_DELETE: u32 = 0x200;
pub const SIGUSR1: i32 = 10;
pub const SIGUSR2: i32 = 12;

// ----------------------------------------------------------------- poll
/// Počká na čitateľnosť deskriptorov; vráti počet pripravených (0 pri timeoute/EINTR).
pub fn poll_in(fds: &mut [PollFd], timeout_ms: i32) -> i32 {
    let r = unsafe { poll(fds.as_mut_ptr(), fds.len() as std::ffi::c_ulong, timeout_ms) };
    if r < 0 {
        0
    } else {
        r
    }
}

// --------------------------------------------------------------- eventfd
/// Budík pre hlavnú slučku: vlákna ním prebúdzajú `poll`.
pub struct Waker(RawFd);
impl Waker {
    pub fn new() -> Arc<Waker> {
        let fd = unsafe { eventfd(0, EFD_NONBLOCK | EFD_CLOEXEC) };
        Arc::new(Waker(fd))
    }
    pub fn fd(&self) -> RawFd {
        self.0
    }
    pub fn wake(&self) {
        let one: u64 = 1;
        unsafe { write(self.0, &one as *const u64 as *const c_void, 8) };
    }
    pub fn drain(&self) {
        let mut v: u64 = 0;
        unsafe { read(self.0, &mut v as *mut u64 as *mut c_void, 8) };
    }
}
impl Drop for Waker {
    fn drop(&mut self) {
        unsafe { close(self.0) };
    }
}

/// Odosielač správ do hlavnej slučky (kanál + prebudenie).
pub struct Tx<T> {
    tx: mpsc::Sender<T>,
    waker: Arc<Waker>,
}
impl<T> Clone for Tx<T> {
    fn clone(&self) -> Self {
        Tx { tx: self.tx.clone(), waker: self.waker.clone() }
    }
}
impl<T> Tx<T> {
    pub fn send(&self, m: T) {
        if self.tx.send(m).is_ok() {
            self.waker.wake();
        }
    }
}
pub fn channel<T>(waker: Arc<Waker>) -> (Tx<T>, mpsc::Receiver<T>) {
    let (tx, rx) = mpsc::channel();
    (Tx { tx, waker }, rx)
}

// -------------------------------------------------------------- inotify
pub struct Inotify(RawFd);
impl Inotify {
    pub fn new() -> Option<Inotify> {
        let fd = unsafe { inotify_init1(IN_NONBLOCK | IN_CLOEXEC) };
        if fd < 0 {
            None
        } else {
            Some(Inotify(fd))
        }
    }
    pub fn fd(&self) -> RawFd {
        self.0
    }
    pub fn watch(&self, path: &str, mask: u32) -> bool {
        match CString::new(path) {
            Ok(c) => unsafe { inotify_add_watch(self.0, c.as_ptr(), mask) >= 0 },
            Err(_) => false,
        }
    }
    /// Prečíta čakajúce udalosti a vráti názvy súborov, ktorých sa týkali.
    pub fn read_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = unsafe { read(self.0, buf.as_mut_ptr() as *mut c_void, buf.len()) };
            if n <= 0 {
                break;
            }
            let n = n as usize;
            let mut off = 0;
            while off + 16 <= n {
                let len = u32::from_ne_bytes([buf[off + 12], buf[off + 13], buf[off + 14], buf[off + 15]]) as usize;
                let name_end = (off + 16 + len).min(n);
                let raw = &buf[off + 16..name_end];
                let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
                out.push(String::from_utf8_lossy(&raw[..end]).into_owned());
                off += 16 + len;
            }
        }
        out
    }
}
impl Drop for Inotify {
    fn drop(&mut self) {
        unsafe { close(self.0) };
    }
}

// ---------------------------------------------------------------- dlopen
pub struct Lib(*mut c_void);
unsafe impl Send for Lib {}
impl Lib {
    pub fn open(names: &[&str]) -> Option<Lib> {
        for n in names {
            if let Ok(c) = CString::new(*n) {
                let h = unsafe { dlopen(c.as_ptr(), RTLD_NOW) };
                if !h.is_null() {
                    return Some(Lib(h));
                }
            }
        }
        None
    }
    pub fn sym(&self, name: &str) -> *const c_void {
        match CString::new(name) {
            Ok(c) => unsafe { dlsym(self.0, c.as_ptr()) as *const c_void },
            Err(_) => std::ptr::null(),
        }
    }
}

// ------------------------------------------------------------------- čas
pub fn set_time_locale(loc: &str) {
    let try_set = |l: &str| -> bool {
        match CString::new(l) {
            Ok(c) => unsafe { !setlocale(LC_TIME, c.as_ptr()).is_null() },
            Err(_) => false,
        }
    };
    if loc.is_empty() || !try_set(loc) {
        let _ = try_set("");
    }
}

pub fn now_unix() -> i64 {
    unsafe { time(std::ptr::null_mut()) }
}

pub fn local_tm(t: i64) -> Tm {
    let mut tm: Tm = unsafe { std::mem::zeroed() };
    unsafe { localtime_r(&t, &mut tm) };
    tm
}

/// Zloží dátum (mesiac 0-11) a normalizuje ho (doplní tm_wday, tm_yday).
pub fn make_tm(year: i32, month0: i32, day: i32) -> Tm {
    let mut tm: Tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year - 1900;
    tm.tm_mon = month0;
    tm.tm_mday = day;
    tm.tm_hour = 12;
    tm.tm_isdst = -1;
    unsafe { mktime(&mut tm) };
    tm
}

pub fn strftime_str(fmt: &str, tm: &Tm) -> String {
    let c = match CString::new(fmt) {
        Ok(c) => c,
        Err(_) => return String::new(),
    };
    let mut buf = vec![0u8; 512];
    let n = unsafe { strftime(buf.as_mut_ptr() as *mut c_char, buf.len(), c.as_ptr(), tm) };
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

// ------------------------------------------------------- signály, procesy
pub fn install_signal(sig: i32, handler: extern "C" fn(i32)) {
    unsafe { signal(sig, handler as usize) };
}

/// Dieťa dostane SIGTERM, keď zanikne rodičovské vlákno (aby neostávali sirotci).
pub fn die_with_parent() {
    unsafe { prctl(1, 15, 0, 0, 0) };
}
/// Pošle SIGTERM procesu `pid`.
pub fn terminate(pid: i32) {
    if pid > 0 {
        unsafe { kill(pid, 15) };
    }
}
pub fn new_session() {
    unsafe { setsid() };
}

/// Prebudí slučku zo signálového handlera (iba `write`, async-signal-safe).
pub fn raw_wake(fd: RawFd) {
    let one: u64 = 1;
    unsafe { write(fd, &one as *const u64 as *const c_void, 8) };
}
