# Typed into Warp by warp.sh, so the output below goes to a real command block.
zmodload zsh/datetime
: ${BENCH_RES:?} ${BENCH_DIR:?}
print -r -- "ready $EPOCHREALTIME" > "$BENCH_RES.tmp"
work() { local n=$1; shift; local s=$EPOCHREALTIME; "$@"; local e=$EPOCHREALTIME; print -r -- "$n $s $e" >> "$BENCH_RES.tmp"; sleep 0.5 }
work color   cat "$BENCH_DIR/color.txt"
work plain   cat "$BENCH_DIR/plain.txt"
work seq     seq 1 3000000
work unicode cat "$BENCH_DIR/unicode.txt"
mv "$BENCH_RES.tmp" "$BENCH_RES"
