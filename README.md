# dynamic-island

Stavový panel („dynamic island“) pre Wayland: jedna zaoblená kapsula v strede hornej hrany obrazovky, ktorá sa podľa
potreby rozbalí na kalendár, tooltip, notifikáciu alebo panel (zvuk, Wi-Fi, Bluetooth, monitor, schránka, kôš,
tapety, vlastné menu). Napísané v Rust, kreslí sa cez EGL/OpenGL ES 2.0 do `wlr-layer-shell` povrchu; bez UI frameworku.

Tento dokument je **technická dokumentácia pre budúceho správcu** (autora): ako sa aplikácia zostavuje a spúšťa, na čom
je závislá, **čo je pevne zadrôtované na aktuálne prostredie** (Debian 13, Wayfire 0.9.0 + vlastný backport `ipc-rules`,
PipeWire, NetworkManager) a **čo upraviť pri zmene distribúcie alebo kompozitora**, a kompletná referencia konfigurácie.

Obsah:

1. [Prehľad](#1-prehľad)
2. [Zostavenie a inštalácia](#2-zostavenie-a-inštalácia)
3. [Spustenie, súbory, služba](#3-spustenie-súbory-služba)
4. [Architektúra](#4-architektúra)
5. [Väzby na prostredie a migrácia](#5-väzby-na-prostredie-a-migrácia)
6. [Referencia konfigurácie](#6-referencia-konfigurácie)
7. [Notifikácie](#7-notifikácie)
8. [Obmedzenia a známe nezrovnalosti](#8-obmedzenia-a-známe-nezrovnalosti)
9. [Riešenie problémov](#9-riešenie-problémov)

---

## 1. Prehľad

**Moduly v kapsule** (poradie určuje `modules = [...]`, viditeľnosť sa riadi stavom, šírka sa plynulo prispôsobuje):

| Modul | Čo ukazuje | Zdroj údajov |
|---|---|---|
| `network` | ikona Wi-Fi (4 úrovne signálu) / ethernet / odpojené, tooltip so SSID, IP, signálom | sysfs, `/proc/net/wireless`, `iw`, `ip` |
| `vpn` | odznak, keď beží rozhranie zodpovedajúce `interfaces` (napr. `tun*`) | `/sys/class/net` |
| `clock` | čas (`strftime`), po kliku kalendár s číslami týždňov | systémový čas, `setlocale` |
| `keyboard` | skratka aktuálneho rozloženia (SK/US), klik prepína | Wayfire IPC |
| `audio` | odznak len pri stlmení; HUD pri zmene hlasitosti | `wpctl`, `pactl subscribe` |
| `capslock` | odznak pri zapnutom Caps Lock | `/sys/class/leds/*::capslock` |
| `inhibitor` | ikona, kým existuje súbor `~/.idle_inhibit` | existencia súboru |
| `recorder` | odznak REC: odpočet a čas nahrávania z panelu `capture`; klik (`on_click = "capture"`) ho zastaví | interný stavový súbor |
| `radio` | ikona, kým beží sledovaný proces (položka menu s `track`) | vlastné sledovanie procesu |
| `media` | ikona, kým je aktívny prehrávač (MPRIS); klik otvorí panel „Práve hrá“ | D-Bus `org.mpris.MediaPlayer2.*` |
| `disks` | ikona USB s počtom pripojených médií, automount, odpojenie/vysunutie | UDisks2 (D-Bus) |
| `[[custom]]` | ľubovoľný text/skript/tlačidlo | vlastný príkaz |

**Panely** (rozbalia sa v kapsule; otvárajú sa kliknutím na modul alebo akciou v menu): kalendár, zoznam diskov,
schránka (cliphist), história notifikácií, „Práve hrá“, kôš, tapety, Wi-Fi, Bluetooth, zvuk, monitor (DDC/CI), snímanie obrazovky (`capture`), kontajner `quickset` so záložkami
(audio/bluetooth/wifi/monitor/vlastné menu), vlastné `[[menu]]` (ikony / text / dashboard).

**Ďalšie funkcie:** notifikačný server (`org.freedesktop.Notifications`) so zobrazením priamo v kapsule a históriou,
HUD hlasitosti a jasu, autohide (skrytie, keď okno zasahuje do pásma lišty), živé farby z CSS súboru (matugen),
automatický reštart po zmene `config.toml`, snímkovanie stavov bez kompozitora (`--snapshot`).

---

## 2. Zostavenie a inštalácia

### 2.1 Požiadavky na zostavenie

- Rust ≥ **1.85** (`rust-version` v `Cargo.toml`, edition 2021, resolver 3). Debian 13 ho má v balíku `cargo`.
- `pkg-config`, `build-essential`, **`libwayland-dev`** (crate `wayland-backend` s funkciou `client_system` sa
  linkuje na `libwayland-client.so`).
- Prístup ku crates.io pri prvom zostavení (žiadne vendorovanie v repozitári).

```sh
sudo apt install build-essential pkg-config cargo libwayland-dev
cargo build --release          # binárka: target/release/dynamic-island
cargo test                     # jednotkové testy (parsery, geometria, konfigurácia…)
cargo check                    # rýchla kontrola bez linkovania
```

Release profil: `opt-level = 3`, `lto = true`, `codegen-units = 1`, `strip = true`.

> **Cargo.lock v repozitári nie je.** Verzie závislostí sa teda pri každom čistom zostavení riešia znova (v rámci
> semver rozsahov z `Cargo.toml`). Pre reprodukovateľnosť po migrácii odporúčam `Cargo.lock` po úspešnom
> zostavení commitnúť.

### 2.2 Závislosti (crates)

Zámerne minimum, všetko ostatné (OpenGL, EGL, poll, inotify, JSON, TOML, D-Bus klient, animácie, layout) je v repozitári.

| Crate | Účel |
|---|---|
| `wayland-client`, `wayland-backend` (`client_system`) | Wayland klient (registry, surface, seat, output) |
| `wayland-protocols` (`client`, `staging`) | štandardné protokoly; po odstránení idle sa už priamo nepoužíva (ostáva len ako tranzitívna závislosť `-wlr`); `staging` je zbytočné a dá sa odstrániť |
| `wayland-protocols-wlr` (`client`) | **`zwlr_layer_shell_v1`** (povrch lišty) |
| `wayland-egl` | EGL okno nad `wl_surface` |
| `fontdb`, `fontdue` | vyhľadanie systémových písem a rasterizácia glyfov |
| `png` | ikony v menu |

Za behu sa **dynamicky načítajú** (`dlopen`, žiadne linkovanie pri zostavení): `libEGL.so.1`, `libGLESv2.so.2`,
`libxkbcommon` (len na preklad klávesnice pri zadávaní hesla Wi-Fi a hľadaní v schránke). Libc funkcie (`poll`,
`eventfd`, `inotify`, `kill`, `setsid`…) sa volajú cez vlastné `extern "C"` v `src/sys.rs` (bez crate `libc`).

### 2.3 Závislosti za behu

Povinné: Wayland kompozitor s `zwlr_layer_shell_v1` a balíky `libwayland-client0`, `libegl1`, `libgles2`
(+ GPU ovládač s EGL/GLES2), `libxkbcommon0`, nainštalované písma.

Voliteľné programy podľa používaných modulov (podrobnosti v [5.3](#53-externé-programy-pevne-v-kóde)):

| Balík (Debian 13) | Program | Potrebné pre |
|---|---|---|
| `wireplumber`, `pipewire-pulse` / `pulseaudio-utils` | `wpctl`, `pactl` | audio, HUD, `--volume`, panel zvuku |
| `iproute2` | `ip` | IP adresa, brána, sieť |
| `iw` (`/usr/sbin/iw`) | `iw` | SSID, frekvencia, signál |
| `network-manager` | (D-Bus), `nmcli`, `nm-connection-editor` | panel Wi-Fi, záložný zisťovač SSID |
| `bluez`, `bluez-tools` | (D-Bus), `bt-agent`, `bluetoothctl` | panel Bluetooth (párovanie) |
| `udisks2` | (D-Bus) | modul `disks` |
| `ddcutil` | `ddcutil` | panel monitor (jas, kontrast, vstup); nočný režim rieši tvoj skript (`night_switcher`) |
| `grim`, `slurp`, `wf-recorder` | `grim`, `slurp`, `wf-recorder` | panel `capture` (snímka / oblasť / nahrávanie videa); `notify-send` (`libnotify-bin`) pre notifikáciu po uložení |
| `librsvg2-bin` | `rsvg-convert` | `--drun`: SVG ikony aplikácií (voliteľné; záloha je ImageMagick) |
| `imagemagick` | `magick` / `convert` | náhľady tapiet, obaly v module `media` |
| `wl-clipboard`, `cliphist` | `wl-paste`, `wl-copy`, `cliphist` | panel schránky, vloženie hesla Wi-Fi |
| `mpv` (len podľa configu) | `mpv` | položky menu s rádiom |
| vlastné skripty | `lumix`, `~/.config/wayfire/scripts/wallpaper`, … | jas, tapety — pozri [5.6](#56-osobné-skripty-a-konvencie) |

### 2.4 Písma

Načítajú sa **iba písma uvedené v konfigurácii** (`font_family`, `icon_family`), žiadne skryté náhrady; systémové
písma sa hľadajú cez `fontdb::load_system_fonts()` (štandardné adresáre, fontconfig sa nevyžaduje).
Viac písem = poradie náhrad; chýbajúci glyf sa nakreslí ako prázdny štvorec. Nenájdené písmo sa vypíše na stderr.

- Predvolené: text `["Adwaita Sans", "DejaVu Sans"]`, ikony `["Hack Nerd Font Propo"]`.
- Ikony v configu sú **Nerd Font glyfy** (zápis `""`, nad U+FFFF `"\U000f0493"`).
- V repozitári je `config/fonty/Inter-Mini.ttf` (podmnožina písma). Ak používaš len podmnožinu Nerd Fontu, over, že
  obsahuje všetky použité glyfy (pozri predvolené ikony záložiek v [6.12](#612-quickset-záložky)).

### 2.5 Inštalácia

`scripts/install.sh` zostaví a nainštaluje binárku do `~/.local/bin/dynamic-island` a skopíruje config, ak neexistuje.
**Pozor:** skript kopíruje `config/config.toml`, no vzorový config je v `config/dynamic-island/config.toml`
(pozri [8](#8-obmedzenia-a-známe-nezrovnalosti)). Ručný postup:

```sh
cargo build --release
install -Dm755 target/release/dynamic-island ~/.local/bin/dynamic-island
mkdir -p ~/.config/dynamic-island
cp -rn config/dynamic-island/. ~/.config/dynamic-island/       # config.toml, colors.css, icons/, scripts/
install -Dm644 scripts/dynamic-island.service ~/.config/systemd/user/dynamic-island.service
install -Dm755 config/dynamic-island/scripts/island-toggle ~/.local/bin/island-toggle
systemctl --user daemon-reload
```

---

## 3. Spustenie, súbory, služba

### 3.1 Príkazový riadok

```
dynamic-island [--config CESTA] [--check] [--snapshot ADRESÁR]
               [--volume up|down|mute|get|N] [--brightness up|down|get|N]
               [--hud brightness %] [--history [N]]
```

| Prepínač | Význam |
|---|---|
| `--config CESTA` | iný konfiguračný súbor (predvolene `$XDG_CONFIG_HOME/dynamic-island/config.toml`, inak `~/.config/dynamic-island/config.toml`) |
| `--check` | načíta konfiguráciu, vypíše počet modulov a skončí (exit 1 pri chybe parsovania) |
| `--snapshot ADRESÁR` | vykreslí ukážkové stavy bez kompozitora (surové RGBA súbory) |
| `--volume ...` | nastaví hlasitosť cez `wpctl` na `@DEFAULT_AUDIO_SINK@` (`up`/`down` o `[audio] step` %, strop `limit`, `mute` = prepnutie, `get`, `N` = percento); bežiaci ostrovček zmenu zachytí a ukáže HUD |
| `--brightness ...` | spustí príkazy z `[brightness]`, vypíše nové percento a pošle HUD bežiacemu ostrovčeku |
| `--hud brightness N` | iba pošle HUD jasu (pre externé skripty): zapíše `brightness N` do `$XDG_RUNTIME_DIR/dynamic-island/hud` |
| `--history [N]` | vypíše posledných N (predvolene 20) notifikácií z histórie |

Signály bežiacemu procesu: **SIGUSR1 = skryť**, **SIGUSR2 = zobraziť**.

Zmena `config.toml` (inotify) vyvolá **reštart procesu cez `exec()`** (rovnaký PID). Pred `exec` sa ukončia sledované
procesy (`track`) aj `pactl subscribe`. Zmena farebného súboru (`colors_file`) sa prejaví **živo, bez reštartu**.

### 3.2 Súbory a adresáre

| Cesta | Obsah |
|---|---|
| `~/.config/dynamic-island/config.toml` | konfigurácia |
| `~/.config/dynamic-island/icons/` | PNG ikony pre menu (položka s `icon = "meno.png"`) |
| `~/.config/dynamic-island/notifications.log` | história notifikácií (JSON riadky); zmeniteľné `[notifications] history_file` |
| `~/.cache/dynamic-island/media-art/` | obaly albumov prekonvertované na PNG (názov = hash cesty a času); súbory staršie ako 90 dní sa pri štarte mažú |
| `~/.cache/dynamic-island/notif-icons/` | PNG ikony vyrobené z `image-data` (názov = hash obrázka); súbory staršie ako 90 dní sa pri štarte mažú |
| `$XDG_RUNTIME_DIR/dynamic-island/hud` | rúra pre `--hud`/`--brightness` → HUD (záloha `/tmp`) |
| `~/.cache/wallpaper-chooser/` | stav tapiet (`current`, `applied-theme`) a náhľady `thumbs/` |
| `~/.idle_inhibit` | súbor-príznak inhibítora nečinnosti (len sa sleduje jeho existencia) |
| `$XDG_RUNTIME_DIR/dynamic-island-rec` | stav nahrávania pre modul `recorder` (píše ho panel `capture`; interný, nenastavuje sa) |

### 3.3 Systemd používateľská služba

`scripts/dynamic-island.service`: `ExecStart=%h/.local/bin/dynamic-island`, `ConditionEnvironment=WAYLAND_DISPLAY`,
`Restart=on-failure` + `RestartForceExitStatus=SIGTERM SIGINT SIGHUP` (reštart aj po „Ukončiť“ v systémovom monitore),
`RestartSec=1`, `KillMode=process` (pri reštarte sa zabije len ostrovček, nie programy spustené klikom).
Po riadnom ukončení (zavretie kompozitora) alebo `systemctl --user stop` sa znova nespustí.
(`Description` v súbore ešte spomína „nečinnosť“ — funkcia bola odstránená, je to len text.)

Služba potrebuje prostredie kompozitora (`WAYLAND_DISPLAY`, **`WAYFIRE_SOCKET`**, `XDG_RUNTIME_DIR`…), preto sa z
autostartu spúšťa po `import-environment`:

```ini
# wayfire.ini
[autostart]
island = sh -c 'systemctl --user import-environment; systemctl --user restart dynamic-island'

[command]
binding_island = <super> <shift> KEY_I
command_island = island-toggle
```

`island-toggle` (`config/dynamic-island/scripts/island-toggle`): beží → zastaví, nebeží → spustí (užitočné pri výmene binárky).

Užitočné: `systemctl --user status dynamic-island`, `journalctl --user -u dynamic-island -f`,
`systemctl --user kill -s USR1 dynamic-island` (skryť) / `USR2` (zobraziť).

### 3.4 Globálne klávesové skratky (hlasitosť, jas)

Globálne klávesy drží kompozitor, preto sa viažu tam; používaj celé cesty (`~/.local/bin` nemusí byť v `PATH` kompozitora):

```ini
[command]
binding_bright_up   = KEY_BRIGHTNESSUP
command_bright_up   = /home/mm/.local/bin/dynamic-island --brightness up
binding_bright_down = KEY_BRIGHTNESSDOWN
command_bright_down = /home/mm/.local/bin/dynamic-island --brightness down
binding_vol_up      = KEY_VOLUMEUP
command_vol_up      = /home/mm/.local/bin/dynamic-island --volume up
binding_vol_down    = KEY_VOLUMEDOWN
command_vol_down    = /home/mm/.local/bin/dynamic-island --volume down
binding_vol_mute    = KEY_MUTE
command_vol_mute    = /home/mm/.local/bin/dynamic-island --volume mute
```

---

## 4. Architektúra

Jeden proces, jedno hlavné vlákno (slučka s `poll`) + pracovné vlákna, ktoré posielajú správy `Msg` do hlavnej slučky
cez frontu s `eventfd`.

| Súbor | Úloha |
|---|---|
| `main.rs` | CLI, `--volume`/`--brightness`/`--hud`/`--history`, spustenie `app::run` |
| `app.rs` | hlavná slučka (`poll`: Wayland, inotify, eventfd, časovače), spúšťanie príkazov (`spawn_detached`, sledované procesy `track`), reštart cez `exec` |
| `config.rs`, `toml.rs` | konfigurácia (vlastný parser podmnožiny TOML), téma, CSS paleta |
| `wl.rs` | Wayland: registry, `wlr-layer-shell` povrch, ukazovateľ, klávesnica (+ `xkb.rs`), mierka, EGL okno |
| `gl.rs`, `render.rs`, `text.rs`, `fonts.rs`, `anim.rs` | EGL/GLES2 (dlopen), primitíva (zaoblené obdĺžniky, tiene), text, písma, animácie |
| `ui.rs` | stav a layout všetkých modulov a panelov, spracovanie vstupu, kreslenie (najväčší súbor) |
| `data.rs` | typy `Msg` a `Data` (stav modulov) |
| `providers.rs` | vlákna zdrojov: sieť/VPN, zvuk, „fast“ poll (caps, inhibitor, recorder, rozloženie), `[[custom]]` |
| `wayfire.rs` | Wayfire IPC (rozloženie klávesnice, autohide) |
| `dbus.rs` | vlastný D-Bus klient (session aj system bus) |
| `sysinfo.rs` | natívne zdroje pre položky menu (`exec = "builtin:…"`): CPU, RAM, swap, teplota, disk, sieť |
| `mpris.rs` | modul `media`: prehrávače MPRIS, výber aktívneho, obal (JPEG → PNG), príkazy play/next/prev |
| `notify.rs`, `nicons.rs`, `history.rs` | notifikačný server, ikony notifikácií (PNG z `image-data`/téme ikon), história a jej načítanie pre panel |
| `disks.rs` | UDisks2 (automount, odpojenie) |
| `wifi.rs`, `bluetooth.rs` | NetworkManager, BlueZ |
| `audio.rs`, `monitor.rs` | panel zvuku (`pactl`), panel monitora (`ddcutil`) |
| `desktop.rs` | spúšťač `--drun`: čítanie `.desktop` súborov (NoDisplay, TryExec, duplicity), Exec, ikony (jedna téma, PNG + SVG cez cache), filtrovanie |
| `grid.rs` | spúšťač `--drun`: geometria mriežky, zásah myšou, navigácia a posun |
| `drun.rs` | spúšťač `--drun`: samostatné okno, vykreslenie, vstup, spustenie |
| `capture.rs` | panel snímania: `grim` / `slurp` / `wf-recorder`, odpočet, odznak REC; aktívne okno cez Wayfire IPC (`wayfire.rs`) |
| `wallpaper.rs`, `trash.rs`, `quickset.rs` | panely tapiet, koša, kontajner záložiek |
| `icons.rs`, `json.rs`, `sys.rs`, `snapshot.rs` | PNG ikony, JSON parser, tenká vrstva nad libc, `--snapshot` |

Poznámky k návrhu:

- Povrch layer-shell je **veľký priehľadný** (`surface_width` × `surface_height`, vrstva `Overlay`, kotva `Top`,
  `exclusive_zone = 0`, bez klávesnice); vstup prijíma iba oblasť samotnej kapsuly (input region). Skrytie (autohide) je
  animácia vnútri povrchu, povrch sa nemení — preto nevznikajú posuny okien.
- Klávesnicu si ostrovček vyžiada (`exclusive`) **iba počas zadávania hesla Wi-Fi a hľadania v schránke**.
- Mierka: celočíselná `wl_surface.set_buffer_scale`.
- Stav sa **sleduje vlastnými vláknami**: `spawn_fast` kontroluje každých `poll_ms` (predvolene 300 ms) caps LED,
  existenciu súboru inhibítora, súbor recordera a rozloženie klávesnice; sieť/VPN sa obnovuje každé 2 s; zvuk je
  riadený udalosťami `pactl subscribe`; disky, Bluetooth, Wi-Fi a notifikácie cez D-Bus signály.
- Každý potomok spustený z ostrovčeka beží v **novej session** (`setsid`), takže prežije ostrovček aj reštart.
  Výnimky: `pactl subscribe` a `[[custom]]` procesy majú `PR_SET_PDEATHSIG` (zaniknú s rodičom).
- Jednoduché príkazy bez shell syntaxe (`mpv URL`) sa spúšťajú **priamo** (bez `sh -c`); ostatné cez `sh -c`.

---

## 5. Väzby na prostredie a migrácia

Táto kapitola je jadrom dokumentu: **čo je v kóde pevne zadané** a čo upraviť pri zmene prostredia.
Stav v čase písania: Debian 13 (trixie), Wayfire 0.9.0 s vlastným backportom pluginu `ipc-rules`, PipeWire/WirePlumber,
NetworkManager, BlueZ, UDisks2, systemd, jedna obrazovka.

### 5.1 Kompozitor: Wayfire IPC (`src/wayfire.rs`) — najväčšia väzba

Pripája sa na UNIX socket z premennej prostredia **`WAYFIRE_SOCKET`**; protokol je JSON s dĺžkovým prefixom
(`{"method": "...", "data": {...}}`). Využívajú ho **dva moduly**:

**a) Rozloženie klávesnice (modul `keyboard`)** — `providers.rs` (`spawn_fast`) a akcia `layout-toggle`:

| Metóda IPC | Použitie |
|---|---|
| `wayfire/get-keyboard-state` | odpoveď `{layout, layout-index, possible-layouts[]}`; názov rozloženia sa mapuje cez `[keyboard.names]` na skratku |
| `wayfire/set-keyboard-state` | `{"layout-index": N}` — prepnutie na ďalšie rozloženie |

Tieto metódy **nie sú v Wayfire 0.9.0 z Debianu**; zabezpečuje ich vlastný backport pluginu `ipc-rules`.

**b) Autohide** — `autohide_session` (iba ak `[autohide] enabled = true`):

| Metóda / udalosť | Použitie |
|---|---|
| `window-rules/get_cursor_position` | poloha kurzora (`{pos:{x,y}}`); **opäť len v backporte**; bez nej autohide skončí s chybou a skúša znova každé 3 s |
| `window-rules/get-focused-output` | geometria aktívneho výstupu (`info.geometry`) |
| `window-rules/list-views` | zoznam okien (`type`, `mapped`, `minimized`, `app-id`, `geometry`, `sticky`) |
| `window-rules/events/watch` | odber udalostí; prepočet pri: `view-mapped`, `view-unmapped`, `view-tiled`, `view-fullscreened`, `view-minimized`, `view-focused`, `view-set-output`, `view-geometry-changed`, `view-wset-changed`, `view-workspace-changed`, `wset-workspace-changed`, `output-gain-focus`, `output-wset-changed`, `view-sticky` |

Algoritmus autohide: kapsula sa skryje, keď nejaké (nie ignorované) okno aktívneho workspace zasahuje do pásma
`bar_height + margin` od hornej hrany **a** kurzor nie je v oblasti lišty; zobrazí sa, keď kurzor vstúpi do tenkého
pásu `edge` px pri hornej hrane (vodorovne stredných `edge_width` % šírky výstupu). Kým okno zasahuje, číta sa
kurzor každých `poll_ms` (predvolene 30 ms).

Požiadavky na **konfiguráciu Wayfire**: zapnuté pluginy `ipc` a `ipc-rules` (v backportovanej verzii); `WAYFIRE_SOCKET`
musí byť v prostredí ostrovčeka (pri systemd službe cez `import-environment`).

**Čo robiť pri zmene kompozitora / verzie:**

- *Nová verzia Wayfire:* ak oficiálny `ipc-rules` už obsahuje `get_cursor_position` a metódy pre klávesnicu, backport
  netreba — over **presné názvy** metód (v novších verziách sa môžu volať inak, napr. `window-rules/...` vs.
  `wayfire/...`) a upraviť reťazce v `wayfire.rs` (je ich ~10 na jednom mieste).
- *Iný kompozitor (sway, Hyprland, river, niri…):* `wayfire.rs` treba nahradiť ekvivalentom:
  - rozloženie klávesnice: sway `swaymsg -t get_inputs` / `input ... xkb_switch_layout next`, Hyprland
    `hyprctl devices -j` / `hyprctl switchxkblayout`; výstup treba mapovať na rovnaké pole (`layout`, `index`, `počet`);
  - autohide: potrebuje (1) zoznam okien s geometriou a stavom, (2) udalosti o ich zmene, (3) polohu kurzora.
    Pre (1)+(2) existuje štandardný protokol `wlr-foreign-toplevel-management` (bez IPC), polohu kurzora ale
    protokol nedáva — riešenie je vlastná tenká vrstva na hornej hrane, ktorá zachytí hover, alebo IPC kompozitora.
  - najjednoduchší prechod: vypnúť autohide (`[autohide] enabled = false`) a skrývať signálmi `SIGUSR1`/`SIGUSR2`;
    modul `keyboard` vtedy z `modules` vynechať (alebo `wayfire.rs` nahradiť stubom, ktorý vráti `None`).
- Bez `WAYFIRE_SOCKET` sa nič nerozbije: IPC vlákna sa len opakovane pokúšajú pripojiť (rozloženie: každých ~20 cyklov,
  autohide: každé 3 s) a modul `keyboard` zostane prázdny.

### 5.2 Wayland protokoly

| Protokol | Použitie | Poznámka |
|---|---|---|
| `wl_compositor` (3–6), `wl_seat` (1–7: pointer, keyboard), `wl_output` | povrch, vstup, mierka | štandard |
| **`zwlr_layer_shell_v1`** (1–4) | povrch lišty (`Overlay`, kotva `Top`, `exclusive_zone 0`) | **povinné**; majú ho wlroots kompozitory (Wayfire, sway, river…), Hyprland, niri, KWin; **nie GNOME/Mutter** |

Ostrovček sa bez `zwlr_layer_shell_v1` ukončí s chybou „kompozitor nepodporuje zwlr_layer_shell_v1“.
Idle protokol (`ext-idle-notify-v1`) sa už nepoužíva (funkcia odstránená; nečinnosť rieši externý `swayidle`).

### 5.3 Externé programy pevne v kóde

Spúšťajú sa podľa **názvu** (cez `PATH`), v konfigurácii sa **nedajú** zmeniť, pokiaľ nie je uvedené inak.

| Program | Miesto | Účel | Pri zmene prostredia |
|---|---|---|---|
| `wpctl` | `providers.rs`, `main.rs`, `app.rs` | čítanie stavu a nastavenie hlasitosti/mute (`@DEFAULT_AUDIO_SINK@`) | nahradiť pri inom audio serveri (PulseAudio: `pactl`/`pamixer`) |
| `pactl` | `providers.rs` (`subscribe` → udalosti), `audio.rs` (zoznam zariadení, predvolené, hlasitosť) | panel zvuku, sledovanie zmien | funguje s `pipewire-pulse` aj PulseAudio |
| `iw` (aj `/usr/sbin/iw`) | `providers.rs` | SSID, frekvencia, signál | záložne `iwgetid`, `nmcli` |
| `ip` | `providers.rs` | IPv4 adresa, predvolená brána | štandard (`iproute2`) |
| `ddcutil` | `monitor.rs` | jas (VCP 0x10), kontrast (0x12), teplotný preset (0x14), vstup (0x60), cez I²C `bus` | bus a kódy sa nastavujú v `[monitor]` |
| `grim`, `slurp`, `wf-recorder` (+ `wl-copy`, `notify-send`, `pkill`) | `capture.rs` | snímka obrazovky / okna / oblasti, nahrávanie videa; aktívne okno a výstup z Wayfire IPC (`window-rules/get-focused-view`, `get-focused-output`) | iný nástroj = zmena kódu |
| *(tvoj skript)* | `monitor.rs` | nočný režim: `night_switcher` (toggle) a voliteľne `night_state` | ľubovoľný nástroj (wlsunset, gammastep, hyprsunset…) |
| `bt-agent` / `bluetoothctl` (+ `pgrep`, `sh`) | `bluetooth.rs` | agent párovania (NoInputNoOutput) | bez agenta párovanie nových zariadení často zlyhá |
| `magick` / `convert` | `wallpaper.rs`, `mpris.rs` | náhľady tapiet; prevod obalov (`mpris:artUrl`, najčastejšie JPEG) na PNG | ImageMagick 6 aj 7 |
| `wl-paste` | `wifi.rs` | vloženie hesla (Ctrl+V) | `wl-clipboard` |
| `sh` | všade | spúšťanie príkazov z configu | |
| `lumix` | **config** (`[brightness] get`/`set`) | jas | nahraditeľné v configu |
| `cliphist`, `wl-copy` | **config** (`[clipboard] list_command`/`copy_command`) | schránka | nahraditeľné v configu |
| `nm-connection-editor` | **config** (`[wifi] editor_command`) | tlačidlo správcu pripojení | nahraditeľné |
| skript tapiet | **config** (`[wallpaper] command`) | nastavenie tapety, pozri [5.6](#56-osobné-skripty-a-konvencie) | nahraditeľné, ale rozhranie je pevné |

### 5.4 D-Bus služby

Vlastný D-Bus klient (`dbus.rs`): session bus cez `/run/user/UID/bus`, system bus cez `/run/dbus/system_bus_socket`.

| Služba / meno | Bus | Modul | Poznámka |
|---|---|---|---|
| `org.freedesktop.Notifications` | session | notifikácie | ostrovček je **server**; nemôže bežať ďalší notifikačný démon (meno je obsadené → ostrovček to vypíše a skúša každých 5 s) |
| `org.freedesktop.NetworkManager` (+ `.Device`, `.Device.Wireless`, `.AccessPoint`, `.Settings`, `.Settings.Connection`) | system | panel Wi-Fi | **iba NetworkManager**; `iwd`/`systemd-networkd` by vyžadovali nový backend |
| `org.mpris.MediaPlayer2.*` (`Player`, `Properties`) | session | modul `media` | ľubovoľný prehrávač s MPRIS (fooyin, mpv s `mpris`, prehliadač, Spotify…) |
| `org.bluez` (`Adapter1`, `Device1`, `Battery1`, ObjectManager) | system | panel Bluetooth | BlueZ |
| `org.freedesktop.UDisks2` (`Block`, `Drive`, `Filesystem`) | system | modul `disks` | **UDisks2** (automount, odpojenie, vysunutie) |

Sú to freedesktop štandardy, takže sú prenositeľné medzi distribúciami a kompozitormi; viazané sú len na konkrétne
služby (nie na Debian).

### 5.5 Linux jadro (sysfs / procfs)

| Cesta | Použitie |
|---|---|
| `/sys/class/net/<iface>/{operstate,wireless,flags}` | stav rozhrania, detekcia Wi-Fi, VPN rozhrania (`flags & 1`) |
| `/proc/net/wireless` | úroveň signálu (dBm) |
| `/sys/class/leds/*::capslock/brightness` | Caps Lock (LED musí existovať a byť čitateľná bez root) |
| `/proc/stat`, `/proc/meminfo`, `/proc/net/dev`, `/proc/net/route` | `builtin:cpu`, `builtin:ram`/`swap`, `builtin:net-*` (rozhranie s predvolenou trasou) |
| `/sys/class/thermal/thermal_zone*/{temp,type}`, `/sys/class/hwmon/*/{name,temp*_input,temp*_label}` | `builtin:temp` |
| `statvfs(3)` (libc) | `builtin:disk*` – rovnaké čísla ako `df` |
| `/proc/mounts`, `/proc/self` | zistenie pripojených diskov a UID pre kôš |
| `/run/media/…` | len v testoch/ukážkach; skutočné mountpointy dáva UDisks2 |

### 5.6 Osobné skripty a konvencie

- **Tapety a témy** (`[wallpaper] command`, predvolene `~/.config/wayfire/scripts/wallpaper`, Python skript nad `awww` +
  `matugen`). Ostrovček sám tapetu ani farby nenastavuje, iba volá skript; **rozhranie je pevné v `wallpaper.rs`**:

  | Volanie | Kedy | Čo skript robí |
  |---|---|---|
  | `command -v CESTA` | klik na náhľad (režim „wallpaper“) | tapeta cez `awww` + farby z obrázka cez `matugen image CESTA --prefer darkness`; zmaže `applied-theme` |
  | `command -w CESTA` | klik na náhľad, keď je vybraná téma | iba tapeta, farby sa nemenia |
  | `command -t CESTA.json` | výber témy v rozbaľovacej ponuke | `matugen json CESTA.json` (iba farby), zapíše názov súboru bez `.json` do `applied-theme`; **argument je cesta k súboru, nie názov** |

  Návratový kód 0 = úspech; až vtedy ostrovček zapíše `cache_dir/current` (pri `-v`/`-w`) resp. zapamätá tému (`-t`).
  Skript sám používa `-r` (náhodná; viazané napr. na `on_click_middle` modulu) a `-p` (obnova po štarte, v autoštarte
  Wayfire); tie ostrovček nevolá. Skript zapisuje `current` iba pri `-r`/`-p`, ostrovček pri `-v`/`-w`.
  Stav v `cache_dir`: `current` (cesta k obrázku), `applied-theme` (názov témy; chýba = režim „wallpaper“, farby podľa
  obrázka). Ostrovček súbor `applied-theme` maže pri návrate do režimu „wallpaper“.
  Súbežnosť: ostrovček serializuje volania vlastným zámkom, skript `flock`-om (`$XDG_RUNTIME_DIR/wallpaper-cli.lock`).
  **Väzba na farby:** ostrovček farby len číta z `colors_file`; ten musí generovať **šablóna v konfigurácii matugenu**
  (`~/.config/matugen/config.toml`, výstup tej istej cesty ako `colors_file`). Pri zmene cesty/názvu `colors_file`
  uprav aj šablónu matugenu, inak sa farby po zmene témy/tapety neprejavia. Témy (`*.json`) sa hľadajú v `themes_directory`.
  Závislosti skriptu (nie ostrovčeka): `python3`, `awww`/`awww-daemon`, `matugen`, voliteľne `notify-send`.
- **Farby** (`[island] colors_file`): CSS s riadkami `@define-color meno #hex;` (formát matugen šablóny pre GTK).
  Vzorový config používa `~/.config/dynamic-island/colors.css`; **predvolená hodnota v kóde je stále
  `~/.config/waybar/waybar-colors.css`** (historická, zmeň v `config.rs`, ak ju nechceš).
- **Inhibítor nečinnosti** (`~/.idle_inhibit`): súbor vytvára/maže Wayfire skript viazaný na Super+Z; ostrovček
  len sleduje existenciu (ikona). Samotné blokovanie nečinnosti rieši kompozitor/`swayidle`.
- **Rekordér**: odznak REC ukazuje odpočet a čas nahrávania z panelu `capture`.
- **Jas** (`lumix`): pozri [6.10](#610-brightness) pre kontrakt príkazu.
- **Skripty dashboardu** (`config/dynamic-island/scripts/*.sh`): ukážkové skripty pre položky menu s `exec`
  (cpu, ram, teplota, disk, kôš…); sú osobné, kopírujú sa s configom.

### 5.7 Lokalizácia a texty

- **UI texty sú po slovensky natvrdo v kóde** (napr. „Obnoviť vybrané“, „Vyprázdniť kôš“, „Natrvalo vymazať N položiek?“,
  „Párujem“, hlásenia chýb). Pre iný jazyk treba texty vytiahnuť zo zdrojákov (grep na diakritiku v `ui.rs`, `trash.rs`,
  `bluetooth.rs`, `wifi.rs`, `monitor.rs`, `disks.rs`).
- Dátum a mesiace: `setlocale(LC_TIME)` z `[clock] locale` (predvolene **`sk_SK.UTF-8`**). Lokalita musí byť
  vygenerovaná v systéme (`locale -a`, balík `locales`), inak sa použije predvolená.
- Názvy rozložení klávesnice sa mapujú z názvov kompozitora (v Wayfire „Slovak“, „English (US)“) cez
  `[keyboard.names]`; predvolené mapovanie: `Slovak → SK`, `English (US) → US`. **Iný kompozitor môže používať iné
  názvy** — doplň ich do configu.

### 5.8 Špecifiká Debianu

- Cesty `/usr/sbin/iw`, `/usr/sbin/iwgetid` (v Debiane `sbin` často nie je v `PATH` bežného používateľa) — preto sa
  skúšajú aj s plnou cestou. Na distribúciách s `iw` v `/usr/bin` stačí `PATH`.
- Názvy balíkov v tabuľke 2.3 sú debianovské.
- Rust ≥ 1.85 je v Debiane 13 v repozitári (`cargo`); na staršej distribúcii treba `rustup`.

### 5.9 Kontrolný zoznam migrácie

1. `cargo build --release` na novom systéme; `cargo test`.
2. Skontroluj runtime knižnice (`libEGL.so.1`, `libGLESv2.so.2`, `libxkbcommon`) a písma (`fc-list`, `icon_family`).
3. Kompozitor: podporuje `zwlr_layer_shell_v1`? Ak nie → ostrovček nepôjde.
4. IPC kompozitora: pozri [5.1](#51-kompozitor-wayfire-ipc-srcwayfirers--najväčšia-väzba); minimálne vypni autohide
   a odstráň modul `keyboard`.
5. Audio: `wpctl status`, `pactl info`. Sieť: `ip`, `iw`; NetworkManager beží? (`nmcli`).
6. D-Bus: `busctl --user status org.freedesktop.Notifications` (nesmie ho držať iný démon); UDisks2, BlueZ bežia.
7. `ddcutil detect` → nastav `[monitor] bus`, over VCP kódy svojho monitora (predvolené sú pre Iiyama PL2493H).
8. Presuň/skontroluj osobné skripty (`lumix`, tapety, `~/.idle_inhibit` skript).
9. Prenes `~/.config/dynamic-island/` (config, `icons/`, `scripts/`, `colors.css`).
10. `dynamic-island --check`, potom spustiť a sledovať stderr/`journalctl`.

---

## 6. Referencia konfigurácie

Súbor: `~/.config/dynamic-island/config.toml` (alebo `--config`). Formát je **podmnožina TOML** (vlastný parser
`toml.rs`): sekcie `[x]`, `[x.y]`, polia tabuliek `[[x]]`, reťazce v `"..."` (escape `\n`, `\uXXXX`, `\UXXXXXXXX`)
a `'...'` (bez escape), čísla, `true`/`false`, polia `[...]`, komentáre `#`. Chýbajúci kľúč = predvolená hodnota,
neplatná číselná hodnota sa orezáva do uvedeného rozsahu. Súbor, ktorý neexistuje, = všetko predvolené.
Úplný ukážkový config: `config/dynamic-island/config.toml`.

V popisoch: **px** = logické pixely (pred násobením mierkou); hodnoty v zátvorke `(a–b)` sú rozsah orezania.

### 6.1 Kľúče najvyššej úrovne

| Kľúč | Predvolené | Popis |
|---|---|---|
| `modules` | `["network","vpn","clock","keyboard","audio","capslock","inhibitor","recorder","radio","disks"]` | poradie modulov v kapsule; názvy `[[custom]]` modulov sa pridajú na koniec, ak v zozname nie sú |

### 6.2 `[island]` — vzhľad kapsule

| Kľúč | Predvolené | Popis |
|---|---|---|
| `font_family` | `["Adwaita Sans","DejaVu Sans"]` | písma textu (pole alebo `"A, B"`) |
| `icon_family` | `["Hack Nerd Font Propo"]` | písma ikon (Nerd Font) |
| `font_size` | `17` | veľkosť písma (px) |
| `font_scale` | `1.0` (0.5–3) | jednotný násobiteľ **všetkých** veľkostí písma (kapsula aj všetky panely); násobí sa po načítaní, takže geometria panelov (výška riadkov) sa prispôsobí. Výšku kapsuly (`height`) a ikony záložiek (`tab_size`, `icon_size`) neovplyvňuje |

**Veľkosti písma.** Každá časť má vlastné základné písmo, nezávislé od `[island] font_size`: kapsula, tooltip, menu, kalendár, zoznam diskov a tapety používajú `[island] font_size` (menšie texty v nich sú jeho násobok 0,8–0,9, tooltip 0,85); panely majú vlastný kľúč — `[media] font_size` (16), `[notifications] font_size` (15), `[clipboard] font_size` (15), `[wifi]`, `[bluetooth]`, `[monitor]`, `[capture]` `font_size` (15), `[audio] panel_font_size` (15) a `[trash] text_size` (14). Vedľajšie texty v paneli (popisky, podnadpisy, stavové riadky) sú pevný pomer 0,78–0,9 písma toho panela, preto ich zväčšíš zväčšením `font_size` panela alebo celkovo cez `font_scale`.
| `height` | `34` | výška zbalenej kapsule (px) |
| `margin_top` | `6` | odstup od hornej hrany (px) |
| `radius` | `17` | polomer zaoblenia zbalenej kapsule |
| `radius_expanded` | `26` | polomer pri rozbalení (kalendár, tooltip, panely) |
| `opacity` | `0.75` | krytie pozadia (0–1) |
| `shadow` | `true` | tieň |
| `surface_width` / `surface_height` | `760` / `560` | veľkosť priehľadného povrchu = **horná hranica veľkosti rozbaleného panela** |
| `tooltip_delay_ms` | `450` | oneskorenie tooltipu po nabehnutí myši na modul |
| `collapse_delay_ms` | `700` | oneskorenie zbalenia po odchode myši |
| `poll_ms` | `300` (min. 50) | interval kontroly caps LED / inhibítora / recordera / rozloženia / súborov `track_file` |
| `colors_file` | `~/.config/waybar/waybar-colors.css` | CSS s `@define-color` (pozri 6.3) |

### 6.3 `[colors]` — farby

Hodnota: `"#rgb"`, `"#rrggbb"`, `"#rrggbbaa"` alebo `"@meno"` (farba z `colors_file`). Neplatná/chýbajúca hodnota →
predvolená (a ak chýba aj paleta, vstavané `#181115` / `#fcb0d6`).

| Kľúč | Predvolené | Použitie |
|---|---|---|
| `background` | `@surface` | pozadie kapsule |
| `foreground` | `@primary` | text a ikony |
| `accent` / `on_accent` | `@primary` / `@surface` | odznaky, aktívne prvky / text na nich |
| `menu_selected` / `menu_selected_text` | `accent` / `on_accent` | vybraná (hover) položka textového menu, schránky |
| `alert` / `on_alert` | `#dc2f2f` / `#ffffff` | recorder, „Vyprázdniť kôš“, odpojené |
| `urgent_border` / `urgent_text` | `#fa3939` / `@primary` | urgentné notifikácie |

`colors_file` sa **sleduje živo**; formát (matugen/GTK CSS): `@define-color meno #hex;`, komentáre `/* … */` sa ignorujú.
Používajú sa mená `surface`, `primary` a ľubovoľné ďalšie, na ktoré sa odkazuje `@meno`.

### 6.4 Tlačidlá modulov (`on_click*`) a tooltipy

Každý modul (aj `[[custom]]`) prijíma:

| Kľúč | Popis |
|---|---|
| `on_click` | ľavé tlačidlo |
| `on_click_right` | pravé tlačidlo |
| `on_click_middle` | stredné tlačidlo (predvolene bez akcie) |
| `tooltip` | text tooltipu (`\n` = nový riadok; prázdne = žiadny) |

Hodnota je **vstavaná akcia** alebo **shell príkaz**. Prázdna hodnota = nič. Vstavané akcie:

| Akcia | Účinok |
|---|---|
| `calendar` | kalendár (koliesko = zmena mesiaca) |
| `clipboard`, `notifications`, `media`, `trash`, `wallpaper`, `wifi`, `bluetooth`, `audio`, `monitor` | príslušný panel (`notifications` = história notifikácií) |
| `capture` | panel snímania ([6.14b](#614b-capture--snímka-obrazovky-a-nahrávanie)); ak práve beží odpočet alebo nahrávanie (svieti odznak REC), klik ich **ukončí** |
| `module:NAZOV` | aj v `on_click` modulu (nielen v menu): `module:capture`, `module:monitor`, `module:power` (iné menu)… |
| `capture-stop` | zrušiť odpočet / ukončiť nahrávanie (aj nahrávanie `wf-recorder` spustené mimo ostrovčeka) |
| `quickset`, `quickset:audio` / `:bluetooth` / `:wifi` / `:monitor` / `:menu:NAZOV` (aj ostatné záložky; starý názov `hardware` ešte funguje) | kontajner záložiek (posledná / konkrétna záložka) |
| `menu` | zoznam diskov (modul `disks`) |
| *názov `[[menu]]`* | vlastné menu |
| `eject-all` | odpojiť všetky disky |
| `inhibit-toggle` | prepnúť inhibítor nečinnosti (vytvorí/zmaže súbor) |
| `layout-toggle` | ďalšie rozloženie klávesnice (Wayfire IPC) |
| `audio-mute` | prepnúť mute (`wpctl`) |
| `track-stop:SKUPINA` | ukončiť sledovaný proces skupiny (napr. `track-stop:radio`) |
| iné | shell príkaz (spustený oddelene; jednoduchý priamo, inak `sh -c`) |

Predvolené akcie: `clock` → `calendar`; `audio` → `audio-mute`; `keyboard` → `layout-toggle`; `inhibitor` →
`inhibit-toggle`; `radio` → `track-stop:radio`; `disks` → ľavé `menu`, pravé `eject-all`; ostatné nič.

V `command` položky `[[menu.item]]` sa vstavané akcie volajú s predponou **`module:`** (`module:calendar`,
`module:power` = iné menu, `module:disks`, `module:clipboard`, `module:notifications`, `module:media`, `module:media-play-pause`, `module:media-next`, `module:media-prev`, `module:trash`, `module:wallpaper`, `module:wifi`,
`module:quickset`, `module:quickset:wifi`, `module:audio-mute`, `module:layout-toggle`, `module:inhibit-toggle`,
`module:eject-all`, `module:track-stop:radio`). Bez predpony je `command` vždy shell príkaz. Neznámy názov sa ignoruje.

Zástupné symboly v tooltipoch: sieť `{ssid} {ifname} {ip} {gateway} {signal} {dbm} {freq}`; VPN `{ifname} {ip}`;
klávesnica `{layout}`; audio `{volume}`; recorder `{text}`; radio `{name}`; disks `{icon} {count} {label} {list}`;
custom `{text} {tooltip} {class}`; hodiny: formát `strftime`.

### 6.5 `[clock]`

| Kľúč | Predvolené | Popis |
|---|---|---|
| `format` | `"%A  %d. %B,  %H:%M"` | `strftime` formát času v kapsule |
| `locale` | `"sk_SK.UTF-8"` | lokalita pre názvy dní/mesiacov (musí byť vygenerovaná v systéme) |
| `tooltip` | `""` | `strftime` formát tooltipu |
| `on_click*` | `calendar` | pozri 6.4 |

### 6.6 `[network]`

| Kľúč | Predvolené | Popis |
|---|---|---|
| `interface` | `"wlan0"` | sledované rozhranie (Wi-Fi alebo ethernet) |
| `icons` | 4 glyfy `f091f f0922 f0925 f0928` | ikony podľa sily signálu (od najslabšieho) |
| `icon_ethernet` | `f0200` | ikona pri drôtovom rozhraní |
| `icon_disconnected` | `f092d` | ikona pri odpojení |
| `tooltip_wifi` | `"SSID: {ssid}\nRozhranie: {ifname}\nIP adresa: {ip}\nSila signálu: {signal}% ({dbm} dBm)\nFrekvencia: {freq} GHz"` | |
| `tooltip_ethernet` | `"Rozhranie: {ifname}\nIP adresa: {ip}\nBrána: {gateway}"` | |
| `tooltip_disconnected` | `""` | |
| `on_click*` | — | napr. `on_click = "wifi"` otvorí panel Wi-Fi |

### 6.7 `[vpn]`

| Kľúč | Predvolené | Popis |
|---|---|---|
| `interfaces` | `["tun*"]` | vzory názvov rozhraní (koncová `*` = prefix); odznak sa zobrazí, kým je niektoré `UP` |
| `text` | `"VPN"` | text odznaku |
| `tooltip` | `"{ifname}: {ip}"` | |
| `on_click*` | — | |

### 6.8 `[keyboard]`, `[capslock]`, `[inhibitor]`, `[recorder]`, `[radio]`

| Sekcia | Kľúč | Predvolené | Popis |
|---|---|---|---|
| `[keyboard]` | `on_click*`, `tooltip` | `layout-toggle`, `"Klávesnica SK/US"` | |
| `[keyboard.names]` | `"Názov v kompozitore" = "SKR"` | `Slovak = "SK"`, `"English (US)" = "US"` | mapovanie názvov rozložení na skratky (dopĺňa predvolené) |
| `[capslock]` | `text` | `"CAPS"` | text odznaku |
| `[inhibitor]` | `icon` | `f0f4` | ikona |
| | `file` | `~/.idle_inhibit` | súbor-príznak (existuje = aktívny) |
| | `on_click*`, `tooltip` | `inhibit-toggle`, `"Idle Inhibitor"` | |
| `[recorder]` | `on_click` | *(prázdne)* | napr. `"capture"`: klik na odznak ukončí odpočet / nahrávanie |
| | `on_click*`, `tooltip` | —, — | tooltip: `{text}` |
| `[radio]` | `icon` | `f0439` | ikona sledovaného procesu skupiny `radio` |
| | `tooltip` | `"{name}\nKlik = vypnúť"` | `{name}` = `label` položky menu |
| | `on_click*` | `track-stop:radio` | |

### 6.8b `[media]` — Práve hrá (MPRIS)

Modul `media` (treba ho dať do `modules`) ukáže ikonu, kým je nejaký prehrávač MPRIS aktívny (hrá alebo je pozastavený
so skladbou); pozastavený je stlmený a má ikonu `icon_paused`. Zdroj: `org.mpris.MediaPlayer2.*` na session D-Bus; stav sa
nesleduje cyklicky, ale signálmi. Pri viacerých prehrávačoch sa zobrazí ten, čo hrá (zvolený zostáva, kým hrá).

| Kľúč | Predvolené | Popis |
|---|---|---|
| `icon` / `icon_paused` | `\uf001` / `\uf04c` | ikona pri prehrávaní / pozastavení |
| `tooltip` | `"{artist} – {title}"` | aj `{album}` |
| `ignore` | `[]` | prehrávače, ktoré sa preskočia (podreťazec mena na zbernici, napr. `"chromium"`) |
| `panel_width` | `460` (300–900) | šírka panela |
| `art_size` | `128` (80–256) | veľkosť obalu (px) |
| `font_size` | `16` (10–32) | písmo panela |
| `on_click` / `on_click_right` / `on_click_middle` | `media` / `media-play-pause` / `media-next` | pozri 6.4 |

**Panel „Práve hrá“** (akcia `media`): obal vľavo, vpravo názov, interpret, album, ukazovateľ pozície (čas | lišta |
dĺžka) a tlačidlá predošlá / play-pauza / ďalšia; vpravo hore je **šípka zbalenia**. Panel sa **nezatvára odchodom myši**,
zbalí sa iba šípkou (alebo opätovným klikom na ikonu); notifikácia ho dočasne nahradí a po doznení sa znova otvorí.
Pozícia sa číta iba kým je panel otvorený a prehrávač hrá (raz za sekundu). Text piesne sa nezobrazuje.

**Obal:** z `mpris:artUrl`. Berie sa iba `file://…`; obrázky iné než PNG (JPEG…) sa cez ImageMagick (`magick`/`convert`,
max. 256 px) prekonvertujú do `~/.cache/dynamic-island/media-art/`. `http(s)://` adresy (napr. prehliadač) sa
nesťahujú – zobrazí sa zástupný štvorec s notou. Bez ImageMagicku sa zobrazia iba obaly vo formáte PNG.
Rohy obalu (a ikôn notifikácií) sa zaoblia v shadri obrázkov (`Item::Image { radius }`, polomer = 14 % veľkosti, vyhladená hrana); ostatné obrázky (ikony menu, náhľady tapiet) majú `radius: 0`.
Overenie prehrávača: `playerctl -p NAZOV metadata` / `busctl --user list | grep -i mpris`.

### 6.9 `[audio]`

| Kľúč | Predvolené | Popis |
|---|---|---|
| `icon_muted` | `eee8` | ikona pri mute (odznak sa zobrazuje **iba pri mute**) |
| `icons` | `f026 f027 f028` | ikony hlasitosti (HUD) |
| `hud` | `true` | ukázať HUD pri zmene hlasitosti |
| `hud_ms` | `1600` | trvanie HUD |
| `step` | `5` | krok hlasitosti v % (skratky aj koliesko nad posuvníkom v paneli) |
| `limit` | `1.0` | strop hlasitosti (1.0 = 100 %) |
| `tooltip` | — | `{volume}` |
| `on_click*` | `audio-mute` | |
| `panel_font_size` | `15` (8–64) | písmo panela zvuku |
| `panel_width` | `380` (200–1000) | šírka panela |
| `panel_rows` | `6` (1–30) | najviac zariadení v jednej sekcii |

### 6.10 `[brightness]`

| Kľúč | Predvolené | Popis |
|---|---|---|
| `step` | `10` | krok pre `--brightness up/down` (%) |
| `get` | `"lumix get"` | príkaz, ktorý vypíše aktuálny jas |
| `set` | `"lumix set \"{value}\""` | príkaz nastavenia; `{value}` = `STEP+` / `STEP-` (napr. `10+`) alebo celé číslo (percento) |
| `icons` | `f00de f00df f00e0` | ikony HUD jasu |

**Kontrakt príkazov:** výstup (stdout + stderr) musí obsahovať **percento** (`NN%`), prvé nájdené sa zobrazí v HUD.
Pri inom nástroji (`brightnessctl`, `ddcutil`) uprav `get`/`set` tak, aby spĺňali túto dohodu (prípadne cez obalový skript).

### 6.11 `[disks]`

| Kľúč | Predvolené | Popis |
|---|---|---|
| `icon` | `f287` | ikona USB |
| `format` | `"{icon}"` | text modulu pri jednom médiu |
| `format_many` | `"{icon} {count}"` | text pri viacerých médiách |
| `automount` | `true` | automaticky pripájať vymeniteľné média (UDisks2) |
| `notify` | `true` | notifikácie o pripojení/odpojení/chybách |
| `notify_timeout_ms` | `5000` | trvanie týchto notifikácií |
| `tooltip` | `"{list}"` | zoznam médií |
| `on_click` / `on_click_right` | `menu` / `eject-all` | ľavý: jedno médium sa odpojí rovno, pri viacerých zoznam |

Modul sa zobrazí len keď je nejaké médium pripojené; počíta sa disk, nie oddiel.

### 6.12 `[quickset]` (záložky)

Kontajner so záložkami sa volá **quickset** (predtým `hardware`). Sekcia `[hardware]` a akcie `hardware…` sa ešte čítajú, ak `[quickset]` chýba, nové configy ich používať nemajú.

Kontajner s lištou záložiek okolo panelov. Záložkou môže byť `audio`, `bluetooth`, `wifi`, `monitor`, `capture`, `clipboard`, `trash`, `notifications` (história notifikácií), `wallpaper` alebo `menu:NAZOV`. Panely ostávajú otvárateľné aj samostatne.

**Kritériá:** záložkou môže byť veľký panel s predvídateľnou veľkosťou, ktorý sa zatvára rovnako ako ostatné (odchodom myši, opakovaným klikom). Preto **nie** `media` (lepkavý panel, ktorý sa odchodom myši nezatvára) a nie kalendár. Výška okna kontajnera je výška najvyššej záložky; veľký panel (`wallpaper`) natiahne všetky, preto pri ňom nastav `width` / `height`.

| Kľúč | Predvolené | Popis |
|---|---|---|
| `tabs_position` | `"left"` | `left` \| `right` \| `top` \| `bottom` |
| `tab_size` | `44` (24–120) | veľkosť tlačidla záložky (px) |
| `icon_size` | `22` (8–64) | veľkosť ikony |
| `tab_gap` | `6` (0–64) | medzera medzi tlačidlami záložiek |
| `inversetab` | `false` | zapne druhú lištu na **opačnej** strane (`tabs_position = "left"` → vpravo, `top` → dole…); akceptuje aj `"true"` |
| `inversetab_gap` | ako `tab_gap` (0–64) | medzera medzi tlačidlami druhej lišty |
| `inversetab1` … `inversetab8` | — | tlačidlo druhej lišty: **príkaz** (`systemctl suspend`) alebo názov panela / menu / `module:NAZOV`, ktorý sa otvorí **samostatne** (nie v kontajneri; `menu:radio` a `radio` sú to isté). Po kliku sa kontajner zatvorí |
| `inversetab1_icon` … | predvolená ikona panela, inak `f0e7` | Nerd Font glyf; veľkosť tlačidla a ikony sa preberá z `tab_size` a `icon_size` |
| `width` / `height` | — (120–3000) | pevná celková veľkosť okna vrátane lišty; bez nich sa riadi obsahom |
| `preload` | `true` | načítať údaje monitora (ddcutil je pomalý) už pri otvorení kontajnera |
| `tab1` … `tab8` | štyri v poradí `audio, bluetooth, wifi, monitor` | modul: `audio`, `bluetooth`, `wifi`, `monitor`, `capture`, `clipboard`, `trash`, `notifications`, `wallpaper` alebo `menu:NAZOV` (`NAZOV` = `name` z `[[menu]]`); neplatný názov sa preskočí |
| `tab1_icon` … | `f057e` audio, `f00af` bluetooth, `f0928` wifi, `f0379` monitor, `f0ea` schránka, `f1f8` kôš, `f0f3` notifikácie, `f03e` tapety, ozubené koleso `f0493` pre menu | Nerd Font glyf |

Opakovaný klik na tú istú záložku kontajner zavrie. Záložka s menu: položky s `exec` sa spustia pri prepnutí,
`wait_exec` sa neuplatní, klik na položku vykoná `command` a zavrie okno. Ak menu neexistuje, záložka sa zobrazí, no nedá
sa otvoriť. Pozor na podmnožiny Nerd Fontu — `f00af` a `f0379` nemusia byť obsiahnuté.

### 6.13 `[wifi]`, `[bluetooth]`

| Kľúč | `[wifi]` | `[bluetooth]` | Popis |
|---|---|---|---|
| `font_size` | `15` (8–64) | `15` | písmo |
| `rows` | `8` (1–30) | `8` | počet viditeľných riadkov (viac = skrolovanie) |
| `width` | `420` (200–1000) | `380` (200–1000) | šírka panela |
| `editor_command` | `"nm-connection-editor"` | `""` | príkaz tlačidla v hlavičke (prázdne = bez tlačidla) |
| `editor_icon` / `editor_label` | `""` / `""` | `""` / `""` | ikona a/alebo text tlačidla; ak sú obe prázdne, tlačidlo sa nezobrazí |
| `scan_ms` | — | `4000` (500–30000) | trvanie skenovania pri obnovení |

**Wi-Fi panel:** klik na uloženú/otvorenú sieť ju pripojí, na aktívnu odpojí; nová zabezpečená sieť otvorí zadávanie
hesla (WPA2/WPA3-PSK; Enter pripojí, Esc zruší, Ctrl+V vloží schránku cez `wl-paste`; pri zlom hesle sa profil zmaže);
pravý klik zabudne uloženú sieť. Hlavička: prepínač rádia, obnovenie, názov siete, tlačidlo editora. Nepodporované:
802.1X, WEP. Vyžaduje právo meniť siete (polkit).

**Skrytá sieť:** posledný riadok zoznamu „Skrytá sieť…“. Klik otvorí dvojkrokové zadanie: najprv názov siete (SSID,
Enter = ďalej), potom heslo. Prázdne heslo = otvorená sieť; s heslom (min. 8 znakov) sa skúsi WPA/WPA2 (`wpa-psk`) a ak
to zlyhá, WPA3 (`sae`). Profil sa v NetworkManageri vytvorí s `hidden=true` (hľadá sa cielene podľa SSID); pri
neúspechu sa zmaže. Po úspešnom pripojení zostane uložený a pripája sa sám.

### 6.14 `[monitor]`

| Kľúč | Predvolené | Popis |
|---|---|---|
| `font_size` | `15` (8–64) | písmo |
| `width` | `380` (240–800) | šírka panela |
| `bus` | `"10"` | I²C bus pre `ddcutil --bus` (`ddcutil detect`) |
| `presets` | `[5, 6, 8]` | VCP hodnoty teplotného presetu (VCP 0x14) |
| `preset_labels` | `["6500 K","7500 K","9300 K"]` | popisy (rovnaký počet ako `presets`, inak `0xNN`) |
| `inputs` | `[15, 17]` | VCP hodnoty vstupu (VCP 0x60; 0x0F = DisplayPort, 0x11 = HDMI) |
| `input_labels` | `["DisplayPort","HDMI"]` | popisy |
| `night_switcher` | *(prázdne)* | príkaz alebo skript, ktorý **prepne** nočný režim (toggle). Spúšťa sa oddelene a ostrovček naň nečaká, takže môže bežať aj natrvalo (napr. `wlsunset` v popredí). Ak chýba, prepínač sa v paneli nezobrazí |
| `night_state` | *(prázdne)* | voliteľný príkaz zisťujúci stav: návratový kód 0 = nočný režim beží (napr. `pgrep -x wlsunset`). Bez neho si ostrovček stav pamätá sám (po štarte „vypnuté“) |
| `night_label` | `"Nočný režim"` | popis prepínača |

Predvolené `presets`/`inputs` sú pre monitor **Iiyama PL2493H**; iný monitor môže mať iné kódy.

### 6.14b `[capture]` — snímka obrazovky a nahrávanie

Panel s voľbami **oblasť** (celá obrazovka / aktívne okno / región), **typ** (obrázok / video), **časovač**, prepínač
**kurzor myši** (obrázok) alebo **zvuk** (video) a hlavné tlačidlo. Otvorí sa akciou `capture` (`module:capture` v menu,
`on_click = "capture"`), ako záložka kontajnera (`tabN = "capture"`) alebo ako tlačidlo druhej lišty (`inversetabN`).

Priebeh: panel sa zatvorí → pri **regióne** sa vyberie obdĺžnik (`slurp`, Esc = zrušiť) → odpočet → snímka (`grim`) alebo
nahrávanie (`wf-recorder`). Pri **okne** sa aktívne okno určuje až po odpočte (Wayfire IPC `window-rules/get-focused-view`,
súradnice sa prepočítajú na globálne podľa aktívneho výstupu); ak ho zistiť nejde, okno sa vyberie klikom (`slurp -r`).
Nahrávanie okna / regiónu sníma pevný obdĺžnik zo štartu (pohyb okna sa nesleduje).

**Odznak REC** (modul `recorder` musí byť v `modules`): panel zapisuje stav do interného súboru (nenastavuje sa). Počas
odpočtu je v odznaku odpočet, počas nahrávania `REC m:ss`, po skončení odznak zmizne. Zastavenie: tlačidlo „Zastaviť“ v paneli, klik na odznak s
`[recorder] on_click = "capture"`, alebo akcia `capture-stop` (pošle `wf-recorder` SIGINT, aby sa súbor korektne dokončil).

Pri snímke sa ostrovček na okamih skryje (`hide_bar`), aby nebol na obrázku. Výsledok (uložený súbor / chyba) sa ukáže v
dolnom riadku panela a — ak je nainštalovaný `notify-send` — aj ako notifikácia. Snímka sa predvolene skopíruje do
schránky (`wl-copy`).

| Kľúč | Predvolené | Popis |
|---|---|---|
| `dir` | `~/Pictures/Screenshots` | adresár snímok (vytvorí sa) |
| `video_dir` | `~/Videos/Recordings` | adresár videí (vytvorí sa) |
| `name` | `%Y-%m-%d_%H-%M-%S` | vzor názvu súboru (`strftime`); prípona sa doplní |
| `delays` | `[0, 3, 5, 10]` | ponúkané časovače v sekundách (najviac 6; 0 = hneď) |
| `mode` | `"region"` | predvolená oblasť: `screen` / `window` / `region` |
| `kind` | `"image"` | predvolený typ: `image` / `video` |
| `cursor` | `false` | kurzor myši na snímke (`grim -c`) |
| `audio` | `false` | zvuk pri nahrávaní (`wf-recorder -a`) |
| `format` | `"png"` | `png` / `jpeg` |
| `video_ext` | `"mp4"` | prípona (kontajner) videa |
| `recorder_args` | *(prázdne)* | ďalšie argumenty pre `wf-recorder` (napr. `-c h264_vaapi -d /dev/dri/renderD128`) |
| `copy` | `true` | snímku skopírovať do schránky |
| `notify` | `true` | po uložení poslať notifikáciu (`notify-send`) |
| `hide_bar` | `true` | pri snímke dočasne skryť ostrovček |
| `slurp_args` | `"-b #ffffff30 -w 0"` | argumenty `slurp` pre výber oblasti / okna (oddelené medzerami, bez úvodzoviek): `-b` farba pozadia `#RRGGBBAA` (nižšie AA = priehľadnejšie), `-c` farba okraja, `-s` výplň výberu, `-w` hrúbka okraja (`0` = bez okraja); prázdne = predvolený vzhľad |
| `settle_ms` | `350` | pauza na zatvorenie panela / skrytie ostrovčeka pred snímkou |
| `width`, `font_size` | `380`, `15` | rozmery panela |

### 6.15 `[wallpaper]`

| Kľúč | Predvolené | Popis |
|---|---|---|
| `directory` | `~/.config/wayfire/media/wallpaper` | adresár s obrázkami |
| `columns` | `4` (1–12) | stĺpce mriežky |
| `rows` | `4` (1–20) | viditeľné riadky (viac = skrolovanie) |
| `thumb_width` | `160` (60–600) | šírka náhľadu; výška 9/16 |
| `hover_ring` | `true` | rámik okolo náhľadu pod kurzorom (`false` = bez rámika) |
| `current_ring` | `true` | rámik okolo náhľadu aktuálne nastavenej tapety |
| `command` | `~/.config/wayfire/scripts/wallpaper` | skript; volá sa `-v CESTA` (tapeta + farby), `-w CESTA` (len tapeta), `-t CESTA.json` (téma), pozri 5.6 |
| `cache_dir` | `~/.cache/wallpaper-chooser` | `current`, `applied-theme`, `thumbs/` |
| `themes_directory` | `~/.config/matugen/themes` | `*.json` témy (`command -t CESTA.json`) |
| `theme_width` | `170` (80–600) | šírka rozbaľovacej ponuky tém |
| `buttonN_label` / `buttonN_command` (N = 1–16) | — | tlačidlá hlavičky (bez `label` sa nezobrazí) |
| `button_max_width` | `0` (vypnuté; 40–1000) | najväčšia šírka tlačidla |

Náhľady vyrába ImageMagick vo veľkosti `thumb_width` × 9/16 so zaoblenými rohmi; pri prvom otvorení sa dorábajú na
pozadí. Klik na náhľad spustí `command -v CESTA`, po úspechu zapíše `cache_dir/current` a panel zavrie.

### 6.16 `[clipboard]`

| Kľúč | Predvolené | Popis |
|---|---|---|
| `font_size` | `15` (8–64) | písmo |
| `rows` | `8` (1–50) | viditeľné riadky |
| `width` | `420` (120–1400) | šírka |
| `max_items` | `200` (1–5000) | najviac načítaných položiek |
| `list_command` | `"cliphist list"` | príkaz; riadky `id<TAB>náhľad` |
| `copy_command` | `"cliphist decode \| wl-copy"` | dostane vybraný riadok na vstup |

História sa musí plniť externe (`wl-paste --watch cliphist store`). Panel má filter písaním (bez poľa): Enter skopíruje,
↑/↓ (Tab, PgUp/PgDn, Home/End) výber, Backspace maže znak, Ctrl+U celý filter, Esc zavrie; viac slov musí sedieť naraz,
bez ohľadu na veľkosť písmen.

**Dva spôsoby spustenia** (rovnako ako `drun`): ako panel/modul ostrovčeka (`on_click = "clipboard"` alebo záložka
`quickset`) a ako **samostatné okno v strede obrazovky** `dynamic-island --clip` (na klávesovú skratku, napr.
`bindsym`/Wayfire `command_clip = dynamic-island --clip`). Samostatné okno je prepínač: druhé spustenie ho zatvorí.
Používa rovnaké voľby `[clipboard]` a krytie `[island] opacity`; zaoblenie rohov nastavuje `window_radius` (24).
Enter / klik skopíruje položku a okno sa zavrie. `search = false` skryje pole hľadania (písaním sa filtruje ďalej,
len bez zobrazeného dotazu).

### 6.17 `[trash]`

| Kľúč | Predvolené | Popis |
|---|---|---|
| `width` | `560` (240–1600) | šírka |
| `rows` | `8` (1–50) | viditeľné riadky |
| `text_size` | `14` (8–64) | písmo |
| `confirm_empty` | `true` | potvrdenie pred vyprázdnením |
| `show_location` | `true` | stĺpec pôvodného umiestnenia |
| `show_date` | `true` | stĺpec dátumu zmazania |

Kôš zo **všetkých pripojených diskov** (`~/.local/share/Trash`, `.Trash-UID`, `.Trash/UID` podľa freedesktop
špecifikácie); klik na riadok prepína výber; „Obnoviť vybrané“ nikdy neprepíše existujúci súbor; „Vyprázdniť kôš“ natrvalo zmaže všetko;
hover nad riadkom ukáže v pätke pôvodnú cestu.

### 6.18 `[[menu]]` a `[[menu.item]]`

Menu sa spúšťa akciou s jeho názvom (`on_click = "apps"`) alebo `module:apps`; **nie je** modulom v `modules`.

| Kľúč `[[menu]]` | Predvolené | Popis |
|---|---|---|
| `name` | — (povinné) | názov |
| `type` | odvodí sa z položiek | `"icons"` \| `"text"` \| `"hybrid"` (ikony + text + oddeľovače) |
| `title` | `""` | nadpis |
| `columns` | `3` | spoločný počet stĺpcov |
| `icon_columns` / `text_columns` | `columns` (text-only: `1`) | stĺpce pre ikony / text |
| `rows` | — | najviac riadkov |
| `icon_size` | `28` (≥8) | veľkosť ikon (px) |
| `text_size` | `15` (≥8) | písmo |
| `cell_width` | `120` (≥20) | šírka bunky |
| `gap` | `8` (≥0) | medzera |
| `padding` | `12` (≥0) | vnútorný okraj |
| `separator_thickness` | `1.5` (≥0.5) | hrúbka oddeľovača |
| `separator_length` | `0.6` (0.1–1.0) | dĺžka oddeľovača ako podiel šírky |
| `wait_exec` | `false` | zobraziť menu až po načítaní všetkých `exec` |
| `wait_timeout_ms` | `1500` (100–30000) | najdlhšie čakanie pri `wait_exec` |

| Kľúč `[[menu.item]]` | Popis |
|---|---|
| `icon` | PNG: len názov → `~/.config/dynamic-island/icons/`; alebo cesta (`~/…`, absolútna). Buď `icon`, alebo `label` |
| `label` | text; pri `exec` šablóna — `{}` sa nahradí výstupom (`"CPU: {} %"`) |
| `command` | shell príkaz, alebo `module:…` (pozri 6.4). **Prázdny = položka len na zobrazenie** (bez hoveru a kliku) |
| `exec` | príkaz, ktorého prvý neprázdny riadok výstupu sa zobrazí (max. 5 s, cez `sh -c`, bez ANSI); `builtin:ZDROJ` = natívny zdroj bez procesu, pozri 6.18b |
| `interval` | obnovovanie `exec` počas otvoreného menu v s; `0` = raz pri otvorení |
| `separator` | `true` = oddeľovací riadok (ostatné kľúče sa ignorujú) |
| `line` | riadok textu cez celú šírku menu (vycentrovaný): `line = "text"`; pri `exec` je to šablóna s `{}` ako pri `label`; `line = ""` = prázdny riadok (vynechanie riadku). `exec`, `interval` a voliteľný `command` fungujú ako pri textovej položke; `icon`/`label` sa ignorujú |
| `track` | skupina sledovaného procesu (pozri 6.19) |

```toml
[[menu.item]]
line = ""                        # prázdny riadok
[[menu.item]]
line = "Zálohy"                  # nadpis cez celú šírku
[[menu.item]]
line = "Uptime: {}"              # text s výstupom príkazu
exec = "uptime -p"
interval = 30
```

Hodnoty `exec` sa pri každom otvorení menu začínajú prázdne (aby sa nezobrazila zastaraná hodnota). Vybraná položka
textového menu používa `menu_selected` / `menu_selected_text`, ikonové menu má jemné zvýraznenie. Odchod myši menu
zatvorí; po otvorení panela sa odchod ignoruje, kým myš nevstúpi do nového panela.

### 6.18b Natívne systémové zdroje (`exec = "builtin:…"`) a `[system]`

Položka menu s `exec = "builtin:ZDROJ[:ARGUMENT]"` sa nevyhodnocuje cez shell, ale priamo v ostrovčeku (`src/sysinfo.rs`):
bez spúšťania procesov, výstup má rovnaký tvar ako pôvodné skripty, takže `label = "CPU: {} %"`, `interval` a
rozloženie ostávajú rovnaké. Vyhodnocuje sa len pri otvorenom menu (a podľa `interval`), nič nebeží na pozadí.

| Zdroj | Výstup | Poznámka |
|---|---|---|
| `cpu` | `7` (%) | rozdiel dvoch vzoriek `/proc/stat` (pri `interval` oproti predošlému vyhodnoteniu, pri prvom po 0,5 s); polia ako `cpu.sh` (bez steal/guest) |
| `ram` | `2,35 GB` / `850 MB` | využitá pamäť = `MemTotal − MemAvailable`; **1 GB = 1024³ B** (ako `free`), 2 desatinné miesta |
| `ram-pct` | `15` (%) | celé číslo (skrátené) |
| `swap` | `0 MB` | využitý swap (formát ako `ram`) |
| `temp` | `52 °C` | najvyššia teplota z thermal zón (ako `temp.sh`), inak z hwmon; `n/a`, ak nič nie je |
| `temp:NÁZOV` | `61 °C` | najvyššia zo snímačov, ktorých názov (`type` zóny, `name` + `temp*_label` v hwmon) obsahuje NÁZOV, bez ohľadu na veľkosť písmen (`coretemp`, `k10temp`, `nvme`, `Package`) |
| `disk[:CESTA]` | `8,4 GB` / `63,2 MB` / `4,1 kB` | **obsadené miesto presne ako `df`/GNOME** (`statvfs`: `(blocks − bfree) × frsize`), **1 GB = 10⁹ B**, 1 desatinné miesto; CESTA = ľubovoľná cesta v danom súborovom systéme (predvolene `/`, `~/` sa rozvinie) |
| `disk-pct[:CESTA]` | `7` (%) | **presne ako `df --output=pcent`**: `used / (used + avail)` zaokrúhlené nahor (rezerva pre root sa nepočíta) |
| `disk-free[:CESTA]`, `disk-total[:CESTA]` | `29,3 GB` | voľné miesto pre bežného používateľa (`bavail`) / veľkosť; formát ako `disk` |
| `net-down`, `net-up` | `0 kB/s` / `1,5 MB/s` | rozhranie s predvolenou trasou (`/proc/net/route`) alebo `[system] net_interface`; od 1 MB/s `MB/s` (1 MB = 1024²), inak celé `kB/s` (1 kB = 1024 B); bez trasy `n/a` |

Neznámy zdroj zobrazí `?`, nedostupný údaj `n/a`. Desatinný oddeľovač určuje `[system] decimal`.

| Kľúč `[system]` | Predvolené | Popis |
|---|---|---|
| `decimal` | `","` | desatinný oddeľovač vo výstupe (`","` alebo `"."`) |
| `net_interface` | `""` | rozhranie pre `net-down` / `net-up`; prázdne = rozhranie s predvolenou trasou |

Skripty z `config/dynamic-island/scripts/` (`cpu.sh`, `ram.sh`, `net.sh`, `disk.sh`, `disk2.sh`, `temp.sh`) zostávajú v repozitári
ako náhrada pre prípad, že by niečo bolo treba upraviť; vzorový config už používa `builtin:`. Pri migrácii na iný systém sú
potrebné len `/proc` a `/sys` (Linux); žiadne externé programy.

### 6.18c Spúšťač aplikácií `--drun`

`dynamic-island --drun` otvorí mriežku aplikácií z `.desktop` súborov (náhrada `rofi -show drun`). Je to **samostatný
proces so samostatným povrchom** v strede obrazovky: nepotrebuje bežiaci ostrovček a po spustení aplikácie, Esc alebo
zatvorení povrchu skončí. Farby (`[colors]`, `colors.css`), písmo a tvar sú rovnaké ako v ostrovčeku.

Wayfire (`wayfire.ini`, plugin `command`):

```ini
[command]
binding_drun = <super> KEY_SPACE
command_drun = dynamic-island --drun
```

`--drun` je **prepínač** (ako `pkill rofi || rofi`): ak spúšťač už beží, druhé spustenie ho zatvorí. Nič ako `pkill` netreba
(`pkill dynamic-island` by zabilo aj ostrovček); PID bežiaceho spúšťača je v `$XDG_RUNTIME_DIR/dynamic-island-drun.pid`.

**Z ostrovčeka:** akcia `drun` je **panel ostrovčeka** ako ostatné moduly (ostrovček sa rozbalí do mriežky aplikácií na
svojom mieste, nie v strede obrazovky). Dá sa dať do `on_click*` ľubovoľného modulu, do položky menu (`module:drun`) alebo na
tlačidlo druhej lišty kontajnera (`inversetab1 = "drun"`, predvolená ikona `\uf00a`); opätovný klik panel zavrie. Panel
používa tie isté voľby `[drun]` (mriežka, `show`, `order`, `icon_theme`…) a rovnaké vykresľovanie; písaním sa filtruje,
šípky menia výber, Enter spustí, Esc zavrie, koleso posúva. Ako záložka kontajnera `quickset` sa neponúka. Samostatný
`dynamic-island --drun` (klávesová skratka) zostáva a otvára sa v strede obrazovky.

**Zoradenie aplikácií** nerieši `order`: ten určuje len smer vypĺňania mriežky. Aplikácie sú zoradené podľa poľa `Name=`
z `.desktop` súboru (anglický základný názov, bez lokalizovaných `Name[sk]`), bez ohľadu na veľkosť písmen. Pri písaní sa
najprv radia zhody na začiatku názvu, potom ostatné (názov, všeobecný názov, kľúčové slová), vždy abecedne.

| Kľúč `[drun]` | Predvolené | Popis |
|---|---|---|
| `columns`, `rows` | `5`, `4` | veľkosť mriežky (viditeľná stránka; ďalej sa posúva kolieskom a šípkami) |
| `opacity` | *(ako `[island] opacity`)* | krytie pozadia samostatného `--drun` (0–1); v paneli ostrovčeka platí `[island] opacity` |
| `order` | `"horizontal"` | zoraďovanie: `horizontal` = ďalšia aplikácia je **vedľa** predošlej (po riadkoch, posun po riadkoch); `vertical` = ďalšia je **pod** predošlou (po stĺpcoch, posun po stĺpcoch; koleso aj šípky idú po stĺpcoch) |
| `show` | `"icon"` | `icon`, `name`, `icon+name-right` (názov vedľa ikony), `icon+name-below` (názov pod ikonou) |
| `icon_size`, `name_size` | `64`, `14` | veľkosť ikony a názvu (px) |
| `cell_width`, `cell_height` | `0` | rozmery bunky; `0` = podľa režimu a veľkostí |
| `padding`, `radius`, `cell_radius` | `15`, `30`, `15` | okraj okna, zaoblenie okna a zvýraznenej bunky |
| `hover` | `true` | zvýraznenie bunky pod ukazovateľom; `false` = zvýrazňuje sa len výber z klávesnice, spúšťa klik |
| `search` | `false` | viditeľný riadok hľadania; písaním sa filtruje vždy |
| `terminal` | `"foot"` | pre aplikácie s `Terminal=true` (`terminal príkaz`) |
| `icon_theme` | `""` | téma ikon (s `Inherits=`); prázdne = téma z nastavení GTK (`gtk-icon-theme-name` v `~/.config/gtk-4.0/settings.ini`, potom `gtk-3.0`), inak `hicolor` |
| `desktop` | `""` | názov prostredia pre `OnlyShowIn` / `NotShowIn` (`Wayfire`); prázdne = nekontrolovať |

Ovládanie: písanie filtruje (všetky slová musia sedieť v názve, všeobecnom názve alebo kľúčových slovách; názvy
začínajúce dotazom idú prvé), šípky / Tab pohybujú výberom (cyklicky), Page Up/Down, Home/End, Enter spustí vybranú
(alebo prvú) aplikáciu, klik spustí bunku pod ukazovateľom, koleso posúva riadky, Ctrl+U zmaže dotaz, Esc zavrie.

**Spustenie čohokoľvek (ako v rofi):** ak napísaný text nič nenájde (napr. aplikácia s `NoDisplay=true`, ako `gt6ct`),
Enter ho spustí ako príkaz (`sh -c`). Ak niečo nájde, Enter spustí vybranú aplikáciu; **Ctrl+Enter** spustí napísaný text
ako príkaz aj vtedy. Platí v samostatnom okne aj v paneli ostrovčeka; pri prázdnom výsledku sa zobrazí nápoveda.

Zoznam sa číta z `$XDG_DATA_HOME/applications` a `$XDG_DATA_DIRS/*/applications` (štandardne `~/.local/share`,
`/usr/local/share`, `/usr/share`). Preskočia sa záznamy s `NoDisplay=true` alebo `Hidden=true` (kdekoľvek v skupine
`[Desktop Entry]`, aj `True`/`1`), s `TryExec` na neexistujúci program a podľa `OnlyShowIn`/`NotShowIn`. **Duplicity:**
rovnaké ID súboru (napr. `firefox.desktop` v `/usr/local/share` aj `/usr/share`) sa berie raz a vyhráva adresár s vyššou
prioritou (najprv `~/.local/share`, potom poradie v `XDG_DATA_DIRS`); navyše sa vynechá záznam s rovnakým názvom a príkazom
pod iným ID. Polia `Exec` (`%f`, `%U`, …) sa odstránia, `%c` je názov.

Ikony pochádzajú vždy z **jednej témy** (`icon_theme`, inak téma GTK; s dedičnosťou `Inherits=` a nakoniec `hicolor`),
potom z `/usr/share/pixmaps`; až ako posledná záchrana z inej témy (kontrastné témy `HighContrast*` sa preskakujú). PNG sa
použije priamo, SVG sa jednorazovo rasterizuje cez `rsvg-convert` (alebo ImageMagick) do
`~/.cache/dynamic-island/app-icons/`; prvé otvorenie s mnohými SVG ikonami je preto o niečo pomalšie. Po zmene témy alebo
pri zlej ikone zmaž priečinok `~/.cache/dynamic-island/app-icons/`.

**Diagnostika:** `dynamic-island --drun-list` vypíše každý nájdený `.desktop` súbor a či sa zobrazí, alebo z akého
dôvodu nie (`NoDisplay=true`, duplicita, `TryExec`, `OnlyShowIn`…), ku každej zobrazenej aplikácii aj súbor, z ktorého sa
berie ikona, použitú tému ikon a ktoré rasterizéry SVG sú k dispozícii. SVG sa rasterizujú v poradí `rsvg-convert` →
`inkscape` → ImageMagick (`magick` / `convert`); vstavaný renderer ImageMagicku nezvláda filtre, masky a niektoré farebné
prechody, preto sa pri zložitejších ikonách (napr. Foliate, Secrets, OpenShot) bez `librsvg2-bin` môžu biele plochy
vykresliť čierne. Odporúčané je mať nainštalovaný `rsvg-convert`.


### 6.19 Sledované procesy (`track`) a modul `radio`

Položka s `track = "skupina"` sa nespúšťa ako obyčajný príkaz: ostrovček proces spustí **sám**, zapamätá si ho a **v
skupine beží najviac jeden proces** (nový štart najprv ukončí celú skupinu procesov predošlého a počká naň, max. 2 s,
potom SIGKILL). Na koniec procesu sa blokujúco čaká v jednom vlákne — nič sa nezisťuje cyklicky.
Modul s názvom skupiny **`radio`** ukáže ikonu, kým proces beží; klik ju ukončí. Ikona zmizne aj keď proces skončí sám.
Pri reštarte ostrovčeka po zmene configu sa sledované procesy ukončia. Sleduje sa iba to, čo spustil ostrovček.
Ikonu pre ďalšie skupiny dá **modul sledovaného procesu**: ľubovoľný názov v `modules`, ktorý nie je vstavaný ani
`[[custom]]` a má vlastnú sekciu `[názov]`. Ikona je vidieť, kým beží proces skupiny `track` (predvolene názov modulu).

| Kľúč `[názov]` | Predvolené | Popis |
|---|---|---|
| `track` | názov modulu | skupina sledovaného procesu |
| `icon` | `󰐹` | ikona / text v kapsule |
| `on_click`, `on_click_right`, `on_click_middle` | `track-stop:SKUPINA`, –, – | akcie |
| `tooltip` | `{name}\nKlik = vypnúť` | `{name}` = `label` položky menu |

```toml
modules = ["network", "clock", "radio", "tv"]

[tv]
icon = "TV"        # skupina je "tv": položky menu majú track = "tv"
```

```toml
[[menu.item]]
label = "Lumen"
command = "mpv https://audio.lumen.sk/live128.mp3 --no-config"
track = "radio"

[[menu.item]]
label = "Vypnúť rádio"
command = "module:track-stop:radio"
```

### 6.20 `[[custom]]` — vlastné moduly

| Kľúč | Predvolené | Popis |
|---|---|---|
| `name` | — (povinné) | názov (pridá sa do `modules`, ak tam nie je) |
| `exec` | — | príkaz; výstup je text modulu |
| `text` | — | statický text/ikona bez procesu (tlačidlo); povinné je `exec` **alebo** `text` |
| `interval` | `0` | `0` = trvalý proces, každý riadok výstupu je nový stav (pri páde sa po 3 s spustí znova); `> 0` = spúšťa sa každých N s, berie sa posledný riadok |
| `json` | `true` | riadok sa skúsi parsovať ako JSON `{"text","tooltip","class"}`; inak ide celý riadok ako text |
| `track_file` | *(prázdne)* | cesta k súboru (`~/` sa rozbalí): modul je v lište ako **odznak** (rovnako ako CAPS), len kým súbor **existuje**; prázdne = modul sa správa ako doteraz. Kontroluje sa každých `poll_ms` |
| `tooltip` | — | prázdne = použije sa `tooltip` z JSON; inak šablóna so `{text} {tooltip} {class}` |
| `on_click*` | — | pozri 6.4 |

`class` sa nepoužíva na štýlovanie, len ako `{class}` v tooltipe. Procesy `exec` zanikajú s ostrovčekom.

Príklad – odznak, ktorý je v lište, kým existuje súbor:

```toml
[[custom]]
name = "busy"
text = "BUSY"
track_file = "/tmp/busy"
on_click = ""
```

### 6.21 `[notifications]`

| Kľúč | Predvolené | Popis |
|---|---|---|
| `enabled` | `true` | zapnúť notifikačný server |
| `width` | `450` | šírka (px) |
| `max_visible` | `3` (≥1) | najviac naraz; ďalšie čakajú (`+N`) |
| `max_lines` | `3` (≥1) | najviac riadkov tela |
| `font_size` | `15` | písmo |
| `timeout_ms` | `10000` | predvolené trvanie (keď aplikácia nežiada iné) |
| `align` | `"center"` | `center` \| iné (= vľavo) |
| `on_click` | `"default-action"` | ľavý klik: `default-action` (akcia `default` + zavrieť), `dismiss`, alebo shell príkaz |
| `on_click_right` | `"dismiss"` | pravý klik: `dismiss` alebo príkaz |
| `history` | `true` | zapisovať históriu |
| `history_max` | `1000` | najviac uložených záznamov (`0` = bez histórie) |
| `history_file` | `notifications.log` vedľa configu | cesta k histórii |
| `icons` | `true` | ikony notifikácií (vo vyskakovacom okne aj v paneli histórie) |
| `panel_width` | `480` (240–1400) | šírka panela histórie |
| `panel_height` | `520` (200–1600) | výška panela histórie |
| `panel_lines` | `3` (≥1) | najviac riadkov tela na kartu v paneli histórie |
| `[notifications.urgent]` `timeout_ms` | `0` | trvanie urgentných (0 = kým sa nezavrú) |
| `[notifications.urgent]` `border_size` | `2` | hrúbka červeného rámika |

Stredné tlačidlo zavrie všetky notifikácie.

**Pravidlá** `[[notifications.rule]]` (prvé zhodné vyhrá; vynechané polia sa neskúmajú; zhoda = podreťazec):

| Kľúč | Popis |
|---|---|
| `summary` / `app` / `body` | text hľadaný v súhrne / názve aplikácie / tele |
| `on_click` | príkaz po kliku (notifikáciu zavrie) |
| `timeout_ms` | vlastné trvanie |

### 6.22 `[autohide]`

Vyžaduje Wayfire IPC (pozri 5.1). Bez neho nastav `enabled = false`; `SIGUSR1`/`SIGUSR2` fungujú vždy.

| Kľúč | Predvolené | Popis |
|---|---|---|
| `enabled` | `true` | zapnúť autohide |
| `bar_height` | `40.0` | výška pásma lišty (px), v ktorom okno spôsobí skrytie, a oblasť, kde kurzor lištu drží zobrazenú |
| `margin` | `0.0` | pridaný okraj k pásmu |
| `edge` | `2.0` | výška pásu pri hornej hrane na opätovné zobrazenie (px) |
| `edge_width` | `30.0` (0–100) | šírka tohto pásu v % šírky výstupu (centrovaný) |
| `poll_ms` | `30` (min. 10) | interval čítania kurzora, kým okno zasahuje |
| `ignore` | `[]` | `app-id` okien, ktoré sa ignorujú |

---

## 7. Notifikácie

Ostrovček je sám serverom `org.freedesktop.Notifications` (vlastný D-Bus klient): oznamuje schopnosti **`body`**,
**`actions`** a **`icon-static`**; bez režimu „nerušiť“ a bez „restore“. Notifikácia sa zobrazí priamo v kapsule (nahradí
jej obsah ako HUD), ďalšie čakajúce sú označené `+N`, urgentné majú rámik `urgent_border`. Ľavý klik = akcia `default`
+ zavrieť, pravý = zavrieť, stredný = zavrieť všetky (konfigurovateľné, pozri 6.21).

Ak je meno `org.freedesktop.Notifications` obsadené iným démonom, ostrovček to vypíše a každých 5 s to skúša znova —
ďalší notifikačný démon preto nesmie bežať.

**Ikony** (`src/nicons.rs`, beží vo vlákne D-Bus servera): poradie podľa špecifikácie `image-data` (aj `image_data`,
`icon_data`) → `image-path` → `app_icon`. Surové pixely z `image-data` (8 bit, 3 alebo 4 zložky, rozmer ≤ 1024 px) sa uložia
ako PNG do `~/.cache/dynamic-island/notif-icons/` (názov = hash, rovnaký obrázok sa neukladá dvakrát), aby sa dali
zobraziť aj v histórii po čase. Cesta (`/abs/cesta.png`, `file://…`) sa použije priamo; **názov ikony** sa hľadá v
`~/.local/share/icons`, `~/.icons`, `/usr/local/share/icons`, `/usr/share/icons` (najprv `hicolor`, potom ostatné témy;
rozmery 64 → 24 px, adresárové rozloženie `téma/rozmer/kontext/` aj `téma/kontext/rozmer/`) a v `/usr/share/pixmaps`.
**Podporuje sa len PNG** (renderer nemá rasterizér SVG; väčšina tém Adwaita/Papirus má ikony ako SVG, preto sa pri nich
ikona nenájde). Notifikácia bez použiteľnej ikony zobrazí v paneli histórie kruh s prvým písmenom aplikácie, vo
vyskakovacom okne nič. Vypnutie: `[notifications] icons = false`.

**História:** každá notifikácia sa zapíše ako JSON riadok do `notifications.log`
(polia `time` (unixový čas), `id`, `app`, `urgency`, `summary`, `body`, `icon` = cesta k PNG); výpis
`dynamic-island --history 20`, spracovanie napr. cez `jq`. Staršie riadky bez poľa `icon` sa načítajú bez ikony.

**Panel histórie** (akcia `notifications`; v configu `on_click* = "notifications"` na module alebo
`command = "module:notifications"` v položke menu): karty v tvare notifikácie – ikona, názov aplikácie, čas
(„Dnes 14:32“, „Včera 21:05“, inak „05.11. 14:32“, s rokom pre staršie), súhrn a najviac `panel_lines` riadkov tela;
urgentné majú červený pásik vľavo. Koliesko/touchpad posúva po kartách (najnovšie hore), nabehnutím na kartu sa namiesto
času ukáže **×** (odstráni záznam zo súboru), tlačidlo **Vymazať** v hlavičke zmaže celú históriu. Načíta sa najviac
300 posledných záznamov pri každom otvorení. Panel sa zatvorí, keď príde nová notifikácia (jej okno ho nahradí).

---

## 8. Obmedzenia a známe nezrovnalosti

- Mierka je celočíselná (`set_buffer_scale`); zlomková zatiaľ nie.
- Caps Lock sa číta zo sysfs LED (musí existovať `*::capslock`).
- Rozloženie klávesnice a autohide sú viazané na Wayfire IPC (pozri 5.1).
- Zadávanie hesla Wi-Fi: iba WPA2/WPA3-PSK; bez 802.1X, WEP a skrytých sietí.
- Ostrovček nespravuje: schránku (`cliphist` plní samostatný proces), polkit agenta, nečinnosť (`swayidle`).
- **`scripts/install.sh`** kopíruje `config/config.toml` (neexistuje; vzorový config je `config/dynamic-island/config.toml`)
  a **neinštaluje** systemd službu — použi ručný postup z 2.5.
- Predvolené `colors_file` v kóde ukazuje na starú cestu (`~/.config/waybar/waybar-colors.css`); vzorový config ju prepisuje.
- Opis služby v `dynamic-island.service` ešte spomína „nečinnosť“ (funkcia bola odstránená).
- V `Cargo.toml` je `wayland-protocols` s funkciou `staging` už zbytočné.
- V repozitári nie je `Cargo.lock`.

---

## 9. Riešenie problémov

| Príznak | Kontrola |
|---|---|
| Nespustí sa: „kompozitor nepodporuje zwlr_layer_shell_v1“ | kompozitor nemá layer-shell (napr. GNOME) |
| Nespustí sa: chýba `libEGL.so.1` | `sudo apt install libegl1 libgles2`, GPU ovládač |
| Chýbajú ikony / tofu štvorce | `icon_family` nenainštalované alebo podmnožina bez glyfu; stderr vypíše nenájdené písmo |
| Modul `keyboard` prázdny, autohide nefunguje | `WAYFIRE_SOCKET` v prostredí služby (`systemctl --user import-environment`); pluginy `ipc` + `ipc-rules` s potrebnými metódami (5.1) |
| Žiadne notifikácie | iný démon drží `org.freedesktop.Notifications` (`busctl --user status org.freedesktop.Notifications`) |
| Disky sa nepripájajú | beží `udisks2`? polkit pravidlá; `[disks] automount` |
| Panel Wi-Fi/Bluetooth prázdny | `systemctl status NetworkManager` / `bluetooth`; D-Bus system bus |
| Monitor panel „ddcutil nie je dostupný alebo monitor neodpovedá“ | `ddcutil detect`, správny `bus`, modul jadra `i2c-dev`, práva k `/dev/i2c-*` |
| Zlý dátum/jazyk hodín | `locale -a` — chýba `sk_SK.UTF-8` (`dpkg-reconfigure locales`) |
| Chyba konfigurácie | `dynamic-island --check`, `journalctl --user -u dynamic-island -f` |
| Zombie `[pactl] <defunct>` | oprava je v kóde (reštart cez `exec` ukončí `pactl subscribe` a vyzdvihne ho); starší binárny stav ich nechával |
