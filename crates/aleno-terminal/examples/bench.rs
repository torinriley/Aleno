//! Engine benchmarks. Run: cargo run --release -p aleno-terminal --example bench
use aleno_terminal::{Cell, Terminal};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Instant;

struct Counting;
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE: AtomicU64 = AtomicU64::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 { ALLOCS.fetch_add(1, Relaxed); BYTES.fetch_add(l.size() as u64, Relaxed); LIVE.fetch_add(l.size() as u64, Relaxed); System.alloc(l) }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { LIVE.fetch_sub(l.size() as u64, Relaxed); System.dealloc(p, l) }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 { ALLOCS.fetch_add(1, Relaxed); BYTES.fetch_add(n as u64, Relaxed); LIVE.fetch_add(n as u64, Relaxed); LIVE.fetch_sub(l.size() as u64, Relaxed); System.realloc(p, l, n) }
}
#[global_allocator]
static A: Counting = Counting;

fn snap() -> (u64, u64) { (ALLOCS.load(Relaxed), BYTES.load(Relaxed)) }

fn feed(name: &str, data: &[u8], reps: usize) {
    let mut t = Terminal::new(200, 60, 10_000);
    t.feed(data); // warm
    let (a0, b0) = snap();
    let start = Instant::now();
    for _ in 0..reps { t.feed(data); }
    let secs = start.elapsed().as_secs_f64();
    let (a1, b1) = snap();
    let mb = (data.len() * reps) as f64 / 1e6;
    println!("{:<34} {:>7.1} MB/s   {:>8.2} allocs/MB  {:>9.1} KB alloc'd/MB", name, mb / secs, (a1 - a0) as f64 / mb, (b1 - b0) as f64 / mb / 1e3);
}

fn pct(v: &mut Vec<f64>, p: f64) -> f64 { v.sort_by(|a, b| a.partial_cmp(b).unwrap()); v[((v.len() as f64 - 1.0) * p) as usize] }

fn main() {
    println!("== VT parsing + screen update throughput (200x60, 10k scrollback) ==");
    let mut plain = Vec::new();
    for i in 0..2000 { plain.extend_from_slice(format!("line {} the quick brown fox jumps over the lazy dog 0123456789\r\n", i).as_bytes()); }
    let mut color = Vec::new();
    for i in 0..2000 { color.extend_from_slice(format!("\x1b[38;2;{};{};{}mcolored \x1b[1;4;48;5;{}mtext\x1b[0m line {}\r\n", i % 256, (i * 7) % 256, (i * 13) % 256, i % 256, i).as_bytes()); }
    let mut uni = Vec::new();
    for _ in 0..2000 { uni.extend_from_slice("日本語のテキスト é ñ ü 🎉 emoji mix ✓ → ═══ │ box\r\n".as_bytes()); }
    let mut clear = Vec::new();
    for i in 0..500 { clear.extend_from_slice(format!("\x1b[H\x1b[2J\x1b[{};1Hscreen {}\x1b[K", i % 50 + 1, i).as_bytes()); }
    // neovim-like: absolute cursor addressing with partial line redraws and scroll regions
    let mut vi = Vec::new();
    for i in 0..4000 { vi.extend_from_slice(format!("\x1b[{};{}H\x1b[38;5;{}m{}\x1b[K\x1b[2;59r\x1b[1S\x1b[r", i % 58 + 1, (i * 3) % 150 + 1, i % 256, "fn main() { println!(\"hello\"); }").as_bytes()); }
    feed("plain text (cat-like)", &plain, 500);
    feed("24-bit color + attributes", &color, 300);
    feed("unicode / wide / emoji", &uni, 300);
    feed("clear-screen churn", &clear, 2000);
    feed("editor-like (CUP + scroll region)", &vi, 300);

    println!("\n== Scrollback ==");
    let mut t = Terminal::new(200, 60, 100_000);
    let start = Instant::now();
    let lines = 1_000_000usize;
    let mut buf = Vec::new();
    for i in 0..lines { buf.extend_from_slice(format!("scrollback line {} lorem ipsum dolor sit amet consectetur\r\n", i).as_bytes()); if buf.len() > 1 << 20 { t.feed(&buf); buf.clear(); } }
    t.feed(&buf);
    let s = start.elapsed().as_secs_f64();
    println!("insert 1M lines (100k cap)         {:.2}s  = {:.2}M lines/s   scrollback_len={}", s, lines as f64 / s / 1e6, t.scrollback_len());

    // navigation: sweep the display offset through the whole history, copying the view each step
    let mut cells = vec![Cell::BLANK; 200 * 60];
    let mut dirty = vec![0u8; 60];
    t.copy_view(200, 60, &mut cells, &mut dirty);
    let mut times = Vec::new();
    let n = t.scrollback_len();
    let mut off = 0;
    while off < n { t.set_display_offset(off); let st = Instant::now(); t.copy_view(200, 60, &mut cells, &mut dirty); times.push(st.elapsed().as_secs_f64() * 1e6); off += 7; }
    println!("scroll nav copy_view (full redraw) p50 {:.1}us  p99 {:.1}us  max {:.1}us  over {} steps", pct(&mut times.clone(), 0.5), pct(&mut times.clone(), 0.99), pct(&mut times, 1.0), n / 7);
    t.set_display_offset(0);

    println!("\n== Other engine operations ==");
    let st = Instant::now();
    let m = t.search("line 99999", true, 1000);
    println!("search 100k lines (1 hit set)      {:.2} ms  ({} matches)", st.elapsed().as_secs_f64() * 1e3, m.len());
    let st = Instant::now();
    let m = t.search("lorem", true, 1_000_000);
    println!("search 100k lines (100k hits)      {:.2} ms  ({} matches)", st.elapsed().as_secs_f64() * 1e3, m.len());
    let st = Instant::now();
    let sn = t.snapshot_vt(2000);
    println!("snapshot (2000 history + screen)   {:.2} ms  ({} KB)", st.elapsed().as_secs_f64() * 1e3, sn.len() / 1024);
    let st = Instant::now();
    let mut t2 = Terminal::new(200, 60, 10_000);
    t2.feed(&sn);
    println!("restore from snapshot              {:.2} ms", st.elapsed().as_secs_f64() * 1e3);
    let st = Instant::now();
    for i in 0..200 { t.resize(100 + (i % 100), 30 + (i % 40)); }
    println!("resize x200 (100k scrollback)      {:.2} ms total  ({:.1} us each)", st.elapsed().as_secs_f64() * 1e3, st.elapsed().as_secs_f64() * 1e6 / 200.0);

    println!("\n== Memory per pane (engine state only) ==");
    for (label, sb) in [("empty, 10k scrollback cap", 10_000usize), ("full 10k scrollback", 10_000), ("full 100k scrollback", 100_000)] {
        let before = LIVE.load(Relaxed);
        let mut t = Terminal::new(200, 60, sb);
        if label.starts_with("full") { for i in 0..sb + 100 { t.feed(format!("line {} lorem ipsum dolor sit amet consectetur adipiscing\r\n", i).as_bytes()); } }
        let after = LIVE.load(Relaxed);
        println!("{:<34} {:>7.2} MB", label, (after - before) as f64 / 1e6);
        drop(t);
    }
}
