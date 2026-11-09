"""Úprava polí tabuliek ([[menu]] + [[menu.item]], [[custom]], [[notifications.rule]]) so zachovaním komentárov.

Každý blok (hlavička + kľúče + komentáre tesne nad hlavičkou) sa pri úprave berie ako celok: zmenené kľúče sa
prepíšu priamo v jeho riadkoch, nové bloky sa pridajú za posledný, zmazané odstránia, a pri zmene poradia sa bloky
vymenia medzi pôvodnými pozíciami.
"""
import re, tomllib

from tomledit import TomlDoc, format_value

_ANY_HDR = re.compile(r'^\s*\[\[?\s*([^\]]+?)\s*\]\]?\s*(#.*)?$')


def _headers(lines):
    out = []
    for i, l in enumerate(lines):
        m = _ANY_HDR.match(l)
        if m:
            out.append((i, m.group(1).strip(), l.lstrip().startswith('[[')))
    return out


def _attach_start(lines, i):
    j = i
    while j > 0 and lines[j - 1].lstrip().startswith('#'):
        j -= 1
    return j


def family_blocks(lines, name):
    """[(start, end, header_idx)] pre všetky `[[name]]`; blok zahŕňa jeho podtabuľky `name.*`."""
    hs = _headers(lines)
    out = []
    for k, (i, n, arr) in enumerate(hs):
        if not (arr and n == name):
            continue
        end = len(lines)
        for (j, n2, _a2) in hs[k + 1:]:
            if n2 == name or not n2.startswith(name + '.'):
                end = _attach_start(lines, j)
                break
        while end > i + 1 and lines[end - 1].strip() == '':
            end -= 1
        out.append((_attach_start(lines, i), end, i))
    return out


def _get_path(data, name):
    cur = data
    for p in name.split('.'):
        cur = cur.get(p) if isinstance(cur, dict) else None
        if cur is None:
            return []
    return cur if isinstance(cur, list) else []


def read_records(text, name, specs, child=None):
    """[{'src': i, 'fields': {...}, 'children': [...]}] zo súboru (len kľúče zo `specs`)."""
    data = tomllib.loads(text)
    arr = _get_path(data, name)
    recs = []
    for i, t in enumerate(arr):
        rec = {'src': i, 'fields': {k: t[k] for k in specs if k in t}}
        if child:
            cname, cspecs = child
            rec['children'] = [{'src': j, 'fields': {k: c[k] for k in cspecs if k in c}} for j, c in enumerate(t.get(cname.split('.')[-1], []))]
        recs.append(rec)
    return recs


def _edit_keys(block_lines, hdr_rel, fields, specs, original_present):
    """Upraví kľúče v riadkoch bloku (len časť pred podtabuľkami). Vráti nové riadky."""
    lines = list(block_lines)
    saved = lines[hdr_rel]
    lines[hdr_rel] = '[b]'
    doc = TomlDoc('\n'.join(lines))
    for key, spec in specs.items():
        has = doc.raw('b', key) is not None
        if key in fields:
            val = fields[key]
            if has:
                try:
                    same = tomllib.loads(f'v = {doc.raw("b", key)}')['v'] == val
                except Exception:  # noqa: BLE001
                    same = False
                if not same:
                    doc.set('b', key, val)
            elif val != spec.get('default') or spec.get('write_default'):
                doc.set('b', key, val)
        elif has:
            doc.remove('b', key, comment_out=False)
    out = doc.text().split('\n')
    k = out.index('[b]')
    out[k] = saved
    return out


def _new_block(name, fields, specs):
    lines = [f'[[{name}]]']
    for key, spec in specs.items():
        if key in fields and (fields[key] != spec.get('default') or spec.get('write_default')):
            lines.append(f'{key} = {format_value(fields[key])}')
    return lines


def apply_family(lines, name, recs, specs, child=None, sep_blank=True):
    """Vráti nové riadky súboru po zosúladení poľa tabuliek `name` s `recs`."""
    blocks = family_blocks(lines, name)

    def render(rec):
        src = rec.get('src')
        if src is None or src >= len(blocks):
            body = _new_block(name, rec['fields'], specs)
            if child:
                cname, cspecs = child
                body = apply_family(body, cname, rec.get('children', []), cspecs, None, sep_blank=False)
            return body
        start, end, hdr = blocks[src]
        blk = lines[start:end]
        rel = hdr - start
        kids = family_blocks(blk, child[0]) if child else []
        first_kid = kids[0][0] if kids else len(blk)
        head = _edit_keys(blk[:first_kid], rel, rec['fields'], specs, True)
        rest = blk[first_kid:]
        if child:
            rest = apply_family(rest, child[0], rec.get('children', []), child[1], None, sep_blank=False) if rest else \
                apply_family([], child[0], rec.get('children', []), child[1], None, sep_blank=False)
            # `apply_family([])` pridá bloky od začiatku – žiadny úvodný prázdny riadok
            while rest and rest[0] == '':
                rest = rest[1:]
        return head + rest

    rendered = [render(r) for r in recs]
    out = list(lines)
    if blocks:
        deleted = len(recs) < len(blocks)
        for k in range(len(blocks) - 1, -1, -1):
            start, end, _h = blocks[k]
            out[start:end] = rendered[k] if k < len(rendered) else []
        extra = rendered[len(blocks):]
        if extra:
            # za posledný blok (pozícia sa mohla posunúť, preto sa nájde znova)
            nb = family_blocks(out, name)
            pos = nb[-1][1] if nb else len(out)
            ins = []
            for r in extra:
                if sep_blank:
                    ins.append('')
                ins += r
            out[pos:pos] = ins
        if deleted:
            # zmazané bloky nenechajú po sebe dva prázdne riadky
            cleaned = []
            for l in out:
                if l == '' and cleaned and cleaned[-1] == '':
                    continue
                cleaned.append(l)
            out = cleaned
    else:
        while out and out[-1].strip() == '':
            out.pop()
        for r in rendered:
            if sep_blank or out:
                if sep_blank:
                    out.append('')
            out += r
    return out
