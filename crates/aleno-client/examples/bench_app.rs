//! End-to-end benchmark of the real macOS app (Aleno.app built in ./build). Run:
//!   cargo run --release -p aleno-client --example bench_app
//! Needs the app built (xcodebuild ... -derivedDataPath build) and a visible desktop session.
use aleno_client::*;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn ps(pid: u32, field: &str) -> String {
    let o = Command::new("ps").args(["-o", &format!("{}=", field), "-p", &pid.to_string()]).output().unwrap();
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}
fn cpu_secs(pid: u32) -> f64 { let t = ps(pid, "time"); let (m, s) = t.split_once(':').unwrap_or(("0", "0")); m.parse::<f64>().unwrap_or(0.0) * 60.0 + s.parse::<f64>().unwrap_or(0.0) }
fn rss_mb(pid: u32) -> f64 { ps(pid, "rss").parse::<f64>().unwrap_or(0.0) / 1024.0 }
fn osa(script: &str) -> String { String::from_utf8_lossy(&Command::new("osascript").args(["-e", script]).output().unwrap().stdout).trim().to_string() }

fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let app = root.join("build/Build/Products/Release/Aleno.app/Contents/MacOS/Aleno");
    let daemon_bin = root.join("target/release/aleno-daemon");
    let dir = std::env::temp_dir().join(format!("aleno-appbench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("d.sock");
    let log = dir.join("app.log");

    // test data
    let big = dir.join("big.txt");
    {
        use std::io::Write;
        let mut f = std::io::BufWriter::new(std::fs::File::create(&big).unwrap());
        for i in 0..1_500_000u32 { writeln!(f, "{:08} the quick brown fox jumps over the lazy dog \x1b[3{}mcolor\x1b[0m 0123456789 abcdefghij", i, i % 8).unwrap(); }
    }
    let size = std::fs::metadata(&big).unwrap().len() as f64 / 1e6;

    let mut daemon = Command::new(&daemon_bin).arg("run").env("ALENO_SOCKET", &sock).env("ALENO_IDLE_EXIT_SECS", "0").env("ALENO_BENCH_DIR", &dir).stderr(Stdio::null()).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(400));
    let ctl = Ctl::connect(&sock).unwrap();
    let s = ctl.create_session("appbench").unwrap();
    let shell = root.join("tests/benchmarks/bench_shell.sh");
    let p = ctl.create_pane(s, 120, 40, shell.to_str().unwrap(), "/tmp", 10_000).unwrap();
    let _ = p;

    let mut child = Command::new(&app)
        .env("ALENO_SOCKET", &sock).env("ALENO_BENCH", "1").env("ALENO_DAEMON_PATH", &daemon_bin)
        .stderr(std::fs::File::create(&log).unwrap()).stdout(Stdio::null()).spawn().expect("launch app");
    let pid = child.id();
    std::thread::sleep(Duration::from_secs(12));
    let front = |pid: u32| -> bool {
        for _ in 0..10 {
            osa(&format!("tell application \"System Events\" to set frontmost of (first process whose unix id is {}) to true", pid));
            std::thread::sleep(Duration::from_millis(500));
            if osa("tell application \"System Events\" to get name of first application process whose frontmost is true") == "Aleno" { return true; }
        }
        false
    };
    println!("Aleno frontmost for the run: {}", front(pid));

    println!("\n== App idle (1 window, 1 pane, window visible) ==");
    let (c0, t0) = (cpu_secs(pid), Instant::now());
    let r0 = rss_mb(pid);
    std::thread::sleep(Duration::from_secs(30));
    let (c1, t1) = (cpu_secs(pid), Instant::now());
    println!("idle CPU over 30 s: {:.3}%   RSS {:.0} -> {:.0} MB", (c1 - c0) / (t1 - t0).as_secs_f64() * 100.0, r0, rss_mb(pid));

    // Flood through the window: the pane's driver script runs cat_timed.sh when triggered, so the window keeps the pty.
    println!("\n== cat_timed.sh through the real app, 3 runs ({:.0} MB; window owns the pty: {}) ==", size, ctl.list_sessions().unwrap()[0].clients >= 1);
    let mut runs = Vec::new();
    for i in 1..=3 {
        let cpu_a = cpu_secs(pid);
        let t = Instant::now();
        std::fs::write(dir.join(format!("go{}", i)), b"1").unwrap();
        let res = dir.join(format!("res{}.txt", i));
        let end = Instant::now() + Duration::from_secs(120);
        while Instant::now() < end && !std::fs::read_to_string(&res).map(|x| x.split_whitespace().count() == 2).unwrap_or(false) { std::thread::sleep(Duration::from_millis(20)); }
        if let Ok(x) = std::fs::read_to_string(&res) { let v: Vec<f64> = x.split_whitespace().filter_map(|y| y.parse().ok()).collect(); if v.len() == 2 { runs.push(v[1] - v[0]); } }
        println!("run {}: app CPU {:.0}% of one core, RSS {:.0} MB", i, (cpu_secs(pid) - cpu_a) / t.elapsed().as_secs_f64() * 100.0, rss_mb(pid));
        std::thread::sleep(Duration::from_millis(2500));
    }
    println!("Aleno cat_timed runs: {}", runs.iter().map(|d| format!("{:.2}s ({:.0} MB/s)", d, size / d)).collect::<Vec<_>>().join(", "));

    println!("\n== Warm window creation (⌘N) ==");
    std::thread::sleep(Duration::from_secs(2));
    for _ in 0..3 {
        osa(&format!("tell application \"System Events\" to set frontmost of (first process whose unix id is {}) to true", pid));
        std::thread::sleep(Duration::from_millis(400));
        if osa("tell application \"System Events\" to get name of first application process whose frontmost is true") == "Aleno" {
            osa("tell application \"System Events\" to tell process \"Aleno\" to keystroke \"n\" using command down");
        }
        std::thread::sleep(Duration::from_millis(1500));
    }

    println!("\n-- raw BENCH lines from the app --");
    let _ = child.kill();
    let _ = child.wait();
    println!("{}", std::fs::read_to_string(&log).unwrap_or_default().lines().filter(|l| l.starts_with("BENCH")).collect::<Vec<_>>().join("\n"));
    let _ = Command::new(&daemon_bin).arg("stop").env("ALENO_SOCKET", &sock).status();
    let _ = daemon.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
