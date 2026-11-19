//! Profile target: feed the real benchmark file (with pty-style CRLF) through the terminal. Usage: prof [seconds]
use aleno_terminal::Terminal;
fn main() {
    let secs: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let raw = std::fs::read("/tmp/ag/big.txt").expect("/tmp/ag/big.txt");
    let mut data = Vec::with_capacity(raw.len() + raw.len() / 80);
    for &b in &raw { if b == b'\n' { data.push(b'\r'); } data.push(b); }
    let mut t = Terminal::new(124, 34, 10_000);
    let st = std::time::Instant::now();
    let mut total = 0usize;
    loop {
        for c in data.chunks(64 * 1024) { t.feed(c); }
        total += data.len();
        if secs == 0 || st.elapsed().as_secs() >= secs { break; }
    }
    let s = st.elapsed().as_secs_f64();
    println!("parse-only: {:.0} MB in {:.2}s = {:.0} MB/s", total as f64 / 1e6, s, total as f64 / 1e6 / s);
}
