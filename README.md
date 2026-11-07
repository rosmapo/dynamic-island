# dynamic-island

Náhrada Waybaru: jeden „dynamic island“ v strede hornej hrany. Rust + EGL/OpenGL ES 2.0, bez
GTK/Qt/Quickshell. Cieľ: Debian 13 (Wayfire / wlroots – potrebuje `zwlr_layer_shell_v1`).

## Zostavenie (Debian 13)

    sudo apt install build-essential pkg-config cargo libwayland-dev libegl1 libgles2 \
                     fontconfig fonts-dejavu-core
    ./scripts/install.sh

Za behu: `iw`, `iproute2`, `wpctl` + `pactl` (PipeWire), `udisks2` (pre modul diskov).
Písma: `font_family` (text) a `icon_family` (ikony) – načítajú sa **len písma z konfigurácie**, žiadne skryté náhrady.
Viac písem (`["A", "B"]` alebo `"A, B"`) = poradie náhrad; chýbajúci znak sa vykreslí ako prázdny štvorec (tofu).
Písmo, ktoré nie je nainštalované, ostrovček vypíše na stderr (`journalctl --user -u dynamic-island`).

## Čo sa zmenilo oproti waybaru

| Waybar | dynamic-island |
|---|---|
| `group/island` s modulmi | rovnaké moduly a poradie (`modules = [...]`), šírka sa plynulo (pružina) mení podľa viditeľných modulov |
| clock + `calendar` (GTK skript) | kliknutie na hodiny **rozbalí kalendár v ostrovčeku** (čísla týždňov, zvýraznený dnešok, koliesko myši = zmena mesiaca); zbalí sa po opustení myšou |
| `Brightness.sh` + notifikácia | **HUD jasu bez skriptu**: `dynamic-island --brightness up\|down\|get\|N` (príkaz `lumix` a krok sú v `[brightness]`) |
| **mako** | ostrovček je sám notifikačný server (`org.freedesktop.Notifications` cez vlastný D-Bus klient): ostrovček sa prerobí na notifikáciu (ako HUD), zoskupenie podľa súhrnu, urgentné s červeným rámikom, ľavý klik = akcia `default` + zavrieť, pravý = zavrieť, stredný = zavrieť všetky |
| tooltipy | po 450 ms nad modulom sa ostrovček rozbalí s textom (sieť, VPN, USB, …) |
| wireplumber (len pri mute) | odznak pri mute + **HUD hlasitosti** pri každej zmene |
| network, VPN, keyboard-state, idle inhibitor, wf-recorder, layout | natívne (sysfs, `iw`, `wpctl`, Wayfire IPC); bez signálu `RTMIN+8` – stav sa sleduje sám |
| `custom/eject` + **udiskie** | modul `disks` (UDisks2 cez D-Bus): automount, ikona USB (s počtom diskov, nie oddielov) len keď je médium pripojené, ľavý klik: jeden disk sa rovno odpojí, pri viacerých menu v ostrovčeku (riadok = odpojiť + vysunúť), pravý klik = odpojiť všetko, notifikácie o pripojení a chybách. `udiskie` a `waybar-eject.py` netreba |
| `waybar_autohide.py` + SIGUSR1/2 | natívny autohide (rovnaké pravidlá/parametre v `[autohide]`); ostrovček sa **animovane skryje** – povrch sa nikdy nemení, takže nevznikajú posuny okien ani výpadky fullscreenu spôsobené skrývaním baru. SIGUSR1/SIGUSR2 fungujú tiež |
| `waybar-colors.css` (matugen) | číta sa priamo (`colors_file`) a **mení sa živo** po prefarbení wallpaperu |

Zmena `config.toml` reštartuje proces sama. `dynamic-island --check` overí konfiguráciu,
`dynamic-island --snapshot adresar` vykreslí ukážkové stavy bez kompozitora (surové RGBA).

## Wayfire

    [autostart]
    island = dynamic-island

(Odstráň waybar a `waybar-wrapper.sh`.)

### Automatický reštart po páde (odporúčané)

