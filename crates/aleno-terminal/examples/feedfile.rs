//! Feeds a file through the engine in 64 KB chunks and reports throughput. usage: feedfile <file> [reps] [cols rows]
use aleno_terminal::Terminal;
use std::time::Instant;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let data = std::fs::read(&a[1]).unwrap();
    let reps: usize = a.get(2).map(|s| s.parse().unwrap()).unwrap_or(3);
    let (c, r) = (a.get(3).map(|s| s.parse().unwrap()).unwrap_or(124), a.get(4).map(|s| s.parse().unwrap()).unwrap_or(34));
    let mut best = f64::MAX;
    for _ in 0..reps {
        let mut t = Terminal::new(c, r, 10_000);
        let s = Instant::now();
        for ch in data.chunks(65536) { t.feed(ch); }
        best = best.min(s.elapsed().as_secs_f64());
    }
    println!("{} MB: best {:.3}s = {:.0} MB/s", data.len() / 1_000_000, best, data.len() as f64 / 1e6 / best);
}
