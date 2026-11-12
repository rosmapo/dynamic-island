"""Schéma nastavení: spája automaticky vygenerované kľúče (schema_gen.json z kódu ostrovčeka: predvolené hodnoty a
rozsahy) s ručne písanými slovenskými názvami, voľbami a špeciálnymi (indexovanými) kľúčmi.

Typy: bool | int | float | str | color | path | command | icon | enum | strlist | numlist | font | modules
"""
import json, os

HERE = os.path.dirname(os.path.abspath(__file__))

# skupiny v navigácii: (názov, [(sekcia, názov stránky), ...]); poradie ako v config.toml
GROUPS = [
    ('Vzhľad', [('', 'Poradie modulov'), ('island', 'Ostrovček'), ('colors', 'Farby')]),
    ('Kontajner a spúšťač', [('quickset', 'Kontajner (quickset)'), ('drun', 'Spúšťač aplikácií (drun)')]),
    ('Vlastné', [('menu', 'Vlastné menu'), ('custom', 'Vlastné moduly')]),
    ('Moduly lišty', [('clock', 'Hodiny'), ('network', 'Sieť'), ('vpn', 'VPN'), ('keyboard', 'Klávesnica'),
                      ('audio', 'Zvuk'), ('media', 'Práve hrá'), ('radio', 'Rádio'), ('recorder', 'Nahrávanie (REC)'),
                      ('inhibitor', 'Zákaz uspania'), ('capslock', 'Caps Lock'), ('disks', 'Externé disky')]),
    ('Panely a nástroje', [('clipboard', 'Schránka'), ('trash', 'Kôš'), ('wallpaper', 'Tapety'), ('wifi', 'Wi-Fi'),
                           ('bluetooth', 'Bluetooth'), ('monitor', 'Monitor'), ('capture', 'Snímanie obrazovky')]),
    ('HUD a systém', [('brightness', 'Jas'), ('system', 'Systém'), ('autohide', 'Automatické skrývanie')]),
    ('Notifikácie', [('notifications', 'Notifikácie'), ('notifications.urgent', 'Urgentné notifikácie'), ('notifications.rule', 'Pravidlá notifikácií')]),
]

# bežné názvy kľúčov
GENERIC = {
    'font_size': 'Veľkosť písma', 'rows': 'Počet viditeľných riadkov', 'columns': 'Počet stĺpcov', 'width': 'Šírka',
    'height': 'Výška', 'radius': 'Zaoblenie rohov', 'opacity': 'Krytie pozadia', 'padding': 'Odsadenie', 'gap': 'Medzera',
    'icon': 'Ikona', 'icon_size': 'Veľkosť ikony', 'on_click': 'Ľavý klik', 'on_click_right': 'Pravý klik',
    'on_click_middle': 'Stredný klik', 'tooltip': 'Tooltip', 'enabled': 'Zapnuté', 'step': 'Krok',
    'editor_command': 'Príkaz tlačidla správcu', 'editor_icon': 'Ikona tlačidla správcu', 'editor_label': 'Text tlačidla správcu',
    'text': 'Text', 'file': 'Súbor', 'notify': 'Posielať notifikácie', 'timeout_ms': 'Čas zobrazenia (ms)',
    'text_size': 'Veľkosť písma', 'hover': 'Zvýraznenie pod kurzorom', 'search': 'Zobraziť pole hľadania',
}

