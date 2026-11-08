//! Aplikácie z `.desktop` súborov pre samostatný režim `--drun`: čítanie záznamov, Exec s poľami `%f`/`%U`…,
//! hľadanie ikon (PNG priamo, SVG cez `rsvg-convert` do cache) a filtrovanie podľa písaného textu.
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct App {
    pub name: String,
    pub generic: String,
    pub keywords: String,
    pub exec: String,
    pub icon: String,
    pub terminal: bool,
}

/// Adresáre s `.desktop` súbormi podľa XDG (používateľove majú prednosť).
pub fn app_dirs() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let data_home = std::env::var("XDG_DATA_HOME").ok().filter(|s| !s.is_empty()).unwrap_or(format!("{home}/.local/share"));
    let data_dirs = std::env::var("XDG_DATA_DIRS").ok().filter(|s| !s.is_empty()).unwrap_or("/usr/local/share:/usr/share".into());
    let mut v = vec![PathBuf::from(data_home).join("applications")];
    for d in data_dirs.split(':').filter(|s| !s.is_empty()) {
        v.push(PathBuf::from(d).join("applications"));
    }
    v
}

/// Jeden nájdený `.desktop` súbor a výsledok jeho spracovania (dôvod, prečo sa nezobrazí).
pub struct Entry {
    pub id: String,
    pub path: PathBuf,
    pub state: Result<App, String>,
}

/// Prejde všetky adresáre (poradie = priorita) a ku každému súboru povie, či sa zobrazí, alebo prečo nie.
/// Rovnaké ID súboru z viacerých adresárov sa berie raz (prvý vyhráva); rovnaký názov + príkaz tiež.
pub fn load_entries(desktop: &str) -> Vec<Entry> {
    let mut seen_id: HashSet<String> = HashSet::new();
    let mut seen_app: std::collections::HashMap<(String, String), PathBuf> = std::collections::HashMap::new();
    let mut out = Vec::new();
    for dir in app_dirs() {
        let mut files = Vec::new();
        collect(&dir, &dir, &mut files);
        for (id, path) in files {
            if !seen_id.insert(id.clone()) {
                out.push(Entry { id, path, state: Err("duplicita: rovnaké ID súboru už bolo načítané z adresára s vyššou prioritou".into()) });
                continue;
            }
            let state = match std::fs::read_to_string(&path) {
                Ok(src) => parse_reason(&src, desktop).and_then(|app| {
                    let key = (app.name.to_lowercase(), exec_line(&app.exec, &app.name, "").to_lowercase());
                    match seen_app.get(&key) {
                        Some(first) => Err(format!("duplicita: rovnaký názov a príkaz ako {}", first.display())),
                        None => {
                            seen_app.insert(key, path.clone());
                            Ok(app)
                        }
                    }
                }),
                Err(e) => Err(format!("nedá sa prečítať: {e}")),
            };
            out.push(Entry { id, path, state });
        }
    }
    out
}

/// Načíta všetky zobraziteľné aplikácie, zoradené podľa mena (bez ohľadu na veľkosť písmen).
pub fn load_all(desktop: &str) -> Vec<App> {
    let mut apps: Vec<App> = load_entries(desktop).into_iter().filter_map(|e| e.state.ok()).collect();
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps
}

/// `dynamic-island --drun-list`: vypíše každý nájdený `.desktop` súbor a či sa zobrazí (diagnostika).
pub fn print_list(desktop: &str) {
    let tools: Vec<String> = ["rsvg-convert", "inkscape", "magick", "convert"].iter().map(|t| format!("{t}: {}", if exec_exists(t) { "áno" } else { "nie" })).collect();
    println!("rasterizácia SVG ({}) — téma ikon: {}", tools.join(", "), {
        let t = gtk_icon_theme();
        if t.is_empty() { "hicolor (v nastaveniach GTK nie je)".to_string() } else { t }
    });
    if !exec_exists("rsvg-convert") {
        println!("UPOZORNENIE: chýba rsvg-convert (balík librsvg2-bin); zložitejšie SVG ikony sa cez ImageMagick môžu vykresliť zle (napr. biele plochy čierne)");
    }
    println!();
    let entries = load_entries(desktop);
    let shown = entries.iter().filter(|e| e.state.is_ok()).count();
    for e in &entries {
        match &e.state {
            Ok(a) => println!("zobrazí   {}  [{}]  icon={} → {}  ({})", a.name, e.id, a.icon, icon_png(&a.icon, 64, ""), e.path.display()),
            Err(why) => println!("SKRYTÉ    {}  — {why}", e.path.display()),
        }
    }
    println!("\n{shown} zobrazených z {} súborov", entries.len());
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect(root, &p, out);
        } else if p.extension().map(|e| e == "desktop").unwrap_or(false) {
            let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('/', "-");
            out.push((rel, p));
        }
    }
}

