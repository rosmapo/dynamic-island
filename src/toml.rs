//! Minimálny parser podmnožiny TOML (sekcie, [[polia tabuliek]], reťazce, čísla, bool, polia).
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Num(f64),
    Bool(bool),
    Arr(Vec<Value>),
    Table(Table),
}
pub type Table = BTreeMap<String, Value>;

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        if let Value::Str(s) = self { Some(s) } else { None }
    }
    pub fn as_f64(&self) -> Option<f64> {
        if let Value::Num(n) = self { Some(*n) } else { None }
    }
    pub fn as_bool(&self) -> Option<bool> {
        if let Value::Bool(b) = self { Some(*b) } else { None }
    }
    pub fn as_arr(&self) -> Option<&Vec<Value>> {
        if let Value::Arr(a) = self { Some(a) } else { None }
    }
    pub fn as_table(&self) -> Option<&Table> {
        if let Value::Table(t) = self { Some(t) } else { None }
    }
}

/// Pomocné čítanie z tabuľky s predvolenými hodnotami.
pub trait TableExt {
    fn str_or(&self, k: &str, d: &str) -> String;
    fn f64_or(&self, k: &str, d: f64) -> f64;
    fn bool_or(&self, k: &str, d: bool) -> bool;
    fn strs(&self, k: &str) -> Option<Vec<String>>;
    fn sub(&self, k: &str) -> Table;
}
impl TableExt for Table {
    fn str_or(&self, k: &str, d: &str) -> String {
        self.get(k).and_then(|v| v.as_str()).unwrap_or(d).to_string()
    }
    fn f64_or(&self, k: &str, d: f64) -> f64 {
        self.get(k).and_then(|v| v.as_f64()).unwrap_or(d)
    }
    fn bool_or(&self, k: &str, d: bool) -> bool {
        self.get(k).and_then(|v| v.as_bool()).unwrap_or(d)
    }
    fn strs(&self, k: &str) -> Option<Vec<String>> {
        self.get(k)
            .and_then(|v| v.as_arr())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
    }
    fn sub(&self, k: &str) -> Table {
        self.get(k).and_then(|v| v.as_table()).cloned().unwrap_or_default()
    }
}

pub fn parse(src: &str) -> Result<Table, String> {
    let mut root = Table::new();
    // cesta aktuálnej sekcie + či ide o prvok poľa tabuliek
    let mut path: Vec<String> = Vec::new();
    let lines: Vec<&str> = src.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let ln = i + 1;
        let line = strip_comment(lines[i]).trim().to_string();
        i += 1;
        if line.is_empty() {
            continue;
        }
        if let Some(h) = line.strip_prefix("[[").and_then(|l| l.strip_suffix("]]")) {
            path = split_path(h);
            let (last, parents) = path.split_last().ok_or(format!("riadok {ln}: prázdny názov"))?;
            let t = descend(&mut root, parents, ln)?;
            let e = t.entry(last.clone()).or_insert_with(|| Value::Arr(Vec::new()));
            match e {
                Value::Arr(a) => a.push(Value::Table(Table::new())),
                _ => return Err(format!("riadok {ln}: {last} nie je pole tabuliek")),
            }
        } else if let Some(h) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            path = split_path(h);
            descend(&mut root, &path, ln)?;
        } else {
            let eq = line.find('=').ok_or(format!("riadok {ln}: chýba '='"))?;
            let key = unquote_key(line[..eq].trim());
            let mut val = line[eq + 1..].trim().to_string();
            // viacriadkové pole
            while depth_open(&val) > 0 && i < lines.len() {
                val.push(' ');
                val.push_str(strip_comment(lines[i]).trim());
                i += 1;
            }
            let v = parse_value(&val).ok_or(format!("riadok {ln}: neplatná hodnota '{val}'"))?;
            let t = descend(&mut root, &path, ln)?;
            t.insert(key, v);
        }
    }
    Ok(root)
}

fn descend<'a>(root: &'a mut Table, path: &[String], ln: usize) -> Result<&'a mut Table, String> {
    let mut cur = root;
    for p in path {
        let e = cur.entry(p.clone()).or_insert_with(|| Value::Table(Table::new()));
        cur = match e {
            Value::Table(t) => t,
            Value::Arr(a) => match a.last_mut() {
                Some(Value::Table(t)) => t,
                _ => return Err(format!("riadok {ln}: zlá sekcia {p}")),
            },
            _ => return Err(format!("riadok {ln}: {p} nie je tabuľka")),
        };
    }
    Ok(cur)
}

fn split_path(h: &str) -> Vec<String> {
    h.split('.').map(|s| unquote_key(s.trim())).filter(|s| !s.is_empty()).collect()
}
fn unquote_key(k: &str) -> String {
    let k = k.trim();
    if k.len() >= 2 && ((k.starts_with('"') && k.ends_with('"')) || (k.starts_with('\'') && k.ends_with('\''))) {
        k[1..k.len() - 1].to_string()
    } else {
        k.to_string()
    }
}