# (sekcia, kľúč) -> názov
LABELS = {
    ('island', 'font_scale'): 'Násobiteľ všetkých písem', ('island', 'font_size'): 'Základná veľkosť písma',
    ('island', 'height'): 'Výška kapsuly', ('island', 'margin_top'): 'Odstup od horného okraja',
    ('island', 'radius'): 'Zaoblenie kapsuly', ('island', 'radius_expanded'): 'Zaoblenie rozbaleného panela',
    ('island', 'shadow'): 'Tieň', ('island', 'window_animation'): 'Animácia otvorenia okna (drun a panely)', ('island', 'surface_width'): 'Šírka priehľadného povrchu',
    ('island', 'surface_height'): 'Výška priehľadného povrchu', ('island', 'tooltip_delay_ms'): 'Oneskorenie tooltipu (ms)',
    ('island', 'collapse_delay_ms'): 'Oneskorenie zbalenia (ms)', ('island', 'poll_ms'): 'Kontrola stavu (ms)',
    ('island', 'colors_file'): 'Súbor s farbami (matugen)', ('island', 'font_family'): 'Písmo (zoznam, záložné za čiarkou)',
    ('island', 'icon_family'): 'Písmo ikon (Nerd Font)',
    ('drun', 'show'): 'Čo zobraziť v bunke', ('drun', 'name_size'): 'Veľkosť názvu', ('drun', 'cell_width'): 'Šírka bunky (0 = auto)',
    ('drun', 'cell_height'): 'Výška bunky (0 = auto)', ('drun', 'cell_radius'): 'Zaoblenie zvýraznenej bunky',
    ('drun', 'radius'): 'Zaoblenie okna', ('drun', 'opacity'): 'Krytie okna (−1 = ako ostrovček)',
    ('drun', 'order'): 'Zoraďovanie', ('drun', 'terminal'): 'Terminál pre aplikácie s Terminal=true',
    ('drun', 'icon_theme'): 'Téma ikon (prázdne = podľa GTK)', ('drun', 'desktop'): 'Pracovné prostredie (OnlyShowIn)',
    ('drun', 'autoclose'): 'Zatvoriť odchodom myši (aj panely v strede)',
    ('clock', 'format'): 'Formát času (strftime)', ('clock', 'locale'): 'Jazyk (locale)',
    ('network', 'interface'): 'Sieťové rozhranie', ('network', 'icons'): 'Ikony sily signálu (od najslabšej)',
    ('network', 'icon_ethernet'): 'Ikona ethernetu', ('network', 'icon_disconnected'): 'Ikona nepripojené',
    ('network', 'tooltip_wifi'): 'Tooltip – Wi-Fi', ('network', 'tooltip_ethernet'): 'Tooltip – ethernet',
    ('network', 'tooltip_disconnected'): 'Tooltip – nepripojené',
    ('vpn', 'interfaces'): 'Rozhrania VPN (vzory, napr. tun*)', ('vpn', 'text'): 'Text odznaku',
    ('audio', 'icon_muted'): 'Ikona stlmené', ('audio', 'icons'): 'Ikony hlasitosti (od najtichšej)', ('audio', 'hud'): 'Zobrazovať HUD hlasitosti',
    ('audio', 'hud_ms'): 'Trvanie HUD (ms)', ('audio', 'step'): 'Krok hlasitosti (%)', ('audio', 'limit'): 'Strop hlasitosti (1.0 = 100 %)',
    ('audio', 'panel_font_size'): 'Panel – veľkosť písma', ('audio', 'panel_width'): 'Panel – šírka', ('audio', 'panel_rows'): 'Panel – riadkov',
    ('media', 'icon_paused'): 'Ikona pozastavené', ('media', 'ignore'): 'Ignorované prehrávače', ('media', 'panel_width'): 'Šírka panela',
    ('media', 'art_size'): 'Veľkosť obalu',
    ('inhibitor', 'file'): 'Súbor, ktorého existencia zapína ikonu',
    ('disks', 'format'): 'Formát (jeden disk)', ('disks', 'format_many'): 'Formát (viac diskov)', ('disks', 'automount'): 'Automaticky pripájať médiá',
    ('disks', 'notify_timeout_ms'): 'Čas notifikácie (ms)',
    ('clipboard', 'max_items'): 'Najviac položiek', ('clipboard', 'list_command'): 'Príkaz na zoznam histórie',
    ('clipboard', 'copy_command'): 'Príkaz na skopírovanie', ('clipboard', 'search'): 'Pole hľadania v okne --clip',
    ('clipboard', 'window_radius'): 'Zaoblenie okna --clip',
    ('trash', 'confirm_empty'): 'Potvrdzovať vysypanie', ('trash', 'show_location'): 'Zobraziť pôvodné umiestnenie', ('trash', 'show_date'): 'Zobraziť dátum vymazania',
    ('wallpaper', 'directory'): 'Adresár s obrázkami', ('wallpaper', 'thumb_width'): 'Šírka náhľadu', ('wallpaper', 'command'): 'Skript na nastavenie tapety',
    ('wallpaper', 'cache_dir'): 'Adresár cache', ('wallpaper', 'themes_directory'): 'Adresár tém (*.json)', ('wallpaper', 'theme_width'): 'Šírka ponuky tém',
    ('wallpaper', 'hover_ring'): 'Rámik pod kurzorom', ('wallpaper', 'current_ring'): 'Rámik aktuálnej tapety',
    ('wallpaper', 'button_max_width'): 'Najväčšia šírka tlačidla (0 = bez limitu)',
    ('bluetooth', 'scan_ms'): 'Trvanie skenovania (ms)',
    ('monitor', 'bus'): 'I²C bus (ddcutil)', ('monitor', 'presets'): 'Presety teploty (VCP 0x14)', ('monitor', 'preset_labels'): 'Popisy presetov',
    ('monitor', 'inputs'): 'Vstupy (VCP 0x60)', ('monitor', 'input_labels'): 'Popisy vstupov',
    ('monitor', 'night_switcher'): 'Skript nočného režimu', ('monitor', 'night_state'): 'Príkaz stavu nočného režimu', ('monitor', 'night_label'): 'Popis nočného režimu',
    ('capture', 'mode'): 'Predvolená oblasť', ('capture', 'kind'): 'Predvolený typ', ('capture', 'format'): 'Formát obrázka', ('capture', 'video_ext'): 'Prípona videa',
    ('capture', 'dir'): 'Adresár snímok', ('capture', 'video_dir'): 'Adresár videí', ('capture', 'name'): 'Vzor názvu súboru (strftime)',
    ('capture', 'cursor'): 'Kurzor na snímke', ('capture', 'audio'): 'Zvuk pri nahrávaní', ('capture', 'slurp_args'): 'Argumenty slurp (vzhľad výberu)',
    ('capture', 'recorder_args'): 'Ďalšie argumenty wf-recorder', ('capture', 'copy'): 'Kopírovať snímku do schránky',
    ('capture', 'hide_bar'): 'Skryť ostrovček pri snímke', ('capture', 'settle_ms'): 'Pauza po skrytí (ms)', ('capture', 'delays'): 'Ponúkané časovače (s)',
    ('brightness', 'get'): 'Príkaz na zistenie jasu', ('brightness', 'set'): 'Príkaz na nastavenie jasu ({value})', ('brightness', 'icons'): 'Ikony jasu',
    ('system', 'decimal'): 'Desatinný oddeľovač', ('system', 'net_interface'): 'Rozhranie pre net-down / net-up',
    ('autohide', 'bar_height'): 'Výška lišty', ('autohide', 'edge'): 'Šírka hrany pre odkrytie', ('autohide', 'edge_width'): 'Šírka zóny pri hrane',
    ('autohide', 'margin'): 'Okraj', ('autohide', 'poll_ms'): 'Kontrola kurzora (ms)', ('autohide', 'ignore'): 'Ignorované okná',
    ('notifications', 'history'): 'Ukladať históriu', ('notifications', 'history_max'): 'Najviac záznamov v histórii', ('notifications', 'history_file'): 'Súbor histórie',
    ('notifications', 'width'): 'Šírka', ('notifications', 'max_visible'): 'Najviac viditeľných', ('notifications', 'max_lines'): 'Najviac riadkov tela',
    ('notifications', 'icons'): 'Ikony notifikácií', ('notifications', 'panel_width'): 'Panel histórie – šírka', ('notifications', 'panel_height'): 'Panel histórie – výška',
    ('notifications', 'panel_lines'): 'Panel histórie – riadkov tela', ('notifications', 'align'): 'Zarovnanie', ('notifications', 'timeout_ms'): 'Čas zobrazenia (ms, 0 = nevyprší)',
    ('notifications.urgent', 'timeout_ms'): 'Čas zobrazenia urgentných (ms)', ('notifications.urgent', 'border_size'): 'Hrúbka rámika',
    ('quickset', 'tab_gap'): 'Medzera medzi záložkami', ('quickset', 'tabs_position'): 'Poloha lišty', ('quickset', 'tab_size'): 'Veľkosť tlačidla záložky',
    ('quickset', 'inversetab_gap'): 'Medzera medzi tlačidlami druhej lišty (0 = ako pri záložkách)', ('quickset', 'preload'): 'Načítať údaje monitora vopred',
    ('quickset', 'inversetab'): 'Druhá lišta (inversetab)', ('quickset', 'width'): 'Celková šírka (px, prázdne = auto)', ('quickset', 'height'): 'Celková výška (px, prázdne = auto)',
}

