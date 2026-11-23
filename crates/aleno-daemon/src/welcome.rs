//! The first-run welcome: `aleno-daemon welcome -- <shell> <arg0> [args...]`.
//!
//! It runs in a new pane before the user's shell: it draws the Aleno wordmark with a left-to-right reveal, prints a
//! short introduction (or, after an update, the version and a short changelog, see `whatsnew.rs`), then replaces itself with the shell (so the shell is the pane's process, exactly as if it had
//! been started directly). Any key skips the animation. When stdout is not a terminal it just starts the shell.

use std::io::{self, Write};
use std::os::unix::process::CommandExt;
use std::process::Command;

/// 8-bit coverage of the wordmark, `LOGO_W` x `LOGO_H`, rows top to bottom (rendered from the SVG by a one-off script).
const LOGO: &[u8] = include_bytes!("welcome_logo.bin");
const LOGO_W: usize = 384;
const LOGO_H: usize = 120;

const FRAMES: usize = 40;
/// How far behind the reveal front a dot still glows.
const GLOW: f32 = 0.075;

/// The wordmark as a grid of braille dots (2 x 4 per terminal cell) with a drop shadow, and a per-dot reveal time.
pub struct Dots {
    /// Grid size in dots; always a whole number of cells (`w` even, `h` a multiple of 4).
    pub w: usize,
    pub h: usize,
    front: Vec<bool>,
    shadow: Vec<bool>,
    /// 0..1: when the dot appears. Mostly left to right, with scatter, so the logo materializes.
    key: Vec<f32>,
}

fn hash(x: usize, y: usize) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA6B);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    (h & 0xFFFF) as f32 / 65535.0
}

/// Rasterize the wordmark into `cells` terminal columns of braille dots.
pub fn logo_dots(cells: usize) -> Dots {
    const SHADOW: usize = 2; // shadow offset in dots, right and down
    let w = cells.clamp(8, 90) * 2;
    let lw = w - SHADOW;
    let lh = (lw * LOGO_H / LOGO_W).max(4);
    // The round letters' tops start one third of the way down (the "l" is the tallest). Shift the logo so that row falls
    // on a cell boundary: otherwise the 2 or 3 dot sliver at the very top of each round letter sits alone in its own
    // cell row and looks like a small block floating above the letter.
    let top = (4 - (lh / 3) % 4) % 4;
    let h = (top + lh + SHADOW).div_ceil(4) * 4;
    let on = |x: usize, y: usize| -> bool {
        let (x0, x1) = (x * LOGO_W / lw, ((x + 1) * LOGO_W / lw).max(x * LOGO_W / lw + 1));
        let (y0, y1) = (y * LOGO_H / lh, ((y + 1) * LOGO_H / lh).max(y * LOGO_H / lh + 1));
        let (mut sum, mut n) = (0u32, 0u32);
        for yy in y0..y1.min(LOGO_H) {
            for xx in x0..x1.min(LOGO_W) {
                sum += LOGO[yy * LOGO_W + xx] as u32;
                n += 1;
            }
        }
        n > 0 && sum / n >= 112
    };
    let mut front = vec![false; w * h];
    let mut shadow = vec![false; w * h];
    let mut key = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            key[y * w + x] = (x as f32 / w as f32) * 0.78 + hash(x, y) * 0.22;
        }
    }
    for y in 0..lh {
        for x in 0..lw {
            if on(x, y) {
                front[(y + top) * w + x] = true;
                shadow[(y + top + SHADOW) * w + x + SHADOW] = true;
            }
        }
    }
    for i in 0..w * h {
        if front[i] {
            shadow[i] = false; // the shadow only shows where the letters are not
        }
    }
    Dots { w, h, front, shadow, key }
}

/// Braille bit for the dot at (`dx`, `dy`) inside a cell (dx 0..2, dy 0..4).
fn braille_bit(dx: usize, dy: usize) -> u32 {
    match (dx, dy) {
        (0, 0) => 0x01,
        (0, 1) => 0x02,
        (0, 2) => 0x04,
        (1, 0) => 0x08,
        (1, 1) => 0x10,
        (1, 2) => 0x20,
        (0, 3) => 0x40,
        _ => 0x80,
    }
}

