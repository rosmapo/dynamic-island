//! Priame DDC/CI (VESA MCCS „get / set VCP feature“) cez `/dev/i2c-N` – bez `ddcutil`.
//!
//! Potrebuje modul `i2c-dev` a právo čítať/zapisovať `/dev/i2c-*` (skupina `i2c`), teda to isté čo `ddcutil`.
//! Každé volanie blokuje desiatky ms (oneskorenia predpísané špecifikáciou), preto sa volá z pracovných vlákien.
//! Prístup na zbernicu je serializovaný a medzi príkazmi sa dodrží pauza (monitory pri rýchlych príkazoch zahadzujú správy).
use std::ffi::{c_int, c_ulong};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::sync::Mutex;
use std::thread::sleep;
use std::time::{Duration, Instant};

extern "C" {
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
}
/// `I2C_SLAVE` (linux/i2c-dev.h)
const I2C_SLAVE: c_ulong = 0x0703;
/// 7-bitová adresa DDC/CI
const DDC_ADDR: c_ulong = 0x37;
/// cieľová adresa pri zápise (0x37 << 1), vstupuje do kontrolného súčtu žiadosti
const DEST_WRITE: u8 = 0x6E;
/// zdrojová adresa hostiteľa
const HOST_SRC: u8 = 0x51;
/// základ kontrolného súčtu odpovede
const REPLY_SEED: u8 = 0x50;
const LEN_FLAG: u8 = 0x80;
const GET_VCP: u8 = 0x01;
const GET_VCP_REPLY: u8 = 0x02;
const SET_VCP: u8 = 0x03;
/// odpoveď na get VCP: adresa, dĺžka, 8 bajtov dát, súčet
const GET_REPLY_LEN: usize = 11;
const GET_REPLY_PAYLOAD: u8 = 8;
const REPLY_DELAY: Duration = Duration::from_millis(45);
const SET_DELAY: Duration = Duration::from_millis(50);
/// najkratší odstup dvoch príkazov
const GAP: Duration = Duration::from_millis(50);
const RETRIES: usize = 3;
const RETRY_DELAY: Duration = Duration::from_millis(40);

#[derive(Debug)]
pub enum Error {
    /// zbernica sa nedá otvoriť (chýba `i2c-dev` alebo práva) – nemá zmysel skúšať znova
    Unavailable(String),
    /// monitor neodpovedal / vrátil nezmysel
    Failed(String),
}

/// Čas konca posledného príkazu; zámok zároveň serializuje prístup na zbernicu.
static LAST: Mutex<Option<Instant>> = Mutex::new(None);

fn checksum(seed: u8, bytes: &[u8]) -> u8 {
    bytes.iter().fold(seed, |a, b| a ^ b)
}

/// Správa hostiteľ → monitor.
fn message(payload: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(payload.len() + 3);
    m.push(HOST_SRC);
    m.push(LEN_FLAG | payload.len() as u8);
    m.extend_from_slice(payload);
    m.push(checksum(DEST_WRITE, &m));
    m
}

/// Odpoveď na get VCP: `(aktuálna, maximum)`.
fn parse_get_reply(r: &[u8], vcp: u8) -> Result<(u16, u16), String> {
    if r.len() < GET_REPLY_LEN || r[1] & !LEN_FLAG != GET_REPLY_PAYLOAD || r[2] != GET_VCP_REPLY || r[4] != vcp {
        return Err(format!("neočakávaná odpoveď DDC/CI {r:02x?}"));
    }
    if checksum(REPLY_SEED, &r[..GET_REPLY_LEN - 1]) != r[GET_REPLY_LEN - 1] {
        return Err("zlý kontrolný súčet odpovede DDC/CI".into());
    }
    if r[3] != 0 {
        return Err(format!("monitor nepodporuje VCP {vcp:#04x}"));
    }
    Ok((u16::from_be_bytes([r[8], r[9]]), u16::from_be_bytes([r[6], r[7]])))
}