`install.sh` nainštaluje používateľskú službu `~/.config/systemd/user/dynamic-island.service`
(`Restart=on-failure`, po 1 s). Namiesto `island = dynamic-island` daj do autostartu:

    [autostart]
    island = sh -c 'systemctl --user import-environment; systemctl --user restart dynamic-island'

Prepínač na skratku: `island-toggle` (beží → zastaví, nebeží → spustí), napr. vo `wayfire.ini`
`binding_island = <super> <shift> KEY_I` / `command_island = island-toggle`.

Príkazy: `systemctl --user status dynamic-island`, `journalctl --user -u dynamic-island -f` (logy),
`systemctl --user kill -s USR1 dynamic-island` (skryť) / `USR2` (zobraziť).
Reštartuje sa aj po SIGTERM (napr. „Ukončiť“ v systémovom monitore); po riadnom ukončení (zavretie
Wayfire) alebo `systemctl --user stop dynamic-island` sa znova nespustí. Pre autohide treba `ipc`, `ipc-rules` s `get_cursor_position`
(ako doteraz) a nastavený `WAYFIRE_SOCKET`.

## Obmedzenia

- Caps Lock sa číta zo `/sys/class/leds/*::capslock` (bez práv root).
- Mierka: celočíselná (`wl_surface.set_buffer_scale`); zlomková zatiaľ nie.
- Rozloženie klávesnice sa číta cez Wayfire IPC (`wayfire/get-keyboard-state`) – iné kompozitory ho nemajú.
- Schránka (`cliphist`), polkit agent a tapeta (`awww` + matugen) ostávajú samostatné programy.

## História notifikácií

Každá notifikácia sa zapíše ako JSON riadok do `~/.config/dynamic-island/notifications.log` (vedľa konfigurácie;
`[notifications] history`, `history_max`, `history_file`). Výpis: `dynamic-island --history 20`.
Pole `time` je unixový čas, ďalej `id`, `app`, `urgency`, `summary`, `body` – dá sa spracovať aj `jq` (náhrada `makoctl history`).

## Skratky hlasitosti a jasu (wayfire.ini)

Globálne klávesy drží kompozitor, preto sa viažu tam – bez skriptu, jeden riadok na skratku
(použi celú cestu, `~/.local/bin` nemusí byť v PATH kompozitora):

    [command]
    binding_bright_up   = KEY_BRIGHTNESSUP
    command_bright_up   = /home/mm/.local/bin/dynamic-island --brightness up
    binding_bright_down = KEY_BRIGHTNESSDOWN
    command_bright_down = /home/mm/.local/bin/dynamic-island --brightness down

    command_vol_up   = /home/mm/.local/bin/dynamic-island --volume up
    command_vol_down = /home/mm/.local/bin/dynamic-island --volume down
    command_vol_mute = /home/mm/.local/bin/dynamic-island --volume mute
    (binding_vol_up = KEY_VOLUMEUP, binding_vol_down = KEY_VOLUMEDOWN, binding_vol_mute = KEY_MUTE)

`--volume up|down|mute|get|N` volá `wpctl` (krok a strop v `[audio]`); HUD sa ukáže sám, lebo ostrovček sleduje PipeWire.

Test z terminálu: `dynamic-island --brightness up` vypíše nové percento (alebo chybu).

## Notifikácie

Odinštaluj mako (`sudo apt remove mako-notifier`) alebo ho aspoň nespúšťaj – obe aplikácie nemôžu držať
meno `org.freedesktop.Notifications`. Ak je obsadené, ostrovček to vypíše a každých 5 s to skúša znova.
Nastavenia sú v `[notifications]`; farby urgentných v `[colors]` (`urgent_border`, `urgent_text`).
Zatiaľ bez ikon aplikácií, bez histórie (`makoctl restore`) a bez režimu „nerušiť“.

Skript s `makoctl list` už netreba: pravidlá `[[notifications.rule]]` priradia kliku príkaz podľa textu
v súhrne (`summary`), aplikácii (`app`) alebo tele (`body`); voliteľne vlastný `timeout_ms`.

## Tlačidlá modulov

