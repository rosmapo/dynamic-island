//! Modul „trash“: zoznam koša zo všetkých pripojených diskov, obnova a vyprázdnenie.
//!
//! Tu je len konfigurácia `[trash]` a logika nad súbormi (freedesktop Trash spec: `info/*.trashinfo` + `files/`),
//! ktorá nezávisí od UI a beží na pozadí; stav a kreslenie sú v `ui.rs`.
use crate::toml::{Table, TableExt};
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Nastavenia sekcie `[trash]`.
#[derive(Clone, Debug)]
pub struct TrashCfg {
    /// šírka panela (logické px)
    pub width: f32,
    /// najviac viditeľných riadkov, viac položiek sa skroluje
    pub rows: usize,
    /// veľkosť písma (px)
    pub font: f32,
    /// pred vyprázdnením koša sa spýta na potvrdenie
    pub confirm_empty: bool,
    /// zobrazovať stĺpec s pôvodným umiestnením
    pub show_location: bool,
    /// zobrazovať stĺpec s dátumom zmazania
    pub show_date: bool,
}

impl TrashCfg {
    pub fn from_table(t: &Table) -> TrashCfg {
        TrashCfg {
            width: t.f64_or("width", 560.0).clamp(240.0, 1600.0) as f32,
            rows: t.f64_or("rows", 8.0).clamp(1.0, 50.0) as usize,
            font: t.f64_or("text_size", 14.0).clamp(8.0, 64.0) as f32,
            confirm_empty: t.bool_or("confirm_empty", true),
            show_location: t.bool_or("show_location", true),
            show_date: t.bool_or("show_date", true),
        }
    }
}

/// Jedna položka koša.
#[derive(Clone, Debug, PartialEq)]
pub struct TrashEntry {
    /// názov súboru / priečinka (podľa pôvodnej cesty)
    pub name: String,
    /// pôvodný priečinok na zobrazenie (domov skrátený na `~`)
    pub location: String,
    /// dátum zmazania na zobrazenie (`dd.mm.rrrr hh:mm`)
    pub date: String,
    /// kľúč na triedenie (ISO dátum), novšie majú väčší
    pub sort: String,
    /// `.trashinfo` súbor
    pub info: PathBuf,
    /// zmazaný súbor / priečinok v `files/`
    pub data: PathBuf,
    /// pôvodná absolútna cesta (kam sa obnovuje)
    pub orig: PathBuf,
}

// ------------------------------------------------------------------ parsovanie