/// The rows of one animation frame at reveal progress `p` (0 = nothing, 1.2 or more = complete). Dots just behind
/// the reveal front glow (bold), the letters get a bright top and a normal body, and the shadow is dim.
pub fn render_frame(d: &Dots, p: f32) -> Vec<String> {
    let rows = d.h / 4;
    (0..rows)
        .map(|r| {
            let mut line = String::new();
            let mut style = 0u8; // 0 normal, 1 bold, 2 dim
            for c in 0..d.w / 2 {
                let (mut front, mut shadow, mut hot) = (0u32, 0u32, false);
                for dy in 0..4 {
                    for dx in 0..2 {
                        let i = (r * 4 + dy) * d.w + c * 2 + dx;
                        if d.front[i] && d.key[i] < p {
                            front |= braille_bit(dx, dy);
                            hot |= p - d.key[i] < GLOW;
                        } else if d.shadow[i] && d.key[i] + 0.06 < p {
                            shadow |= braille_bit(dx, dy);
                        }
                    }
                }
                let (bits, want) = if front != 0 {
                    (front, if hot || r * 5 < rows * 2 { 1 } else { 0 })
                } else if shadow != 0 {
                    (shadow, 2)
                } else {
                    (0, style)
                };
                if bits != 0 && want != style {
                    line.push_str(match want {
                        1 => "\x1b[22;1m",
                        2 => "\x1b[22;2m",
                        _ => "\x1b[22m",
                    });
                    style = want;
                }
                line.push(if bits == 0 { ' ' } else { char::from_u32(0x2800 + bits).unwrap_or(' ') });
            }
            if style != 0 {
                line.push_str("\x1b[22m");
            }
            line
        })
        .collect()
}

fn text_lines(cols: usize) -> Vec<String> {
    let b = |s: &str| format!("\x1b[1m{}\x1b[22m", s);
    let d = |s: &str| format!("\x1b[2m{}\x1b[22m", s);
    let mut v = vec![String::new(), b("Welcome to Aleno"), d(if cols >= 50 { "A fast, minimal, native terminal for macOS." } else { "A native terminal for macOS." }), String::new()];
    if cols >= 80 {
        v.push("\u{2318}T  new tab        \u{2318}D  split right      \u{2318}\u{21E7}T  reopen a closed tab".into());
        v.push("\u{2318}F  find           \u{2318}K  clear            \u{2318},   open settings".into());
    } else {
        v.push("\u{2318}T  new tab         \u{2318}D  split right".into());
        v.push("\u{2318}F  find            \u{2318}K  clear".into());
        v.push("\u{2318}\u{21E7}T reopen tab    \u{2318},  settings".into());
    }
    v.push(String::new());
    if cols >= 80 {
        v.push("Failed commands get a red bar; \u{2325}\u{2318}C copies the last output.".into());
        v.push("Long commands notify you when they finish; brew and cargo show progress.".into());
    } else {
        v.push("Failed commands get a red bar;".into());
        v.push("\u{2325}\u{2318}C copies the last output.".into());
    }
    v.push(String::new());
    if cols >= 80 {
        v.push(format!("{}  ~/.config/aleno/settings.json  {}", b("Settings"), d("(\u{2318}, opens it)")));
    } else {
        v.push(format!("{}  \u{2318}, opens settings.json", b("Settings")));
    }
    v.push(format!("{}  github.com/torinriley/Aleno", b(if cols >= 80 { "Docs    " } else { "Docs" })));
    v.push(String::new());
    v
}

fn terminal_size() -> (usize, usize) {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(1, libc::TIOCGWINSZ as _, &mut ws) == 0 && ws.ws_col > 0 {
            return (ws.ws_col as usize, ws.ws_row as usize);
        }
    }
    (80, 24)
}

/// Waits up to `ms`; returns true when a key was pressed (and swallows it).
fn key_pressed_within(ms: u64) -> bool {
    unsafe {
        let mut p = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
        if libc::poll(&mut p, 1, ms as libc::c_int) > 0 && p.revents & libc::POLLIN != 0 {
            let mut buf = [0u8; 64];
            libc::read(0, buf.as_mut_ptr() as *mut _, buf.len());
            return true;
        }
    }
    false
}

fn animate(out: &mut impl Write, cols: usize, rows: usize, lines: Vec<String>) {
    let indent = 2usize;
    let wide = cols >= 40 && rows >= 14;
    let _ = write!(out, "\x1b[?25l");
    let mut skipped = false;
    if wide {
        let dots = logo_dots((cols - indent * 2).min(76));
        let n = dots.h / 4;
        let pad = " ".repeat(indent);
        let _ = write!(out, "\r\n{}", "\r\n".repeat(n));
        for f in 0..=FRAMES {
            let p = if skipped { 2.0 } else { 1.25 * f as f32 / FRAMES as f32 };
            let _ = write!(out, "\x1b[{}A", n);
            for row in render_frame(&dots, p) {
                let _ = write!(out, "\r{}{}\x1b[K\r\n", pad, row);
            }
            let _ = out.flush();
            if f < FRAMES && !skipped && key_pressed_within(24) {
                skipped = true;
            }
            if skipped && f < FRAMES {
                // draw the finished frame once, then leave the loop
                let _ = write!(out, "\x1b[{}A", n);
                for row in render_frame(&dots, 2.0) {
                    let _ = write!(out, "\r{}{}\x1b[K\r\n", pad, row);
                }
                break;
            }
        }
    }
    let pad = " ".repeat(indent);
    for line in lines {
        let _ = write!(out, "\r{}{}\x1b[K\r\n", pad, line);
        let _ = out.flush();
        if !skipped && key_pressed_within(if wide { 38 } else { 0 }) {
            skipped = true;
        }
    }
    let _ = write!(out, "\x1b[?25h");
    let _ = out.flush();
}