# výber z možností
CHOICES = {
    ('drun', 'show'): ['icon', 'name', 'icon+name-right', 'icon+name-below'], ('drun', 'order'): ['horizontal', 'vertical'],
    ('capture', 'mode'): ['screen', 'window', 'region'], ('capture', 'kind'): ['image', 'video'],
    ('capture', 'format'): ['png', 'jpeg'], ('capture', 'video_ext'): ['mp4', 'mkv', 'webm'],
    ('quickset', 'tabs_position'): ['left', 'right', 'top', 'bottom'], ('notifications', 'align'): ['center', 'left'],
    ('system', 'decimal'): [',', '.'],
}

TYPE_OVERRIDE = {('island', 'colors_file'): 'path', ('wallpaper', 'directory'): 'path', ('wallpaper', 'cache_dir'): 'path',
                 ('wallpaper', 'themes_directory'): 'path', ('wallpaper', 'command'): 'command', ('capture', 'dir'): 'path',
                 ('capture', 'video_dir'): 'path', ('notifications', 'history_file'): 'path',
                 ('inhibitor', 'file'): 'path', ('clipboard', 'list_command'): 'command', ('clipboard', 'copy_command'): 'command',
                 ('monitor', 'night_switcher'): 'command', ('monitor', 'night_state'): 'command',
                 ('brightness', 'get'): 'command', ('brightness', 'set'): 'command', ('drun', 'terminal'): 'command',
                 ('drun', 'opacity'): 'float', ('island', 'opacity'): 'float', ('audio', 'limit'): 'float',
                 ('quickset', 'inversetab_gap'): 'int'}

