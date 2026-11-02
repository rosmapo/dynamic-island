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
        "{{\"time\":{ts},\"id\":{},\"app\":\"{}\",\"urgency\":{},\"summary\":\"{}\",\"body\":\"{}\"}}",
        n.id,
        esc(&n.app),
        n.urgency,
        esc(&n.summary),
        esc(&n.body)
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
        assert_eq!(l, "{\"time\":100,\"id\":3,\"app\":\"a\\\"b\",\"urgency\":2,\"summary\":\"s\\nx\",\"body\":\"t\\\\\"}");
        let v = crate::json::parse(&l).unwrap();
        assert_eq!(v.get("summary").and_then(|x| x.as_str()), Some("s\nx"));
    }
}
