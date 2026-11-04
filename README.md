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
Hodnota je shell príkaz, alebo vstavaná akcia: `calendar`, `clipboard`, názov `[[menu]]`, `menu` (zoznam diskov),
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

## Idle (náhrada swayidle)

`[[idle]]` s `timeout` (s), `on_idle`, `on_resume` používa `ext-idle-notify-v1`, rovnako ako `swayidle`; odstráň
`swayidle -w timeout 600 'systemctl suspend' &` z autostartu. Inhibit (Super+Z vo Wayfire) ho blokuje rovnako ako swayidle.
Ikona inhibítora je už len ukazovateľ (`on_click = ""`).

## Schránka (cliphist)

Panel s históriou schránky: náhrada `cliphist list | rofi -dmenu … | cliphist decode | wl-copy`. Otvorí sa
`on_click = "clipboard"` na ľubovoľnom module (napr. `on_click_right = "clipboard"` pri `[keyboard]`), alebo položkou
menu s `command = "module:clipboard"`. Zobrazí náhľady položiek (najnovšie hore), **klik** položku skopíruje do
schránky a panel zavrie. Ak je položiek viac ako `rows`, panel sa skroluje kolieskom myši (vpravo je posuvník);
zatvára sa odchodom myši ako kalendár. História sa musí plniť ako doteraz (`wl-paste --watch cliphist store`).

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
