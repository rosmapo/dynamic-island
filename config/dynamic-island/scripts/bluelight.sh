#!/bin/bash

# Skript na toggle wlsunset: 4500K ↔ vypnuté

if pgrep -x wlsunset > /dev/null; then
    killall wlsunset
else
    wlsunset -t 4500 -T 4501
fi
