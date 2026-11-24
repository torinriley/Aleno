//! Aleno terminal engine: incremental VT parser + screen model.
//!
//! The engine is used in two places: inside the session daemon (authoritative state,
//! answers device queries, produces snapshots) and inside the GUI client (mirror of the
//! daemon state that the Metal renderer reads).

mod cell;
mod screen;

pub use cell::*;
pub use screen::{mode, Block, Screen};

use unicode_width::UnicodeWidthChar;

pub struct Terminal {
    parser: vte::Parser,
    s: Screen,
    /// Decode runs of non-ASCII text directly (see `Screen::print_text`). Tests turn it off to get the reference result.
    text_fast: bool,
}

/// One OSC 133 command block. `repr(C)` for FFI; rows are line indexes into history+screen, -1 when unknown.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BlockInfo {
    pub prompt_row: u32,
    /// First output line, or -1 while the command has not started.
    pub output_row: i32,
    /// First line after the output, or -1 while it is still running.
    pub end_row: i32,
    /// Exit status, or `i32::MIN` when unknown.
    pub exit: i32,
    pub dur_ms: i32,
    pub running: u32,
}

/// A command that just finished (live, not a snapshot replay). `repr(C)` for FFI.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FinishedInfo {
    /// Exit status, or `i32::MIN` when unknown.
    pub exit: i32,
    pub dur_ms: i32,
    /// Line index of the last prompt line (where the command text is), or -1 if it scrolled away.
    pub cmd_row: i32,
    pub _pad: i32,
}

