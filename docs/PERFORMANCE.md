# Performance

Aleno parses output off the main thread, rebuilds only the rows that changed, and asks the GPU for almost nothing per frame. Everything on this page was measured on an Apple M5 (10 cores, 16 GB) with macOS 27.0.1 and release builds, using the scripts listed at the bottom.

## At a glance

| | |
|---|---|
| Launch to first frame | ~165 ms |
| New window (⌘N) to first frame | 17-33 ms |
| CPU while idle, cursor blinking | 0.1% |
| Memory, one window and one pane | ~94 MB |
| GPU time per frame | 0.6 ms median, 2.7 ms at p99 |
| Terminal sync + instance rebuild | 0.01 ms median, 0.18 ms at p99 |
| `cat` of 100 MB of plain text | 0.33 s (~300 MB/s), level with Ghostty |
| `cat` of 50 MB of unicode and emoji | 0.18 s (~280 MB/s), level with Ghostty |
| `cat` of 135 MB of dense 24-bit colour | 0.52 s (~260 MB/s) |
| Key to shell and back, through the daemon | 33 µs median |

## Compared with other terminals

Each terminal runs the same workloads in a bare zsh, with its default window and settings. The terminals take turns, one warm-up round is discarded, and each figure is the median of five runs.

| | Aleno | Ghostty 1.4.0 | Alacritty 0.17.0 | kitty 0.51.1 | WezTerm | Terminal.app 2.15 | iTerm2 3.7.9 | Warp |
|---|---|---|---|---|---|---|---|---|
| Launch to shell | **195 ms** | 217 ms | 256 ms | 559 ms | 229 ms | 224 ms | 558 ms | - |
| `cat` 100 MB plain text | 0.33 s | **0.32 s** | 0.64 s | 0.66 s | 2.15 s | 2.19 s | 0.89 s | 2.16 s |
| `cat` 50 MB unicode and emoji | 0.18 s | **0.18 s** | 0.32 s | 0.40 s | 1.16 s | 1.37 s | 8.19 s | 1.13 s |
| `cat` 135 MB dense colour | **0.52 s** | 0.57 s | 0.91 s | 0.93 s | 3.03 s | 4.17 s | 6.68 s | 2.25 s |
| `seq 1 3000000` | 2.81 s | 2.96 s | 2.80 s | 3.36 s | 2.88 s | **2.75 s** | 4.14 s | 3.14 s |
| Idle CPU | 0.2% | **0.0%** | **0.0%** | 0.2% | **0.0%** | 0.8% | 0.4% | - |
| Memory after the run | 161 MB | 187 MB | **140 MB** | 186 MB | 154 MB | 538 MB | 498 MB | - |

Warp is timed as a typed command block by `tests/benchmarks/compare/warp.sh`; launch time, idle CPU and memory are not measured for it. Alacritty is built from source by `make_alacritty_app.sh`, because Homebrew no longer ships it. `seq` mostly measures how fast `seq` itself prints.

## Throughput

A bare reader that does nothing but spin on the pty drains the 100 MB plain file in 0.24 s and the unicode file in 0.12 s. macOS hands over about 1 KB per read while `cat` refills, so that is the floor for any terminal, and Aleno is within about 0.1 s of it on both.

How it gets there:

1. Parser fast paths for printable ASCII, plain colour sequences and common controls, and cheap history rows.
2. A direct text path: runs of ASCII and valid UTF-8 are decoded in place and written as whole row segments, with a precomputed width table. Wraps, zero-width marks, joiners and anything invalid fall back to the general path, which a differential test checks against the parser on random text.
3. The app reads the shell's pty itself (the daemon passes the file descriptor over), so output is parsed once, in the app.
4. A two-stage pipeline: a gather thread drains the pty into rotating 64 KB buffers while a parse thread feeds the terminal, so reading never waits for parsing. Small interactive output is delivered immediately, so typing gains no latency.
5. A full-screen scroll sets one dirty flag instead of marking every row, and evicted history buffers are reused.

## Terminal engine

Single thread, 200x60 grid, 10k lines of scrollback.

| Workload | Throughput |
|---|---|
| Plain text | ~370 MB/s |
| 24-bit colour and attributes | ~400 MB/s |
| Unicode, wide characters and emoji | ~430 MB/s |
| Editor-like (cursor addressing, scroll regions) | ~240 MB/s, no allocations |
| Clear screen | ~5 µs |
| Scrollback insert | ~4.6M lines/s |
| Search across 100k lines | ~22 ms |
| Resize with 100k lines of history (reflow) | ~6.5 ms |
| Snapshot / restore of 2000 lines | 2.0 ms / 0.6 ms |

Memory per pane is 0.39 MB empty and about 10 MB with a full 10k-line scrollback.

## Sessions and IPC

Measured against the real daemon process over its Unix socket, with real pseudo-terminals.

| Metric | Result |
|---|---|
| Input echo round trip | p50 33 µs, p99 286 µs |
| Attach to a pane | 0.6 ms (empty), 4.5 ms (2k to 10k lines of history) |
| Create a pane (spawn shell and pty) | 1.6 ms |
| 50 panes each printing 20k lines at once | all done in 1.4 s |
| End-to-end `cat` of 135 MB, daemon and client | 0.41 s (~326 MB/s) |
| Idle daemon CPU | 0.05% |
| Daemon memory per idle pane | ~0.12 MB |

## Reproduce

| What | Command |
|---|---|
| Engine | `cargo run --release -p aleno-terminal --example bench` |
| Engine on any file | `cargo run --release -p aleno-terminal --example feedfile -- <file>` |
| Daemon and IPC | `cargo build --release -p aleno-daemon && cargo run --release -p aleno-client --example bench_session` |
| The app: idle, flood, frames, windows | build the app, then `cargo run --release -p aleno-client --example bench_app` (needs a visible desktop, about 2.5 minutes) |
| Launch time (six launches) | `tests/benchmarks/startup.sh` |
| The pty floor for a file | `cargo run --release -p aleno-client --example ptyfloor -- <file> spin` |
| Terminals side by side | `python3 tests/benchmarks/compare/gen_data.py /tmp/aleno-bench/data`, then `python3 tests/benchmarks/compare/run.py` (skips terminals that are not installed) |
| Warp | `tests/benchmarks/compare/warp.sh` |
| Alacritty | `tests/benchmarks/compare/make_alacritty_app.sh` |
| `cat` throughput in any terminal | `tests/benchmarks/cat_timed.sh <file> <result-file>` |
| Per-frame timing | run the app with `ALENO_BENCH=1` |
