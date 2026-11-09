#!/usr/bin/env python3
"""Vygeneruje schema_gen.json zo zdrojového kódu ostrovčeka (config.rs a moduly) a z komentárov vo vzorovom config.toml.

Použitie:  tools/gen_schema.py [CESTA_K_REPOZITARU_ISLANDU]
Hľadá volania `tabulka.str_or("kľúč", predvolená)`, `f64_or`, `bool_or`, `strs` aj ich `.clamp()/.max()/.min()`, takže
predvolené hodnoty a rozsahy v GUI sú vždy rovnaké ako v kóde.
"""
import json, os, re, sys, collections

REPO = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), '..', '..', 'dynamic-island')
SRC = os.path.join(REPO, 'src') + os.sep

FILE_SECTION = {'wifi.rs': 'wifi', 'bluetooth.rs': 'bluetooth', 'monitor.rs': 'monitor', 'capture.rs': 'capture',
                'trash.rs': 'trash', 'wallpaper.rs': 'wallpaper', 'quickset.rs': 'quickset', 'audio.rs': 'audio'}
# moduly s tlačidlami `btn(...)`: sekcia -> (on_click, on_click_right, tooltip) predvolené
BTN = {'clock': ('calendar', '', ''), 'network': ('', '', ''), 'vpn': ('', '', '{ifname}: {ip}'),
       'audio': ('audio-mute', '', ''), 'keyboard': ('layout-toggle', '', 'Klávesnica SK/US'), 'capslock': ('', '', ''),
       'inhibitor': ('inhibit-toggle', '', 'Idle Inhibitor'), 'radio': ('track-stop:radio', '', '{name}\nKlik = vypnúť'),
       'media': ('media', 'media-play-pause', '{artist} – {title}'), 'recorder': ('', '', ''), 'disks': ('menu', 'eject-all', '{list}')}

PAT = re.compile(r'(\b[a-z_0-9]+(?:\.sub\("\w+"\))*)\.(str_or|f64_or|bool_or|strs)\("(\w+)"(?:\s*,\s*((?:[^()]|\([^()]*\))*?))?\)(\.(?:clamp|max|min)\([^)]*\))?')


def unescape(s):
    s = s[1:-1]
    s = re.sub(r'\\u\{([0-9a-fA-F]+)\}', lambda m: chr(int(m.group(1), 16)), s)
    return s.replace('\\"', '"').replace('\\n', '\n').replace('\\\\', '\\')


def literal(text, kind):
    text = (text or '').strip()
    if kind == 'str_or' and text.startswith('"') and text.endswith('"'):
        return True, unescape(text)
    if kind == 'f64_or':
        try:
            return True, float(text.replace('_', ''))
        except ValueError:
            return False, None
    if kind == 'bool_or' and text in ('true', 'false'):
        return True, text == 'true'
    if kind == 'strs':
        return True, []
    return False, None


def bounds(suffix):
    lo = hi = None
    if suffix:
        m = re.match(r'\.(clamp|max|min)\(([^)]*)\)', suffix)
        args = [float(a.replace('_', '')) for a in m.group(2).split(',') if re.match(r'^\s*-?[\d_.]+\s*$', a)]
        if m.group(1) == 'clamp' and len(args) == 2:
            lo, hi = args
        elif m.group(1) == 'max' and args:
            lo = args[0]
        elif m.group(1) == 'min' and args:
            hi = args[0]
    return lo, hi


def sample_comments(path):
    """(sekcia, kľúč) -> komentár za hodnotou vo vzorovom config.toml (aj pri zakomentovanom kľúči)."""
    out = {}
    sec = ''
    if not os.path.exists(path):
        return out
    for line in open(path, encoding='utf8'):
        m = re.match(r'^\s*\[\[?([^\]]+)\]\]?', line)
        if m:
            sec = m.group(1).strip()
            continue
        m = re.match(r'^\s*#?\s*([A-Za-z0-9_]+)\s*=\s*(.*?)\s*#\s*(.+?)\s*$', line)
        if m and not m.group(2).startswith('#'):
            out.setdefault((sec, m.group(1)), m.group(3).strip())
    return out


def main():
    cfg = open(SRC + 'config.rs', encoding='utf8').read()
    var2sec = {m.group(1): m.group(2) for m in re.finditer(r'let (\w+) = (?:t|\w+)\.sub\("(\w+)"\)', cfg)}
    # rozsah DrunCfg::from_table
    d0 = cfg.index('impl DrunCfg')
    d1 = cfg.index('\n}\n', d0)
    rows = collections.OrderedDict()
    for fn in ['config.rs'] + sorted(FILE_SECTION):
        s = open(SRC + fn, encoding='utf8').read()
        cut = s.find('#[cfg(test)]')
        limit = cut if cut >= 0 else len(s)
        for m in PAT.finditer(s, 0, limit):
            expr, kind, key = m.group(1), m.group(2), m.group(3)
            mm = re.match(r'(\w+)((?:\.sub\("\w+"\))*)', expr)
            base, subs = mm.group(1), re.findall(r'\.sub\("(\w+)"\)', mm.group(2))
            sec = None
            if fn == 'config.rs' and d0 <= m.start() <= d1:
                sec = 'drun'
            elif fn in FILE_SECTION and (base in ('t', 'd', 'c', 'q') and not subs):
                sec = FILE_SECTION[fn]
            elif base in var2sec:
                sec = '.'.join([var2sec[base]] + subs)
            elif base == 't' and subs:
                sec = '.'.join(subs)
            if sec is None or (sec, key) in rows:
                continue
            ok, default = literal(m.group(4), kind)
            lo, hi = bounds(m.group(5))
            rows[(sec, key)] = dict(section=sec, key=key, kind=kind, default=default if ok else None, min=lo, max=hi)
    # tlačidlá modulov
    for sec, (c, r, tip) in BTN.items():
        for key, d in (('on_click', c), ('on_click_right', r), ('on_click_middle', ''), ('tooltip', tip)):
            rows.setdefault((sec, key), dict(section=sec, key=key, kind='str_or', default=d, min=None, max=None))
    comments = sample_comments(os.path.join(REPO, 'config', 'dynamic-island', 'config.toml'))
    for (sec, key), r in rows.items():
        r['comment'] = comments.get((sec, key), '')
    out = list(rows.values())
    json.dump(out, open(os.path.join(os.path.dirname(__file__), '..', 'schema_gen.json'), 'w', encoding='utf8'), ensure_ascii=False, indent=1)
    print(len(out), 'kľúčov,', len({r['section'] for r in out}), 'sekcií')


if __name__ == '__main__':
    main()
