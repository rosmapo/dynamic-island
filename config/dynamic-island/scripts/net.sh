#!/bin/sh
# Rýchlosť siete za 1 s: net.sh down  alebo  net.sh up   (rozhranie s predvolenou trasou)
dir="${1:-down}"
ifc=$(ip route show default 2>/dev/null | awk '{for (i=1;i<NF;i++) if ($i=="dev") {print $(i+1); exit}}')
[ -n "$ifc" ] || { echo "–"; exit 0; }
col=2; [ "$dir" = up ] && col=10
# `sed` oddelí názov rozhrania od čísla (pri veľkých počítadlách sa v /proc/net/dev zlepia: "wlan0:1234567890")
bytes() { sed 's/:/ /' "${NETDEV:-/proc/net/dev}" | awk -v i="$ifc" -v c="$col" '$1==i {print $c}'; }
b1=$(bytes)
sleep 1
b2=$(bytes)
awk -v d=$(( b2 - b1 )) 'BEGIN { if (d >= 1048576) printf "%.1f MB/s\n", d/1048576; else printf "%d kB/s\n", d/1024 }'