# kľúče, ktoré kód číta inak než cez `*_or` (v schema_gen.json nie sú)
EXTRA = [
    dict(section='', key='modules', type='modules', default=['network', 'vpn', 'clock', 'keyboard', 'audio', 'radio', 'media', 'capslock', 'inhibitor', 'recorder', 'disks'],
         label='Poradie modulov v lište', desc='Poradie zľava doprava; môžeš pridať názvy vlastných modulov ([[custom]]).'),
    dict(section='island', key='font_family', type='strlist', default=['Inter'], desc='Prvé nájdené písmo sa použije; ďalšie sú záložné.'),
    dict(section='island', key='icon_family', type='strlist', default=['Symbols Nerd Font Mono']),
    dict(section='quickset', key='inversetab', type='bool', default=False, desc='Druhá lišta na opačnej strane; jej tlačidlá neotvárajú panel v kontajneri.'),
    dict(section='quickset', key='width', type='int', default=None, min=120, max=3000, nullable=True),
    dict(section='quickset', key='height', type='int', default=None, min=120, max=3000, nullable=True),
    dict(section='network', key='icons', type='strlist', default=['\U000f091f', '\U000f0922', '\U000f0925', '\U000f0928'], desc='Ikony od najslabšieho po najsilnejší signál.', icon_list=True),
    dict(section='network', key='tooltip_wifi', type='str', default='SSID: {ssid}\nRozhranie: {ifname}\nIP adresa: {ip}\nSila signálu: {signal}% ({dbm} dBm)\nFrekvencia: {freq} GHz',
         desc='Zástupné symboly: {ssid} {ifname} {ip} {gateway} {signal} {dbm} {freq}; \\n = nový riadok.'),
    dict(section='audio', key='icons', type='strlist', default=['', '', ''], icon_list=True),
    dict(section='brightness', key='icons', type='strlist', default=[], icon_list=True, desc='Prázdne = predvolené ikony.'),
    dict(section='monitor', key='presets', type='numlist', default=[5, 6, 8], desc='VCP hodnoty teplotného presetu (0x14); desiatkovo.'),
    dict(section='monitor', key='inputs', type='numlist', default=[15, 17], desc='VCP hodnoty vstupu (0x60): 15 = DisplayPort, 17 = HDMI.'),
    dict(section='capture', key='delays', type='numlist', default=[0, 3, 5, 10]),
] + [dict(section='colors', key=k, type='color', default=d, label=l, desc=dd) for k, d, l, dd in [
    ('background', '@surface', 'Pozadie', 'Hodnota „@meno“ z colors_file alebo „#rrggbb“.'),
    ('foreground', '@primary', 'Text', ''), ('accent', '@primary', 'Zvýraznenie', ''), ('on_accent', '@surface', 'Text na zvýraznení', ''),
    ('menu_selected', '', 'Vybraná položka – pozadie', 'Prázdne = ako zvýraznenie.'), ('menu_selected_text', '', 'Vybraná položka – text', 'Prázdne = ako text na zvýraznení.'),
    ('alert', '#dc2f2f', 'Výstraha', ''), ('on_alert', '#ffffff', 'Text na výstrahe', ''),
    ('urgent_border', '#fa3939', 'Rámik urgentnej notifikácie', ''), ('urgent_text', '@primary', 'Text urgentnej notifikácie', '')]]

