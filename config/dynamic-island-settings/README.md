# dynamic-island-settings

Grafické nastavenia pre [dynamic-island](https://github.com/rosmapo/dynamic-island). Samostatný nástroj (Python + GTK4 +
libadwaita), nie súčasť ostrovčeka. Upravuje `~/.config/dynamic-island/config.toml` a **zachová komentáre**, poradie
a formátovanie súboru.

## Závislosti (Debian 13)

    sudo apt install python3-gi gir1.2-adw-1 gir1.2-gtk-4.0

Nič iné netreba (Python 3.11+ má `tomllib`; zápis rieši vlastný `tomledit.py`).

## Spustenie

    ./dynamic-island-settings                 # ~/.config/dynamic-island/config.toml
    ./dynamic-island-settings -c cesta.toml   # iný súbor

## Ovládanie

* **Taby hore** podľa oblasti (Vzhľad, Kontajner, Lišta, Panely, Vlastné, Systém) — v bočnom paneli sú len
  sekcie aktuálneho tabu (nie všetkých 30 naraz).
* Vpravo možnosti vybranej sekcie. Hore hľadanie vo **všetkých** kľúčoch; z výsledku sa dá skočiť priamo na sekciu.
* **Vlastné menu** je master–detail: vľavo zoznam menu, vpravo vlastnosti vybraného menu a jeho položky
  (pridať / zmazať / posunúť). Ostatné polia tabuliek (`[[custom]]`, pravidlá) majú klasický zoznam expanderov.
* Zmeny sa zapíšu až tlačidlom **Použiť**. Ostrovček sa po zmene súboru reštartuje sám. Pred zápisom sa vytvorí záloha
  `config.toml.bak-RRRRMMDD-HHMMSS` (ponecháva sa 5) a súbor sa overí parserom; pri chybe sa nič nezapíše.
* Pri každej možnosti, ktorá sa líši od predvolenej hodnoty (alebo je uvedená v súbore), je tlačidlo ↶ **Vrátiť predvolenú
  hodnotu** (v tooltipe je predvolená hodnota). Po „Použiť“ sa riadok v súbore zakomentuje (`# kľúč = hodnota`), takže ostane
  vidieť pôvodná hodnota a komentár, a ostrovček použije predvolenú z kódu.
* Menu ⋮: *Zobraziť zmeny…* (diff pred zápisom), *Zahodiť neuložené zmeny*, *Načítať súbor znova*.
* V tabe *Systém* je stránka *Ostatné* — kľúče, ktoré GUI nepozná. Ostanú v súbore nedotknuté.

## Čo je pokryté

Všetky skalárne kľúče zo všetkých sekcií (vyše 200; predvolené hodnoty a rozsahy sa berú priamo zo zdrojového kódu
ostrovčeka), poradie modulov, záložky quickset a druhá lišta (`tabN`, `inversetabN`), tlačidlá hlavičky tapiet, názvy
rozložení klávesnice a farby.

**Polia tabuliek** majú vlastné editory: *Vlastné menu* (`[[menu]]` + `[[menu.item]]`: pridať / zmazať / presunúť menu aj
položky, všetky ich kľúče), *Vlastné moduly* (`[[custom]]`) a *Pravidlá notifikácií* (`[[notifications.rule]]`). Upravujú sa
priamo v riadkoch súboru: nezmenené bloky, komentáre a bannery nad blokmi ostanú, zmenené kľúče sa prepíšu na mieste,
zmazaný blok zmizne aj so svojím komentárom nad hlavičkou, nové sa pridajú za posledný.

**Zatiaľ nie:** vlastné sekcie sledovaných modulov (`[meno]` s `track`), viacriadkové texty bez `\n`.

## Údržba schémy

Predvolené hodnoty a rozsahy generuje `tools/gen_schema.py` zo zdrojov ostrovčeka (`schema_gen.json`); slovenské názvy,
voľby a špeciálne kľúče sú v `schema.py`. Po pridaní novej voľby do ostrovčeka:

    tools/gen_schema.py /cesta/k/dynamic-island
    python3 -m unittest discover -s tests

Test `test_schema_coverage` zlyhá, ak v kóde pribudol kľúč, ktorý v GUI chýba.

## Testy

    python3 -m unittest discover -s tests     # model, zápis TOML, pokrytie schémy
    python3 tests/smoke_gui.py                # dymový test kódu GUI bez GTK (atrapy)

Testované proti `config.toml` z repozitára ostrovčeka (`DI_CONFIG=cesta` pre iný súbor).