/// Spracuje jeden `.desktop` súbor; `None` ak sa aplikácia nemá zobraziť.
/// `desktop` = názov prostredia pre `OnlyShowIn` / `NotShowIn` (napr. "Wayfire"), prázdne = nekontrolovať.
pub fn parse(src: &str, desktop: &str) -> Option<App> {
    parse_reason(src, desktop).ok()
}

/// Logická hodnota v `.desktop` súbore (štandard: `true`/`false`; tolerujeme aj veľké písmená, `1`, `yes`).
fn is_true(v: &str) -> bool {
    matches!(v.trim().to_lowercase().as_str(), "true" | "1" | "yes")
}

/// Existuje program z `TryExec` (absolútna cesta alebo príkaz v `PATH`)?
fn exec_exists(cmd: &str) -> bool {
    let cmd = cmd.trim().trim_matches('"');
    if cmd.is_empty() {
        return true;
    }
    if cmd.contains('/') {
        return Path::new(cmd).is_file();
    }
    std::env::var("PATH").unwrap_or_default().split(':').filter(|d| !d.is_empty()).any(|d| Path::new(d).join(cmd).is_file())
}

/// Ako [`parse`], ale s dôvodom, prečo sa záznam preskočil.
pub fn parse_reason(src: &str, desktop: &str) -> Result<App, String> {
    let mut in_entry = false;
    let (mut name, mut generic, mut keywords, mut exec, mut icon) = (String::new(), String::new(), String::new(), String::new(), String::new());
    let (mut typ, mut no_display, mut hidden, mut terminal) = (String::new(), false, false, false);
    let mut try_exec = String::new();
    let (mut only, mut not): (Vec<String>, Vec<String>) = (vec![], vec![]);
    for line in src.trim_start_matches('\u{feff}').lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_entry = l == "[Desktop Entry]";
            continue;
        }
        if !in_entry || l.is_empty() || l.starts_with('#') {
            continue;
        }
        let Some((k, v)) = l.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        let list = |v: &str| v.split(';').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect::<Vec<_>>();
        match k {
            "Name" => name = unescape(v),
            "GenericName" => generic = unescape(v),
            "Keywords" => keywords = unescape(v),
            "Exec" => exec = v.to_string(),
            "TryExec" => try_exec = v.to_string(),
            "Icon" => icon = v.to_string(),
            "Type" => typ = v.to_string(),
            "NoDisplay" => no_display = is_true(v),
            "Hidden" => hidden = is_true(v),
            "Terminal" => terminal = is_true(v),
            "OnlyShowIn" => only = list(v),
            "NotShowIn" => not = list(v),
            _ => {}
        }
    }
    if typ != "Application" {
        return Err(format!("Type={typ} (nie Application)"));
    }
    if no_display {
        return Err("NoDisplay=true".into());
    }
    if hidden {
        return Err("Hidden=true".into());
    }
    if name.is_empty() || exec.is_empty() {
        return Err("chýba Name alebo Exec".into());
    }
    if !exec_exists(&try_exec) {
        return Err(format!("TryExec: {try_exec} neexistuje"));
    }
    if !desktop.is_empty() {
        let d = desktop.to_lowercase();
        if !only.is_empty() && !only.iter().any(|s| s.to_lowercase() == d) {
            return Err(format!("OnlyShowIn={} (prostredie {desktop})", only.join(";")));
        }
        if not.iter().any(|s| s.to_lowercase() == d) {
            return Err(format!("NotShowIn obsahuje {desktop}"));
        }
    }
    Ok(App { name, generic, keywords, exec, icon, terminal })
}

