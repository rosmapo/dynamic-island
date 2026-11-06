#!/bin/sh
# Obsadenosť pripojného bodu v % – použitie: disk.sh /  alebo  disk.sh /home
df --output=pcent "${1:-/}" 2>/dev/null | tail -n 1 | tr -d ' %\n'
