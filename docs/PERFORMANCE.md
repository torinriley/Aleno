# Performance

Aleno is built so the terminal never gets in the way: output is parsed off the main thread, only changed rows are rebuilt, and the GPU does almost nothing per frame. Everything on this page was measured on an Apple M5 (10 cores, 16 GB) with macOS 27 and release builds, using the scripts listed at the bottom.

## At a glance

| | |
|---|---|
| Launch to first frame | ~150 ms |
| New window (⌘N) to first frame | 15-17 ms |
| CPU while idle, cursor blinking | 0.0% |
| Memory, one window and one pane | ~87 MB |
| GPU time per frame | 0.8 ms median, 1.9 ms at p99 |
| Terminal sync + instance rebuild, full redraw | 0.11 ms median |
| `cat` of a 135 MB coloured file | 0.4-0.6 s (about 220-315 MB/s), level with Ghostty |
| Key to shell and back, through the daemon | 93 µs median |

## Throughput

`tests/benchmarks/cat_timed.sh` runs `cat` on a 135 MB coloured file inside a terminal and measures wall-clock time until `cat` exits. The same script was run in Ghostty and in Aleno on the same Mac:

| Terminal | Runs | Throughput |
|---|---|---|
| Ghostty | 0.46 to 0.58 s | ~230-290 MB/s |
| Aleno | 0.41 to 0.61 s | ~220-330 MB/s |

The two trade places from run to run. On a quiet machine Aleno's median was 0.43 s against Ghostty's 0.47 s; on a busy one both slow down and finish within a few percent of each other.

A bare loop that only reads that file from a pseudo-terminal, with no parsing at all, takes about 0.37 s. macOS hands over roughly 1 KB per read, so that is the floor for any terminal. Aleno is within about 10% of it.

How it gets there:

1. Parser fast paths for printable ASCII and plain colour sequences, and cheap history rows.
2. The app reads the shell's pty itself (the daemon passes the file descriptor over), so output is parsed once, in the app.
3. A two-stage pipeline: a gather thread drains the pty into rotating 64 KB buffers while a parse thread feeds the terminal. Reading never waits for parsing. Small interactive output is delivered immediately, so there is no added latency when you type.
4. A full-screen scroll sets one dirty flag instead of marking every row, and evicted history buffers are reused.

## Compared with other terminals

`tests/benchmarks/compare/run.py` starts a fresh instance of each terminal with a bare zsh (no prompt theme, no plugins) and times the same workloads inside it: `cat` of a 135 MB file of dense 24-bit colour (about one escape sequence per 15 characters), `cat` of 100 MB of plain text, `cat` of 50 MB of unicode and emoji, and `seq 1 3000000`. It also records launch to shell, memory and idle CPU. The terminals take turns, one warm-up round is thrown away, and each number is the median of five runs. Default windows and settings, one Apple M5 on macOS 27.0.1.

| | Aleno | Ghostty 1.4.0 | Terminal.app 2.15 | iTerm2 3.7.9 |
|---|---|---|---|---|
| Launch to shell ready | 197 ms | 217 ms | 261 ms | 543 ms |
| `cat` 100 MB plain text | 0.34 s | 0.33 s | 2.22 s | 0.90 s |
| `cat` 50 MB unicode and emoji | 0.20 s | 0.18 s | 1.43 s | 8.42 s |
| `cat` 135 MB dense 24-bit colour | 0.53 s | 0.66 s | 4.25 s | 6.90 s |
| `seq 1 3000000` | 2.93 s | 3.01 s | 2.86 s | 4.29 s |
| Idle CPU | 0.0% | 0.0% | 0.6% | 0.2% |
| Memory after the run | 160 MB | 182 MB | 474 MB | 462 MB |

Aleno's column is a build with the direct UTF-8 text path (unicode was 0.34 s before it); the other terminals were measured in an earlier round on the same machine, so small differences are within the noise between rounds. `seq` is limited by how fast `seq` itself prints, so it mostly measures `seq`. This is speed at the shell, not input latency. Aleno's window opens larger than the 80x24 of Terminal.app and iTerm2, and its memory includes the background daemon.

## Terminal engine

Single thread, 200x60 grid, 10k lines of scrollback.

| Workload | Throughput |
|---|---|
| Plain text | ~317 MB/s |
| 24-bit colour and attributes | ~205 MB/s |
| Unicode, wide characters and emoji | ~415 MB/s |
| Editor-like (cursor addressing, scroll regions) | ~157-177 MB/s, no allocations |
| Clear screen | ~4 µs |
| Scrollback insert | ~2.0M lines/s |
| Search across 100k lines | ~24 ms |
| Resize with 100k lines of history | 48 µs |
| Snapshot / restore of 2000 lines | 2.1 ms / 1.1 ms |

Memory per pane is 0.39 MB empty and about 10 MB with a full 10k-line scrollback.

## Sessions and IPC

Measured against the real daemon process over its Unix socket, with real pseudo-terminals.

| Metric | Result |
|---|---|
| Input echo round trip | p50 93 µs, p99 151 µs |
| Attach to a pane | 0.6 ms (empty), 5.6 ms (up to 10k lines of history) |
| Create a pane (spawn shell and pty) | 1.5 ms |
| 50 panes each printing 20k lines at once | all done in 1.4 s |
| Idle daemon CPU | 0.000% |
| Daemon memory per idle pane | ~0.12 MB |

## Reproduce

| What | Command |
|---|---|
| Engine | `cargo run --release -p aleno-terminal --example bench` |
| Daemon and IPC | `cargo build --release -p aleno-daemon && cargo run --release -p aleno-client --example bench_session` |
| The app: idle, flood, frames, windows | build the app, then `cargo run --release -p aleno-client --example bench_app` (needs a visible desktop, about 2.5 minutes) |
| Launch time (six launches) | `tests/benchmarks/startup.sh` |
| `cat` throughput in any terminal | `tests/benchmarks/cat_timed.sh <file> <result-file>` |
| Aleno, Ghostty, Terminal.app and iTerm2 side by side | `python3 tests/benchmarks/compare/gen_data.py /tmp/aleno-bench/data`, then `python3 tests/benchmarks/compare/run.py` |
| The engine on any file | `cargo run --release -p aleno-terminal --example feedfile -- <file>` |
| Per-frame timing | run the app with `ALENO_BENCH=1` |
| Ghostty on the same file | `tests/benchmarks/ghostty_bench.sh` |