# doplnené hranice (kód ich nekontroluje, ale hodnoty mimo nedávajú zmysel)
BOUNDS = {('island', 'opacity'): (0.0, 1.0), ('drun', 'opacity'): (-1.0, 1.0), ('audio', 'limit'): (0.1, 2.0)}

# indexované kľúče: (sekcia, názov skupiny, [(vzor kľúča, názov poľa, typ)], najviac)
INDEXED = [
    ('quickset', 'Záložky kontajnera', [('tab{i}', 'Panel', 'tab'), ('tab{i}_icon', 'Ikona', 'icon')], 8,
     'Panel: audio, bluetooth, wifi, monitor, clipboard, trash, notifications, wallpaper, capture alebo menu:NÁZOV.'),
    ('quickset', 'Tlačidlá druhej lišty', [('inversetab{i}', 'Príkaz / panel', 'command'), ('inversetab{i}_icon', 'Ikona', 'icon')], 8,
     'Príkaz alebo akcia (napr. menu:capture, wifi, systemctl suspend).'),
    ('wallpaper', 'Tlačidlá v hlavičke tapiet', [('button{i}_label', 'Text', 'str'), ('button{i}_command', 'Príkaz', 'command')], 16,
     'Tlačidlo sa zobrazí len ak má text.'),
]
# tabuľka kľúč -> hodnota: (sekcia, názov, popis)
MAPS = [('keyboard.names', 'Názvy rozložení klávesnice', 'Pôvodný názov (napr. Slovak) -> zobrazený text (SK).')]


def _f(key, type_, label, default=None, desc='', **kw):
    d = dict(key=key, type=type_, label=label, default=default, desc=desc, min=None, max=None)
    d.update(kw)
    return d


def _specs(*fields):
    return {f['key']: f for f in fields}


