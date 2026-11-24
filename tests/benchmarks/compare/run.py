#!/usr/bin/env python3
"""Compares Aleno, Terminal.app, iTerm2, Ghostty, kitty, WezTerm and Alacritty on the same Mac with identical workloads.

usage: run.py [--runs N] [--warmup N] [--data DIR] [--out results.json] [Aleno Terminal iTerm Ghostty ...]

Each run starts a fresh instance of the terminal (open -n) with ZDOTDIR pointing at zdot/, so the shell inside runs
zdot/.zshrc: it records when the shell was ready, then times four workloads (cat of a 135 MB 24-bit colour file, cat of
100 MB of plain text, `seq 1 3000000`, cat of 50 MB of unicode and emoji) and writes the timestamps to a file. The harness
then measures memory and idle CPU of the instance, and kills only the processes that instance started.

Terminals that are not installed are skipped. Warp is measured separately by warp.sh: it needs the command to run as a
real command block. The terminals take turns (round robin) so that drift in the machine affects all of them equally, and the first --warmup
rounds are thrown away: a first launch is slower than every later one (Gatekeeper, first-run setup, cold caches).

The terminals run with their default windows and settings, so this is the out-of-the-box experience. Close other
windows and leave the machine alone while it runs.
"""
import argparse, json, os, platform, statistics, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
APPS = {
    "Aleno": "/Applications/Aleno.app",
    "Terminal": "/System/Applications/Utilities/Terminal.app",
    "iTerm": "/Applications/iTerm.app",
    "Ghostty": "/Applications/Ghostty.app",
    "kitty": "/Applications/kitty.app",
    "WezTerm": "/Applications/WezTerm.app",
    # Homebrew no longer ships an Alacritty cask (the official build fails Gatekeeper), so make_alacritty_app.sh builds
    # it from source into this bundle.
    "Alacritty": "/tmp/aleno-bench/Alacritty.app",
}
# Extra arguments per terminal. AppKit terminals are told not to restore windows; Ghostty gets its built-in defaults
# (not the user's config) and no saved window state.
ARGS = {
    "Ghostty": ["--config-default-files=false", "--window-save-state=never"],
    "kitty": ["--config", "NONE"],
    "WezTerm": ["--skip-config"],
    "Alacritty": ["--config-file", "/dev/null"],
}
DEFAULT_ARGS = ["-ApplePersistenceIgnoreState", "YES"]
WORK = ["color", "plain", "seq", "unicode"]
MB = {"color": 135.0, "plain": 100.0, "unicode": 50.0}


def procs():
    out = subprocess.run(["ps", "-axo", "pid=,rss=,time=,command="], capture_output=True, text=True).stdout
    rows = {}
    for line in out.splitlines():
        p = line.split(None, 3)
        if len(p) == 4:
            rows[int(p[0])] = (int(p[1]), p[2], p[3])
    return rows


def cpu_seconds(t):
    parts = t.replace("-", ":").split(":")
    secs = 0.0
    for x in parts:
        secs = secs * 60 + float(x)
    return secs


def instance_procs(bundle, before):
    return {pid: v for pid, v in procs().items() if pid not in before and (bundle + "/") in v[2]}