fn unescape(s: &str) -> String {
    s.replace("\\s", " ").replace("\\n", " ").replace("\\t", " ").replace("\\\\", "\\")
}

/// Z `Exec` vyhodí polia `%f %F %u %U %i %c %k …` (`%%` → `%`); `%c` = meno, `%i` = `--icon ikona`.
pub fn exec_line(exec: &str, name: &str, icon: &str) -> String {
    let mut out = String::new();
    let mut it = exec.chars().peekable();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('%') => out.push('%'),
            Some('c') => out.push_str(&quote(name)),
            Some('i') if !icon.is_empty() => {
                out.push_str("--icon ");
                out.push_str(&quote(icon));
            }
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Príkaz na spustenie: s `Terminal=true` ho obalí terminálom (`terminal` napr. "foot").
pub fn launch_cmd(app: &App, terminal: &str) -> String {
    let line = exec_line(&app.exec, &app.name, &app.icon);
    if app.terminal && !terminal.trim().is_empty() {
        format!("{} {}", terminal.trim(), line)
    } else {
        line
    }
}

/// Vyhovuje aplikácia hľadanému textu? Každé slovo dotazu musí byť v názve, všeobecnom názve alebo kľúčových slovách.
pub fn matches(app: &App, query: &str) -> bool {
    let hay = format!("{} {} {}", app.name, app.generic, app.keywords).to_lowercase();
    query.to_lowercase().split_whitespace().all(|w| hay.contains(w))
}

/// Zoradenie výsledkov: názvy začínajúce dotazom najprv, potom ostatné (stabilne, podľa abecedy).
pub fn filter(apps: &[App], query: &str) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    let mut hits: Vec<(u8, usize)> = apps
        .iter()
        .enumerate()
        .filter(|(_, a)| matches(a, &q))
        .map(|(i, a)| {
            let n = a.name.to_lowercase();
            let rank = if q.is_empty() || n.starts_with(&q) { 0 } else if n.split_whitespace().any(|w| w.starts_with(&q)) { 1 } else { 2 };
            (rank, i)
        })
        .collect();
    hits.sort_by_key(|(r, i)| (*r, *i));
    hits.into_iter().map(|(_, i)| i).collect()
}

// ------------------------------------------------------------ ikony

/// Téma ikon z nastavení GTK (`gtk-icon-theme-name` v `settings.ini`); prázdne = nenastavená.
pub fn gtk_icon_theme() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let cfg = std::env::var("XDG_CONFIG_HOME").ok().filter(|s| !s.is_empty()).unwrap_or(format!("{home}/.config"));
    for f in ["gtk-4.0", "gtk-3.0"] {
        if let Ok(src) = std::fs::read_to_string(format!("{cfg}/{f}/settings.ini")) {
            for l in src.lines() {
                if let Some((k, v)) = l.split_once('=') {
                    if k.trim() == "gtk-icon-theme-name" && !v.trim().is_empty() {
                        return v.trim().trim_matches('"').to_string();
                    }
                }
            }
        }
    }
    String::new()
}

/// Cesta k PNG pre ikonu aplikácie; `size` = požadovaná veľkosť v px, `theme` = téma ikon
/// (prázdne = téma z nastavení GTK, inak `hicolor`). Téma sa prehľadáva aj s `Inherits=`, takže ikona vždy
/// pochádza z jednej témy (nie z náhodnej, napr. kontrastnej či tmavej variantu). SVG sa raz rasterizuje do cache.
/// Prázdny reťazec = ikona sa nenašla.
pub fn icon_png(icon: &str, size: u32, theme: &str) -> String {
    if icon.is_empty() {
        return String::new();
    }
    if icon.starts_with('/') {
        return as_png(Path::new(icon), size);
    }
    let theme = if theme.trim().is_empty() { gtk_icon_theme() } else { theme.trim().to_string() };
    let theme = if theme.is_empty() { "hicolor".to_string() } else { theme };
    if let Some(p) = find_in_theme(icon, size, &theme) {
        return as_png(&p, size);
    }
    for dir in ["/usr/share/pixmaps", "/usr/local/share/pixmaps"] {
        for ext in ["png", "svg"] {
            let f = Path::new(dir).join(format!("{icon}.{ext}"));
            if f.is_file() {
                return as_png(&f, size);
            }
        }
    }
    // poslednou záchranou ľubovoľná iná téma (kontrastné témy sa preskakujú)
    if let Some(p) = crate::nicons::find_theme_icon(icon) {
        return p.to_string_lossy().into_owned();
    }
    match find_svg(icon) {
        Some(svg) => as_png(&svg, size),
        None => String::new(),
    }
}