/// Geometry and cursor state returned alongside a viewport copy. `repr(C)` for FFI.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ViewInfo {
    pub cols: u32,
    pub rows: u32,
    pub cursor_row: u32,
    pub cursor_col: u32,
    pub cursor_style: u32,
    /// 1 when the cursor is visible in the current viewport.
    pub cursor_visible: u32,
    pub display_offset: u32,
    pub scrollback_len: u32,
    /// Absolute line index (into history+screen) of the first displayed row.
    pub top_abs: u32,
    pub modes: u32,
    /// 1 if the caller's buffers did not match the current size; nothing was copied.
    pub needs_realloc: u32,
    pub _pad: u32,
    pub evicted: u64,
    pub bell_count: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    pub row: usize,
    pub col_start: usize,
    pub col_end: usize,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize, scrollback: usize) -> Terminal {
        Terminal { parser: vte::Parser::new(), s: Screen::new(cols, rows, scrollback), text_fast: true }
    }

    /// When false (GUI mirror), device-status queries are not answered; the daemon answers them.
    pub fn set_respond(&mut self, on: bool) {
        self.s.respond = on;
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        let mut i = 0;
        let n = bytes.len();
        while i < n {
            if self.parser.is_ground() {
                // Run of printable ASCII: bypass the state machine.
                let start = i;
                while i < n && (0x20..0x7f).contains(&bytes[i]) {
                    i += 1;
                }
                if i > start {
                    self.s.print_ascii(&bytes[start..i]);
                    continue;
                }
                // Text with non-ASCII characters: decode and write runs directly.
                if bytes[i] >= 0xC2 && self.text_fast {
                    let used = self.s.print_text(&bytes[i..]);
                    if used > 0 {
                        i += used;
                        continue;
                    }
                }
                // Plain SGR (`ESC [ n ; n m`, digits and ';' only): parse inline, skipping the general CSI machinery.
                if bytes[i] == 0x1b && i + 2 < n && bytes[i + 1] == b'[' {
                    let mut vals = [0u16; 16];
                    let (mut count, mut cur, mut have, mut j) = (0usize, 0u32, false, i + 2);
                    let mut ok = false;
                    while j < n && j < i + 40 {
                        match bytes[j] {
                            b'0'..=b'9' => {
                                cur = cur * 10 + (bytes[j] - b'0') as u32;
                                have = true;
                                if cur > 65535 { break; }
                            }
                            b';' => {
                                if count >= 16 { break; }
                                vals[count] = cur as u16;
                                count += 1;
                                cur = 0;
                                have = false;
                            }
                            b'm' => {
                                if have || count > 0 {
                                    if count >= 16 { break; }
                                    vals[count] = cur as u16;
                                    count += 1;
                                }
                                ok = true;
                                break;
                            }
                            _ => break,
                        }
                        j += 1;
                    }
                    if ok {
                        self.s.fast_sgr(&vals[..count]);
                        i = j + 1;
                        continue;
                    }
                }
                // Common C0 controls behave identically in ground state: skip the state machine.
                if matches!(bytes[i], 0x07 | 0x08 | 0x09 | 0x0a | 0x0b | 0x0c | 0x0d) {
                    vte::Perform::execute(&mut self.s, bytes[i]);
                    i += 1;
                    continue;
                }
                // Not printable ASCII (control, ESC, UTF-8 lead...). Hand the bytes up to the next printable
                // run of length >= 8 to the parser in one call; the parser may be mid-sequence afterwards, in
                // which case the loop below feeds it byte by byte until it is back in ground state.
                let mut j = i + 1;
                'scan: while j < n {
                    if (0x20..0x7f).contains(&bytes[j]) {
                        let end = (j + 8).min(n);
                        if end - j == 8 && bytes[j..end].iter().all(|b| (0x20..0x7f).contains(b)) {
                            break 'scan;
                        }
                    }
                    j += 1;
                }
                self.parser.advance(&mut self.s, &bytes[i..j]);
                i = j;
            } else {
                // Inside an escape sequence / partial UTF-8: feed up to and including the next byte that can end
                // one (final bytes 0x40-0x7E, BEL, CAN, SUB, ESC, ST), then re-check for ground state. Chunking never
                // changes what the parser sees; it only decides where we look for the fast path again.
                let mut k = i;
                while k < n && !matches!(bytes[k], 0x40..=0x7e | 0x07 | 0x18 | 0x1a | 0x1b | 0x9c) {
                    k += 1;
                }
                let end = (k + 1).min(n);
                self.parser.advance(&mut self.s, &bytes[i..end]);
                i = end;
            }
        }
    }

    pub fn take_responses(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.s.responses)
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.s.resize(cols, rows);
    }

    pub fn cols(&self) -> usize {
        self.s.cols
    }
    pub fn rows(&self) -> usize {
        self.s.rows
    }
    pub fn title(&self) -> &str {
        &self.s.title
    }
    pub fn cwd(&self) -> &str {
        &self.s.cwd
    }
    pub fn take_title_changed(&mut self) -> bool {
        std::mem::take(&mut self.s.title_dirty)
    }
    /// Terminal-reported progress (OSC 9;4): `(state, percent)`, state 0 = none, 1 = normal, 2 = error, 3 = indeterminate, 4 = paused.
    pub fn progress(&self) -> (u8, u8) {
        self.s.progress
    }
    pub fn take_progress_changed(&mut self) -> bool {
        std::mem::take(&mut self.s.progress_dirty)
    }
    pub fn bell_count(&self) -> u64 {
        self.s.bell_count
    }
    pub fn modes(&self) -> u32 {
        self.s.modes
    }
    pub fn is_alt_screen(&self) -> bool {
        self.s.alt
    }
    pub fn cursor(&self) -> (usize, usize) {
        (self.s.crow, self.s.ccol)
    }
    pub fn set_scrollback_limit(&mut self, n: usize) {
        let g = &mut self.s.grids[0];
        g.max_sb = n;
        while g.lines.len() > g.rows + n {
            g.lines.pop_front();
            self.s.evicted += 1;
        }
    }

    fn block_info(&self, b: &Block) -> BlockInfo {
        let ev = self.s.evicted;
        let len = self.s.grids[0].lines.len() as u64;
        let row = |a: u64| -> i32 { a.saturating_sub(ev).min(len) as i32 };
        BlockInfo {
            prompt_row: b.prompt.saturating_sub(ev) as u32,
            output_row: b.output.map_or(-1, row),
            end_row: b.end.map_or(-1, row),
            exit: b.exit.unwrap_or(i32::MIN),
            dur_ms: b.dur_ms.map_or(-1, |d| d.min(i32::MAX as u32) as i32),
            running: (b.output.is_some() && b.end.is_none()) as u32,
        }
    }

    /// Shell command blocks (OSC 133) overlapping the line indexes `[r0, r1)` (`text_range` / `top_abs`
    /// coordinates), oldest first. Blocks whose prompt has scrolled out of the buffer are omitted.
    pub fn blocks_in(&self, r0: usize, r1: usize) -> Vec<BlockInfo> {
        let ev = self.s.evicted;
        let len = self.s.grids[0].lines.len() as u64;
        let blocks = &self.s.blocks;
        let hi = blocks.partition_point(|b| b.prompt < ev.saturating_add(r1 as u64));
        let mut out = Vec::new();
        for b in blocks.iter().take(hi).rev() {
            if b.prompt < ev || b.prompt - ev >= len {
                break;
            }
            let info = self.block_info(b);
            let extent_end = if info.end_row >= 0 {
                info.end_row as usize
            } else if info.output_row >= 0 {
                len as usize
            } else {
                info.prompt_row as usize + 1
            }
            .max(info.prompt_row as usize + 1);
            if extent_end <= r0 {
                break;
            }
            out.push(info);
        }
        out.reverse();
        out
    }

    pub fn has_finished(&self) -> bool {
        !self.s.finished.is_empty()
    }

    /// Commands that finished since the last call.
    pub fn take_finished(&mut self) -> Vec<FinishedInfo> {
        let ev = self.s.evicted;
        std::mem::take(&mut self.s.finished)
            .into_iter()
            .map(|(prompt, output, exit, dur)| FinishedInfo {
                exit: exit.unwrap_or(i32::MIN),
                dur_ms: dur.min(i32::MAX as u32) as i32,
                cmd_row: if output > prompt && output - 1 >= ev { (output - 1 - ev) as i32 } else if prompt >= ev { (prompt - ev) as i32 } else { -1 },
                _pad: 0,
            })
            .collect()
    }

    pub fn blocks(&self) -> Vec<BlockInfo> {
        self.blocks_in(0, usize::MAX >> 1)
    }

    /// Nearest prompt line strictly above (`dir < 0`) or below (`dir > 0`) `from`, as a line index.
    pub fn prompt_nav(&self, from: usize, dir: i32) -> Option<usize> {
        let ev = self.s.evicted;
        let len = self.s.grids[0].lines.len() as u64;
        let ok = |b: &&Block| b.prompt >= ev && b.prompt - ev < len;
        let abs_from = ev + from as u64;
        if dir < 0 {
            self.s.blocks.iter().rev().filter(ok).find(|b| b.prompt < abs_from).map(|b| (b.prompt - ev) as usize)
        } else {
            self.s.blocks.iter().filter(ok).find(|b| b.prompt > abs_from).map(|b| (b.prompt - ev) as usize)
        }
    }

    /// The most recent block that has started producing output (`finished_only`: that has also ended).
    pub fn last_block(&self, finished_only: bool) -> Option<BlockInfo> {
        let ev = self.s.evicted;
        self.s
            .blocks
            .iter()
            .rev()
            .find(|b| b.prompt >= ev && b.output.is_some() && (!finished_only || b.end.is_some()))
            .map(|b| self.block_info(b))
    }

    // ------------------------------------------------------------- access (viewport coordinates)

    pub fn cell(&self, row: usize, col: usize) -> Cell {
        let g = &self.s.grids[self.s.alt as usize];
        g.lines[g.base() + row].cells[col]
    }

    /// Text of a viewport row with trailing blanks removed.
    pub fn row_text(&self, row: usize) -> String {
        let g = &self.s.grids[self.s.alt as usize];
        self.line_text(&g.lines[g.base() + row], 0, usize::MAX)
    }

    pub fn screen_text(&self) -> String {
        (0..self.rows()).map(|r| self.row_text(r)).collect::<Vec<_>>().join("\n")
    }

    fn line_text(&self, line: &Line, from: usize, to_incl: usize) -> String {
        self.line_text_opt(line, from, to_incl, true)
    }

    /// `trim`: drop trailing blanks. Soft-wrapped lines keep them (a space at the wrap point is real text).
    fn line_text_opt(&self, line: &Line, from: usize, to_incl: usize, trim: bool) -> String {
        let mut last = line.cells.len();
        while trim && last > 0 && line.cells[last - 1].is_blank() {
            last -= 1;
        }
        let end = last.min(to_incl.saturating_add(1));
        let mut out = String::new();
        for c in line.cells.iter().take(end).skip(from) {
            if c.flags & flags::WIDE_CONT == 0 {
                out.push_str(&self.s.cell_text(c.ch));
            }
        }
        out
    }

    pub fn cluster(&self, id: u32) -> String {
        self.s.cell_text(id)
    }
    pub fn link(&self, id: u16) -> &str {
        self.s.links.get(id as usize).map(|s| s.as_str()).unwrap_or("")
    }

    // ------------------------------------------------------------- scrollback / display

    pub fn scrollback_len(&self) -> usize {
        let g = &self.s.grids[self.s.alt as usize];
        g.lines.len() - g.rows
    }

    pub fn display_offset(&self) -> usize {
        self.s.offset
    }

    pub fn set_display_offset(&mut self, off: usize) {
        let o = off.min(self.scrollback_len());
        if o != self.s.offset {
            self.s.offset = o;
            self.s.mark_all();
        }
    }

    pub fn scroll_display(&mut self, delta: isize) {
        let cur = self.s.offset as isize;
        self.set_display_offset((cur + delta).max(0) as usize);
    }

    /// Absolute line index of the first viewport row at display offset 0.
    pub fn bottom_base(&self) -> usize {
        let g = &self.s.grids[self.s.alt as usize];
        g.base()
    }

    pub fn total_lines(&self) -> usize {
        self.s.grids[self.s.alt as usize].lines.len()
    }

    /// The command line of the block whose prompt is on line `prompt_row`, if the shell reported it.
    pub fn block_command(&self, prompt_row: usize) -> Option<String> {
        let want = self.s.evicted + prompt_row as u64;
        self.s.blocks.iter().find(|b| b.prompt == want).and_then(|b| b.cmd.clone())
    }

    /// Copy the displayed rows into `cells` (rows*cols) and per-row dirty flags into `dirty`.
    /// Only dirty rows are rewritten; the caller keeps the buffers between frames.
    pub fn copy_view(&mut self, expect_cols: usize, expect_rows: usize, cells: &mut [Cell], dirty: &mut [u8]) -> ViewInfo {
        let s = &mut self.s;
        let g = &s.grids[s.alt as usize];
        let mut info = ViewInfo {
            cols: s.cols as u32,
            rows: s.rows as u32,
            cursor_row: (s.crow + s.offset) as u32,
            cursor_col: s.ccol as u32,
            cursor_style: s.cursor_style as u32,
            cursor_visible: (s.has_mode(mode::CURSOR_VISIBLE) && s.crow + s.offset < s.rows) as u32,
            display_offset: s.offset as u32,
            scrollback_len: (g.lines.len() - g.rows) as u32,
            top_abs: (g.base() - s.offset) as u32,
            modes: s.modes,
            needs_realloc: 0,
            _pad: 0,
            evicted: s.evicted,
            bell_count: s.bell_count,
        };
        if expect_cols != s.cols || expect_rows != s.rows || cells.len() < s.cols * s.rows || dirty.len() < s.rows {
            info.needs_realloc = 1;
            return info;
        }
        let top = g.base() - s.offset;
        for r in 0..s.rows {
            let d = s.all_dirty || s.dirty[r];
            dirty[r] = d as u8;
            if d {
                let line = &g.lines[top + r];
                // History lines are stored trimmed; pad the remainder with blanks.
                let n = line.cells.len().min(s.cols);
                let dst = &mut cells[r * s.cols..(r + 1) * s.cols];
                dst[..n].copy_from_slice(&line.cells[..n]);
                for c in &mut dst[n..] {
                    *c = Cell::BLANK;
                }
            }
        }
        s.all_dirty = false;
        s.dirty.iter_mut().for_each(|d| *d = false);
        info
    }

    // ------------------------------------------------------------- text extraction & search

    /// Text between two absolute positions (inclusive), as a stream selection.
    pub fn text_range(&self, r0: usize, c0: usize, r1: usize, c1: usize) -> String {
        let g = &self.s.grids[self.s.alt as usize];
        let ((r0, c0), (r1, c1)) = if (r0, c0) <= (r1, c1) { ((r0, c0), (r1, c1)) } else { ((r1, c1), (r0, c0)) };
        let mut out = String::new();
        for r in r0..=r1.min(g.lines.len().saturating_sub(1)) {
            let line = &g.lines[r];
            let from = if r == r0 { c0 } else { 0 };
            let to = if r == r1 { c1 } else { usize::MAX };
            if r != r1 && line.wrapped {
                out.push_str(&self.line_text_opt(line, from, to, false));
                // History stores lines without trailing blanks; restore the ones a soft wrap implies.
                let have: usize = line.cells.iter().skip(from).filter(|c| c.flags & flags::WIDE_CONT == 0).count();
                let want = self.s.cols.saturating_sub(from);
                for _ in have..want {
                    out.push(' ');
                }
            } else {
                out.push_str(&self.line_text(line, from, to));
                if r != r1 {
                    out.push('\n');
                }
            }
        }
        out
    }

    pub fn search(&self, query: &str, case_insensitive: bool, limit: usize) -> Vec<Match> {
        let mut res = Vec::new();
        if query.is_empty() {
            return res;
        }
        let q = if case_insensitive { query.to_lowercase() } else { query.to_string() };
        let g = &self.s.grids[self.s.alt as usize];
        let mut text = String::with_capacity(256);
        let mut map: Vec<usize> = Vec::with_capacity(256); // byte offset -> column
        for (ri, line) in g.lines.iter().enumerate() {
            text.clear();
            map.clear();
            for (ci, c) in line.cells.iter().enumerate() {
                if c.flags & flags::WIDE_CONT != 0 {
                    continue;
                }
                let before = text.len();
                if c.ch & CLUSTER_FLAG != 0 {
                    let t = self.s.cell_text(c.ch);
                    if case_insensitive { text.extend(t.chars().flat_map(|x| x.to_lowercase())) } else { text.push_str(&t) }
                } else if let Some(ch) = char::from_u32(c.ch) {
                    if case_insensitive { text.extend(ch.to_lowercase()) } else { text.push(ch) }
                } else {
                    text.push('\u{FFFD}');
                }
                for _ in before..text.len() {
                    map.push(ci);
                }
            }
            let mut start = 0;
            while let Some(pos) = text[start..].find(&q) {
                let s0 = start + pos;
                let e = s0 + q.len();
                let last_col = map[e - 1];
                let wide = line.cells[last_col].flags & flags::WIDE != 0;
                res.push(Match { row: ri, col_start: map[s0], col_end: last_col + wide as usize });
                if res.len() >= limit {
                    return res;
                }
                start = e;
            }
        }
        res
    }

    pub fn snapshot_vt(&self, max_history: usize) -> Vec<u8> {
        self.s.snapshot_vt(max_history)
    }

}

