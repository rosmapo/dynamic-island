//! História notifikácií: jeden JSON objekt na riadok v súbore vedľa konfigurácie.
use crate::data::Notification;
use std::io::Write;
use std::path::PathBuf;

pub struct History {
    path: PathBuf,
    max: usize,
    lines: usize,
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

pub fn line(ts: i64, n: &Notification) -> String {
    format!(
        "{{\"time\":{ts},\"id\":{},\"app\":\"{}\",\"urgency\":{},\"summary\":\"{}\",\"body\":\"{}\",\"icon\":\"{}\"}}",
        n.id,
        esc(&n.app),
        n.urgency,
        esc(&n.summary),
        esc(&n.body),
        esc(&n.icon)
    )
}

impl History {
    pub fn open(path: PathBuf, max: usize) -> History {
        let lines = std::fs::read_to_string(&path).map(|s| s.lines().count()).unwrap_or(0);
        History { path, max: max.max(10), lines }
    }

    pub fn append(&mut self, n: &Notification) {
        let l = line(crate::sys::now_unix(), n);
        if let Some(d) = self.path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{l}");
            self.lines += 1;
        }
        // orezanie: ponechaj posledných `max` záznamov
        if self.lines > self.max + self.max / 4 + 10 {
            if let Ok(s) = std::fs::read_to_string(&self.path) {
                let all: Vec<&str> = s.lines().collect();
                let keep = &all[all.len().saturating_sub(self.max)..];
                let tmp = self.path.with_extension("tmp");
                if std::fs::write(&tmp, keep.join("\n") + "\n").is_ok() && std::fs::rename(&tmp, &self.path).is_ok() {
                    self.lines = keep.len();
                }
            }
        }
    }
}

/// Záznam histórie pre panel notifikácií.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct HistEntry {
    pub time: i64,
    pub id: u32,
    pub app: String,
    pub urgency: u8,
    pub summary: String,
    pub body: String,
    /// cesta k PNG ikone (prázdna = bez ikony)
    pub icon: String,
}

/// Jeden riadok súboru → záznam (`None` pri neplatnom riadku).
pub fn parse_line(l: &str) -> Option<HistEntry> {
    let v = crate::json::parse(l)?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let n = |k: &str| v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
    Some(HistEntry { time: n("time") as i64, id: n("id") as u32, app: s("app"), urgency: n("urgency") as u8, summary: s("summary"), body: s("body"), icon: s("icon") })
}

/// Načíta najviac `max` posledných záznamov, najnovšie prvé.
pub fn load(path: &std::path::Path, max: usize) -> Vec<HistEntry> {
    let Ok(s) = std::fs::read_to_string(path) else { return Vec::new() };
    s.lines().rev().filter_map(parse_line).take(max).collect()
}

/// Odstráni záznam s daným časom a id (prvý zhodný od konca). Vráti, či sa niečo zmenilo.
pub fn remove(path: &std::path::Path, time: i64, id: u32) -> bool {
    let Ok(s) = std::fs::read_to_string(path) else { return false };
    let mut lines: Vec<&str> = s.lines().collect();
    let Some(pos) = lines.iter().rposition(|l| parse_line(l).map(|e| e.time == time && e.id == id).unwrap_or(false)) else { return false };
    lines.remove(pos);
    let tmp = path.with_extension("tmp");
    let body = if lines.is_empty() { String::new() } else { lines.join("\n") + "\n" };
    std::fs::write(&tmp, body).is_ok() && std::fs::rename(&tmp, path).is_ok()
}

/// Vymaže celú históriu.
pub fn clear(path: &std::path::Path) -> bool {
    std::fs::write(path, "").is_ok()
}

/// Čas záznamu pre kartu: „Dnes 14:32“, „Včera 21:05“, inak „05.11. 14:32“ (s rokom, ak je iný ako terajší).
pub fn when_label(ts: i64) -> String {
    let now = crate::sys::now_unix();
    let (tm, today, yday) = (crate::sys::local_tm(ts), crate::sys::local_tm(now), crate::sys::local_tm(now - 86400));
    let same = |a: &crate::sys::Tm, b: &crate::sys::Tm| a.tm_year == b.tm_year && a.tm_mon == b.tm_mon && a.tm_mday == b.tm_mday;
    let hm = crate::sys::strftime_str("%H:%M", &tm);
    if same(&tm, &today) {
        format!("Dnes {hm}")
    } else if same(&tm, &yday) {
        format!("Včera {hm}")
    } else if tm.tm_year == today.tm_year {
        crate::sys::strftime_str("%d.%m. %H:%M", &tm)
    } else {
        crate::sys::strftime_str("%d.%m.%Y %H:%M", &tm)
    }
}

/// Zoradený výpis pre `--history`: "2026-11-02 04:12  [app] súhrn – telo".
pub fn print(path: &std::path::Path, last: usize) -> i32 {
    let Ok(s) = std::fs::read_to_string(path) else {
        eprintln!("história neexistuje: {}", path.display());
        return 1;
    };
    let lines: Vec<&str> = s.lines().collect();
    for l in &lines[lines.len().saturating_sub(last)..] {
        let Some(v) = crate::json::parse(l) else { continue };
        let ts = v.get("time").and_then(|x| x.as_f64()).unwrap_or(0.0) as i64;
        let tm = crate::sys::local_tm(ts);
        let when = crate::sys::strftime_str("%Y-%m-%d %H:%M", &tm);
        let g = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let body = g("body").replace('\n', " ");
        println!("{when}  [{}] {}{}{}", g("app"), g("summary"), if body.is_empty() { "" } else { " – " }, body);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_line() {
        let n = Notification { id: 3, app: "a\"b".into(), summary: "s\nx".into(), body: "t\\".into(), urgency: 2, ..Default::default() };
        let l = line(100, &n);
        assert_eq!(l, "{\"time\":100,\"id\":3,\"app\":\"a\\\"b\",\"urgency\":2,\"summary\":\"s\\nx\",\"body\":\"t\\\\\",\"icon\":\"\"}");
        let v = crate::json::parse(&l).unwrap();
        assert_eq!(v.get("summary").and_then(|x| x.as_str()), Some("s\nx"));
    }

    #[test]
    fn parse_load_remove() {
        let dir = std::env::temp_dir().join(format!("di-hist-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("n.log");
        let mk = |id: u32, t: i64, s: &str| line(t, &Notification { id, app: "app".into(), summary: s.into(), icon: "/x/i.png".into(), ..Default::default() });
        std::fs::write(&f, [mk(1, 10, "a"), mk(2, 20, "b"), mk(3, 30, "c")].join("\n") + "\n").unwrap();
        let v = load(&f, 2);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].id, v[0].time, v[0].summary.as_str(), v[0].icon.as_str()), (3, 30, "c", "/x/i.png"));
        assert_eq!(v[1].id, 2);
        assert!(remove(&f, 20, 2));
        assert!(!remove(&f, 20, 2));
        assert_eq!(load(&f, 10).iter().map(|e| e.id).collect::<Vec<_>>(), vec![3, 1]);
        assert!(clear(&f));
        assert!(load(&f, 10).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
