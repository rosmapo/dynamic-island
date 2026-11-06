#!/bin/sh
# Vyťaženie CPU v % (rozdiel dvoch vzoriek /proc/stat počas 0,5 s)
read -r _ u1 n1 s1 i1 w1 q1 sq1 _ < /proc/stat
sleep 0.5
read -r _ u2 n2 s2 i2 w2 q2 sq2 _ < /proc/stat
idle=$(( (i2 + w2) - (i1 + w1) ))
total=$(( (u2 + n2 + s2 + i2 + w2 + q2 + sq2) - (u1 + n1 + s1 + i1 + w1 + q1 + sq1) ))
[ "$total" -gt 0 ] && echo $(( 100 * (total - idle) / total )) || echo 0