/// Terminal cell width of a string (used by the GUI for sizing helpers and in tests).
pub fn str_width(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random numbers (xorshift), so a failure reproduces.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    fn same(a: &Terminal, b: &Terminal, what: &str) {
        assert_eq!(a.cursor(), b.cursor(), "cursor, {}", what);
        assert_eq!(a.scrollback_len(), b.scrollback_len(), "scrollback, {}", what);
        assert_eq!(a.snapshot_vt(100_000), b.snapshot_vt(100_000), "snapshot, {}", what);
        // Cluster ids depend on the order clusters were first seen, so compare what they spell.
        let norm = |t: &Terminal, r: usize, c: usize| {
            let mut cell = t.cell(r, c);
            let text = if cell.ch & CLUSTER_FLAG != 0 { Some(t.cluster(cell.ch)) } else { None };
            cell.ch = if text.is_some() { CLUSTER_FLAG } else { cell.ch };
            (cell, text)
        };
        for r in 0..a.rows() {
            for c in 0..a.cols() {
                assert_eq!(norm(a, r, c), norm(b, r, c), "cell {},{}, {}", r, c, what);
            }
        }
    }

    #[test]
    fn direct_width_ranges_match_the_unicode_tables() {
        let mut bad = Vec::new();
        for u in 0u32..0x11_0000 {
            if let Some(c) = char::from_u32(u) {
                if screen::char_width(c) != c.width() {
                    bad.push(format!("U+{:04X}", u));
                }
            }
        }
        assert!(bad.is_empty(), "direct ranges disagree with the tables for: {:?}", bad);
    }

    #[test]
    fn width_table_matches_the_unicode_tables() {
        let (table, limit, none) = screen::width_table_for_test();
        assert_eq!(table.len(), limit);
        for u in 0..limit as u32 {
            let want = char::from_u32(u).and_then(|c| c.width()).map(|w| w as u8).unwrap_or(none);
            assert_eq!(table[u as usize], want, "U+{:04X}", u);
        }
    }

    #[test]
    fn utf8_decoder_agrees_with_std() {
        for u in 0x80u32..0x11_0000 {
            if let Some(c) = char::from_u32(u) {
                let mut b = [0u8; 4];
                let e = c.encode_utf8(&mut b).as_bytes().to_vec();
                assert_eq!(screen::decode_utf8_for_test(&e), Some((c, e.len())), "U+{:04X}", u);
                if e.len() > 1 {
                    assert_eq!(screen::decode_utf8_for_test(&e[..e.len() - 1]), None, "truncated U+{:04X}", u);
                }
            }
        }
        for bad in [&[0xC0u8, 0x80][..], &[0xE0, 0x80, 0x80], &[0xED, 0xA0, 0x80], &[0xF4, 0x90, 0x80, 0x80], &[0xFF], &[0x80], &[0xC3, 0x28]] {
            assert_eq!(screen::decode_utf8_for_test(bad), None, "{:?}", bad);
        }
    }

    /// The fast text path must leave exactly the state the VT parser path would: random text mixing ASCII, wide and
    /// combining characters, emoji sequences, controls, SGR, invalid bytes, in narrow windows and split into random
    /// chunks.
    #[test]
    fn text_fast_path_matches_the_parser() {
        let pieces: Vec<&[u8]> = vec![
            b"hello", b" ", b"world ", b"x", b"\r\n", b"\r", b"\n", b"\t", b"\x08", b"\x1b[31m", b"\x1b[0m", b"\x1b[1;4m",
            "日本語".as_bytes(), "한국어".as_bytes(), "ひらがな".as_bytes(), "é".as_bytes(), "naïve".as_bytes(),
            "e\u{301}".as_bytes(), "😀".as_bytes(), "🚀🔥".as_bytes(), "👨\u{200d}👩\u{200d}👧".as_bytes(), "❤\u{fe0f}".as_bytes(),
            "🇯🇵".as_bytes(), "\u{200d}".as_bytes(), "\u{fe0f}".as_bytes(), "✓→═│".as_bytes(), "\u{85}".as_bytes(),
            b"\xff", b"\xc3", b"\xe6\x97", b"\x1b[2K", b"\x1b[3G", b"\x1b[?7l", b"\x1b[?7h", b"\x1b[4h", b"\x1b[4l", b"\x1b(0", b"\x1b(B",
            b"\x1b[10;5H", b"\x1b[2J", b"\x1b[3b",
        ];
        for seed in 1..=400u64 {
            let mut rng = Rng(seed * 0x9E37_79B9_7F4A_7C15 | 1);
            let cols = 4 + rng.below(30);
            let rows = 2 + rng.below(8);
            let mut fast = Terminal::new(cols, rows, 200);
            let mut reference = Terminal::new(cols, rows, 200);
            reference.text_fast = false; // the same feed, without the direct text path
            for _ in 0..40 {
                let mut data = Vec::new();
                for _ in 0..1 + rng.below(25) {
                    data.extend_from_slice(pieces[rng.below(pieces.len())]);
                }
                // Same bytes, split at random places, so partial sequences cross reads.
                let mut at = 0;
                while at < data.len() {
                    let n = 1 + rng.below(9);
                    let end = (at + n).min(data.len());
                    fast.feed(&data[at..end]);
                    reference.feed(&data[at..end]);
                    same(&fast, &reference, &format!("seed {}, {}x{}, chunk {:?} of {:?}", seed, cols, rows, String::from_utf8_lossy(&data[at..end]), String::from_utf8_lossy(&data)));
                    at = end;
                }
                same(&fast, &reference, &format!("seed {}, {}x{}, last data {:?}", seed, cols, rows, String::from_utf8_lossy(&data)));
            }
        }
    }
}
