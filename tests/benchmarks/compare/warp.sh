#!/bin/bash
# Times the same workloads in Warp: tests/benchmarks/compare/warp.sh [runs] [data-dir]
# Warp runs a command as a block only when it is typed at its prompt (output that arrives with no running block takes a
# separate path meant for typeahead), so the shell-startup-file trick run.py uses for other terminals does not measure
# it fairly. This writes a temporary launch configuration to ~/.warp/launch_configurations that types the benchmark
# into a new tab, opens it with warp://launch/, and reads the timings back. One warm-up run is discarded.
set -euo pipefail
RUNS=${1:-5}; DATA=${2:-/tmp/aleno-bench/data}
HERE="$(cd "$(dirname "$0")" && pwd)"
CFG_DIR="$HOME/.warp/launch_configurations"; CFG="$CFG_DIR/aleno-bench.yaml"
mkdir -p "$CFG_DIR" /tmp/aleno-bench
WAS_RUNNING=0; pgrep -f "Warp.app/Contents/MacOS" >/dev/null && WAS_RUNNING=1
cleanup() { rm -f "$CFG"; rmdir "$CFG_DIR" 2>/dev/null || true; [ "$WAS_RUNNING" = 0 ] && pkill -9 -f "Warp.app/Contents/MacOS" || true; }
trap cleanup EXIT
for i in $(seq 0 "$RUNS"); do
  RES=/tmp/aleno-bench/res_warp_$i; rm -f "$RES" "$RES.tmp"
  cat > "$CFG" <<YAML
---
name: aleno-bench
windows:
  - tabs:
      - title: bench
        layout:
          cwd: /tmp
          commands:
            - exec: BENCH_RES=$RES BENCH_DIR=$DATA /bin/zsh -f $HERE/warp-bench.zsh
YAML
  open "warp://launch/aleno-bench"
  for _ in $(seq 1 300); do [ -s "$RES" ] && break; sleep 1; done
  [ -s "$RES" ] || { echo "no result from Warp (run $i)" >&2; exit 1; }
done
python3 - "$RUNS" <<'PY'
import statistics as st, sys
runs = int(sys.argv[1]); rows = []
for i in range(1, runs + 1):                      # run 0 is the warm-up
    d = {}
    for l in open(f"/tmp/aleno-bench/res_warp_{i}"):
        p = l.split()
        if len(p) == 3: d[p[0]] = float(p[2]) - float(p[1])
    rows.append(d)
print(f"Warp, median of {runs} runs:  " + "  ".join(f"{k} {st.median(r[k] for r in rows):.2f}s" for k in ("color", "plain", "seq", "unicode")))
PY