Každý modul (aj `[[custom]]`) má `on_click` (ľavé), `on_click_right` (pravé) a `on_click_middle` (stredné tlačidlo).
Hodnota je shell príkaz, alebo vstavaná akcia: `calendar`, `clipboard`, `trash`, `wallpaper`, `wifi`, názov `[[menu]]`, `menu` (zoznam diskov),
`eject-all`, `inhibit-toggle`, `layout-toggle`, `audio-mute`. Prázdna hodnota = tlačidlo nič nerobí.

    [clock]
    on_click        = "calendar"
    on_click_right  = "~/.config/wayfire/scripts/wallpaper -r"
    on_click_middle = "clipboard"

## Tooltipy

Každý modul má `tooltip = "..."` (prázdne = žiadny). Zástupné symboly: sieť `{ssid} {ifname} {ip} {gateway} {signal} {dbm} {freq}`
(`tooltip_wifi`, `tooltip_ethernet`, `tooltip_disconnected`), VPN `{ifname} {ip}`, klávesnica `{layout}`, audio `{volume}`,
recorder `{text}`, custom `{text} {tooltip} {class}`; pri hodinách je to strftime formát. `\n` = nový riadok.
SSID sa zisťuje cez `iw`, potom `iwgetid` / `nmcli`; ak nič z toho nie je nainštalované, SSID ostane prázdne.

Notifikácie sa zobrazujú priamo v ostrovčeku (nahradia jeho obsah ako HUD), ďalšie čakajúce sú označené `+N`.

## Idle inhibitor

Ikona inhibítora je len ukazovateľ (`on_click = ""`): zobrazí sa, kým existuje súbor `[inhibitor] file` (predvolene
`~/.idle_inhibit`), ktorý spravuje Wayfire skript (Super+Z). Samotná nečinnosť (uspanie, zamknutie) sa rieši mimo
ostrovčeka, napr. cez `swayidle`.

## Schránka (cliphist)

Panel s históriou schránky: náhrada `cliphist list | rofi -dmenu … | cliphist decode | wl-copy`. Otvorí sa
`on_click = "clipboard"` na ľubovoľnom module (napr. `on_click_right = "clipboard"` pri `[keyboard]`), alebo položkou
menu s `command = "module:clipboard"`. Zobrazí náhľady položiek (najnovšie hore), **klik** položku skopíruje do
schránky a panel zavrie. Ak je položiek viac ako `rows`, panel sa skroluje kolieskom myši (vpravo je posuvník);
zatvára sa odchodom myši ako kalendár. História sa musí plniť ako doteraz (`wl-paste --watch cliphist store`).

**Vyhľadávanie ako v rofi** – bez vyhľadávacieho políčka: kým je panel otvorený, má ostrovček klávesnicu a písanie
priamo filtruje zoznam (bez ohľadu na veľkosť písmen; viac slov oddelených medzerou musí sedieť naraz). Po písaní je
vybratá prvá zhoda. Klávesy: **Enter** skopíruje vybranú položku, **↑/↓** (aj Tab, PgUp/PgDn, Home/End) menia výber,
**Backspace** maže posledný znak, **Ctrl+U** zmaže celý filter, **Esc** panel zavrie. Kým je filter rozpísaný, panel sa
odchodom myši nezatvorí. Ak filter nič nenájde, panel to napíše aj s hľadaným textom.

```toml
[clipboard]
font_size = 15      # veľkosť písma (px)
rows = 8            # koľko riadkov je vidieť naraz; viac položiek = skrolovanie
width = 420         # šírka panela (px)
max_items = 200     # najviac koľko najnovších položiek sa načíta
# list_command = "cliphist list"                # príkaz, ktorého riadky `id<TAB>náhľad` sa zobrazia
# copy_command = "cliphist decode | wl-copy"    # dostane vybraný riadok na vstup
```

Vybraná (hover) položka používa farby `menu_selected` / `menu_selected_text` z `[colors]` (pozri nižšie).

## Kôš (trash)

