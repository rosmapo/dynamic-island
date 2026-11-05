#!/bin/bash

rsync -avP --delete --exclude='/.*' /media/mm/backup/chroot/home/miro/ /home/mm/Linux/Software/sources/

# Všeobecný zoznam vylúčených vecí pre ostatné priečinky
EXCLUDE_LIST=$(cat << EOF
*.lock
*.socket
*.pid
*.tmp
SingletonCookie
SingletonLock
SingletonSocket
firefox/*.default-release/lock
logs/
kos.png
EOF
)

DEST="/home/mm/Linux/Debian/dotfiles"
mkdir -p "$DEST"
mkdir -p "$DEST/.config"
mkdir -p "$DEST/.config/google-chrome/Default"
mkdir -p "$DEST/.local"
mkdir -p "$DEST/.local/share"

# Funkcia na klasický rsync s vylúčením
function rsync_with_exclude() {
    rsync -avP --delete --exclude-from=<(echo "$EXCLUDE_LIST") "$1" "$2"
}

# --- 1. SEKCIA: CHROME (Záloha len dôležitých dát z Default) ---
echo "Zálohuje sa Google Chrome (iba vybrané súbory)..."

# Definujeme presne to, čo z priečinka Default chceme zachovať
# Vďaka '*/' na začiatku rsync povolí prechod štruktúrou priečinkov
CHROME_INCLUDES=$(cat << EOF
+ /Bookmarks
+ /Login Data
+ /Favicons
+ /Preferences
+ /Extensions/
+ /Extensions/**
+ /Local Extension Settings/
+ /Local Extension Settings/**
+ /Sync Extension Settings/
+ /Sync Extension Settings/**
- /*
EOF
)

# RSync pre Chrome s presným filtrom
rsync -avP --delete --include-from=<(echo "$CHROME_INCLUDES") "/home/mm/.config/google-chrome/Default/" "$DEST/.config/google-chrome/Default/"


# --- 2. SEKCIA: OSTATNÉ KONFIGURÁCIE (Pôvodný skript) ---

# .config priečinky
declare -a CONFIG_DIRS=(
  "wayfire" "wireplumber" "pipewire" "Pinta" "Kvantum" "solaar" "wstroke"
  "bleachbit" "doublecmd" "fastfetch" "foot" "systemd" "mpv" "remmina" "rofi"
  "fooyin" "gtk-3.0" "gtk-4.0" "swayimg" "qt6ct" "qt5ct" "nwg-look" "QDirStat" "galeria" "xsettingsd" "mc" "onlyoffice" "rclone" "zathura"
  "mmedit" "terminal-colors.d" "cropfast" "simplenote-gtk" "matugen" "dynamic-island"
  "frame"
)

for dir in "${CONFIG_DIRS[@]}"; do
  rsync_with_exclude "/home/mm/.config/$dir" "$DEST/.config/"
done

# .local/share priečinky
declare -a LOCAL_DIRS=(
  "applications" "com.github.johnfactotum.Foliate" "fonts" "chrome"
  "icons" "remmina"
)

for dir in "${LOCAL_DIRS[@]}"; do
  rsync_with_exclude "/home/mm/.local/share/$dir" "$DEST/.local/share/"
done

# .local/bin
rsync_with_exclude "/home/mm/.local/bin" "$DEST/.local/"

# Ostatné priečinky v domovskom adresári
declare -a OTHER_DIRS=(
  ".cert" ".anydesk" ".java" ".megaCmd" ".themes" ".openshot_qt"
)

for dir in "${OTHER_DIRS[@]}"; do
  rsync_with_exclude "/home/mm/$dir" "$DEST/"
done

# Súbory v .config
declare -a CONF_FILES=(
  "mimeapps.list"
)

for file in "${CONF_FILES[@]}"; do
  rsync_with_exclude "/home/mm/.config/$file" "$DEST/.config/"
done

# Ostatné voľné súbory
declare -a OTHER_FILES=(
  ".bashrc" ".cruft-ignore" ".bash_profile"
)

for file in "${OTHER_FILES[@]}"; do
  rsync_with_exclude "/home/mm/$file" "$DEST/"
done

echo "Záloha úspešne dokončená!"