fn open(bus: u32) -> Result<std::fs::File, Error> {
    let f = OpenOptions::new().read(true).write(true).open(format!("/dev/i2c-{bus}")).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied => Error::Unavailable(format!("/dev/i2c-{bus}: {e}")),
        _ => Error::Failed(format!("/dev/i2c-{bus}: {e}")),
    })?;
    if unsafe { ioctl(f.as_raw_fd(), I2C_SLAVE, DDC_ADDR) } < 0 {
        return Err(Error::Failed(format!("I2C_SLAVE: {}", std::io::Error::last_os_error())));
    }
    Ok(f)
}

/// Jedna transakcia s opakovaním; `op` dostane otvorenú zbernicu.
fn transact<T>(bus: u32, mut op: impl FnMut(&mut std::fs::File) -> Result<T, String>) -> Result<T, Error> {
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(t) = *last {
        let el = t.elapsed();
        if el < GAP {
            sleep(GAP - el);
        }
    }
    let mut err = String::new();
    let mut out = None;
    let mut unavailable = None;
    for attempt in 0..RETRIES {
        match open(bus) {
            Ok(mut f) => match op(&mut f) {
                Ok(v) => {
                    out = Some(v);
                    break;
                }
                Err(e) => err = e,
            },
            Err(Error::Unavailable(e)) => {
                unavailable = Some(e);
                break;
            }
            Err(Error::Failed(e)) => err = e,
        }
        if attempt + 1 < RETRIES {
            sleep(RETRY_DELAY);
        }
    }
    *last = Some(Instant::now());
    match (out, unavailable) {
        (Some(v), _) => Ok(v),
        (None, Some(e)) => Err(Error::Unavailable(e)),
        (None, None) => Err(Error::Failed(err)),
    }
}

/// Prečíta VCP funkciu: `(aktuálna, maximum)`.
pub fn get_vcp(bus: u32, vcp: u8) -> Result<(u16, u16), Error> {
    transact(bus, |f| {
        f.write_all(&message(&[GET_VCP, vcp])).map_err(|e| e.to_string())?;
        sleep(REPLY_DELAY);
        let mut r = [0u8; GET_REPLY_LEN];
        f.read_exact(&mut r).map_err(|e| e.to_string())?;
        parse_get_reply(&r, vcp)
    })
}

/// Zapíše VCP funkciu.
pub fn set_vcp(bus: u32, vcp: u8, value: u16) -> Result<(), Error> {
    let [hi, lo] = value.to_be_bytes();
    transact(bus, |f| {
        f.write_all(&message(&[SET_VCP, vcp, hi, lo])).map_err(|e| e.to_string())?;
        sleep(SET_DELAY);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_checksums_match_the_spec() {
        // klasická žiadosť „čítaj jas“: 51 82 01 10 ac
        assert_eq!(message(&[GET_VCP, 0x10]), vec![0x51, 0x82, 0x01, 0x10, 0xac]);
        let set = message(&[SET_VCP, 0x10, 0x00, 0x32]);
        assert_eq!(&set[..6], &[0x51, 0x84, 0x03, 0x10, 0x00, 0x32]);
        assert_eq!(set[6], checksum(DEST_WRITE, &set[..6]));
    }

    #[test]
    fn parses_replies() {
        // 6e 88 02 00 10 00 00 64 00 32 <súčet>: jas 50 zo 100
        let mut r = vec![0x6e, 0x88, 0x02, 0x00, 0x10, 0x00, 0x00, 0x64, 0x00, 0x32];
        r.push(checksum(REPLY_SEED, &r));
        assert_eq!(parse_get_reply(&r, 0x10), Ok((50, 100)));
        assert!(parse_get_reply(&r, 0x12).is_err(), "iná funkcia, než sa pýtalo");
        let mut bad = r.clone();
        bad[10] ^= 1;
        assert!(parse_get_reply(&bad, 0x10).is_err());
        let mut unsupported = r[..10].to_vec();
        unsupported[3] = 1;
        unsupported.push(checksum(REPLY_SEED, &unsupported));
        assert!(parse_get_reply(&unsupported, 0x10).unwrap_err().contains("nepodporuje"));
        assert!(parse_get_reply(&r[..5], 0x10).is_err());
    }
}
