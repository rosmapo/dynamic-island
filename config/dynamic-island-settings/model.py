"""Model nastavení bez GUI: číta config.toml, drží neuložené zmeny a zapíše ich späť cez TomlDoc (komentáre ostanú)."""
import os, re, shutil, time, tomllib

import copy

import arrays
import schema
from tomledit import TomlDoc

RESET = object()          # značka „vrátiť na predvolené“ (kľúč sa zo súboru odstráni)
ARRAY_TABLES = {'custom', 'menu', 'notifications.rule'}


def default_path():
    base = os.environ.get('XDG_CONFIG_HOME') or os.path.expanduser('~/.config')
    return os.path.join(base, 'dynamic-island', 'config.toml')


def _get(d, section):
    cur = d
    for part in section.split('.') if section else []:
        cur = cur.get(part) if isinstance(cur, dict) else None
        if cur is None:
            return {}
    return cur if isinstance(cur, dict) else {}


class Model:
    def __init__(self, path=None):
        self.path = path or default_path()
        self.schema = schema.build()
        self.load()

    # ------------------------------------------------------------ načítanie
    def load(self):
        try:
            with open(self.path, encoding='utf8') as f:
                self.text = f.read()
        except FileNotFoundError:
            self.text = ''
        self.doc = TomlDoc(self.text)
        self.data = tomllib.loads(self.text)
        self.pending = {}          # (sekcia, kľúč) -> hodnota | RESET
        self.pending_indexed = {}  # (sekcia, názov skupiny) -> zoznam dictov
        self.pending_maps = {}     # sekcia -> dict
        self.pending_arrays = {}   # id poľa tabuliek -> zoznam záznamov
        self._array_orig = {}

    # ------------------------------------------------------------ hodnoty
    def entry(self, section, key):
        return self.schema.get((section, key))

    def in_file(self, section, key):
        return key in _get(self.data, section)

    def file_value(self, section, key):
        return _get(self.data, section).get(key)

    def value(self, section, key):
        """Aktuálna (aj neuložená) hodnota; bez nej predvolená z kódu."""
        p = self.pending.get((section, key), None)
        if p is RESET:
            return self.schema[(section, key)]['default']
        if (section, key) in self.pending:
            return p
        if self.in_file(section, key):
            return self.file_value(section, key)
        e = self.schema.get((section, key))
        return e['default'] if e else None

    def is_default(self, section, key):
        """Kľúč nie je v súbore (alebo bude zrušený) = platí predvolená hodnota."""
        p = self.pending.get((section, key), None)
        if p is RESET:
            return True
        if (section, key) in self.pending:
            return p == self.schema[(section, key)]['default'] and not self.in_file(section, key)
        return not self.in_file(section, key)

    def set(self, section, key, value):
        e = self.schema.get((section, key))
        if e is not None and e['type'] in ('int', 'float') and value is not None:
            value = int(value) if e['type'] == 'int' else float(value)
        if self.in_file(section, key):
            if value == self.file_value(section, key):
                self.pending.pop((section, key), None)
                return
        elif e is not None and value == e['default']:
            self.pending.pop((section, key), None)   # rovnaké ako predvolené a v súbore nie je – nič sa nezapíše
            return
        self.pending[(section, key)] = value

    def reset(self, section, key):
        if self.in_file(section, key):
            self.pending[(section, key)] = RESET
        else:
            self.pending.pop((section, key), None)

    # ------------------------------------------------------------ indexované kľúče
    def indexed_items(self, section, patterns, maxn):
        """Zoznam položiek [{pole: hodnota}] zo súboru (po najvyšší nastavený index, medzery sa zachovajú)."""
        sec = _get(self.data, section)
        last = 0
        for i in range(1, maxn + 1):
            if any(str(sec.get(p.format(i=i), '')).strip() for p, _l, _t in patterns):
                last = i
        return [{p: sec.get(p.format(i=i), '') for p, _l, _t in patterns} for i in range(1, last + 1)]

    def set_indexed(self, section, group, items):
        self.pending_indexed[(section, group)] = items

    def get_indexed(self, section, group, patterns, maxn):
        if (section, group) in self.pending_indexed:
            return self.pending_indexed[(section, group)]
        return self.indexed_items(section, patterns, maxn)

    # ------------------------------------------------------------ tabuľky kľúč -> hodnota
    def get_map(self, section):
        if section in self.pending_maps:
            return dict(self.pending_maps[section])
        return {k: v for k, v in _get(self.data, section).items() if isinstance(v, str)}

    def set_map(self, section, d):
        self.pending_maps[section] = dict(d)

    # ------------------------------------------------------------ polia tabuliek ([[menu]], [[custom]], pravidlá)
    def get_array(self, aid):
        """Pracovná kópia záznamov [{'src','fields','children'}] (neuložená, ak už bola upravená)."""
        if aid in self.pending_arrays:
            return self.pending_arrays[aid]
        a = schema.ARRAYS[aid]
        child = (a['child'][0], a['child'][1]) if a['child'] else None
        recs = arrays.read_records(self.text, a['family'], a['specs'], child)
        self.pending_arrays[aid] = recs
        self._array_orig[aid] = copy.deepcopy(recs)
        return recs

    def array_dirty(self, aid):
        return aid in self.pending_arrays and self.pending_arrays[aid] != self._array_orig.get(aid)

    # ------------------------------------------------------------ neznáme kľúče
    def unknown(self):
        """[(sekcia, kľúč, hodnota)] pre kľúče, ktoré schéma nepozná (nezahodia sa, len sa ukážu)."""
        known = set(self.schema)
        idx = {}
        for sec, _g, pats, maxn, *_ in schema.INDEXED:
            idx.setdefault(sec, []).extend(pats)
        maps = {m[0] for m in schema.MAPS}
        out = []

        def walk(path, t):
            for k, v in t.items():
                full = '.'.join(path)
                if isinstance(v, dict):
                    if '.'.join(path + [k]) in maps:
                        continue
                    walk(path + [k], v)
                elif isinstance(v, list) and v and all(isinstance(x, dict) for x in v):
                    continue   # pole tabuliek ([[menu]], [[custom]], …)
                else:
                    if (full, k) in known:
                        continue
                    if any(re.fullmatch(re.sub(r'\\\{i\\\}', r'\\d+', re.escape(p)), k) for p, _l, _t in idx.get(full, [])):
                        continue
                    if full in ('', ) and k in ('modules',):
                        continue
                    out.append((full, k, v))
        walk([], self.data)
        return out

    # ------------------------------------------------------------ zápis
    def dirty(self):
        return bool(self.pending or self.pending_indexed or self.pending_maps or any(self.array_dirty(a) for a in self.pending_arrays))

    def discard(self):
        self.pending.clear(); self.pending_indexed.clear(); self.pending_maps.clear(); self.pending_arrays.clear()

    def _apply(self, doc):
        for (sec, key), val in self.pending.items():
            if val is RESET:
                doc.remove(sec, key)
            else:
                doc.set(sec, key, val)
        for (sec, group), items in self.pending_indexed.items():
            g = next(x for x in schema.INDEXED if x[0] == sec and x[1] == group)
            pats, maxn = g[2], g[3]
            for i in range(1, maxn + 1):
                item = items[i - 1] if i <= len(items) else None
                for p, _l, _t in pats:
                    key = p.format(i=i)
                    raw = (item or {}).get(p, '')
                    if raw != '' and raw is not None:
                        doc.set(sec, key, raw)
                    elif doc.raw(sec, key) is not None:
                        doc.remove(sec, key)
        for sec, d in self.pending_maps.items():
            old = {k for k, v in _get(self.data, sec).items() if isinstance(v, str)}
            for k in old - set(d):
                doc.remove(sec, k, comment_out=False)
            for k, v in d.items():
                doc.set(sec, k, v)

    def preview(self):
        """Text súboru po použití zmien (nič nezapíše)."""
        doc = TomlDoc(self.text)
        self._apply(doc)
        text = doc.text()
        for aid, recs in self.pending_arrays.items():
            if not self.array_dirty(aid):
                continue
            a = schema.ARRAYS[aid]
            text = '\n'.join(arrays.apply_family(text.split('\n'), a['family'], recs, a['specs'], a['child']))
        return text

    def save(self, keep_backups=5):
        """Zapíše zmeny (s kontrolou a zálohou). Vráti cestu k zálohe alebo None."""
        new = self.preview()
        tomllib.loads(new)   # kontrola: chybná syntax vyhodí výnimku a nič sa nezapíše
        os.makedirs(os.path.dirname(self.path) or '.', exist_ok=True)
        backup = None
        if os.path.exists(self.path):
            backup = f"{self.path}.bak-{time.strftime('%Y%m%d-%H%M%S')}"
            shutil.copy2(self.path, backup)
            self._prune(keep_backups)
        tmp = self.path + '.tmp'
        with open(tmp, 'w', encoding='utf8') as f:
            f.write(new)
        os.replace(tmp, self.path)
        self.load()
        return backup

    def _prune(self, keep):
        d, base = os.path.split(self.path)
        names = sorted(n for n in os.listdir(d or '.') if n.startswith(base + '.bak-'))
        for n in names[:-keep] if keep else names:
            try:
                os.remove(os.path.join(d, n))
            except OSError:
                pass