fn icon_bases() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    vec![
        PathBuf::from(&home).join(".local/share/icons"),
        PathBuf::from(&home).join(".icons"),
        PathBuf::from("/usr/local/share/icons"),
        PathBuf::from("/usr/share/icons"),
    ]
}

/// Téma + témy z `Inherits=` (do hĺbky 4) + `hicolor`.
pub fn theme_chain(theme: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut todo = vec![theme.to_string()];
    while let Some(t) = todo.pop() {
        if t.is_empty() || out.contains(&t) || out.len() > 8 {
            continue;
        }
        out.push(t.clone());
        for base in icon_bases() {
            if let Ok(s) = std::fs::read_to_string(base.join(&t).join("index.theme")) {
                for l in s.lines() {
                    if let Some(v) = l.trim().strip_prefix("Inherits=") {
                        for p in v.split(',').rev() {
                            todo.push(p.trim().to_string());
                        }
                    }
                }
                break;
            }
        }
    }
    if !out.iter().any(|t| t == "hicolor") {
        out.push("hicolor".into());
    }
    out
}

/// Poradie veľkostí: najbližšie väčšie (alebo rovné) od najmenšej, potom menšie od najväčšej.
pub fn size_order(want: u32) -> Vec<u32> {
    const ALL: [u32; 10] = [16, 22, 24, 32, 48, 64, 96, 128, 256, 512];
    let mut v: Vec<u32> = ALL.iter().copied().filter(|s| *s >= want).collect();
    let mut small: Vec<u32> = ALL.iter().copied().filter(|s| *s < want).collect();
    small.reverse();
    v.extend(small);
    v
}

/// Hľadá ikonu najprv v zadanej téme (s dedičnosťou): PNG vhodnej veľkosti, potom SVG, potom menšie PNG.
fn find_in_theme(name: &str, size: u32, theme: &str) -> Option<PathBuf> {
    if theme.is_empty() || name.contains('/') || name.contains("..") {
        return None;
    }
    let order = size_order(size);
    let split = order.iter().take_while(|s| **s >= size).count();
    for th in theme_chain(theme) {
        for base in icon_bases() {
            let root = base.join(&th);
            if !root.is_dir() {
                continue;
            }
            let png = |sizes: &[u32]| -> Option<PathBuf> {
                for s in sizes {
                    for ctx in ["apps", "applications"] {
                        for c in [root.join(format!("{s}x{s}")).join(ctx), root.join(ctx).join(s.to_string())] {
                            let f = c.join(format!("{name}.png"));
                            if f.is_file() {
                                return Some(f);
                            }
                        }
                    }
                }
                None
            };
            if let Some(p) = png(&order[..split]) {
                return Some(p);
            }
            for c in ["scalable/apps", "apps/scalable", "scalable/applications"] {
                let f = root.join(c).join(format!("{name}.svg"));
                if f.is_file() {
                    return Some(f);
                }
            }
            if let Some(p) = png(&order[split..]) {
                return Some(p);
            }
        }
    }
    None
}

fn as_png(p: &Path, size: u32) -> String {
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    if ext == "png" {
        return if p.is_file() { p.to_string_lossy().into_owned() } else { String::new() };
    }
    if ext == "svg" || ext == "svgz" {
        return svg_to_png(p, size).map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
    }
    String::new()
}

fn cache_dir() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var("HOME").ok().filter(|s| !s.is_empty()).map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("dynamic-island").join("app-icons")
}

