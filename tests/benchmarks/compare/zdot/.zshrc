# Runs inside the terminal under test (zsh reads this because run.py sets ZDOTDIR). No prompt theme, no plugins, so the
# only differences between terminals are the terminals. Each workload writes straight to the terminal; the wall time is
# from before the command starts until it exits, like tests/benchmarks/cat_timed.sh.
HISTFILE=/dev/null            # keep zsh from writing history and session files into this directory
SHELL_SESSIONS_DISABLE=1
# Only run in the terminal's own shell. Some terminals also start a throwaway interactive shell of their own (Aleno
# runs `zsh -ilc` once to look up $EDITOR); with stdout not a terminal that shell must not run the benchmark.
[[ -t 0 && -t 1 ]] || return
zmodload zsh/datetime
: ${BENCH_RES:?} ${BENCH_DIR:?}
print -r -- "ready $EPOCHREALTIME" > "$BENCH_RES.tmp"
work() {
  local name=$1; shift
  local s=$EPOCHREALTIME
  "$@"
  local e=$EPOCHREALTIME
  print -r -- "$name $s $e" >> "$BENCH_RES.tmp"
  sleep 0.5
}
work color   cat "$BENCH_DIR/color.txt"
work plain   cat "$BENCH_DIR/plain.txt"
work seq     seq 1 3000000
work unicode cat "$BENCH_DIR/unicode.txt"
print -r -- "done $EPOCHREALTIME" >> "$BENCH_RES.tmp"
mv "$BENCH_RES.tmp" "$BENCH_RES"
sleep 3600