Panel s košom zo **všetkých pripojených diskov** (domovský `~/.local/share/Trash` aj `.Trash-UID` / `.Trash/UID` na
pripojených diskoch podľa freedesktop špecifikácie) – náhrada samostatného správcu koša. Otvorí sa `on_click = "trash"` na
ľubovoľnom module, alebo položkou menu s `command = "module:trash"` (napr. v dashboarde: položka „Kôš“ s `exec`
a `command = "module:trash"`). Zoznam sa pri každom otvorení načíta znova a je zoradený od naposledy zmazaného;
zobrazuje názov, pôvodné umiestnenie (domov skrátený na `~`) a dátum zmazania.

- **Výber**: každý riadok má zaškrtávacie políčko, **klik na riadok** prepne jeho výber (myš nenesie Ctrl ani Shift, preto
  sa výber viacerých robí obyčajnými klikmi). Políčko v hlavičke zoznamu vyberie všetko / zruší výber.
- **Obnoviť vybrané** (vľavo, aktívne len keď je niečo vybrané) vráti položky na pôvodné miesto, chýbajúce priečinky
  vytvorí a nikdy neprepíše existujúci súbor – takú položku nechá v koši a v pätke napíše, koľko chýb nastalo.
- **Vyprázdniť kôš** (vpravo, červené) natrvalo zmaže *všetky* položky. S `confirm_empty = true` sa najprv panel
  prepne na otázku „Natrvalo vymazať N položiek?“ s tlačidlami Zrušiť / Vymazať (kým je otázka vidieť, panel sa
  odchodom myši nezatvorí).
- Pätka ukazuje počet položiek a vybraných, po operácii výsledok (`Obnovené: 3`, `Vymazané: 12`).
- Viac položiek ako `rows` sa skroluje kolieskom (vpravo je posuvník).

```toml
[trash]
width = 560            # šírka panela (px)
rows = 8               # koľko riadkov je vidieť; viac položiek = skrolovanie
text_size = 14         # veľkosť písma (px)
confirm_empty = true   # pred vyprázdnením sa spýtať
show_location = true   # stĺpec s pôvodným umiestnením
show_date = true       # stĺpec s dátumom zmazania
```

Tlačidlo „Vyprázdniť kôš“ používa farby `alert` / `on_alert`, „Obnoviť vybrané“ `accent` / `on_accent` z `[colors]`.

## Hardware (záložky)

Kontajner, ktorý obalí štyri panely – audio, bluetooth, wifi a monitor – lištou so záložkami. Panely sú tie isté ako pri
samostatnom otvorení (`on_click = "wifi"` a pod. fungujú ďalej), kontajner len pridá lištu s ikonami a prepínanie bez
zatvárania. Otvára sa akciou `hardware` (naposledy použitá záložka) alebo `hardware:audio` / `hardware:bluetooth` /
`hardware:wifi` / `hardware:monitor` (konkrétna záložka) – ako `on_click*` ľubovoľného modulu, alebo v menu cez
`module:hardware`. Opakovaný klik na tú istú záložku kontajner zavrie.

Záložkou môže byť aj **vlastné menu**: `tabN = "menu:NAZOV"` (`NAZOV` = `name` z `[[menu]]`, veľkosť písmen sa zachová).
Tak vznikne napr. záložka s tlačidlami (label alebo icon) pre nastavenia externých aplikácií. Menu sa v kontajneri správa
ako samostatne: položky s `exec` sa spustia pri prepnutí na záložku (voľba `wait_exec` sa tu neuplatní, okno sa otvára
hneď), klik na položku vykoná jej `command` a zavrie okno. Záložku vyberieš aj akciou `hardware:menu:NAZOV`. Ak menu
s daným názvom neexistuje, záložka sa zobrazí, ale nedá sa otvoriť. Predvolená ikona takej záložky je ozubené koleso.

    [hardware]
    tabs_position = "left"   # left | right | top | bottom
    tab_size = 44
    icon_size = 22
    # tab_gap = 6            # medzera medzi tlačidlami záložiek (px, 0–64)
    # width = 560            # celková šírka vrátane lišty; bez nej sa šírka mení podľa aktívnej záložky
    # height = 420           # celková výška vrátane lišty; bez nej podľa najvyššej záložky (okno pri prepínaní neskáče)
    # preload = false        # predvolene true: údaje monitora (ddcutil je pomalý) sa načítajú už pri otvorení kontajnera
    tab1 = "audio"
    tab1_icon = "\uf028"     # Nerd Font glyf; bez nej predvolená
    tab2 = "wifi"            # poradie a výber záložiek (`tab1` … `tab8`)
    tab3 = "menu:nastavenia" # vlastné [[menu]] name = "nastavenia"
    tab3_icon = "\U000f0493"  # glyfy nad U+FFFF sa zapisujú ako \UXXXXXXXX

