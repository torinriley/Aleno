//! Daemon / IPC benchmarks against a real `aleno-daemon` process. Run:
//!   cargo build --release -p aleno-daemon && cargo run --release -p aleno-client --example bench_session
use aleno_client::*;
use std::process::{Child, Command};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

fn pct(v: &mut Vec<f64>, p: f64) -> f64 { v.sort_by(|a, b| a.partial_cmp(b).unwrap()); v[((v.len() as f64 - 1.0) * p) as usize] }
fn rss_kb(pid: u32) -> u64 {
    let o = Command::new("ps").args(["-o", "rss=", "-p", &pid.to_string()]).output().unwrap();
    String::from_utf8_lossy(&o.stdout).trim().parse().unwrap_or(0)
}

fn attach(sock: &std::path::Path, s: u32, p: u32, cols: u16, rows: u16) -> (PaneClient, Receiver<u32>) {
    let (tx, rx) = channel();
    let c = PaneClient::attach(sock, s, p, cols, rows, 10_000, Box::new(move |ev| { let _ = tx.send(ev); })).expect("attach");
    (c, rx)
}

fn wait_text(c: &PaneClient, rx: &Receiver<u32>, needle: &str, secs: u64) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        if c.shared.term.lock().unwrap().screen_text().contains(needle) { return true; }
        if Instant::now() > end { return false; }
        let _ = rx.recv_timeout(Duration::from_millis(5));
    }
}

