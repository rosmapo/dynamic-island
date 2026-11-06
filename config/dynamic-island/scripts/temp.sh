#!/bin/sh
# Najvyššia teplota zo všetkých thermal zón v °C
max=0
for f in /sys/class/thermal/thermal_zone*/temp; do
    [ -r "$f" ] || continue
    t=$(cat "$f" 2>/dev/null) || continue
    [ "$t" -gt "$max" ] 2>/dev/null && max=$t
done
[ "$max" -gt 0 ] && echo "$((max / 1000)) °C" || echo "n/a"
