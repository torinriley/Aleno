#!/bin/sh
# Pane "shell" used by the bench_app harness: waits for trigger files and runs cat_timed.sh, so the harness never has to
# attach a second client (which would take the pty away from the window under test).
D="${ALENO_BENCH_DIR:?}"
HERE="$(cd "$(dirname "$0")" && pwd)"
i=1
while [ $i -le 3 ]; do
  while [ ! -e "$D/go$i" ]; do sleep 0.05; done
  "$HERE/cat_timed.sh" "$D/big.txt" "$D/res$i.txt"
  i=$((i+1))
done
sleep 600