/// Odstráni komentár mimo reťazcov.
fn strip_comment(l: &str) -> &str {
    let mut q: Option<char> = None;
    let mut esc = false;
    for (i, c) in l.char_indices() {
        match q {
            Some(qc) => {
                if esc {
                    esc = false;
                } else if c == '\\' && qc == '"' {
                    esc = true;
                } else if c == qc {
                    q = None;
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    q = Some(c);
                } else if c == '#' {
                    return &l[..i];
                }
            }
        }
    }
    l
}

fn depth_open(v: &str) -> i32 {
    let mut d = 0;
    let mut q: Option<char> = None;
    let mut esc = false;
    for c in v.chars() {
        match q {
            Some(qc) => {
                if esc {
                    esc = false;
                } else if c == '\\' && qc == '"' {
                    esc = true;
                } else if c == qc {
                    q = None;
                }
            }
            None => match c {
                '"' | '\'' => q = Some(c),
                '[' => d += 1,
                ']' => d -= 1,
                _ => {}
            },
        }
    }
    d
}

fn parse_value(v: &str) -> Option<Value> {
    let v = v.trim();
    if v.starts_with('"') {
        return parse_basic(v).map(Value::Str);
    }
    if v.starts_with('\'') {
        return (v.len() >= 2 && v.ends_with('\'')).then(|| Value::Str(v[1..v.len() - 1].to_string()));
    }
    if v == "true" {
        return Some(Value::Bool(true));
    }
    if v == "false" {
        return Some(Value::Bool(false));
    }
    if v.starts_with('[') && v.ends_with(']') {
        let inner = &v[1..v.len() - 1];
        let mut items = Vec::new();
        for part in split_top(inner) {
            let p = part.trim();
            if p.is_empty() {
                continue;
            }
            items.push(parse_value(p)?);
        }
        return Some(Value::Arr(items));
    }
    if v.starts_with('{') && v.ends_with('}') {
        let mut t = Table::new();
        for part in split_top(&v[1..v.len() - 1]) {
            let p = part.trim();
            if p.is_empty() {
                continue;
            }
            let eq = p.find('=')?;
            t.insert(unquote_key(p[..eq].trim()), parse_value(&p[eq + 1..])?);
        }
        return Some(Value::Table(t));
    }
    v.replace('_', "").parse::<f64>().ok().map(Value::Num)
}

fn split_top(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut d = 0;
    let mut q: Option<char> = None;
    let mut esc = false;
    for c in s.chars() {
        match q {
            Some(qc) => {
                cur.push(c);
                if esc {
                    esc = false;
                } else if c == '\\' && qc == '"' {
                    esc = true;
                } else if c == qc {
                    q = None;
                }
            }
            None => match c {
                '"' | '\'' => {
                    q = Some(c);
                    cur.push(c)
                }
                '[' | '{' => {
                    d += 1;
                    cur.push(c)
                }
                ']' | '}' => {
                    d -= 1;
                    cur.push(c)
                }
                ',' if d == 0 => out.push(std::mem::take(&mut cur)),
                _ => cur.push(c),
            },
        }
    }
    out.push(cur);
    out
}

fn parse_basic(v: &str) -> Option<String> {
    let mut chars = v.chars();
    if chars.next()? != '"' {
        return None;
    }
    let mut out = String::new();
    loop {
        match chars.next()? {
            '"' => return chars.as_str().trim().is_empty().then_some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                'u' | 'U' => {
                    // \uXXXX alebo \UXXXXXXXX
                    let n = if v.contains("\\U") { 8 } else { 4 };
                    let mut hex = String::new();
                    for _ in 0..n {
                        hex.push(chars.next()?);
                    }
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                _ => return None,
            },
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_config_like_input() {
        let t = parse(
            r##"
# komentár
top = 5
[island]
font = "Adwaita Sans"   # trailing
icons = ["a", "b#c",
  "d"]
radius = 17.5
on = true
[island.names]
Slovak = "SK"
[[custom]]
name = "eject"
exec = 'echo "x"'
[[custom]]
name = "second"
glyph = " é"
"##,
        )
        .unwrap();
        assert_eq!(t.f64_or("top", 0.0), 5.0);
        let isl = t.sub("island");
        assert_eq!(isl.str_or("font", ""), "Adwaita Sans");
        assert_eq!(isl.strs("icons").unwrap(), vec!["a", "b#c", "d"]);
        assert_eq!(isl.f64_or("radius", 0.0), 17.5);
        assert!(isl.bool_or("on", false));
        assert_eq!(isl.sub("names").str_or("Slovak", ""), "SK");
        let c = t.get("custom").unwrap().as_arr().unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].as_table().unwrap().str_or("exec", ""), "echo \"x\"");
        assert_eq!(c[1].as_table().unwrap().str_or("glyph", ""), "\u{f287} é");
    }
    #[test]
    fn errors() {
        assert!(parse("a = ").is_err());
        assert!(parse("novalue").is_err());
    }
}