Panel záložky je zarovnaný hore, pätička (ovládacie tlačidlá) dole; ak je okno vyššie než obsah, medzi nimi ostane
voľné miesto. Ikony sa berú z `icon_family`. Predvolené sú `f057e` (audio), `f00af` (bluetooth), `f0928` (wifi) a
`f0379` (monitor); ak máš nainštalovanú len podmnožinu Nerd Fontu (napr. `SymbolsNerdFontMono-Mini.ttf` z `config/fonty`),
glyfy `f00af` a `f0379` v nej nie sú a ikona sa nezobrazí – doplň ich do podmnožiny, alebo nastav vlastné `tabN_icon`.

## Wi-Fi

Panel nad **NetworkManagerom** (D-Bus, systémová zbernica). Otvorí sa `on_click = "wifi"` na ľubovoľnom module
(typicky `[network]`), alebo položkou menu s `command = "module:wifi"`. Zobrazí zoznam sietí (aktívna hore, potom
podľa signálu), pri otvorení spustí vyhľadávanie.

- **klik** na uloženú alebo otvorenú sieť ju pripojí, na aktívnu ju odpojí
- **klik** na zabezpečenú novú sieť otvorí zadávanie hesla priamo v ostrovčeku (WPA2/WPA3-PSK); **Enter** pripojí,
  **Esc** zruší, **Ctrl+V** vloží schránku (`wl-paste`). Pri zlom hesle sa vytvorený profil zmaže
- **pravý klik** na uloženú sieť ju zabudne
- hlavička: vľavo prepínač rádia Wi-Fi a tlačidlo obnovenia, v strede názov pripojenej siete, vpravo tlačidlo
  správcu pripojení (`editor_command`, predvolene `nm-connection-editor`). Tlačidlo má len ikonu (`editor_icon`) a / alebo
  text (`editor_label`) z configu, pozadie sa kreslí iba pri hoveri; ak nie je nastavená ani ikona, ani text, tlačidlo
  sa nezobrazí. Rovnako funguje `editor_command` / `editor_icon` / `editor_label` v `[bluetooth]`.
- zoznam má stĺpec zabezpečenia (WPA2, WPA3, WPA2/WPA3, WEP, Otvorená …); aktuálne pripojená sieť má len značku ✓
  (riadok sa zvýrazňuje iba pod myšou; rovnako pri audio zariadeniach a pripojených bluetooth zariadeniach)
- siete s 802.1X (firemné), WEP a skryté siete zatiaľ nie sú podporované

Klávesnicu ostrovček zachytáva **iba počas zadávania hesla** (layer-shell `exclusive`), inak ju nechá aplikáciám.
Vyžaduje `libxkbcommon` (načíta sa za behu) a oprávnenie meniť siete (polkit – bežná používateľská relácia stačí).

```toml
[network]
on_click = "wifi"

[wifi]
font_size = 15   # písmo
rows = 8         # koľko sietí je vidieť naraz, viac = skrolovanie kolieskom
width = 420      # šírka panela
editor_command = "nm-connection-editor"   # príkaz tlačidla v hlavičke (prázdne = bez tlačidla)
editor_icon = "\U000f0493"               # ikona tlačidla (Nerd Font); bez ikony aj textu sa tlačidlo nezobrazí
# editor_label = "Pripojenia"            # voliteľný text (aj spolu s ikonou)
```

## Wallpaper

Mriežka náhľadov obrázkov z adresára. Otvorí sa `on_click = "wallpaper"` na module, alebo položkou menu
s `command = "module:wallpaper"`. **Ľavý klik** na náhľad spustí `command -v CESTA` (skript nastaví pozadie, farby
a pošle prípadné notifikácie) a panel sa zavrie. Ak skript skončí úspešne, ostrovček zapíše cestu do
`cache_dir/current` a náhľad označí rámikom. Ak je riadkov viac ako `rows`, mriežka sa skroluje kolieskom.