/// Rasterizuje SVG pomocou `rsvg-convert` (alebo ImageMagick) do `~/.cache/dynamic-island/app-icons/`.
/// Kľúč cache = cesta + čas zmeny + veľkosť, takže zmenená ikona sa vyrobí znova.
fn svg_to_png(svg: &Path, size: u32) -> Option<PathBuf> {
    let mtime = std::fs::metadata(svg).ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let key = format!("v2|{}|{}|{}", svg.display(), mtime, size);
    let dir = cache_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let out = dir.join(format!("{:016x}.png", crate::nicons::fnv(key.as_bytes(), size, size)));
    if out.is_file() {
        return Some(out);
    }
    let tmp = out.with_extension("tmp.png");
    let s = size.to_string();
    let run = |prog: &str, args: &[&str]| -> bool {
        exec_exists(prog)
            && std::process::Command::new(prog)
                .args(args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|st| st.success())
                .unwrap_or(false)
    };
    let (svg_s, tmp_s) = (svg.to_string_lossy().into_owned(), tmp.to_string_lossy().into_owned());
    let im_out = format!("PNG32:{tmp_s}");
    let im_size = format!("{s}x{s}");
    // poradie podľa kvality: librsvg a Inkscape zvládnu filtre, masky aj farebné prechody; ImageMagick (vstavaný
    // renderer MSVG) ich často nevie, preto ide až na koniec
    let ok = run("rsvg-convert", &["-w", &s, "-h", &s, "-o", &tmp_s, &svg_s])
        || run("inkscape", &[&svg_s, "--export-type=png", "-w", &s, "-h", &s, "-o", &tmp_s])
        || ["magick", "convert"].iter().any(|im| run(im, &["-background", "none", "-density", "384", &svg_s, "-resize", &im_size, "-colorspace", "sRGB", &im_out]));
    if ok && tmp.is_file() && std::fs::rename(&tmp, &out).is_ok() {
        Some(out)
    } else {
        let _ = std::fs::remove_file(&tmp);
        None
    }
}

