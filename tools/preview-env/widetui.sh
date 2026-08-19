#!/bin/sh
# Deterministic stand-in for a full-screen TUI sized to the owner's grid:
# repaints 55 rows x ~185 cols with absolute positioning every 400ms.
i=0
printf '\033[?25l'
while :; do
  i=$((i+1))
  printf '\033[H'
  r=1
  while [ $r -le 55 ]; do
    printf '\033[%d;1H' "$r"
    printf 'row %02d cycle %04d | the quick brown fox jumps over the lazy dog and keeps painting wide terminal rows for the takeover pollution proof padding padding padding padding padding END' "$r" "$i"
    r=$((r+1))
  done
  sleep 0.4
done