MENU_SPECS = _specs(
    _f('name', 'str', 'Názov menu', '', 'Odkazuje sa naň on_click = "názov" alebo záložka tab = "menu:názov".', write_default=True),
    _f('title', 'str', 'Nadpis', '', 'Prázdne = bez nadpisu.'),
    _f('type', 'enum', 'Typ', '', 'Automaticky podľa položiek (ikony / text / zmiešané).', choices=['', 'icons', 'text', 'hybrid'],
       choice_labels={'': 'automaticky', 'icons': 'ikony', 'text': 'text', 'hybrid': 'zmiešané'}),
    _f('columns', 'int', 'Stĺpce (spoločné)', 3, 'Záloha pre icon_columns a text_columns.', min=1),
    _f('icon_columns', 'int', 'Stĺpce ikon', None, '0 = podľa spoločných stĺpcov.', min=1, nullable=True),
    _f('text_columns', 'int', 'Stĺpce textu', None, '0 = podľa spoločných stĺpcov (pri čistom texte 1).', min=1, nullable=True),
    _f('rows', 'int', 'Najviac riadkov', None, '0 = všetky; viac položiek sa posúva kolieskom.', min=1, nullable=True),
    _f('icon_size', 'int', 'Veľkosť ikony', 28, min=8), _f('text_size', 'int', 'Veľkosť písma', 15, min=8),
    _f('cell_width', 'int', 'Šírka bunky', 120, min=20), _f('gap', 'int', 'Medzera', 8, min=0), _f('padding', 'int', 'Odsadenie', 12, min=0),
    _f('separator_thickness', 'float', 'Hrúbka oddeľovača', 1.5, min=0.5, max=20.0),
    _f('separator_length', 'float', 'Dĺžka oddeľovača (0.1–1)', 0.6, min=0.1, max=1.0),
    _f('wait_exec', 'bool', 'Počkať na výstupy pred otvorením', False, 'Menu sa otvorí, až keď sa načítajú hodnoty položiek s exec.'),
    _f('wait_timeout_ms', 'int', 'Najdlhšie čakanie (ms)', 1500, min=100, max=30000),
)
ITEM_SPECS = _specs(
    _f('separator', 'enum', 'Oddeľovač', '', 'Vodorovná čiara namiesto položky; poloha čiary v riadku.', choices=['', 'top', 'center', 'bottom'],
       choice_labels={'': 'nie je oddeľovač', 'top': 'čiara hore', 'center': 'čiara v strede', 'bottom': 'čiara dole'}),
    _f('line', 'str', 'Riadok (text na celú šírku)', '', 'Namiesto položky: text cez celú šírku menu ({} = výstup exec). Prázdny riadok len v config.toml: line = "".'),
    _f('icon', 'str', 'Ikona (súbor)', '', 'PNG v icons/ vedľa config.toml alebo plná cesta.'),
    _f('label', 'str', 'Text', '', 'S exec je to šablóna, {} sa nahradí výstupom.'),
    _f('command', 'command', 'Príkaz / akcia', '', 'Príkaz alebo akcia (napr. module:wallpaper).'),
    _f('exec', 'command', 'Zdroj hodnoty (exec)', '', 'Príkaz alebo builtin:cpu, builtin:ram, builtin:disk:/ …'),
    _f('interval', 'int', 'Obnovovanie exec (s)', 0, '0 = raz pri otvorení.', min=0),
    _f('track', 'str', 'Skupina sledovania (track)', '', 'Príkaz sa spustí samotným ostrovčekom, jeden proces na skupinu.'),
)
CUSTOM_SPECS = _specs(
    _f('name', 'str', 'Názov modulu', '', 'Názov, ktorý sa uvedie v poradí modulov.', write_default=True),
    _f('exec', 'command', 'Príkaz (výstup sa zobrazí)', '', 'Trvalý proces (interval 0) alebo periodický príkaz.'),
    _f('text', 'str', 'Pevný text / ikona', '', 'Namiesto exec – statické tlačidlo.'),
    _f('interval', 'int', 'Obnovovanie (s)', 0, '0 = trvalý proces.', min=0),
    _f('json', 'bool', 'Výstup je JSON', True, '{"text","tooltip","class"}; vypnuté = čistý text.'),
    _f('track_file', 'path', 'Sledovaný súbor (odznak)', '', 'Modul je v lište ako odznak (ako CAPS), len kým súbor existuje; prázdne = bežný modul.'),
    _f('on_click', 'command', 'Ľavý klik', ''), _f('on_click_right', 'command', 'Pravý klik', ''),
    _f('on_click_middle', 'command', 'Stredný klik', ''), _f('tooltip', 'str', 'Tooltip', ''),
)
RULE_SPECS = _specs(
    _f('summary', 'str', 'Súhrn obsahuje', '', 'Text, ktorý má obsahovať súhrn notifikácie.'),
    _f('app', 'str', 'Aplikácia obsahuje', ''), _f('body', 'str', 'Telo obsahuje', ''),
    _f('on_click', 'command', 'Príkaz po kliknutí', '', 'Notifikácia sa potom zavrie.'),
    _f('timeout_ms', 'int', 'Čas zobrazenia (ms)', None, '0 = nevyprší; nenastavené = predvolené.', min=0, nullable=True),
)
# pole tabuliek: id stránky -> popis
ARRAYS = {
    'menu': dict(title='Vlastné menu', family='menu', specs=MENU_SPECS, child=('menu.item', ITEM_SPECS), key='name', add='Pridať menu',
                 child_add='Pridať položku', desc='Panely otvárané on_click = "názov" na ľubovoľnom module alebo ako záložka menu:názov.'),
    'custom': dict(title='Vlastné moduly', family='custom', specs=CUSTOM_SPECS, child=None, key='name', add='Pridať modul',
                   desc='Externé moduly ako waybar „custom“. Názov pridaj aj do poradia modulov.'),
    'notifications.rule': dict(title='Pravidlá notifikácií', family='notifications.rule', specs=RULE_SPECS, child=None, key='summary',
                               add='Pridať pravidlo', desc='Prvé zhodné pravidlo vyhrá: klik spustí príkaz a notifikáciu zavrie.'),
}