fn main() {
    let dir = std::env::temp_dir().join(format!("aleno-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("d.sock");
    let exe = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/release/aleno-daemon");
    let mut daemon: Child = Command::new(&exe).arg("run").env("ALENO_SOCKET", &sock).env("ALENO_IDLE_EXIT_SECS", "0").spawn().expect("daemon");
    let end = Instant::now() + Duration::from_secs(5);
    let ctl = loop { match Ctl::connect(&sock) { Ok(c) => break c, Err(_) if Instant::now() < end => std::thread::sleep(Duration::from_millis(10)), Err(e) => panic!("{e}") } };
    let dpid = daemon.id();
    println!("daemon pid {} rss at start: {:.1} MB", dpid, rss_kb(dpid) as f64 / 1024.0);

    // ---- pane scaling
    println!("\n== Pane scaling (idle /bin/sh panes, 80x24, 10k scrollback) ==");
    let s = ctl.create_session("many").unwrap();
    let base = rss_kb(dpid);
    let st = Instant::now();
    let mut ids = Vec::new();
    for _ in 0..50 { ids.push(ctl.create_pane(s, 80, 24, "/bin/sh", "/tmp", 10_000).unwrap()); }
    let create = st.elapsed().as_secs_f64() * 1e3 / 50.0;
    std::thread::sleep(Duration::from_millis(800));
    let after = rss_kb(dpid);
    println!("create 50 panes: {:.2} ms each; daemon RSS {:.1} -> {:.1} MB = {:.2} MB/pane (daemon only, excluding shells)", create, base as f64 / 1024.0, after as f64 / 1024.0, (after as f64 - base as f64) / 1024.0 / 50.0);
    let clients: Vec<_> = ids.iter().map(|&p| attach(&sock, s, p, 80, 24)).collect();
    let st = Instant::now();
    for (c, _) in &clients { c.send_input(b"i=0; while [ $i -lt 20000 ]; do echo multi $i; i=$((i+1)); done; echo ALLDONE\n"); }
    let mut ok = 0;
    for (c, rx) in &clients { if wait_text(c, rx, "ALLDONE\n", 120) { ok += 1; } }
    println!("50 panes each printing 20k lines concurrently: {}/50 finished in {:.2}s; daemon RSS {:.1} MB", ok, st.elapsed().as_secs_f64(), rss_kb(dpid) as f64 / 1024.0);
    drop(clients);
    ctl.kill_session(s).unwrap();


    // ---- input echo round trip
    let s = ctl.create_session("bench").unwrap();
    let p = ctl.create_pane(s, 1000, 50, "/bin/cat", "/tmp", 10_000).unwrap();
    let (c, rx) = attach(&sock, s, p, 1000, 50);
    std::thread::sleep(Duration::from_millis(300));
    let mut lat = Vec::new();
    for i in 0..3000 {
        // The tty's canonical mode caps a line at 1024 bytes; end the line regularly (not sampled).
        if i % 100 == 99 { c.send_input(b"\n"); std::thread::sleep(Duration::from_millis(30)); continue; }
        let before = { let t = c.shared.term.lock().unwrap(); let (r, col) = t.cursor(); r * 1000 + col };
        let st = Instant::now();
        c.send_input(b"a");
        loop {
            let now = { let t = c.shared.term.lock().unwrap(); let (r, col) = t.cursor(); r * 1000 + col };
            if now != before { break; }
            std::hint::spin_loop();
            if st.elapsed() > Duration::from_millis(500) { break; }
        }
        if i >= 200 { lat.push(st.elapsed().as_secs_f64() * 1e6); }
        std::thread::sleep(Duration::from_micros(500));
    }
    println!("\n== Input echo round trip (key -> daemon -> pty -> daemon -> client mirror), {} samples ==", lat.len());
    println!("p50 {:.0} us   p90 {:.0} us   p99 {:.0} us   max {:.0} us", pct(&mut lat.clone(), 0.5), pct(&mut lat.clone(), 0.9), pct(&mut lat.clone(), 0.99), pct(&mut lat, 1.0));
    drop(c);
    ctl.kill_session(s).unwrap();

    // ---- end-to-end throughput
    let big = dir.join("big.txt");
    {
        use std::io::Write;
        let mut f = std::io::BufWriter::new(std::fs::File::create(&big).unwrap());
        for i in 0..1_500_000u32 { writeln!(f, "{:08} the quick brown fox jumps over the lazy dog \x1b[3{}mcolor\x1b[0m 0123456789 abcdefghij", i, i % 8).unwrap(); }
    }
    let size = std::fs::metadata(&big).unwrap().len() as f64 / 1e6;
    println!("\n== End-to-end cat throughput ({:.0} MB colored text file) ==", size);
    for (label, cols, rows) in [("200x60", 200u16, 60u16), ("80x24", 80, 24)] {
        let s = ctl.create_session("tp").unwrap();
        let p = ctl.create_pane(s, cols, rows, "/bin/sh", "/tmp", 10_000).unwrap();
        let (c, rx) = attach(&sock, s, p, cols, rows);
        std::thread::sleep(Duration::from_millis(300));
        let st = Instant::now();
        c.send_input(format!("cat {}; echo DONE$((40+2))\n", big.display()).as_bytes());
        let ok = wait_text(&c, &rx, "DONE42\n", 120);
        let secs = st.elapsed().as_secs_f64();
        println!("{:<8} {}  {:.2}s  = {:.0} MB/s   (daemon rss {:.0} MB)", label, if ok { "ok  " } else { "TIMEOUT" }, secs, size / secs, rss_kb(dpid) as f64 / 1024.0);
        drop(c);
        ctl.kill_session(s).unwrap();
    }
    let st = Instant::now();
    let _ = Command::new("sh").arg("-c").arg(format!("cat {} > /dev/null", big.display())).status().unwrap();
    println!("reference: cat > /dev/null          {:.2}s  = {:.0} MB/s", st.elapsed().as_secs_f64(), size / st.elapsed().as_secs_f64());

    // ---- attach latency
    println!("\n== Attach latency (AttachPane -> snapshot applied to the client mirror) ==");
    for hist in [0usize, 2_000, 10_000] {
        let s = ctl.create_session("att").unwrap();
        let p = ctl.create_pane(s, 200, 60, "/bin/sh", "/tmp", 10_000).unwrap();
        let (c, rx) = attach(&sock, s, p, 200, 60);
        std::thread::sleep(Duration::from_millis(300));
        c.send_input(format!("i=0; while [ $i -lt {} ]; do echo history line $i; i=$((i+1)); done; echo HISTDONE\n", hist.max(1)).as_bytes());
        wait_text(&c, &rx, "HISTDONE\n", 60);
        drop(c);
        std::thread::sleep(Duration::from_millis(200));
        let mut v = Vec::new();
        for _ in 0..30 {
            let st = Instant::now();
            let (c2, rx2) = attach(&sock, s, p, 200, 60);
            loop {
                if c2.shared.term.lock().unwrap().screen_text().contains("HISTDONE") { break; }
                let _ = rx2.recv_timeout(Duration::from_micros(200));
                if st.elapsed() > Duration::from_secs(2) { break; }
            }
            v.push(st.elapsed().as_secs_f64() * 1e3);
            drop(c2);
            std::thread::sleep(Duration::from_millis(20));
        }
        println!("history {:>6} lines   p50 {:.2} ms   p99 {:.2} ms", hist, pct(&mut v.clone(), 0.5), pct(&mut v, 0.99));
        ctl.kill_session(s).unwrap();
    }

    // ---- idle daemon CPU
    std::thread::sleep(Duration::from_secs(1));
    let cpu = |pid: u32| -> f64 { let o = Command::new("ps").args(["-o", "time=", "-p", &pid.to_string()]).output().unwrap(); let t = String::from_utf8_lossy(&o.stdout).trim().to_string(); let (m, s) = t.split_once(':').unwrap(); m.parse::<f64>().unwrap() * 60.0 + s.parse::<f64>().unwrap() };
    let s2 = ctl.create_session("idle").unwrap();
    let _ = ctl.create_pane(s2, 80, 24, "/bin/sh", "/tmp", 10_000).unwrap();
    std::thread::sleep(Duration::from_secs(2));
    let c0 = cpu(dpid); std::thread::sleep(Duration::from_secs(20)); let c1 = cpu(dpid);
    println!("\n== Idle daemon (1 idle pane, 20 s) == CPU {:.3}%  RSS {:.1} MB", (c1 - c0) / 20.0 * 100.0, rss_kb(dpid) as f64 / 1024.0);

    let _ = Command::new(&exe).arg("stop").env("ALENO_SOCKET", &sock).status();
    let _ = daemon.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