def run_once(name, bundle, data, tag):
    res = f"/tmp/aleno-bench/res_{name}_{tag}.json"
    for f in (res, res + ".tmp"):
        if os.path.exists(f):
            os.remove(f)
    before = set(procs())
    env = [f"ZDOTDIR={HERE}/zdot", f"BENCH_RES={res}", f"BENCH_DIR={data}", "ALENO_WELCOME_SKIP=1",
           f"ALENO_SOCKET=/tmp/aleno-bench/{tag}.sock"]
    env += [e for e in os.environ.get("BENCH_EXTRA_ENV", "").split(",") if e]   # extra NAME=value pairs for the app
    cmd = ["open", "-n", "-a", bundle]
    for e in env:
        cmd += ["--env", e]
    cmd += ["--args"] + ARGS.get(name, DEFAULT_ARGS)
    t0 = time.time()
    subprocess.run(cmd, check=True)
    deadline = t0 + 400
    while not os.path.exists(res) and time.time() < deadline:
        time.sleep(0.25)
    out = {}
    if os.path.exists(res):
        marks = {}
        for line in open(res):
            k, *v = line.split()
            if k in marks:       # two shells ran the benchmark and wrote to the same file: the run is not valid
                raise SystemExit(f"{name}: {k} was recorded twice; another shell ran zdot/.zshrc")
            marks[k] = [float(x) for x in v]
        out["launch_to_shell_ms"] = (marks["ready"][0] - t0) * 1000
        for w in WORK:
            if w in marks:
                out[w] = marks[w][1] - marks[w][0]
        # Idle: let the output settle, then sample memory and CPU over five seconds.
        time.sleep(4)
        a = instance_procs(bundle, before)
        time.sleep(5)
        b = instance_procs(bundle, before)
        out["rss_mb"] = sum(v[0] for v in b.values()) / 1024
        cpu = sum(cpu_seconds(v[1]) for v in b.values()) - sum(cpu_seconds(v[1]) for k, v in a.items() if k in b)
        out["idle_cpu_pct"] = cpu / 5 * 100
        out["procs"] = len(b)
    for pid in instance_procs(bundle, before):
        try:
            os.kill(pid, 9)
        except ProcessLookupError:
            pass
    time.sleep(1.5)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--warmup", type=int, default=1)
    ap.add_argument("--data", default="/tmp/aleno-bench/data")
    ap.add_argument("--out", default="/tmp/aleno-bench/results.json")
    ap.add_argument("--bundle", action="append", default=[], metavar="NAME=PATH",
                    help="add or replace a terminal, for example --bundle AlenoNew=build/Build/Products/Release/Aleno.app")
    ap.add_argument("apps", nargs="*", default=None)
    a = ap.parse_args()
    for b in a.bundle:
        name, path = b.split("=", 1)
        APPS[name] = os.path.abspath(path)
    a.apps = a.apps or [n for n, b in APPS.items() if os.path.exists(b)]
    missing = [n for n in a.apps if not os.path.exists(APPS[n])]
    if missing:
        raise SystemExit("not installed: " + ", ".join(f"{n} ({APPS[n]})" for n in missing))
    os.makedirs("/tmp/aleno-bench", exist_ok=True)
    results = {name: [] for name in a.apps}
    for i in range(a.warmup + a.runs):
        for name in a.apps:
            r = run_once(name, APPS[name], a.data, f"{name}{i}")
            warm = i < a.warmup
            if not warm:
                results[name].append(r)
            print(name, "warm-up" if warm else i - a.warmup + 1, json.dumps({k: round(v, 2) for k, v in r.items()}), flush=True)
    info = {"machine": subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"], capture_output=True, text=True).stdout.strip(),
            "macos": platform.mac_ver()[0]}
    for name, b in APPS.items():
        p = os.path.join(b, "Contents/Info.plist")
        if os.path.exists(p):
            info[name] = subprocess.run(["/usr/libexec/PlistBuddy", "-c", "Print :CFBundleShortVersionString", p], capture_output=True, text=True).stdout.strip()
    json.dump({"info": info, "results": results}, open(a.out, "w"), indent=1)

    print("\nMedian of", a.runs, "runs   (", info, ")")
    print(f"{'':10}{'launch ms':>11}{'color s':>9}{'plain s':>9}{'seq s':>8}{'unicode s':>11}{'RSS MB':>9}{'idle CPU%':>11}")
    for name, runs in results.items():
        runs = [r for r in runs if "color" in r]
        if not runs:
            print(f"{name:10} no results")
            continue
        med = lambda k: statistics.median(r[k] for r in runs if k in r)
        print(f"{name:10}{med('launch_to_shell_ms'):11.0f}{med('color'):9.2f}{med('plain'):9.2f}{med('seq'):8.2f}{med('unicode'):11.2f}{med('rss_mb'):9.0f}{med('idle_cpu_pct'):11.1f}")


main()