Náhľady vyrába ImageMagick (`magick` alebo `convert`) presne vo veľkosti `thumb_width` × 9/16, so zaoblenými
rohmi, do `cache_dir/thumbs`. Pri prvom otvorení sa dorábajú na pozadí a objavujú sa postupne.

```toml
[wallpaper]
directory = "~/.config/wayfire/media/wallpaper"
columns = 4
rows = 4                    # koľko riadkov je vidieť naraz
thumb_width = 160           # šírka náhľadu (px), výška je 9/16
command = "~/.config/wayfire/scripts/wallpaper"
cache_dir = "~/.cache/wallpaper-chooser"
```

## Vlastné menu

Panel ako kalendár/disky: mriežka ikon alebo textu. **Nie je** samostatný modul v `modules` – spúšťa sa
`on_click = "meno_menu"` na ľubovoľnom module (custom, clock, …). Zavrie sa klikom na položku alebo odchodom myši (ako kalendár).

```toml
[[menu]]
name = "apps"
type = "icons"          # "icons" | "text"
columns = 3
icon_size = 28          # len icons
gap = 8
padding = 12
# title = "Aplikácie"   # voliteľné; prázdne = bez nadpisu
# rows = 2              # voliteľné

[[menu.item]]
icon = "firefox.png"     # PNG: len názov → ~/.config/dynamic-island/icons/
# icon = "~/obrazky/app.png"  # alebo plná / expandovaná cesta
command = "firefox"

[[menu]]
name = "power"
type = "text"
columns = 1
text_size = 15
cell_width = 140

[[menu.item]]
label = "Reštart"
command = "systemctl reboot"

# spúšťanie napr. cez custom modul:
[[custom]]
name = "apps_btn"
text = "󰍜"
on_click = "apps"
```

### Položky s `exec` (textový dashboard)

Položka textového menu môže namiesto pevného textu zobrazovať **prvý neprázdny riadok výstupu** príkazu alebo skriptu
(`exec`). Text sa kreslí ako `label` (rovnaké `text_size`, `cell_width`, `text_columns`, vycentrovaný), takže z menu sa dá
poskladať jednoduchý dashboard.

- **`label` ako šablóna:** ak obsahuje `{}`, nahradí sa výstupom (`label = "CPU: {} %"`). Kým výstup nie je načítaný, je `{}`
  prázdne, takže rozloženie ostane rovnaké (`CPU:   %`). Bez `{}` sa do načítania zobrazí `label` a potom samotný výstup.
- **`interval`** (sekundy): `0` (predvolené) = príkaz sa spustí raz pri otvorení menu; `> 0` = obnovuje sa, kým je menu
  otvorené (nový beh sa spustí až po dobehnutí predošlého, takže sa nenakopia).
- **Nič sa neukladá medzi otvoreniami**: pri každom otvorení menu sa hodnoty začínajú prázdne, aby sa nezobrazila
  zastaraná hodnota z minula.
- **`wait_exec`** (voľba menu, predvolene `false`): `false` = menu sa zobrazí hneď a hodnoty sa doplnia, keď dobehnú;
  `true` = menu sa zobrazí až po načítaní všetkých položiek s `exec`, najneskôr po `wait_timeout_ms` (predvolene 1500).
  Počas čakania ostrovček nič nezobrazuje; ďalší klik na modul alebo iný panel otváranie zruší.
- **Prázdny `command`** = položka je len na zobrazenie: bez hoveru a bez reakcie na klik (platí pre všetky položky menu).
- Príkaz beží cez `sh -c` (skript musí byť spustiteľný, alebo `bash ~/skript.sh`) najviac 5 sekúnd, potom sa ukončí
  a hodnota ostane prázdna. Berie sa len prvý neprázdny riadok, bez riadiacich znakov (ANSI farby nie sú podporené).