def _kind_type(r, sec, key):
    if (sec, key) in TYPE_OVERRIDE:
        return TYPE_OVERRIDE[(sec, key)]
    if (sec, key) in CHOICES:
        return 'enum'
    k = r['kind']
    if k == 'bool_or':
        return 'bool'
    if k == 'strs':
        return 'strlist'
    if k == 'f64_or':
        d = r.get('default')
        return 'int' if d is None or float(d).is_integer() else 'float'
    if key in ('icon', 'icon_paused', 'icon_muted', 'icon_ethernet', 'icon_disconnected', 'editor_icon'):
        return 'icon'
    if key in ('on_click', 'on_click_right', 'on_click_middle', 'exec', 'command', 'editor_command') or key.endswith('_command'):
        return 'command'
    return 'str'


def build():
    """Zoznam kľúčov (dict) so všetkými vlastnosťami, v poradí GROUPS."""
    gen = json.load(open(os.path.join(HERE, 'schema_gen.json'), encoding='utf8'))
    out = {}
    for r in gen:
        sec, key = r['section'], r['key']
        t = _kind_type(r, sec, key)
        e = dict(section=sec, key=key, type=t, default=r['default'], min=r['min'], max=r['max'], desc=r['comment'])
        if t == 'int' and e['default'] is not None:
            e['default'] = int(e['default'])
        if t in ('int', 'float') and e['default'] is None:
            e['nullable'] = True
        if t == 'enum':
            e['choices'] = CHOICES[(sec, key)]
        out[(sec, key)] = e
    for x in EXTRA:
        e = dict(min=None, max=None, desc='', default=None)
        e.update(x)
        out[(e['section'], e['key'])] = e
    for (sec, key), (lo, hi) in BOUNDS.items():
        if (sec, key) in out:
            out[(sec, key)]['min'], out[(sec, key)]['max'] = lo, hi
    for (sec, key), e in out.items():
        e.setdefault('label', None)
        e['label'] = e['label'] or LABELS.get((sec, key)) or GENERIC.get(key) or key
        if 'icon_list' in e:
            e['type'] = 'strlist'
    return out


def sections_in_order():
    for g, items in GROUPS:
        for sec, title in items:
            yield g, sec, title
