#!/bin/sh
# Využitá pamäť v % (MemTotal - MemAvailable)
awk '/^MemTotal:/ {t=$2} /^MemAvailable:/ {a=$2} END {if (t>0) printf "%d\n", (t-a)*100/t}' /proc/meminfo