```toml
[[menu]]
name = "dashboard"
type = "text"
text_columns = 2
cell_width = 150
text_size = 16
wait_exec = false

[[menu.item]]
label = "CPU: {} %"
exec = "~/.config/dynamic-island/scripts/cpu.sh"
interval = 2

[[menu.item]]
label = "RAM: {} %"
exec = "~/.config/dynamic-island/scripts/ram.sh"
interval = 2

[[menu.item]]
label = "Kôš: {}"
exec = "~/.config/dynamic-island/scripts/trash.sh"   # vypíše napr. "3 položky" alebo "prázdny"; bez interval = raz pri otvorení
command = "xdg-open trash:///"                        # voliteľné: klik otvorí kôš (bez command je položka len informačná)
```

### Sledované procesy (`track`) a modul `radio`

Položka menu s `track = "skupina"` sa nespustí ako obyčajný príkaz, ale ostrovček si proces **sám spustí a pamätá si ho**. V jednej skupine beží najviac jeden proces: nový štart najprv ukončí (celú skupinu procesov) predošlý, takže netreba `killall`. O skončení procesu sa ostrovček dozvie blokujúcim čakaním na jeho koniec – nič sa nezisťuje cyklicky.

Modul **`radio`** (pridaj ho do `modules`) zobrazí ikonu, kým beží proces zo skupiny `radio`. **Klik na ikonu** proces ukončí; ikona zmizne aj vtedy, keď proces skončí sám. Tooltip ukáže `label` položky. Vypnutie z menu: `command = "module:track-stop:radio"`. Pri reštarte ostrovčeka po zmene configu sa sledované procesy ukončia.

```
[[menu.item]]
label = "Lumen"
command = "mpv https://audio.lumen.sk/live128.mp3 --no-config"
track = "radio"

[[menu.item]]
label = "Vypnúť rádio"
command = "module:track-stop:radio"

[radio]                 # voliteľné
icon = "\U000f0439"
tooltip = "{name}\nKlik = vypnúť"   # {name} = label položky
```

### Príkaz `module:` v položke menu

`command` položky je bežne shell príkaz. S predponou `module:` sa namiesto toho spustí vnútorná akcia ostrovčeka –
menu sa zatvorí a otvorí sa požadovaný panel (alebo sa vykoná akcia):

| `command` | výsledok |
|---|---|
| `"calendar"` | spustí sa program `calendar` (ak existuje) ako každý iný shell príkaz |
| `"module:calendar"` | otvorí sa kalendár v ostrovčeku |
| `"module:power"` | otvorí sa iné menu s `name = "power"` (ľubovoľné `[[menu]]`) |
| `"module:disks"` | zoznam diskov (len ak je niečo pripojené) |
| `"module:audio-mute"`, `module:layout-toggle`, `module:inhibit-toggle`, `module:eject-all` | vstavané akcie ako pri `on_click` modulov |

| `"module:hardware"` | kontajner `hardware` so záložkami (naposledy použitá záložka) |
| `"module:hardware:wifi"` | kontajner rovno na záložke `wifi` (`audio`, `bluetooth`, `wifi`, `monitor`) |

Neznámy názov sa ignoruje (menu sa len zavrie).

Po otvorení panela (kliknutím na modul ostrovčeka alebo cez `module:`) sa odchod myši ignoruje, kým myš nevstúpi do
nového panela: nový panel býva menší ako široký ostrovček, takže myš po kliku zvyčajne skončí mimo neho a kalendár,
disky či menu by sa hneď zavreli. Po vstupe myši do panela platí bežné zatváranie odchodom myši.

```toml
[[menu.item]]
label = "Kalendár"
command = "module:calendar"

[[menu.item]]
label = "Napájanie"
command = "module:power"
```

Ikonové menu: **PNG** súbory (`icons/` vedľa `config.toml`, alebo plná cesta). Textové menu: Nerd Font / text. Vybraná (hover) položka textového menu: pozadie `menu_selected`, text `menu_selected_text` v `[colors]`
(`"@meno"` z `colors_file` alebo `"#rrggbb"`; predvolene `accent` / `on_accent`, takže bez nastavenia sa nič nemení);
ikonové menu = jemné zvýraznenie.

```toml
[colors]
menu_selected      = "#3b6ea5"
menu_selected_text = "#ffffff"
```
