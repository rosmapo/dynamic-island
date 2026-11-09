"""Úprava TOML súboru po riadkoch so zachovaním komentárov, poradia a formátovania.

Podporuje to, čo používa config.toml ostrovčeka pre skalárne kľúče a polia reťazcov v tabuľkách `[sekcia]` / `[a.b]`
a v koreni súboru. Polia tabuliek (`[[menu]]`) nechá nedotknuté (adresujú sa len cez `set_array_table`, neskôr).
"""
import re

_HDR = re.compile(r'^\s*(\[\[?)\s*([^\]]+?)\s*\]\]?\s*(#.*)?$')
_KEY = re.compile(r'^(\s*)([A-Za-z0-9_\-]+|"[^"]*")(\s*=\s*)')


def format_value(v):
    """Python hodnota -> TOML text."""
    if isinstance(v, bool):
        return 'true' if v else 'false'
    if isinstance(v, int):
        return str(v)
    if isinstance(v, float):
        s = repr(v)
        return s[:-2] if s.endswith('.0') and False else s
    if isinstance(v, str):
        out = ['"']
        for ch in v:
            if ch == '"':
                out.append('\\"')
            elif ch == '\\':
                out.append('\\\\')
            elif ch == '\n':
                out.append('\\n')
            elif ch == '\t':
                out.append('\\t')
            elif ch == '\r':
                out.append('\\r')
            elif ord(ch) < 0x20:
                out.append('\\u%04x' % ord(ch))
            else:
                out.append(ch)
        out.append('"')
        return ''.join(out)
    if isinstance(v, (list, tuple)):
        return '[' + ', '.join(format_value(x) for x in v) + ']'
    raise TypeError(f'nepodporovaný typ {type(v)}')


def _scan_value_end(line, start, depth=0):
    """Nájde koniec hodnoty na riadku od `start`: (index_konca, hĺbka_poľa). Hĺbka > 0 = pokračuje na ďalšom riadku."""
    i, n = start, len(line)
    quote = None
    while i < n:
        c = line[i]
        if quote:
            if quote == '"' and c == '\\':
                i += 2
                continue
            if c == quote:
                quote = None
        else:
            if c in '"\'':
                quote = c
            elif c == '[':
                depth += 1
            elif c == ']':
                depth -= 1
            elif c == '#':
                break
        i += 1
    # koniec hodnoty = pred komentárom a koncovými medzerami
    end = i
    while end > start and line[end - 1] in ' \t':
        end -= 1
    return end, depth


class TomlDoc:
    def __init__(self, text):
        self.lines = text.split('\n')
        # súbor končiaci koncom riadku dáva posledný prázdny prvok – zachová sa
        self.reload_index()

    # -------------------------------------------------------------- index
    def reload_index(self):
        """Pre každú tabuľku: {(názov, je_pole): (riadok_hlavičky, koniec_exkl)}; kľúče: (sekcia,kľúč) -> (start, end_excl, val_col)."""
        self.sections = []  # (name, is_array, header_line)
        cur = ('', False, -1)
        self.sections.append(cur)
        self.keys = {}
        self.commented = {}
        self.last_key_line = {}
        i = 0
        n = len(self.lines)
        sec_name, is_arr = '', False
        while i < n:
            line = self.lines[i]
            m = _HDR.match(line)
            if m:
                sec_name, is_arr = m.group(2).strip(), m.group(1) == '[['
                self.sections.append((sec_name, is_arr, i))
                self.last_key_line.setdefault((sec_name, is_arr), i)
                i += 1
                continue
            km = _KEY.match(line)
            if km and not line.lstrip().startswith('#'):
                key = km.group(2).strip('"')
                vstart = km.end()
                end, depth = _scan_value_end(line, vstart)
                last = i
                while depth > 0 and last + 1 < n:
                    last += 1
                    end, depth = _scan_value_end(self.lines[last], 0, depth)
                if not is_arr:
                    self.keys.setdefault((sec_name, key), (i, last + 1, vstart))
                self.last_key_line[(sec_name, is_arr)] = last
                i = last + 1
                continue
            cm = re.match(r'^\s*#+\s*([A-Za-z0-9_\-]+)\s*=', line)
            if cm and not is_arr:
                self.commented.setdefault((sec_name, cm.group(1)), i)
                # komentovaný kľúč tiež kotví vkladanie nových kľúčov
                self.last_key_line[(sec_name, is_arr)] = max(self.last_key_line.get((sec_name, is_arr), -1), i)
            i += 1

    def has_section(self, section):
        return any(s == section and not a for s, a, _ in self.sections)

    def raw(self, section, key):
        """Surový text hodnoty (bez komentára) alebo None."""
        e = self.keys.get((section, key))
        if not e:
            return None
        start, stop, col = e
        if stop - start == 1:
            line = self.lines[start]
            end, _ = _scan_value_end(line, col)
            return line[col:end]
        parts = [self.lines[start][col:]] + self.lines[start + 1:stop]
        return '\n'.join(parts)

    # -------------------------------------------------------------- zmeny
    def set(self, section, key, value):
        text = format_value(value)
        e = self.keys.get((section, key))
        if e:
            start, stop, col = e
            first = self.lines[start]
            if stop - start == 1:
                end, _ = _scan_value_end(first, col)
                self.lines[start] = first[:col] + text + first[end:]
            else:
                # viacriadková hodnota: nahradí sa jedným riadkom (komentár za poslednou zátvorkou sa zachová)
                last = self.lines[stop - 1]
                end, _ = _scan_value_end(last, 0, 1)
                tail = last[end:]
                self.lines[start:stop] = [first[:col] + text + tail]
        else:
            new = f'{key} = {text}'
            self._insert(section, key, new)
        self.reload_index()

    def _insert(self, section, key, line):
        c = self.commented.get((section, key))
        if c is not None:
            self.lines.insert(c + 1, line)
            return
        anchor = self.last_key_line.get((section, False))
        if anchor is not None and self.has_section(section) or (section == '' and anchor is not None):
            self.lines.insert(anchor + 1, line)
            return
        if section == '':
            # koreň: pred prvou hlavičkou
            pos = next((i for i, l in enumerate(self.lines) if _HDR.match(l)), len(self.lines))
            self.lines.insert(pos, line)
            return
        # nová sekcia na konci súboru
        while self.lines and self.lines[-1].strip() == '':
            self.lines.pop()
        self.lines += ['', f'[{section}]', line, '']

    def remove(self, section, key, comment_out=True):
        """Odstráni kľúč. S `comment_out` ho nahradí komentárom (ak taký riadok ešte nie je), aby zostala dokumentácia."""
        e = self.keys.get((section, key))
        if not e:
            return False
        start, stop, col = e
        if comment_out and (section, key) not in self.commented:
            self.lines[start:stop] = ['# ' + l for l in self.lines[start:stop]]
        else:
            del self.lines[start:stop]
        self.reload_index()
        return True

    def text(self):
        return '\n'.join(self.lines)