/// `args` is everything after `welcome`: `-- <shell> <arg0> [shell args...]`.
pub fn run(args: &[String]) -> i32 {
    let rest: Vec<&String> = args.iter().skip_while(|a| a.as_str() != "--").skip(1).collect();
    let Some(shell) = rest.first().map(|s| s.to_string()) else {
        eprintln!("usage: aleno-daemon welcome -- <shell> <arg0> [args...]");
        return 2;
    };
    let arg0 = rest.get(1).map(|s| s.to_string()).unwrap_or_else(|| shell.clone());
    let shell_args: Vec<String> = rest.iter().skip(2).map(|s| s.to_string()).collect();

    let is_tty = unsafe { libc::isatty(1) == 1 && libc::isatty(0) == 1 };
    if is_tty && std::env::var_os("ALENO_WELCOME_SKIP").is_none() {
        // Raw-ish input so a single key press skips the animation without echoing.
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        let have = unsafe { libc::tcgetattr(0, &mut saved) } == 0;
        if have {
            let mut raw = saved;
            raw.c_lflag &= !(libc::ICANON | libc::ECHO);
            raw.c_cc[libc::VMIN] = 0;
            raw.c_cc[libc::VTIME] = 0;
            unsafe { libc::tcsetattr(0, libc::TCSANOW, &raw) };
        }
        let (cols, rows) = terminal_size();
        let lines = match crate::whatsnew::pending_update() {
            Some(from) => crate::whatsnew::banner_lines(cols, from.as_deref()),
            None => text_lines(cols),
        };
        animate(&mut io::stdout().lock(), cols, rows, lines);
        crate::whatsnew::record_seen();
        if have {
            unsafe { libc::tcsetattr(0, libc::TCSANOW, &saved) };
        }
    }
    let err = Command::new(&shell).arg0(arg0).args(shell_args).exec();
    eprintln!("aleno: could not start {}: {}", shell, err);
    127
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(s: &str) -> String {
        let mut out = String::new();
        let mut it = s.chars();
        while let Some(c) = it.next() {
            if c == '\x1b' {
                for n in it.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn logo_has_ink_and_the_right_shape() {
        for cells in [24usize, 40, 64, 76] {
            let d = logo_dots(cells);
            assert_eq!((d.w % 2, d.h % 4), (0, 0));
            let rows = render_frame(&d, 2.0);
            assert_eq!(rows.len(), d.h / 4);
            assert!(rows.iter().all(|r| strip(r).chars().count() == d.w / 2), "every row is {} cells wide", d.w / 2);
            let ink = rows.iter().flat_map(|r| strip(r).chars().collect::<Vec<_>>()).filter(|c| *c != ' ').count();
            assert!(ink > cells, "the wordmark must draw something at {} cells", cells);
        }
        let d = logo_dots(76);
        assert!((10..=14).contains(&(d.h / 4)), "rows {}", d.h / 4);
    }

    #[test]
    fn frames_materialize_and_end_complete() {
        let d = logo_dots(60);
        let count = |p: f32| render_frame(&d, p).iter().flat_map(|r| strip(r).chars().collect::<Vec<_>>()).filter(|c| *c != ' ').count();
        assert_eq!(count(0.0), 0, "nothing at the start");
        let (a, b, c) = (count(0.3), count(0.7), count(1.3));
        assert!(0 < a && a < b && b < c, "monotonic reveal: {} {} {}", a, b, c);
        assert_eq!(count(1.3), count(5.0), "complete by the end");
        assert!(render_frame(&d, 0.5).iter().any(|r| r.contains("\x1b[22;1m")), "a glowing front while revealing");
        assert!(render_frame(&d, 5.0).iter().any(|r| r.contains("\x1b[22;2m")), "the shadow is dim");
    }

    #[test]
    fn text_fits_narrow_and_wide_terminals() {
        let visible = |s: &str| {
            let mut out = String::new();
            let mut it = s.chars().peekable();
            while let Some(c) = it.next() {
                if c == '\x1b' {
                    while let Some(n) = it.next() {
                        if n.is_ascii_alphabetic() {
                            break;
                        }
                    }
                } else {
                    out.push(c);
                }
            }
            out.chars().count()
        };
        for cols in [40usize, 60, 80, 120] {
            for l in text_lines(cols) {
                assert!(visible(&l) + 4 <= cols, "line too wide for {} columns: {:?}", cols, l);
            }
        }
    }
}
