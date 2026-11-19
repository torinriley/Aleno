#!/bin/bash
# Runs cat_timed.sh in a *separate* Ghostty instance (open -n) with a fixed grid, 3 times, then closes only that instance.
FILE=${1:-/tmp/ag/big.txt}; COLS=${2:-124}; ROWS=${3:-34}
HERE="$(cd "$(dirname "$0")" && pwd)"
for i in 1 2 3; do
  RES=/tmp/ag/ghostty_res_$i.txt; rm -f $RES
  BEFORE=$(pgrep -f "Ghostty.app/Contents/MacOS/ghostty" | sort)
  open -n -a /Applications/Ghostty.app --args --window-width=$COLS --window-height=$ROWS --confirm-close-surface=false -e "$HERE/cat_timed.sh" "$FILE" "$RES"
  for k in $(seq 1 240); do [ -s $RES ] && break; sleep 0.5; done
  sleep 0.5
  AFTER=$(pgrep -f "Ghostty.app/Contents/MacOS/ghostty" | sort)
  NEW=$(comm -13 <(echo "$BEFORE") <(echo "$AFTER"))
  [ -n "$NEW" ] && kill $NEW 2>/dev/null     # only the instance this script started
  if [ -s $RES ]; then python3 -c "
s,e=open('$RES').read().split(); d=float(e)-float(s); print('ghostty run $i: %.2fs = %.0f MB/s'%(d,135.0/d))"; else echo "ghostty run $i: no result"; fi
  sleep 1
done
