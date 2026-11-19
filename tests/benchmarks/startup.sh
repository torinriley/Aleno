#!/bin/bash
# Launch the built app N times with its own PTY helper and report launch->first-frame (cold process, warm disk cache).
APP="$(cd "$(dirname "$0")/../.." && pwd)/build/Build/Products/Release/Aleno.app/Contents/MacOS/Aleno"
export ALENO_BENCH=1
for i in 1 2 3 4 5 6; do
  LOG=$(mktemp); "$APP" 2>"$LOG" & AP=$!; sleep 3.5; kill $AP 2>/dev/null; wait $AP 2>/dev/null
  echo "run $i: $(grep -E 'launch->first frame' "$LOG" | sed 's/BENCH //')  |  $(grep -E 'phase' "$LOG" | sed -E 's/BENCH phase +//; s/ since process start//' | tr '\n' ';')"
  rm -f "$LOG"
done