/// Dekóduje `%XX` v ceste z `.trashinfo` na surové bajty.
pub fn percent_decode(s: &str) -> Vec<u8> {
    fn hex(c: u8) -> Option<u8> {
        (c as char).to_digit(16).map(|d| d as u8)
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// Z obsahu `.trashinfo` vyberie `Path=` a `DeletionDate=` (druhé môže byť prázdne).
pub fn parse_trashinfo(content: &str) -> Option<(String, String)> {
    let (mut path, mut date) = (None, String::new());
    for line in content.lines() {
        let line = line.trim_end();
        if let Some(v) = line.strip_prefix("Path=") {
            path = Some(v.to_string());
        } else if let Some(v) = line.strip_prefix("DeletionDate=") {
            date = v.to_string();
        }
    }
    path.filter(|p| !p.is_empty()).map(|p| (p, date))
}

/// `2024-05-03T14:22:10` -> (`03.05.2024 14:22`, kľúč na triedenie). Nečitateľný dátum sa zobrazí tak, ako je.
pub fn format_date(raw: &str) -> (String, String) {
    let b = raw.as_bytes();
    let digits = |r: std::ops::Range<usize>| b.get(r).map_or(false, |s| s.iter().all(u8::is_ascii_digit));
    let ok = raw.is_ascii() && raw.len() >= 16 && digits(0..4) && b[4] == b'-' && digits(5..7) && b[7] == b'-' && digits(8..10) && b[10] == b'T' && digits(11..13) && b[13] == b':' && digits(14..16);
    if ok {
        (format!("{}.{}.{} {}:{}", &raw[8..10], &raw[5..7], &raw[0..4], &raw[11..13], &raw[14..16]), raw.to_string())
    } else {
        (raw.to_string(), String::new())
    }
}

/// Pripojné body z obsahu `/proc/mounts` (bez pseudo súborových systémov, `\040` a pod. dekódované).
pub fn parse_mounts(text: &str) -> Vec<PathBuf> {
    const SKIP: [&str; 19] = [
        "proc", "sysfs", "devtmpfs", "devpts", "cgroup", "cgroup2", "securityfs", "debugfs", "tracefs", "configfs", "fusectl", "mqueue", "bpf", "pstore", "autofs",
        "hugetlbfs", "binfmt_misc", "rpc_pipefs", "squashfs",
    ];
    fn unescape(s: &str) -> Vec<u8> {
        let b = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
                let v = (b[i + 1] - b'0') as u32 * 64 + (b[i + 2] - b'0') as u32 * 8 + (b[i + 3] - b'0') as u32;
                out.push(v as u8);
                i += 4;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        out
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let mut f = line.split_whitespace();
        let (Some(_dev), Some(mp), Some(fs)) = (f.next(), f.next(), f.next()) else { continue };
        if SKIP.contains(&fs) {
            continue;
        }
        let p = PathBuf::from(OsString::from_vec(unescape(mp)));
        if seen.insert(p.clone()) {
            out.push(p);
        }
    }
    out
}

// ------------------------------------------------------------------ nájdenie košov

fn canon(p: &Path) -> PathBuf {
    fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

fn is_trash_dir(p: &Path) -> bool {
    p.join("info").is_dir() && p.join("files").is_dir()
}

/// Koše na načítanie: domáci kôš a pre každý pripojený disk `.Trash-UID` a `.Trash/UID` (podľa špecifikácie len ak je
/// `.Trash` skutočný adresár so sticky bitom). Druhá zložka je pripojný bod disku (pre relatívne cesty), `None` = domov.
pub fn trash_dirs(home_trash: &Path, uid: u32, mounts: &[PathBuf]) -> Vec<(PathBuf, Option<PathBuf>)> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    if is_trash_dir(home_trash) {
        seen.insert(canon(home_trash));
        out.push((home_trash.to_path_buf(), None));
    }
    for top in mounts {
        let shared = top.join(".Trash");
        if let Ok(md) = fs::symlink_metadata(&shared) {
            if md.is_dir() && md.permissions().mode() & 0o1000 != 0 {
                let c = shared.join(uid.to_string());
                if is_trash_dir(&c) && seen.insert(canon(&c)) {
                    out.push((c, Some(top.clone())));
                }
            }
        }
        let own = top.join(format!(".Trash-{uid}"));
        if is_trash_dir(&own) && seen.insert(canon(&own)) {
            out.push((own, Some(top.clone())));
        }
    }
    out
}

/// Zloží položky zo všetkých košov, najnovšie zmazané prvé.
pub fn load_entries(dirs: &[(PathBuf, Option<PathBuf>)], home: &Path) -> Vec<TrashEntry> {
    let mut all = Vec::new();
    for (dir, top) in dirs {
        let Ok(rd) = fs::read_dir(dir.join("info")) else { continue };
        for ent in rd.flatten() {
            let fname = ent.file_name().to_string_lossy().into_owned();
            let Some(stem) = fname.strip_suffix(".trashinfo") else { continue };
            let data = dir.join("files").join(stem);
            // zmazaná vec musí existovať (aj pokazený symlink je platná položka)
            if fs::symlink_metadata(&data).is_err() {
                continue;
            }
            let info = ent.path();
            let Ok(content) = fs::read_to_string(&info) else { continue };
            let Some((raw_path, raw_date)) = parse_trashinfo(&content) else { continue };
            let mut orig = PathBuf::from(OsString::from_vec(percent_decode(&raw_path)));
            if orig.is_relative() {
                // mimo domova je `Path=` relatívna k pripojnému bodu disku
                orig = top.clone().unwrap_or_else(|| PathBuf::from("/")).join(orig);
            }
            let name = orig.file_name().map_or_else(|| stem.to_string(), |n| n.to_string_lossy().into_owned());
            let parent = orig.parent().unwrap_or(Path::new("/"));
            let location = match parent.strip_prefix(home) {
                Ok(rest) if !home.as_os_str().is_empty() => {
                    if rest.as_os_str().is_empty() {
                        "~".to_string()
                    } else {
                        format!("~/{}", rest.display())
                    }
                }
                _ => parent.display().to_string(),
            };
            let (date, sort) = format_date(&raw_date);
            all.push(TrashEntry { name, location, date, sort, info, data, orig });
        }
    }
    all.sort_by(|a, b| b.sort.cmp(&a.sort).then_with(|| a.name.cmp(&b.name)));
    all
}

/// Načíta koše zo všetkých pripojených diskov (domov a UID sa zistia z prostredia a `/proc`).
pub fn load_all() -> Vec<TrashEntry> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let data_home = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(".local/share"));
    let uid = fs::metadata("/proc/self").map(|m| std::os::unix::fs::MetadataExt::uid(&m)).unwrap_or(0);
    let mounts = parse_mounts(&fs::read_to_string("/proc/mounts").unwrap_or_default());
    load_entries(&trash_dirs(&data_home.join("Trash"), uid, &mounts), &home)
}

