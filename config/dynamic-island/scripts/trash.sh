#!/bin/sh
# Počet položiek v koši na všetkých dostupných diskoch (freedesktop).

uid=$(id -u)
user=${USER:-$(id -un)}
n=0
seen='
'

count_trash() {
    root=$1
    info_dir=$root/info
    files_dir=$root/files

    [ -d "$info_dir" ] && [ -d "$files_dir" ] || return

    # Počítaj iba položky, ktoré majú .trashinfo aj zodpovedajúci súbor.
    count=$(
        find "$info_dir" -maxdepth 1 -type f -name '*.trashinfo' -printf '%f\n' 2>/dev/null |
        while IFS= read -r info_file; do
            data_file=$files_dir/${info_file%.trashinfo}
            if [ -e "$data_file" ] || [ -L "$data_file" ]; then
                printf '%s\n' 1
            fi
        done | wc -l
    )
    n=$((n + count))
}

add_trash() {
    root=$1
    [ -d "$root" ] || return

    # Zabráň dvojitému započítaniu rovnakej cesty.
    case "$seen" in
        *"
$root
"*) return ;;
    esac
    seen="${seen}${root}
"
    count_trash "$root"
}

# Používateľský kôš na systémovom disku.
add_trash "${XDG_DATA_HOME:-$HOME/.local/share}/Trash"

# Koše na pripojených diskoch.
for mount_path in /media/*/* /run/media/"$user"/*; do
    [ -d "$mount_path" ] || continue

    add_trash "$mount_path/.Trash-$uid"
    add_trash "$mount_path/.Trash/$uid"
    add_trash "$mount_path/.Trash"
done

if [ "$n" -eq 0 ]; then
    echo "je prázdny."
else
    echo ": $n pol."
fi