/// Nájde SVG ikonu podľa názvu v témach (scalable/apps a pod.) a v `/usr/share/pixmaps`.
fn find_svg(name: &str) -> Option<PathBuf> {
    if name.contains('/') || name.contains("..") {
        return None;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let bases = [
        PathBuf::from(&home).join(".local/share/icons"),
        PathBuf::from(&home).join(".icons"),
        PathBuf::from("/usr/local/share/icons"),
        PathBuf::from("/usr/share/icons"),
    ];
    let file = format!("{name}.svg");
    for base in &bases {
        let Ok(rd) = std::fs::read_dir(base) else { continue };
        let mut themes: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && !p.file_name().map(|n| n.to_string_lossy().to_lowercase().contains("highcontrast")).unwrap_or(false))
            .collect();
        themes.sort_by_key(|p| (p.file_name().map(|n| n != "hicolor").unwrap_or(true), p.clone()));
        for theme in themes {
            for sub in ["scalable/apps", "apps/scalable", "scalable/applications", "64x64/apps", "48x48/apps", "apps/64", "apps/48"] {
                let c = theme.join(sub).join(&file);
                if c.is_file() {
                    return Some(c);
                }
            }
        }
    }
    let px = Path::new("/usr/share/pixmaps").join(&file);
    px.is_file().then_some(px)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FF: &str = "[Desktop Entry]\nType=Application\nName=Firefox\nName[sk]=Líška\nGenericName=Web Browser\nKeywords=internet;www;\nExec=firefox %u\nIcon=firefox\nTerminal=false\n\n[Desktop Action new-window]\nName=New Window\nExec=firefox --new-window\n";

    #[test]
    fn parses_main_entry_only() {
        let a = parse(FF, "").unwrap();
        assert_eq!(a.name, "Firefox");
        assert_eq!(a.exec, "firefox %u");
        assert_eq!(a.icon, "firefox");
        assert!(!a.terminal);
    }

    #[test]
    fn hidden_and_non_apps_skipped() {
        assert!(parse("[Desktop Entry]\nType=Application\nName=X\nExec=x\nNoDisplay=true\n", "").is_none());
        assert!(parse("[Desktop Entry]\nType=Link\nName=X\nExec=x\n", "").is_none());
        assert!(parse("[Desktop Entry]\nType=Application\nName=X\n", "").is_none());
    }

    #[test]
    fn no_display_in_any_form_and_position() {
        // NoDisplay môže byť kdekoľvek v skupine [Desktop Entry] (aj za Exec / ikonou), aj ako True / 1
        for v in ["true", "True", "1", "yes"] {
            let src = format!("[Desktop Entry]\nType=Application\nName=X\nExec=x\nIcon=i\nNoDisplay = {v}\nTerminal=false\n");
            assert_eq!(parse_reason(&src, "").unwrap_err(), "NoDisplay=true");
        }
        let early = "[Desktop Entry]\nNoDisplay=true\nType=Application\nName=X\nExec=x\n";
        assert!(parse(early, "").is_none());
        // mimo skupiny [Desktop Entry] (napr. v akcii) sa nepočíta
        let action = "[Desktop Entry]\nType=Application\nName=X\nExec=x\n[Desktop Action a]\nNoDisplay=true\n";
        assert!(parse(action, "").is_some());
        assert!(parse("[Desktop Entry]\nType=Application\nName=X\nExec=x\nNoDisplay=false\n", "").is_some());
        assert!(parse("\u{feff}[Desktop Entry]\nType=Application\nName=X\nExec=x\n", "").is_some());
    }

    #[test]
    fn try_exec_must_exist() {
        let base = "[Desktop Entry]\nType=Application\nName=X\nExec=x\n";
        assert!(parse(&format!("{base}TryExec=sh\n"), "").is_some());
        assert!(parse(&format!("{base}TryExec=/nonexistent/prog-zzz\n"), "").is_none());
        assert!(parse(&format!("{base}TryExec=prog-zzz-nenajdeny\n"), "").is_none());
    }

    #[test]
    fn show_in_rules() {
        let only = "[Desktop Entry]\nType=Application\nName=X\nExec=x\nOnlyShowIn=GNOME;\n";
        assert!(parse(only, "Wayfire").is_none());
        assert!(parse(only, "").is_some());
        assert!(parse(only, "gnome").is_some());
        let not = "[Desktop Entry]\nType=Application\nName=X\nExec=x\nNotShowIn=Wayfire;\n";
        assert!(parse(not, "Wayfire").is_none());
    }

    #[test]
    fn exec_fields() {
        assert_eq!(exec_line("firefox %u", "Firefox", "ff"), "firefox");
        assert_eq!(exec_line("env A=1 app %F --x 100%%", "N", ""), "env A=1 app --x 100%");
        assert_eq!(exec_line("app %c %i", "My App", "ico"), "app 'My App' --icon 'ico'");
    }

    #[test]
    fn size_order_prefers_larger() {
        assert_eq!(size_order(64), vec![64, 96, 128, 256, 512, 48, 32, 24, 22, 16]);
        assert_eq!(size_order(100)[0], 128);
        assert_eq!(size_order(8)[0], 16);
    }

    #[test]
    fn terminal_wrap() {
        let mut a = parse(FF, "").unwrap();
        a.terminal = true;
        assert_eq!(launch_cmd(&a, "foot"), "foot firefox");
        a.terminal = false;
        assert_eq!(launch_cmd(&a, "foot"), "firefox");
    }

    #[test]
    fn filter_ranks_prefix_first() {
        let mk = |n: &str, k: &str| App { name: n.into(), generic: String::new(), keywords: k.into(), exec: "x".into(), icon: String::new(), terminal: false };
        let apps = vec![mk("Calculator", ""), mk("Document Viewer", ""), mk("Files", "calc"), mk("Calendar", "")];
        assert_eq!(filter(&apps, ""), vec![0, 1, 2, 3]);
        assert_eq!(filter(&apps, "cal"), vec![0, 3, 2]);
        assert_eq!(filter(&apps, "view doc"), vec![1]);
        assert!(filter(&apps, "zzz").is_empty());
    }
}