// ------------------------------------------------------------------ operácie

fn remove_any(p: &Path) -> io::Result<()> {
    match fs::symlink_metadata(p) {
        Ok(md) if md.is_dir() => fs::remove_dir_all(p),
        Ok(_) => fs::remove_file(p),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn copy_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    let md = fs::symlink_metadata(src)?;
    if md.file_type().is_symlink() {
        std::os::unix::fs::symlink(fs::read_link(src)?, dst)
    } else if md.is_dir() {
        fs::create_dir(dst)?;
        for e in fs::read_dir(src)? {
            let e = e?;
            copy_recursive(&e.path(), &dst.join(e.file_name()))?;
        }
        fs::set_permissions(dst, md.permissions())
    } else {
        fs::copy(src, dst).map(|_| ())
    }
}

/// Presun ako `shutil.move`: `rename`, a ak je cieľ na inom disku, skopíruje a pôvodné zmaže.
fn move_path(src: &Path, dst: &Path) -> io::Result<()> {
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        // EXDEV: iný súborový systém
        Err(e) if e.raw_os_error() == Some(18) => {
            copy_recursive(src, dst)?;
            remove_any(src)
        }
        Err(e) => Err(e),
    }
}

/// Obnoví položku na pôvodné miesto (vytvorí chýbajúce priečinky). Nikdy neprepíše existujúci súbor.
pub fn restore(e: &TrashEntry) -> Result<(), String> {
    if fs::symlink_metadata(&e.data).is_err() {
        return Err(format!("{}: položka v koši už neexistuje", e.name));
    }
    if fs::symlink_metadata(&e.orig).is_ok() {
        return Err(format!("{}: na cieli už existuje {}", e.name, e.orig.display()));
    }
    if let Some(p) = e.orig.parent() {
        fs::create_dir_all(p).map_err(|er| format!("{}: {er}", e.name))?;
    }
    move_path(&e.data, &e.orig).map_err(|er| format!("{}: {er}", e.name))?;
    let _ = fs::remove_file(&e.info);
    Ok(())
}

/// Natrvalo zmaže položku z koša (súbor aj jeho `.trashinfo`).
pub fn delete_forever(e: &TrashEntry) -> Result<(), String> {
    remove_any(&e.data).map_err(|er| format!("{}: {er}", e.name))?;
    match fs::remove_file(&e.info) {
        Ok(()) => Ok(()),
        Err(er) if er.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(er) => Err(format!("{}: {er}", e.name)),
    }
}

/// Súhrn dávky pre používateľa: `Obnovené: 3` alebo `Obnovené: 2, chýb: 1 (prvá chyba)`.
fn summary(verb: &str, ok: usize, errs: &[String]) -> String {
    match errs.first() {
        None => format!("{verb}: {ok}"),
        Some(first) => format!("{verb}: {ok}, chýb: {} ({first})", errs.len()),
    }
}

/// Obnoví všetky `entries`; vráti súhrnnú hlášku.
pub fn restore_all(entries: &[TrashEntry]) -> String {
    let errs: Vec<String> = entries.iter().filter_map(|e| restore(e).err()).collect();
    summary("Obnovené", entries.len() - errs.len(), &errs)
}

/// Natrvalo zmaže všetky `entries`; vráti súhrnnú hlášku.
pub fn empty_all(entries: &[TrashEntry]) -> String {
    let errs: Vec<String> = entries.iter().filter_map(|e| delete_forever(e).err()).collect();
    summary("Vymazané", entries.len() - errs.len(), &errs)
}

// ------------------------------------------------------------------ geometria

/// „1 položka“, „3 položky“, „5 položiek“.
pub fn count_label(n: usize) -> String {
    let w = match n {
        1 => "položka",
        2..=4 => "položky",
        _ => "položiek",
    };
    format!("{n} {w}")
}

/// Koľko riadkov zoznamu sa zobrazí: podľa počtu položiek, najviac `max_rows`, aspoň 2 (aby sa zmestila hláška).
pub fn rows_shown(n: usize, max_rows: usize) -> usize {
    n.min(max_rows).max(2)
}

