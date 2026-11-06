#!/bin/bash
# Obsadené miesto na disku – výstup ako v GNOME System Monitor

mountpoint="${1:-/}"

df -B1 --output=used "$mountpoint" 2>/dev/null |
tail -n 1 |
awk '{
    size = $1
    if (size >= 1000000000)
        printf "%.1f GB\n", size / 1000000000
    else if (size >= 1000000)
        printf "%.1f MB\n", size / 1000000
    else
        printf "%.1f kB\n", size / 1000
}'
