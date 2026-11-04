#!/bin/bash

# Definovanie ciest a adresárov
DIRS=("Dokumenty" "Fotky" "Hudba" "Obrázky" "Video" "Zaloha")
SRC="/media/mm/Data/"
DEST="/media/mm/EXT_HDD_2/"

echo -e "--- KONTROLA ODSTRÁNENÝCH SÚBOROV (DRY RUN) ---\n"
has_deletes=0
summary=""

# Prejdeme všetky adresáre a urobíme skúšobný beh
for dir in "${DIRS[@]}"; do
	# UPRAVENÉ: Pridaný grep -v pre odfiltrovanie rozšírení z Chrome
	deletes=$(rsync -avn --delete "$SRC$dir/" "$DEST$dir/" | grep "^deleting " | grep -v -E "Extensions/|Extension Settings/" || true)
	
	if [ -n "$deletes" ]; then
		echo "V adresári $dir sa vymažú tieto súbory/adresáre:"
		echo "$deletes"
		echo "---------------------------------------------------"
		has_deletes=1
		
		# Spočítanie riadkov (súborov) a uloženie do sumára
		count=$(echo "$deletes" | wc -l)
		summary="${summary}- $dir: $count súborov/adresárov\n"
	fi
done

# Ak sa našli súbory na zmazanie, zobrazíme sumár a vyžiadame si potvrdenie
if [ $has_deletes -eq 1 ]; then
	echo -e "\n=== ZHRNUTIE ODSTRÁNENÝCH SÚBOROV ==="
	echo -e "$summary"
	echo "POZOR: Tieto súbory budú natrvalo odstránené z EXT_HDD_2!"
	read -p "Chceš pokračovať v zálohovaní? (y/n): " confirm
	if [[ "$confirm" != "y" && "$confirm" != "Y" ]]; then
		echo "Zálohovanie zrušené."
		sleep 2
		exec bash
	fi
else
	echo "Nenašli sa žiadne súbory na odstránenie. Pokračujem v zálohe..."
fi

echo -e "\n--- SPUŠŤAM ZÁLOHU ---"
for dir in "${DIRS[@]}"; do
	rsync -avP --delete "$SRC$dir/" "$DEST$dir/"
done

sync
echo -e "\nZáloha dokončená!"
exec bash

