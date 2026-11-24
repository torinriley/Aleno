//! Feeds a file through the engine in 64 KB chunks and reports throughput. usage: feedfile <file> [reps] [cols rows scrollback]   (RAW=1: feed the bytes as they are)
use aleno_terminal::Terminal;
use std::time::Instant;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut data = std::fs::read(&a[1]).unwrap();
    // A terminal sees "\r\n": the tty turns every "\n" into one (ONLCR). Do the same unless RAW=1.
    if std::env::var_os("RAW").is_none() {
        let mut out = Vec::with_capacity(data.len() + data.len() / 60);
        for &b in &data {
            if b == b'\n' { out.push(b'\r'); }
            out.push(b);
        }
        data = out;
    }
    let reps: usize = a.get(2).map(|s| s.parse().unwrap()).unwrap_or(3);
    let (c, r) = (a.get(3).map(|s| s.parse().unwrap()).unwrap_or(124), a.get(4).map(|s| s.parse().unwrap()).unwrap_or(34));
    let sb: usize = a.get(5).map(|s| s.parse().unwrap()).unwrap_or(10_000);
    let mut best = f64::MAX;
    for _ in 0..reps {
        let mut t = Terminal::new(c, r, sb);
        let s = Instant::now();
        for ch in data.chunks(65536) { t.feed(ch); }
        best = best.min(s.elapsed().as_secs_f64());
    }
    println!("{} MB: best {:.3}s = {:.0} MB/s", data.len() / 1_000_000, best, data.len() as f64 / 1e6 / best);
}