/// Šírky stĺpcov `(názov, umiestnenie, dátum)` z dostupnej šírky `avail` (bez zaškrtávacieho políčka). Dátum má pevnú
/// šírku `date_w`, zvyšok sa delí 40 % názov / 60 % umiestnenie (bez umiestnenia berie všetko názov).
pub fn columns(avail: f32, date_w: f32, gap: f32, show_location: bool, show_date: bool) -> (f32, f32, f32) {
    let date = if show_date { date_w } else { 0.0 };
    let n_cols = 1 + show_location as usize + show_date as usize;
    let rest = (avail - date - (n_cols as f32 - 1.0) * gap).max(0.0);
    if show_location {
        let name = rest * 0.4;
        (name, rest - name, date)
    } else {
        (rest, 0.0, date)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toml;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tmp() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!("di-trash-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    /// Vytvorí kôš `dir` s položkou: súbor v `files/` a `.trashinfo`.
    fn add(dir: &Path, name: &str, path_line: &str, date: &str, content: &str) {
        fs::create_dir_all(dir.join("info")).unwrap();
        fs::create_dir_all(dir.join("files")).unwrap();
        fs::write(dir.join("files").join(name), content).unwrap();
        fs::write(dir.join("info").join(format!("{name}.trashinfo")), format!("[Trash Info]\nPath={path_line}\nDeletionDate={date}\n")).unwrap();
    }

    #[test]
    fn config_defaults_and_values() {
        let c = TrashCfg::from_table(&Table::new());
        assert_eq!((c.width, c.rows, c.font, c.confirm_empty, c.show_location, c.show_date), (560.0, 8, 14.0, true, true, true));
        let c = TrashCfg::from_table(&toml::parse("width=700\nrows=0\ntext_size=18\nconfirm_empty=false\nshow_location=false\nshow_date=false\n").unwrap());
        assert_eq!((c.width, c.rows, c.font, c.confirm_empty, c.show_location, c.show_date), (700.0, 1, 18.0, false, false, false));
    }

    #[test]
    fn decoding() {
        assert_eq!(percent_decode("/home/a/M%C3%B4j%20s%C3%BAbor.txt"), "/home/a/Môj súbor.txt".as_bytes());
        // neplatné alebo neúplné escape sekvencie ostanú
        assert_eq!(percent_decode("100%"), b"100%");
        assert_eq!(percent_decode("a%zzb%4"), b"a%zzb%4");
        assert_eq!(percent_decode("%2F"), b"/");
    }

    #[test]
    fn trashinfo_and_dates() {
        let (p, d) = parse_trashinfo("[Trash Info]\nPath=/home/a/x%20y\nDeletionDate=2024-05-03T14:22:10\n").unwrap();
        assert_eq!((p.as_str(), d.as_str()), ("/home/a/x%20y", "2024-05-03T14:22:10"));
        assert_eq!(parse_trashinfo("[Trash Info]\nPath=a\n").unwrap().1, "");
        assert!(parse_trashinfo("[Trash Info]\nDeletionDate=2024-05-03T14:22:10\n").is_none());
        assert!(parse_trashinfo("[Trash Info]\nPath=\n").is_none());
        assert_eq!(format_date("2024-05-03T14:22:10"), ("03.05.2024 14:22".to_string(), "2024-05-03T14:22:10".to_string()));
        assert_eq!(format_date("kedysi"), ("kedysi".to_string(), String::new()));
        assert_eq!(format_date("2024-05-03"), ("2024-05-03".to_string(), String::new()));
        assert_eq!(format_date("2024-13-0xT14:22:10").1, String::new());
    }

    #[test]
    fn mounts() {
        let txt = "proc /proc proc rw 0 0\n/dev/sda1 / ext4 rw 0 0\n/dev/sdb1 /media/mm/My\\040Disk ext4 rw 0 0\n/dev/sdb1 /media/mm/My\\040Disk ext4 rw 0 0\n/dev/loop3 /snap/core squashfs ro 0 0\ntmpfs /run tmpfs rw 0 0\n";
        let m = parse_mounts(txt);
        assert_eq!(m, vec![PathBuf::from("/"), PathBuf::from("/media/mm/My Disk"), PathBuf::from("/run")]);
    }

    #[test]
    fn finds_trash_dirs_per_spec() {
        let root = tmp();
        let home_trash = root.join("home/.local/share/Trash");
        add(&home_trash, "a.txt", "/x/a.txt", "2024-01-01T10:00:00", "1");
        let mnt1 = root.join("mnt1");
        add(&mnt1.join(".Trash-1000"), "b.txt", "dir/b.txt", "2024-02-01T10:00:00", "2");
        // .Trash bez sticky bitu sa podľa špecifikácie ignoruje
        let mnt2 = root.join("mnt2");
        add(&mnt2.join(".Trash/1000"), "c.txt", "c.txt", "2024-03-01T10:00:00", "3");
        // .Trash so sticky bitom je platný
        let mnt3 = root.join("mnt3");
        add(&mnt3.join(".Trash/1000"), "d.txt", "d.txt", "2024-04-01T10:00:00", "4");
        fs::set_permissions(mnt3.join(".Trash"), fs::Permissions::from_mode(0o1777)).unwrap();
        // kôš iného používateľa sa nenačíta
        let mnt4 = root.join("mnt4");
        add(&mnt4.join(".Trash-1001"), "e.txt", "e.txt", "2024-05-01T10:00:00", "5");
        let mounts = vec![mnt1.clone(), mnt2.clone(), mnt3.clone(), mnt4.clone(), root.join("neexistuje")];
        let dirs = trash_dirs(&home_trash, 1000, &mounts);
        let tops: Vec<Option<PathBuf>> = dirs.iter().map(|d| d.1.clone()).collect();
        assert_eq!(tops, vec![None, Some(mnt1.clone()), Some(mnt3.clone())]);
        // duplicitné pripojenie toho istého disku sa nezdvojí
        let mut twice = mounts.clone();
        twice.push(mnt1.clone());
        assert_eq!(trash_dirs(&home_trash, 1000, &twice).len(), 3);

        let home = root.join("home");
        let items = load_entries(&dirs, &home);
        let names: Vec<&str> = items.iter().map(|e| e.name.as_str()).collect();
        // najnovšie prvé
        assert_eq!(names, vec!["d.txt", "b.txt", "a.txt"]);
        let b = items.iter().find(|e| e.name == "b.txt").unwrap();
        // relatívna cesta na externom disku sa viaže na pripojný bod
        assert_eq!(b.orig, mnt1.join("dir/b.txt"));
        assert_eq!(b.date, "01.02.2024 10:00");
        let a = items.iter().find(|e| e.name == "a.txt").unwrap();
        assert_eq!(a.orig, PathBuf::from("/x/a.txt"));
        assert_eq!(a.location, "/x");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn home_is_shortened_and_names_decoded() {
        let root = tmp();
        let home = root.join("home");
        let trash = home.join(".local/share/Trash");
        add(&trash, "Môj súbor.txt", &format!("{}/Dokumenty/M%C3%B4j%20s%C3%BAbor.txt", home.display()), "2024-01-01T10:00:00", "x");
        add(&trash, "top.txt", &format!("{}/top.txt", home.display()), "2024-01-02T10:00:00", "y");
        // položka bez súboru v files/ aj s pokazeným .trashinfo sa preskočí
        fs::write(trash.join("info/duch.trashinfo"), "[Trash Info]\nPath=/x/duch\n").unwrap();
        add(&trash, "zle.txt", "", "2024-01-03T10:00:00", "z");
        let items = load_entries(&trash_dirs(&trash, 1000, &[]), &home);
        assert_eq!(items.len(), 2);
        assert_eq!((items[0].name.as_str(), items[0].location.as_str()), ("top.txt", "~"));
        assert_eq!((items[1].name.as_str(), items[1].location.as_str()), ("Môj súbor.txt", "~/Dokumenty"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn restore_and_delete() {
        let root = tmp();
        let home = root.join("home");
        let trash = home.join(".local/share/Trash");
        add(&trash, "a.txt", &format!("{}/novy/hlboko/a.txt", home.display()), "2024-01-01T10:00:00", "obsah-a");
        add(&trash, "b.txt", &format!("{}/b.txt", home.display()), "2024-01-02T10:00:00", "obsah-b");
        // priečinok s obsahom
        fs::create_dir_all(trash.join("files/priecinok/vnutri")).unwrap();
        fs::write(trash.join("files/priecinok/vnutri/f.txt"), "f").unwrap();
        fs::write(trash.join("info/priecinok.trashinfo"), format!("[Trash Info]\nPath={}/priecinok\nDeletionDate=2024-01-03T10:00:00\n", home.display())).unwrap();
        let items = load_entries(&trash_dirs(&trash, 1000, &[]), &home);
        assert_eq!(items.len(), 3);
        let by = |n: &str| items.iter().find(|e| e.name == n).unwrap().clone();

        // obnova vytvorí chýbajúce priečinky a zmaže .trashinfo
        let a = by("a.txt");
        restore(&a).unwrap();
        assert_eq!(fs::read_to_string(home.join("novy/hlboko/a.txt")).unwrap(), "obsah-a");
        assert!(!a.info.exists() && !a.data.exists());
        // druhá obnova tej istej položky zlyhá, nič sa nezmení
        assert!(restore(&a).unwrap_err().contains("neexistuje"));

        // existujúci cieľ sa neprepíše
        fs::write(home.join("b.txt"), "nove-b").unwrap();
        let b = by("b.txt");
        assert!(restore(&b).unwrap_err().contains("už existuje"));
        assert_eq!(fs::read_to_string(home.join("b.txt")).unwrap(), "nove-b");
        assert!(b.data.exists() && b.info.exists());

        // priečinok sa obnoví celý
        restore(&by("priecinok")).unwrap();
        assert_eq!(fs::read_to_string(home.join("priecinok/vnutri/f.txt")).unwrap(), "f");

        // zmazanie navždy: b.txt (súbor), druhé volanie je v poriadku (už nie je čo mazať)
        delete_forever(&b).unwrap();
        assert!(!b.data.exists() && !b.info.exists());
        delete_forever(&b).unwrap();
        assert!(load_entries(&trash_dirs(&trash, 1000, &[]), &home).is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn batch_summaries_and_symlinks() {
        let root = tmp();
        let home = root.join("home");
        let trash = home.join(".local/share/Trash");
        for n in ["a", "b", "c"] {
            add(&trash, n, &format!("{}/out/{n}", home.display()), "2024-01-01T10:00:00", n);
        }
        // pokazený symlink v koši je platná položka a dá sa zmazať aj obnoviť
        std::os::unix::fs::symlink("/neexistuje/nikde", trash.join("files/lnk")).unwrap();
        fs::write(trash.join("info/lnk.trashinfo"), format!("[Trash Info]\nPath={}/out/lnk\nDeletionDate=2024-01-01T09:00:00\n", home.display())).unwrap();
        let items = load_entries(&trash_dirs(&trash, 1000, &[]), &home);
        assert_eq!(items.len(), 4);
        // jedna kolízia -> súhrn s chybou
        fs::create_dir_all(home.join("out")).unwrap();
        fs::write(home.join("out/b"), "obsadene").unwrap();
        let note = restore_all(&items);
        assert!(note.starts_with("Obnovené: 3, chýb: 1 ("), "{note}");
        assert!(note.contains("b: na cieli už existuje"), "{note}");
        assert!(fs::symlink_metadata(home.join("out/lnk")).unwrap().file_type().is_symlink());
        // zvyšok (b) sa vymaže
        let rest = load_entries(&trash_dirs(&trash, 1000, &[]), &home);
        assert_eq!(rest.len(), 1);
        assert_eq!(empty_all(&rest), "Vymazané: 1");
        assert_eq!(empty_all(&[]), "Vymazané: 0");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn counts() {
        assert_eq!(count_label(0), "0 položiek");
        assert_eq!(count_label(1), "1 položka");
        assert_eq!(count_label(4), "4 položky");
        assert_eq!(count_label(5), "5 položiek");
        assert_eq!(count_label(21), "21 položiek");
    }

    #[test]
    fn geometry() {
        assert_eq!(rows_shown(0, 8), 2);
        assert_eq!(rows_shown(1, 8), 2);
        assert_eq!(rows_shown(5, 8), 5);
        assert_eq!(rows_shown(50, 8), 8);
        assert_eq!(rows_shown(50, 1), 2);
        // všetky stĺpce: dátum pevne, zvyšok 40/60, dve medzery
        let (n, l, d) = columns(500.0, 100.0, 10.0, true, true);
        assert_eq!((n, d), (152.0, 100.0));
        assert!((l - 228.0).abs() < 1e-3 && (n + l + d + 20.0 - 500.0).abs() < 1e-3);
        // bez umiestnenia berie všetko názov
        assert_eq!(columns(500.0, 100.0, 10.0, false, true), (390.0, 0.0, 100.0));
        assert_eq!(columns(500.0, 100.0, 10.0, false, false), (500.0, 0.0, 0.0));
        // príliš úzke okno nedá záporné šírky
        let (n, l, _) = columns(50.0, 100.0, 10.0, true, true);
        assert!(n >= 0.0 && l >= 0.0);
    }
}
