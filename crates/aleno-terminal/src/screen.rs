//! Terminal screen state and the VT `Perform` implementation.

use crate::cell::*;
use std::collections::{HashMap, VecDeque};
use unicode_width::UnicodeWidthChar;
use vte::{Params, Perform};

pub mod mode {
    pub const APP_CURSOR: u32 = 1 << 0;
    pub const APP_KEYPAD: u32 = 1 << 1;
    pub const BRACKETED_PASTE: u32 = 1 << 2;
    pub const MOUSE_PRESS: u32 = 1 << 3; // 1000
    pub const MOUSE_DRAG: u32 = 1 << 4; // 1002
    pub const MOUSE_MOTION: u32 = 1 << 5; // 1003
    pub const MOUSE_SGR: u32 = 1 << 6; // 1006
    pub const FOCUS_EVENTS: u32 = 1 << 7; // 1004
    pub const ALT_SCREEN: u32 = 1 << 8;
    pub const CURSOR_VISIBLE: u32 = 1 << 9;
    pub const AUTOWRAP: u32 = 1 << 10;
    pub const ALT_SCROLL: u32 = 1 << 11; // 1007
    pub const SYNC_OUTPUT: u32 = 1 << 12; // 2026
    pub const ORIGIN: u32 = 1 << 13;
}

/// One shell command as reported by OSC 133 (prompt start, output start, command end). Lines are absolute
/// ids (`evicted + index` into the primary grid) so they survive scrolling.
#[derive(Clone, Debug)]
pub struct Block {
    pub prompt: u64,
    pub output: Option<u64>,
    /// First line after the output (exclusive).
    pub end: Option<u64>,
    pub exit: Option<i32>,
    pub dur_ms: Option<u32>,
    started: Option<std::time::Instant>,
    /// The command line as typed (from the shell integration), when known.
    pub cmd: Option<String>,
}

const MAX_BLOCKS: usize = 20_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Charset {
    Ascii,
    Graphics,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Pen {
    pub fg: u32,
    pub bg: u32,
    pub flags: u16,
}

impl Pen {
    const DEFAULT: Pen = Pen { fg: COLOR_DEFAULT, bg: COLOR_DEFAULT, flags: 0 };
}

#[derive(Clone, Copy)]
struct SavedCursor {
    row: usize,
    col: usize,
    pen: Pen,
    origin: bool,
    pending_wrap: bool,
    g0: Charset,
    g1: Charset,
    gl: u8,
}

pub(crate) struct Grid {
    pub lines: VecDeque<Line>,
    pub cols: usize,
    pub rows: usize,
    pub max_sb: usize,
    /// Full-width row buffers recycled when a line moves into (trimmed) history.
    pool: Vec<(Vec<Cell>, usize)>,
    /// Small buffers from evicted history lines, reused for the next compacted line (no malloc/free per scrolled line).
    short_pool: Vec<Vec<Cell>>,
}

impl Grid {
    fn new(cols: usize, rows: usize, max_sb: usize) -> Grid {
        let mut lines = VecDeque::with_capacity(rows + 64);
        for _ in 0..rows {
            lines.push_back(Line::blank(cols, Cell::BLANK));
        }
        Grid { lines, cols, rows, max_sb, pool: Vec::new(), short_pool: Vec::new() }
    }
    #[inline]
    pub fn base(&self) -> usize {
        self.lines.len() - self.rows
    }
    /// A blank full-width line, reusing a pooled buffer when available.
    fn fresh_line(&mut self, fill: Cell) -> Line {
        let cols = self.cols;
        if let Some((mut v, dirty)) = self.pool.pop() {
            if fill == Cell::BLANK && v.len() == cols && dirty <= cols {
                // Everything past `dirty` is still exactly BLANK (it was trimmed as such): refill only the prefix.
                v[..dirty].fill(Cell::BLANK);
            } else {
                v.clear();
                v.resize(cols, fill);
            }
            Line { cells: v, wrapped: false }
        } else {
            Line::blank(cols, fill)
        }
    }

    /// Called when the viewport's top line scrolls into history: store it without trailing blanks
    /// (history lines are usually far shorter than the terminal is wide) and recycle the big buffer.
    fn compact_history_line(&mut self, idx: usize) {
        let line = &mut self.lines[idx];
        // Trim only cells that are *exactly* the default blank, so the recycled buffer's tail stays pristine.
        let mut used = line.cells.len();
        while used > 0 && line.cells[used - 1].is_exactly_blank() {
            used -= 1;
        }
        if used == line.cells.len() {
            return;
        }
        let mut short = self.short_pool.pop().unwrap_or_default();
        short.clear();
        short.extend_from_slice(&line.cells[..used]);
        let big = std::mem::replace(&mut line.cells, short);
        if self.pool.len() < 64 {
            self.pool.push((big, used));
        }
    }

    #[inline]
    fn row_mut(&mut self, r: usize) -> &mut Line {
        let b = self.base();
        &mut self.lines[b + r]
    }
}

/// Break a wide character that straddles `col` of `line` so no orphan half remains.
#[inline]
fn fix_wide_line(line: &mut Line, cols: usize, col: usize) {
    let f = line.cells[col].flags;
    if f & flags::WIDE != 0 && col + 1 < cols {
        line.cells[col + 1].flags &= !flags::WIDE_CONT;
        line.cells[col + 1].ch = b' ' as u32;
    }
    if f & flags::WIDE_CONT != 0 && col > 0 {
        line.cells[col - 1].flags &= !flags::WIDE;
        line.cells[col - 1].ch = b' ' as u32;
    }
}

/// Display width of `c`: the common ranges answered directly, everything else by the Unicode tables. `None` for
/// controls (C0, DEL, C1). A test checks the direct ranges against the tables.
#[inline]
pub(crate) fn char_width(c: char) -> Option<usize> {
    match c as u32 {
        0x20..=0x7e | 0xa0..=0xac | 0xae..=0x2ff => Some(1), // 0xad (soft hyphen) is zero width
        0x4e00..=0x9fff | 0xac00..=0xd7a3 | 0x3041..=0x3096 | 0x30a1..=0x30fa => Some(2),
        _ => c.width(),
    }
}

/// Characters below this have their width in `width_table`; the rest ask the Unicode tables.
const TABLE_LIMIT: usize = 0x2_0000;
/// One byte per character below `TABLE_LIMIT` (the BMP and the emoji plane): 0, 1 or 2 columns, `NO_WIDTH` for
/// controls. Built once from the Unicode tables, so one indexed load replaces a table walk per character.
const NO_WIDTH: u8 = 255;
fn width_table() -> &'static [u8] {
    static TABLE: std::sync::OnceLock<Box<[u8]>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        (0..TABLE_LIMIT as u32)
            .map(|u| match char::from_u32(u).and_then(|c| c.width()) {
                Some(w) => w as u8,
                None => NO_WIDTH,
            })
            .collect()
    })
}

/// Decode one UTF-8 sequence at the start of `s` (a non-ASCII lead byte). `None` for anything invalid, overlong,
/// a surrogate, or cut off by the end of `s`: the caller leaves those to the VT parser, which handles them (and
/// sequences split across reads) itself.
#[inline(always)]
fn decode_utf8(s: &[u8]) -> Option<(char, usize)> {
    let b0 = s[0] as u32;
    match s[0] {
        0xC2..=0xDF => {
            let b1 = *s.get(1)? as u32;
            if b1 & 0xC0 != 0x80 {
                return None;
            }
            Some((char::from_u32(((b0 & 0x1F) << 6) | (b1 & 0x3F))?, 2))
        }
        0xE0..=0xEF => {
            let (b1, b2) = (*s.get(1)? as u32, *s.get(2)? as u32);
            if (b1 | b2) & 0xC0 != 0x80 || (b1 & 0xC0) != 0x80 || (b2 & 0xC0) != 0x80 {
                return None;
            }
            let u = ((b0 & 0x0F) << 12) | ((b1 & 0x3F) << 6) | (b2 & 0x3F);
            if u < 0x800 {
                return None; // overlong
            }
            Some((char::from_u32(u)?, 3)) // from_u32 rejects surrogates
        }
        0xF0..=0xF4 => {
            let (b1, b2, b3) = (*s.get(1)? as u32, *s.get(2)? as u32, *s.get(3)? as u32);
            if (b1 & 0xC0) != 0x80 || (b2 & 0xC0) != 0x80 || (b3 & 0xC0) != 0x80 {
                return None;
            }
            let u = ((b0 & 0x07) << 18) | ((b1 & 0x3F) << 12) | ((b2 & 0x3F) << 6) | (b3 & 0x3F);
            if u < 0x1_0000 {
                return None; // overlong
            }
            Some((char::from_u32(u)?, 4)) // from_u32 rejects anything above U+10FFFF
        }
        _ => None,
    }
}

#[cfg(test)]
pub(crate) fn width_table_for_test() -> (&'static [u8], usize, u8) {
    (width_table(), TABLE_LIMIT, NO_WIDTH)
}

#[cfg(test)]
pub(crate) fn decode_utf8_for_test(s: &[u8]) -> Option<(char, usize)> {
    decode_utf8(s)
}

/// Maximum interned grapheme clusters / hyperlinks; beyond this, extra marks are dropped.
const MAX_CLUSTERS: usize = 1 << 20;
const MAX_LINKS: usize = 60000;
const MAX_TITLE: usize = 1024;
const MAX_URI: usize = 4096;

pub struct Screen {
    pub(crate) grids: [Grid; 2],
    pub(crate) alt: bool,
    pub(crate) cols: usize,
    pub(crate) rows: usize,
    // cursor
    pub(crate) crow: usize,
    pub(crate) ccol: usize,
    pub(crate) pending_wrap: bool,
    pub(crate) pen: Pen,
    pub(crate) cursor_style: u8,
    saved: Option<SavedCursor>,
    saved_primary_for_1049: Option<SavedCursor>,
    // regions & modes
    pub(crate) top: usize,
    pub(crate) bottom: usize,
    pub(crate) modes: u32,
    insert_mode: bool,
    tabs: Vec<bool>,
    g0: Charset,
    g1: Charset,
    gl: u8,
    // text
    pub(crate) clusters: Vec<String>,
    cluster_map: HashMap<String, u32>,
    pub(crate) links: Vec<String>,
    link_map: HashMap<String, u16>,
    cur_link: u16,
    last_zwj: bool,
    last_char: Option<char>,
    // view state
    pub(crate) offset: usize,
    pub(crate) dirty: Vec<bool>,
    pub(crate) all_dirty: bool,
    pub(crate) evicted: u64,
    pub(crate) blocks: VecDeque<Block>,
    /// Commands that finished live (not replayed from a snapshot): (prompt, output start, exit, duration ms).
    pub(crate) finished: Vec<(u64, u64, Option<i32>, u32)>,
    // host-visible events
    pub(crate) title: String,
    pub(crate) title_dirty: bool,
    /// OSC 9;4 progress: (state, percent). State 0 = none, 1 = normal, 2 = error, 3 = indeterminate, 4 = paused.
    pub(crate) progress: (u8, u8),
    pub(crate) progress_dirty: bool,
    pub(crate) cwd: String,
    pub(crate) bell_count: u64,
    pub(crate) respond: bool,
    pub(crate) responses: Vec<u8>,
}

impl Screen {
    pub fn new(cols: usize, rows: usize, scrollback: usize) -> Screen {
        let cols = cols.clamp(1, 1000);
        let rows = rows.clamp(1, 1000);
        let mut s = Screen {
            grids: [Grid::new(cols, rows, scrollback), Grid::new(cols, rows, 0)],
            alt: false,
            cols,
            rows,
            crow: 0,
            ccol: 0,
            pending_wrap: false,
            pen: Pen::DEFAULT,
            cursor_style: 0,
            saved: None,
            saved_primary_for_1049: None,
            top: 0,
            bottom: rows - 1,
            modes: mode::CURSOR_VISIBLE | mode::AUTOWRAP,
            insert_mode: false,
            tabs: vec![],
            g0: Charset::Ascii,
            g1: Charset::Ascii,
            gl: 0,
            clusters: Vec::new(),
            cluster_map: HashMap::new(),
            links: vec![String::new()],
            link_map: HashMap::new(),
            cur_link: 0,
            last_zwj: false,
            last_char: None,
            offset: 0,
            dirty: vec![true; rows],
            all_dirty: true,
            evicted: 0,
            blocks: VecDeque::new(),
            finished: Vec::new(),
            title: String::new(),
            title_dirty: false,
            progress: (0, 0),
            progress_dirty: false,
            cwd: String::new(),
            bell_count: 0,
            respond: true,
            responses: Vec::new(),
        };
        s.reset_tabs();
        s
    }

    fn reset_tabs(&mut self) {
        self.tabs = (0..self.cols).map(|c| c % 8 == 0 && c != 0).collect();
    }

    #[inline]
    fn g(&mut self) -> &mut Grid {
        &mut self.grids[self.alt as usize]
    }

    #[inline]
    fn blank(&self) -> Cell {
        Cell { ch: b' ' as u32, fg: COLOR_DEFAULT, bg: self.pen.bg, flags: 0, link: 0 }
    }

    #[inline]
    fn mark(&mut self, row: usize) {
        if row < self.dirty.len() {
            self.dirty[row] = true;
        }
    }
    fn mark_range(&mut self, a: usize, b: usize) {
        for r in a..=b.min(self.rows.saturating_sub(1)) {
            self.dirty[r] = true;
        }
    }
    pub(crate) fn mark_all(&mut self) {
        self.all_dirty = true;
    }

    fn respond(&mut self, bytes: &[u8]) {
        if self.respond && self.responses.len() < 1 << 20 {
            self.responses.extend_from_slice(bytes);
        }
    }

    pub fn has_mode(&self, m: u32) -> bool {
        self.modes & m != 0
    }

    // ---------------------------------------------------------------- scrolling

    fn scroll_up_region(&mut self, top: usize, bottom: usize, n: usize) {
        let n = n.min(bottom + 1 - top);
        if n == 0 {
            return;
        }
        let blank = self.blank();
        let rows = self.rows;
        let alt = self.alt;
        let full = top == 0 && bottom == rows - 1;
        let g = &mut self.grids[alt as usize];
        if full {
            for _ in 0..n {
                let line = g.fresh_line(blank);
                g.lines.push_back(line);
                if g.max_sb > 0 {
                    let idx = g.lines.len() - g.rows - 1;
                    g.compact_history_line(idx);
                }
                if g.lines.len() > g.rows + g.max_sb {
                    if let Some(old) = g.lines.pop_front() {
                        if g.short_pool.len() < 64 {
                            g.short_pool.push(old.cells);
                        }
                    }
                    if !alt {
                        self.evicted += 1;
                    }
                }
            }
            if self.offset > 0 {
                let sb = g.lines.len() - g.rows;
                self.offset = (self.offset + n).min(sb);
            }
            // Every row changed: one flag instead of marking each row dirty.
            self.all_dirty = true;
            return;
        } else {
            let b = g.base();
            for _ in 0..n {
                // Recycle the removed line's buffer instead of allocating a new one.
                let mut l = g.lines.remove(b + top).unwrap();
                l.cells.fill(blank);
                l.wrapped = false;
                g.lines.insert(b + bottom, l);
            }
        }
        self.mark_range(top, bottom);
    }

    fn scroll_down_region(&mut self, top: usize, bottom: usize, n: usize) {
        let n = n.min(bottom + 1 - top);
        if n == 0 {
            return;
        }
        let blank = self.blank();
        let g = self.g();
        let b = g.base();
        for _ in 0..n {
            let mut l = g.lines.remove(b + bottom).unwrap();
            l.cells.fill(blank);
            l.wrapped = false;
            g.lines.insert(b + top, l);
        }
        self.mark_range(top, bottom);
    }

    fn linefeed(&mut self) {
        if self.crow == self.bottom {
            self.scroll_up_region(self.top, self.bottom, 1);
        } else if self.crow + 1 < self.rows {
            self.crow += 1;
        }
    }

    fn reverse_index(&mut self) {
        if self.crow == self.top {
            self.scroll_down_region(self.top, self.bottom, 1);
        } else if self.crow > 0 {
            self.crow -= 1;
        }
    }

    // ---------------------------------------------------------------- printing

    fn intern_cluster(&mut self, s: String) -> Option<u32> {
        if let Some(&i) = self.cluster_map.get(&s) {
            return Some(CLUSTER_FLAG | i);
        }
        if self.clusters.len() >= MAX_CLUSTERS {
            return None;
        }
        let i = self.clusters.len() as u32;
        self.clusters.push(s.clone());
        self.cluster_map.insert(s, i);
        Some(CLUSTER_FLAG | i)
    }

    pub(crate) fn cell_text(&self, ch: u32) -> String {
        if ch & CLUSTER_FLAG != 0 {
            self.clusters.get((ch & !CLUSTER_FLAG) as usize).cloned().unwrap_or_default()
        } else {
            char::from_u32(ch).map(|c| c.to_string()).unwrap_or_else(|| "\u{FFFD}".into())
        }
    }

    fn map_charset(&self, c: char) -> char {
        let cs = if self.gl == 0 { self.g0 } else { self.g1 };
        if cs == Charset::Graphics {
            match c {
                '`' => '◆',
                'a' => '▒',
                'f' => '°',
                'g' => '±',
                'j' => '┘',
                'k' => '┐',
                'l' => '┌',
                'm' => '└',
                'n' => '┼',
                'o' => '⎺',
                'p' => '⎻',
                'q' => '─',
                'r' => '⎼',
                's' => '⎽',
                't' => '├',
                'u' => '┤',
                'v' => '┴',
                'w' => '┬',
                'x' => '│',
                'y' => '≤',
                'z' => '≥',
                '{' => 'π',
                '|' => '≠',
                '}' => '£',
                '~' => '·',
                _ => c,
            }
        } else {
            c
        }
    }

    /// Break a wide character that straddles `col` so no orphan half remains.
    fn fix_wide(&mut self, row: usize, col: usize) {
        let cols = self.cols;
        fix_wide_line(self.g().row_mut(row), cols, col);
    }

    fn put_char(&mut self, c: char) {
        let c = self.map_charset(c);
        let w = match char_width(c) {
            Some(w) => w,
            None => return,
        };
        if w == 0 {
            self.combine(c);
            return;
        }
        if self.last_zwj {
            self.last_zwj = false;
            // A joiner only joins the character after it when that is not plain ASCII (an emoji, say): a space or
            // a letter after it is its own cell, whichever code path delivered it.
            if !c.is_ascii() {
                self.combine(c);
                return;
            }
        }
        let cols = self.cols;
        if self.pending_wrap {
            if self.has_mode(mode::AUTOWRAP) {
                let r = self.crow;
                self.g().row_mut(r).wrapped = true;
                self.linefeed();
                self.ccol = 0;
            }
            self.pending_wrap = false;
        }
        if w == 2 && self.ccol + 1 >= cols {
            if cols < 2 {
                return;
            }
            if self.has_mode(mode::AUTOWRAP) {
                let (r, cc) = (self.crow, self.ccol);
                let blank = self.blank();
                self.g().row_mut(r).cells[cc] = blank;
                self.g().row_mut(r).wrapped = true;
                self.linefeed();
                self.ccol = 0;
            } else {
                self.ccol = cols - 2;
            }
        }
        let (r, cc) = (self.crow, self.ccol);
        if self.insert_mode {
            let blank = self.blank();
            let line = self.g().row_mut(r);
            for _ in 0..w {
                line.cells.pop();
                line.cells.insert(cc, blank);
            }
        }
        self.fix_wide(r, cc);
        if w == 2 {
            self.fix_wide(r, cc + 1);
        }
        let pen = self.pen;
        let link = self.cur_link;
        let line = self.g().row_mut(r);
        line.cells[cc] = Cell {
            ch: c as u32,
            fg: pen.fg,
            bg: pen.bg,
            flags: pen.flags | if w == 2 { flags::WIDE } else { 0 },
            link,
        };
        if w == 2 {
            line.cells[cc + 1] = Cell {
                ch: b' ' as u32,
                fg: pen.fg,
                bg: pen.bg,
                flags: pen.flags | flags::WIDE_CONT,
                link,
            };
        }
        self.mark(r);
        self.ccol += w;
        if self.ccol >= cols {
            self.ccol = cols - 1;
            self.pending_wrap = true;
        }
        self.last_char = Some(c);
    }

    /// Fast path for `ESC [ <digits and ;> m` (plain SGR): applies the attributes without the general CSI machinery.
    pub(crate) fn fast_sgr(&mut self, params: &[u16]) {
        self.last_zwj = false;
        let mut refs: [&[u16]; 16] = [&[]; 16];
        let mut n = 0;
        for k in 0..params.len().min(16) {
            refs[n] = &params[k..k + 1];
            n += 1;
        }
        self.sgr(&refs[..n]);
    }

    /// Fast path for a run of printable ASCII (0x20..=0x7E): writes whole row segments at once instead of
    /// one `put_char` per byte. Only valid when the parser is in ground state.
    pub(crate) fn print_ascii(&mut self, mut s: &[u8]) {
        self.last_zwj = false;
        let cur_cs = if self.gl == 0 { self.g0 } else { self.g1 };
        if cur_cs != Charset::Ascii || self.insert_mode {
            for &b in s {
                self.put_char(b as char);
            }
            return;
        }
        let cols = self.cols;
        while !s.is_empty() {
            if self.pending_wrap {
                if self.has_mode(mode::AUTOWRAP) {
                    let r = self.crow;
                    self.g().row_mut(r).wrapped = true;
                    self.linefeed();
                    self.ccol = 0;
                }
                self.pending_wrap = false;
            }
            let (r, cc) = (self.crow, self.ccol);
            let n = s.len().min(cols - cc);
            // Overwriting half of a wide character needs the slow fix-up (rare): find those cells first.
            let mut needs_fix = false;
            {
                let line = self.g().row_mut(r);
                for c in &line.cells[cc..cc + n] {
                    if c.flags & (flags::WIDE | flags::WIDE_CONT) != 0 {
                        needs_fix = true;
                        break;
                    }
                }
                if cc + n < line.cells.len() && line.cells[cc + n].flags & flags::WIDE_CONT != 0 {
                    needs_fix = true;
                }
            }
            if needs_fix {
                for k in 0..n {
                    self.fix_wide(r, cc + k);
                }
                if cc + n < cols {
                    self.fix_wide(r, cc + n - 1);
                }
            }
            let (fg, bg, fl, link) = (self.pen.fg, self.pen.bg, self.pen.flags, self.cur_link);
            let line = self.g().row_mut(r);
            for (cell, &b) in line.cells[cc..cc + n].iter_mut().zip(&s[..n]) {
                *cell = Cell { ch: b as u32, fg, bg, flags: fl, link };
            }
            self.mark(r);
            self.last_char = Some(s[n - 1] as char);
            self.ccol += n;
            if self.ccol >= cols {
                self.ccol = cols - 1;
                self.pending_wrap = true;
            }
            s = &s[n..];
        }
    }

    /// Fast path for text: a run of printable ASCII and valid UTF-8, in ground state, with the plain charset. Decodes
    /// the characters itself (skipping the VT parser's per-character work) and writes whole row segments. Anything that
    /// needs care (a character that would wrap or straddle the last column, zero-width marks, joiners, a pending wrap)
    /// goes through `put_char`, so the result is identical to feeding the parser. Returns the bytes consumed: it
    /// stops at a control byte, an escape, or anything it will not decode (invalid, or cut off by the end of `s`).
    pub(crate) fn print_text(&mut self, s: &[u8]) -> usize {
        let cur_cs = if self.gl == 0 { self.g0 } else { self.g1 };
        if cur_cs != Charset::Ascii || self.insert_mode {
            return 0;
        }
        let cols = self.cols;
        let widths = width_table();
        let mut i = 0;
        while i < s.len() {
            let r = self.crow;
            let mut cc = self.ccol;
            let (fg, bg, fl, link) = (self.pen.fg, self.pen.bg, self.pen.flags, self.cur_link);
            let mut zwj = self.last_zwj;
            let mut last = None;
            if !self.pending_wrap {
                let line = self.grids[self.alt as usize].row_mut(r);
                while i < s.len() {
                    let b = s[i];
                    let (c, len) = if b < 0x80 {
                        if !(0x20..0x7f).contains(&b) {
                            break;
                        }
                        zwj = false; // printable ASCII ends a ZWJ sequence, as in print_ascii
                        (b as char, 1)
                    } else {
                        match decode_utf8(&s[i..]) {
                            Some(x) => x,
                            None => break,
                        }
                    };
                    let w = match widths.get(c as usize).copied() {
                        Some(w @ 1..=2) => w as usize,
                        Some(_) => break, // zero width or a control: put_char decides
                        None => match c.width() {
                            Some(w @ 1..=2) => w,
                            _ => break,
                        },
                    };
                    if (zwj && b >= 0x80) || cc + w >= cols {
                        break; // joins the previous cell, or reaches the last column (wrap state): put_char
                    }
                    if line.cells[cc].flags & (flags::WIDE | flags::WIDE_CONT) != 0 {
                        fix_wide_line(line, cols, cc);
                    }
                    if w == 1 {
                        line.cells[cc] = Cell { ch: c as u32, fg, bg, flags: fl, link };
                    } else {
                        if line.cells[cc + 1].flags & (flags::WIDE | flags::WIDE_CONT) != 0 {
                            fix_wide_line(line, cols, cc + 1);
                        }
                        line.cells[cc] = Cell { ch: c as u32, fg, bg, flags: fl | flags::WIDE, link };
                        line.cells[cc + 1] = Cell { ch: b' ' as u32, fg, bg, flags: fl | flags::WIDE_CONT, link };
                    }
                    cc += w;
                    last = Some(c);
                    i += len;
                }
            }
            if let Some(c) = last {
                self.mark(r);
                self.ccol = cc;
                self.last_char = Some(c);
            }
            self.last_zwj = zwj;
            if i >= s.len() {
                break;
            }
            // The next character needs the general path (or ends the run).
            let b = s[i];
            let (c, len) = if b < 0x80 {
                if !(0x20..0x7f).contains(&b) {
                    break;
                }
                self.last_zwj = false;
                (b as char, 1)
            } else {
                match decode_utf8(&s[i..]) {
                    Some(x) => x,
                    None => break,
                }
            };
            self.put_char(c);
            i += len;
        }
        i
    }

    /// Attach a zero-width character to the previously printed cell.
    fn combine(&mut self, c: char) {
        let mut col = if self.pending_wrap { self.ccol } else if self.ccol > 0 { self.ccol - 1 } else { return };
        let r = self.crow;
        let cols = self.cols;
        {
            let line = self.g().row_mut(r);
            if line.cells[col].flags & flags::WIDE_CONT != 0 && col > 0 {
                col -= 1;
            }
        }
        let cell = self.g().row_mut(r).cells[col];
        let mut s = self.cell_text(cell.ch);
        if s == " " && cell.ch == b' ' as u32 {
            // Nothing to attach to.
            return;
        }
        s.push(c);
        if let Some(id) = self.intern_cluster(s) {
            self.g().row_mut(r).cells[col].ch = id;
            self.mark(r);
        }
        self.last_zwj = c == '\u{200D}';
        // VS16 upgrades a narrow emoji base to two cells.
        if c == '\u{FE0F}' {
            let is_narrow = cell.flags & flags::WIDE == 0;
            if is_narrow && col + 1 < cols && col + 1 == self.ccol && !self.pending_wrap {
                let pen = self.pen;
                let line = self.g().row_mut(r);
                line.cells[col].flags |= flags::WIDE;
                line.cells[col + 1] = Cell {
                    ch: b' ' as u32,
                    fg: pen.fg,
                    bg: pen.bg,
                    flags: flags::WIDE_CONT,
                    link: 0,
                };
                if col + 2 >= cols {
                    self.ccol = cols - 1;
                    self.pending_wrap = true;
                } else {
                    self.ccol = col + 2;
                }
            }
        }
    }

    // ---------------------------------------------------------------- erase / edit

    fn erase_cells(&mut self, row: usize, from: usize, to: usize) {
        // [from, to)
        let to = to.min(self.cols);
        if from >= to {
            return;
        }
        let full_row = from == 0 && to >= self.cols;
        if !full_row {
            if from < self.cols {
                self.fix_wide(row, from);
            }
            if to > 0 && to <= self.cols {
                self.fix_wide(row, to - 1);
            }
        }
        let blank = self.blank();
        let line = self.g().row_mut(row);
        for c in &mut line.cells[from..to] {
            *c = blank;
        }
        if to == self.cols {
            self.g().row_mut(row).wrapped = false;
        }
        self.mark(row);
    }

    fn erase_display(&mut self, n: usize) {
        let (r, c) = (self.crow, self.ccol);
        let cols = self.cols;
        match n {
            0 => {
                self.erase_cells(r, c, cols);
                for rr in r + 1..self.rows {
                    self.erase_cells(rr, 0, cols);
                }
            }
            1 => {
                for rr in 0..r {
                    self.erase_cells(rr, 0, cols);
                }
                self.erase_cells(r, 0, c + 1);
            }
            2 => {
                for rr in 0..self.rows {
                    self.erase_cells(rr, 0, cols);
                }
                self.drop_blocks_touching_screen();
            }
            3 => {
                let rows = self.rows;
                let alt = self.alt;
                let g = self.g();
                let mut dropped = 0u64;
                while g.lines.len() > rows {
                    g.lines.pop_front();
                    dropped += 1;
                }
                if !alt {
                    self.evicted += dropped;
                }
                self.drop_blocks_touching_screen();
                self.offset = 0;
                self.all_dirty = true;
            }
            _ => {}
        }
    }

    /// The visible screen was wiped (`clear`, Ctrl-L): command blocks that had any line on it are gone with it.
    /// Blocks that finished entirely in scrollback stay.
    fn drop_blocks_touching_screen(&mut self) {
        if self.alt {
            return;
        }
        let top = self.evicted + self.grids[0].base() as u64;
        self.blocks.retain(|b| b.end.map_or(false, |e| e < top));
        self.finished.clear();
    }

    fn erase_line(&mut self, n: usize) {
        let (r, c) = (self.crow, self.ccol);
        let cols = self.cols;
        match n {
            0 => self.erase_cells(r, c, cols),
            1 => self.erase_cells(r, 0, c + 1),
            2 => self.erase_cells(r, 0, cols),
            _ => {}
        }
    }

    fn insert_chars(&mut self, n: usize) {
        let (r, c) = (self.crow, self.ccol);
        let blank = self.blank();
        let cols = self.cols;
        self.fix_wide(r, c);
        let line = self.g().row_mut(r);
        let n = n.min(cols - c);
        for _ in 0..n {
            line.cells.pop();
            line.cells.insert(c, blank);
        }
        self.pending_wrap = false;
        self.mark(r);
    }

    fn delete_chars(&mut self, n: usize) {
        let (r, c) = (self.crow, self.ccol);
        let blank = self.blank();
        let cols = self.cols;
        self.fix_wide(r, c);
        let line = self.g().row_mut(r);
        let n = n.min(cols - c);
        for _ in 0..n {
            line.cells.remove(c);
            line.cells.push(blank);
        }
        self.pending_wrap = false;
        self.mark(r);
    }

    // ---------------------------------------------------------------- cursor helpers

    fn goto(&mut self, row: usize, col: usize) {
        let (rmin, rmax) = if self.has_mode(mode::ORIGIN) { (self.top, self.bottom) } else { (0, self.rows - 1) };
        let base = if self.has_mode(mode::ORIGIN) { self.top } else { 0 };
        self.crow = (base + row).clamp(rmin, rmax);
        self.ccol = col.min(self.cols - 1);
        self.pending_wrap = false;
    }

    fn save_cursor(&mut self) {
        self.saved = Some(SavedCursor {
            row: self.crow,
            col: self.ccol,
            pen: self.pen,
            origin: self.has_mode(mode::ORIGIN),
            pending_wrap: self.pending_wrap,
            g0: self.g0,
            g1: self.g1,
            gl: self.gl,
        });
    }

    fn restore_cursor(&mut self) {
        if let Some(s) = self.saved {
            self.apply_saved(s);
        } else {
            self.crow = 0;
            self.ccol = 0;
            self.pen = Pen::DEFAULT;
            self.pending_wrap = false;
        }
    }

    fn apply_saved(&mut self, s: SavedCursor) {
        self.crow = s.row.min(self.rows - 1);
        self.ccol = s.col.min(self.cols - 1);
        self.pen = s.pen;
        self.pending_wrap = s.pending_wrap;
        self.g0 = s.g0;
        self.g1 = s.g1;
        self.gl = s.gl;
        if s.origin {
            self.modes |= mode::ORIGIN;
        } else {
            self.modes &= !mode::ORIGIN;
        }
    }

    fn next_tab(&self, col: usize) -> usize {
        for c in col + 1..self.cols {
            if self.tabs[c] {
                return c;
            }
        }
        self.cols - 1
    }

    fn prev_tab(&self, col: usize) -> usize {
        for c in (0..col).rev() {
            if self.tabs[c] {
                return c;
            }
        }
        0
    }

    // ---------------------------------------------------------------- alt screen

    fn enter_alt(&mut self, save_cursor: bool, clear: bool) {
        if self.alt {
            return;
        }
        if save_cursor {
            self.save_cursor();
            self.saved_primary_for_1049 = self.saved;
        }
        self.alt = true;
        self.modes |= mode::ALT_SCREEN;
        self.offset = 0;
        if clear {
            let (cols, rows) = (self.cols, self.rows);
            self.grids[1] = Grid::new(cols, rows, 0);
        }
        self.all_dirty = true;
    }

    fn leave_alt(&mut self, restore_cursor: bool) {
        if !self.alt {
            return;
        }
        self.alt = false;
        self.modes &= !mode::ALT_SCREEN;
        self.all_dirty = true;
        if restore_cursor {
            if let Some(s) = self.saved_primary_for_1049.take() {
                self.apply_saved(s);
            }
        }
    }

    // ---------------------------------------------------------------- modes

    fn set_dec_mode(&mut self, m: u16, on: bool) {
        let bit = |s: &mut Screen, b: u32| {
            if on {
                s.modes |= b
            } else {
                s.modes &= !b
            }
        };
        match m {
            1 => bit(self, mode::APP_CURSOR),
            6 => {
                bit(self, mode::ORIGIN);
                self.goto(0, 0);
            }
            7 => bit(self, mode::AUTOWRAP),
            25 => bit(self, mode::CURSOR_VISIBLE),
            47 | 1047 => {
                if on {
                    self.enter_alt(false, m == 1047)
                } else {
                    self.leave_alt(false)
                }
            }
            1049 => {
                if on {
                    self.enter_alt(true, true)
                } else {
                    self.leave_alt(true)
                }
            }
            1000 => bit(self, mode::MOUSE_PRESS),
            1002 => bit(self, mode::MOUSE_DRAG),
            1003 => bit(self, mode::MOUSE_MOTION),
            1004 => bit(self, mode::FOCUS_EVENTS),
            1006 => bit(self, mode::MOUSE_SGR),
            1007 => bit(self, mode::ALT_SCROLL),
            2004 => bit(self, mode::BRACKETED_PASTE),
            2026 => bit(self, mode::SYNC_OUTPUT),
            _ => {}
        }
    }

    fn dec_mode_state(&self, m: u16) -> u8 {
        let b = |bit: u32| if self.has_mode(bit) { 1 } else { 2 };
        match m {
            1 => b(mode::APP_CURSOR),
            6 => b(mode::ORIGIN),
            7 => b(mode::AUTOWRAP),
            25 => b(mode::CURSOR_VISIBLE),
            47 | 1047 | 1049 => b(mode::ALT_SCREEN),
            1000 => b(mode::MOUSE_PRESS),
            1002 => b(mode::MOUSE_DRAG),
            1003 => b(mode::MOUSE_MOTION),
            1004 => b(mode::FOCUS_EVENTS),
            1006 => b(mode::MOUSE_SGR),
            1007 => b(mode::ALT_SCROLL),
            2004 => b(mode::BRACKETED_PASTE),
            2026 => b(mode::SYNC_OUTPUT),
            _ => 0,
        }
    }

    // ---------------------------------------------------------------- SGR

    fn sgr(&mut self, ps: &[&[u16]]) {
        if ps.is_empty() {
            self.pen = Pen::DEFAULT;
            return;
        }
        let mut i = 0;
        while i < ps.len() {
            let p = ps[i];
            let v = p.first().copied().unwrap_or(0);
            match v {
                0 => self.pen = Pen::DEFAULT,
                1 => self.pen.flags |= flags::BOLD,
                2 => self.pen.flags |= flags::DIM,
                3 => self.pen.flags |= flags::ITALIC,
                4 => {
                    // 4:x sub-parameter selects style.
                    match p.get(1).copied() {
                        Some(0) => self.pen.flags &= !(flags::UNDERLINE | flags::DOUBLE_UNDERLINE),
                        Some(2) => {
                            self.pen.flags &= !flags::UNDERLINE;
                            self.pen.flags |= flags::DOUBLE_UNDERLINE
                        }
                        _ => {
                            self.pen.flags &= !flags::DOUBLE_UNDERLINE;
                            self.pen.flags |= flags::UNDERLINE
                        }
                    }
                }
                5 | 6 => self.pen.flags |= flags::BLINK,
                7 => self.pen.flags |= flags::INVERSE,
                8 => self.pen.flags |= flags::HIDDEN,
                9 => self.pen.flags |= flags::STRIKE,
                21 => {
                    self.pen.flags &= !flags::UNDERLINE;
                    self.pen.flags |= flags::DOUBLE_UNDERLINE
                }
                22 => self.pen.flags &= !(flags::BOLD | flags::DIM),
                23 => self.pen.flags &= !flags::ITALIC,
                24 => self.pen.flags &= !(flags::UNDERLINE | flags::DOUBLE_UNDERLINE),
                25 => self.pen.flags &= !flags::BLINK,
                27 => self.pen.flags &= !flags::INVERSE,
                28 => self.pen.flags &= !flags::HIDDEN,
                29 => self.pen.flags &= !flags::STRIKE,
                30..=37 => self.pen.fg = color_indexed((v - 30) as u8),
                39 => self.pen.fg = COLOR_DEFAULT,
                40..=47 => self.pen.bg = color_indexed((v - 40) as u8),
                49 => self.pen.bg = COLOR_DEFAULT,
                90..=97 => self.pen.fg = color_indexed((v - 90 + 8) as u8),
                100..=107 => self.pen.bg = color_indexed((v - 100 + 8) as u8),
                38 | 48 | 58 => {
                    let (color, consumed) = parse_extended_color(ps, i);
                    if let Some(c) = color {
                        match v {
                            38 => self.pen.fg = c,
                            48 => self.pen.bg = c,
                            _ => {}
                        }
                    }
                    i += consumed;
                }
                _ => {}
            }
            i += 1;
        }
    }

    // ---------------------------------------------------------------- reset

    pub(crate) fn full_reset(&mut self) {
        let (cols, rows) = (self.cols, self.rows);
        let sb = self.grids[0].max_sb;
        self.grids = [Grid::new(cols, rows, sb), Grid::new(cols, rows, 0)];
        self.alt = false;
        self.crow = 0;
        self.ccol = 0;
        self.pending_wrap = false;
        self.pen = Pen::DEFAULT;
        self.cursor_style = 0;
        self.saved = None;
        self.saved_primary_for_1049 = None;
        self.top = 0;
        self.bottom = rows - 1;
        self.modes = mode::CURSOR_VISIBLE | mode::AUTOWRAP;
        self.insert_mode = false;
        self.g0 = Charset::Ascii;
        self.g1 = Charset::Ascii;
        self.gl = 0;
        self.cur_link = 0;
        self.offset = 0;
        self.blocks.clear();
        self.reset_tabs();
        self.all_dirty = true;
    }

    fn soft_reset(&mut self) {
        self.pen = Pen::DEFAULT;
        self.modes = (self.modes & mode::ALT_SCREEN) | mode::CURSOR_VISIBLE | mode::AUTOWRAP;
        self.insert_mode = false;
        self.top = 0;
        self.bottom = self.rows - 1;
        self.g0 = Charset::Ascii;
        self.g1 = Charset::Ascii;
        self.gl = 0;
        self.saved = None;
        self.cur_link = 0;
    }

    // ---------------------------------------------------------------- resize

    pub(crate) fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.clamp(1, 1000);
        let rows = rows.clamp(1, 1000);
        if cols == self.cols && rows == self.rows {
            return;
        }
        if cols != self.grids[0].cols {
            self.clear_active_prompt();
            self.reflow_primary(cols);
        }
        for which in 0..2 {
            let is_active = (which == 1) == self.alt;
            let mut crow = if is_active { self.crow } else { 0 };
            self.evicted += resize_grid(&mut self.grids[which], cols, rows, &mut crow, which == 0);
            if is_active {
                self.crow = crow;
            }
        }
        // The saved primary cursor (1049) and DECSC cursor are simply clamped.
        self.cols = cols;
        self.rows = rows;
        self.crow = self.crow.min(rows - 1);
        self.ccol = self.ccol.min(cols - 1);
        self.pending_wrap = false;
        self.top = 0;
        self.bottom = rows - 1;
        self.dirty = vec![true; rows];
        self.all_dirty = true;
        self.reset_tabs();
        let sb = self.grids[0].lines.len() - self.grids[0].rows;
        self.offset = self.offset.min(sb);
        if let Some(s) = &mut self.saved {
            s.row = s.row.min(rows - 1);
            s.col = s.col.min(cols - 1);
        }
    }

    /// Re-wrap the primary screen and its history for a new width: soft-wrapped lines are joined and split again,
    /// the cursor stays on the same character, and command-block marks follow their lines. Hard line breaks stay.
    /// While the shell is sitting at a prompt, wipe the prompt (from its first line down) before reflowing: the shell
    /// redraws it on SIGWINCH, and reflowing a prompt it is about to repaint leaves stray copies behind.
    fn clear_active_prompt(&mut self) {
        if self.alt {
            return;
        }
        let Some(last) = self.blocks.back() else { return };
        if last.output.is_some() {
            return;
        }
        let prompt = last.prompt;
        let g = &mut self.grids[0];
        let base = g.base() as u64;
        if prompt < self.evicted + base {
            return;
        }
        let row = (prompt - self.evicted - base) as usize;
        if row > self.crow || row >= g.rows {
            return;
        }
        for r in row..g.rows {
            let idx = base as usize + r;
            let cols = g.cols;
            g.lines[idx] = Line::blank(cols, Cell::BLANK);
        }
        // The cursor stays on its row. The shell repaints from where it believes the cursor is (zsh and Powerlevel10k
        // go up by the prompt's height from the input line, then clear below), so moving the cursor to the prompt's
        // first line would put every repaint one row too high. The block stays too: a redraw sends no new prompt mark.
        self.pending_wrap = false;
        for d in self.dirty.iter_mut() {
            *d = true;
        }
    }

    fn reflow_primary(&mut self, new_cols: usize) {
        let old_cols = self.grids[0].cols;
        if new_cols == old_cols || new_cols == 0 {
            return;
        }
        let ev_old = self.evicted;
        let rows = self.grids[0].rows;
        let (c_row, c_col) = if self.alt {
            self.saved_primary_for_1049.map_or((0, 0), |s| (s.row, s.col))
        } else {
            // After printing in the last column the cursor logically sits just past it.
            (self.crow, self.ccol + self.pending_wrap as usize)
        };
        let g = &mut self.grids[0];
        let old_len = g.lines.len();
        let base = g.base();
        let cursor_idx = base + c_row.min(rows - 1);
        // Blank lines below the cursor are set aside and re-added afterwards, so growth in line count pushes them out
        // of the viewport before it pushes real content into history.
        let mut keep_len = old_len;
        while keep_len > cursor_idx + 1 {
            let l = &g.lines[keep_len - 1];
            if l.wrapped || !l.cells.iter().all(|c| c.is_exactly_blank()) {
                break;
            }
            keep_len -= 1;
        }
        let max_sb = g.max_sb;
        let old: Vec<Line> = g.lines.drain(..).collect();
        let mut new_lines: Vec<Line> = Vec::with_capacity(old_len + old_len / 8 + rows);
        let mut map: Vec<u32> = vec![0; old_len];
        let mut new_cursor: (usize, usize) = (0, 0);

        let mut i = 0;
        let mut cells: Vec<Cell> = Vec::new();
        let mut starts: Vec<usize> = Vec::new();
        let mut offs: Vec<usize> = Vec::new();
        while i < keep_len {
            let mut j = i;
            while j + 1 < keep_len && old[j].wrapped {
                j += 1;
            }
            cells.clear();
            offs.clear();
            for k in i..=j {
                let line = &old[k];
                offs.push(cells.len());
                if k < j {
                    let start = cells.len();
                    cells.extend_from_slice(&line.cells[..line.cells.len().min(old_cols)]);
                    cells.resize(start + old_cols, Cell::BLANK); // wrapped lines are full width (history stores them trimmed)
                    // A double-width character that did not fit left a padding blank at the end of the row.
                    if old[k + 1].cells.first().map_or(false, |c| c.flags & flags::WIDE != 0)
                        && cells.last().map_or(false, |c| c.is_exactly_blank())
                    {
                        cells.pop();
                    }
                } else {
                    let mut used = line.cells.len().min(old_cols);
                    while used > 0 && line.cells[used - 1].is_exactly_blank() {
                        used -= 1;
                    }
                    cells.extend_from_slice(&line.cells[..used]);
                }
            }
            let cursor_here = cursor_idx >= i && cursor_idx <= j;
            let cursor_off = if cursor_here { offs[cursor_idx - i] + c_col } else { 0 };
            if cursor_here && cells.len() < cursor_off + 1 {
                cells.resize(cursor_off + 1, Cell::BLANK);
            }
            // Split into rows of `new_cols`, never separating a double-width character from its spacer.
            let total = cells.len();
            starts.clear();
            let first_new = new_lines.len();
            let mut pos = 0;
            loop {
                let mut end = (pos + new_cols).min(total);
                if end < total && end > pos && cells[end - 1].flags & flags::WIDE != 0 {
                    end -= 1;
                }
                if end <= pos {
                    end = (pos + 1).min(total.max(pos + 1));
                }
                let end = end.min(total);
                let mut chunk: Vec<Cell> = cells[pos..end].to_vec();
                if let Some(last) = chunk.last_mut() {
                    if last.flags & flags::WIDE != 0 {
                        *last = Cell::BLANK; // a wide character cannot fit (1-column terminal)
                    }
                }
                starts.push(pos);
                let more = end < total;
                new_lines.push(Line { cells: chunk, wrapped: more });
                if !more {
                    break;
                }
                pos = end;
            }
            let chunk_of = |off: usize| starts.partition_point(|&s| s <= off).saturating_sub(1);
            for k in i..=j {
                map[k] = (first_new + chunk_of(offs[k - i])) as u32;
            }
            if cursor_here {
                let ci = chunk_of(cursor_off);
                new_cursor = (first_new + ci, (cursor_off - starts[ci]).min(new_cols - 1));
            }
            i = j + 1;
        }
        let reflowed = new_lines.len();
        let added = reflowed.saturating_sub(keep_len);
        let blanks = (old_len - keep_len).saturating_sub(added);
        for k in keep_len..old_len {
            map[k] = (reflowed + (k - keep_len)) as u32;
        }
        for _ in 0..blanks {
            new_lines.push(Line { cells: Vec::new(), wrapped: false });
        }
        while new_lines.len() < rows {
            new_lines.push(Line { cells: Vec::new(), wrapped: false });
        }
        // History limit.
        let limit = rows + max_sb;
        let mut dropped = 0;
        if new_lines.len() > limit {
            dropped = new_lines.len() - limit;
            new_lines.drain(..dropped);
        }
        let new_base = new_lines.len() - rows;
        let cur_idx = new_cursor.0.saturating_sub(dropped);
        for (idx, l) in new_lines.iter_mut().enumerate() {
            if idx >= new_base {
                l.cells.resize(new_cols, Cell::BLANK); // the screen is stored full width
            } else if !l.wrapped {
                while l.cells.last().map_or(false, |c| c.is_exactly_blank()) {
                    l.cells.pop();
                }
            }
        }
        g.lines = new_lines.into();
        g.cols = new_cols;
        g.pool.clear();
        g.short_pool.clear();
        let (nr, nc) = (cur_idx.saturating_sub(new_base).min(rows - 1), new_cursor.1);
        if self.alt {
            if let Some(s) = &mut self.saved_primary_for_1049 {
                s.row = nr;
                s.col = nc;
            }
        } else {
            self.crow = nr;
            self.ccol = nc;
        }
        // Marks follow their lines.
        let remap = |a: u64| -> u64 {
            if a < ev_old {
                return a;
            }
            let idx = (a - ev_old) as usize;
            if idx < old_len { ev_old + map[idx] as u64 } else { a }
        };
        for b in self.blocks.iter_mut() {
            b.prompt = remap(b.prompt);
            b.output = b.output.map(remap);
            b.end = b.end.map(remap);
        }
        for f in self.finished.iter_mut() {
            f.0 = remap(f.0);
            f.1 = remap(f.1);
        }
        self.evicted += dropped as u64;
        self.pending_wrap = false;
        self.all_dirty = true;
    }

    // ---------------------------------------------------------------- snapshot

    /// Serialize the visible state as VT bytes that rebuild an equivalent terminal.
    pub fn snapshot_vt(&self, max_history: usize) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::with_capacity(self.cols * self.rows * 2);
        out.extend_from_slice(b"\x1bc"); // RIS: start from a clean slate
        let prim = &self.grids[0];
        let rows = self.rows;

        // History: injected in batches of <= rows lines, then pushed into the client's scrollback with CSI n S.
        let sb_len = prim.lines.len() - prim.rows;
        let hist = sb_len.min(max_history);
        let mut idx = sb_len - hist;
        let marks = self.mark_bytes();
        while idx < sb_len {
            let n = (sb_len - idx).min(rows);
            out.extend_from_slice(b"\x1b[H");
            for k in 0..n {
                if k > 0 {
                    out.extend_from_slice(b"\r\n");
                }
                if let Some(m) = marks.get(&(self.evicted + (idx + k) as u64)) {
                    out.extend_from_slice(m);
                }
                self.write_line(&mut out, &prim.lines[idx + k]);
            }
            out.extend_from_slice(format!("\x1b[0m\x1b[{}S", n).as_bytes());
            idx += n;
        }

        // Primary screen.
        self.write_screen(&mut out, prim, Some(&marks));
        if self.alt {
            let (pr, pc) = match self.saved_primary_for_1049 {
                Some(s) => (s.row, s.col),
                None => (self.crow, self.ccol),
            };
            out.extend_from_slice(format!("\x1b[0m\x1b[{};{}H\x1b[?1049h", pr + 1, pc + 1).as_bytes());
            self.write_screen(&mut out, &self.grids[1], None);
        }

        // Modes, margins, cursor.
        if self.top != 0 || self.bottom != rows - 1 {
            out.extend_from_slice(format!("\x1b[{};{}r", self.top + 1, self.bottom + 1).as_bytes());
        }
        let m = self.modes;
        let on = |b: u32, code: &str, out: &mut Vec<u8>| {
            if m & b != 0 {
                out.extend_from_slice(format!("\x1b[?{}h", code).as_bytes());
            }
        };
        on(mode::APP_CURSOR, "1", &mut out);
        on(mode::BRACKETED_PASTE, "2004", &mut out);
        on(mode::MOUSE_PRESS, "1000", &mut out);
        on(mode::MOUSE_DRAG, "1002", &mut out);
        on(mode::MOUSE_MOTION, "1003", &mut out);
        on(mode::FOCUS_EVENTS, "1004", &mut out);
        on(mode::MOUSE_SGR, "1006", &mut out);
        on(mode::ALT_SCROLL, "1007", &mut out);
        if m & mode::AUTOWRAP == 0 {
            out.extend_from_slice(b"\x1b[?7l");
        }
        if m & mode::APP_KEYPAD != 0 {
            out.extend_from_slice(b"\x1b=");
        }
        if self.cursor_style != 0 {
            out.extend_from_slice(format!("\x1b[{} q", self.cursor_style).as_bytes());
        }
        if m & mode::ORIGIN != 0 {
            out.extend_from_slice(b"\x1b[?6h");
            out.extend_from_slice(format!("\x1b[{};{}H", self.crow + 1 - self.top, self.ccol + 1).as_bytes());
        } else {
            out.extend_from_slice(format!("\x1b[{};{}H", self.crow + 1, self.ccol + 1).as_bytes());
        }
        write_sgr(&mut out, &self.pen);
        if !self.cwd.is_empty() && !self.cwd.chars().any(|c| c.is_control()) {
            out.extend_from_slice(b"\x1b]7;");
            out.extend_from_slice(self.cwd.as_bytes());
            out.extend_from_slice(b"\x07");
        }
        if !self.title.is_empty() {
            out.extend_from_slice(b"\x1b]2;");
            out.extend_from_slice(self.title.as_bytes());
            out.extend_from_slice(b"\x1b\\");
        }
        if m & mode::CURSOR_VISIBLE == 0 {
            out.extend_from_slice(b"\x1b[?25l");
        }
        out
    }

    fn write_screen(&self, out: &mut Vec<u8>, g: &Grid, marks: Option<&HashMap<u64, Vec<u8>>>) {
        out.extend_from_slice(b"\x1b[0m\x1b[H\x1b[J"); // erase without ED2: replay must not drop restored command blocks
        let base = g.base();
        for r in 0..g.rows {
            let line = &g.lines[base + r];
            let m = marks.and_then(|m| m.get(&(self.evicted + (base + r) as u64)));
            if line.is_blank() && m.is_none() {
                continue;
            }
            out.extend_from_slice(format!("\x1b[{};1H", r + 1).as_bytes());
            if let Some(m) = m {
                out.extend_from_slice(m);
            }
            self.write_line(out, line);
        }
        out.extend_from_slice(b"\x1b[0m");
    }

    fn write_line(&self, out: &mut Vec<u8>, line: &Line) {
        let mut last = line.cells.len();
        while last > 0 && line.cells[last - 1].is_blank() && line.cells[last - 1].bg == COLOR_DEFAULT {
            last -= 1;
        }
        let mut pen = Pen::DEFAULT;
        for c in &line.cells[..last] {
            if c.flags & flags::WIDE_CONT != 0 {
                continue;
            }
            if c.fg != pen.fg || c.bg != pen.bg || (c.flags & 0xFF | c.flags & flags::DOUBLE_UNDERLINE) != pen.flags {
                pen = Pen { fg: c.fg, bg: c.bg, flags: c.flags & (0xFF | flags::DOUBLE_UNDERLINE) };
                write_sgr(out, &pen);
            }
            let s = self.cell_text(c.ch);
            out.extend_from_slice(s.as_bytes());
        }
        if pen.fg != COLOR_DEFAULT || pen.bg != COLOR_DEFAULT || pen.flags != 0 {
            out.extend_from_slice(b"\x1b[0m");
        }
    }
}

fn write_sgr(out: &mut Vec<u8>, pen: &Pen) {
    let mut s = String::from("\x1b[0");
    let f = pen.flags;
    if f & flags::BOLD != 0 {
        s += ";1";
    }
    if f & flags::DIM != 0 {
        s += ";2";
    }
    if f & flags::ITALIC != 0 {
        s += ";3";
    }
    if f & flags::UNDERLINE != 0 {
        s += ";4";
    }
    if f & flags::DOUBLE_UNDERLINE != 0 {
        s += ";21";
    }
    if f & flags::BLINK != 0 {
        s += ";5";
    }
    if f & flags::INVERSE != 0 {
        s += ";7";
    }
    if f & flags::HIDDEN != 0 {
        s += ";8";
    }
    if f & flags::STRIKE != 0 {
        s += ";9";
    }
    for (c, base) in [(pen.fg, 38), (pen.bg, 48)] {
        match c & COLOR_TAG_MASK {
            COLOR_TAG_PALETTE => s += &format!(";{};5;{}", base, c & 0xFF),
            COLOR_TAG_RGB => s += &format!(";{};2;{};{};{}", base, (c >> 16) & 0xFF, (c >> 8) & 0xFF, c & 0xFF),
            _ => {}
        }
    }
    s.push('m');
    out.extend_from_slice(s.as_bytes());
}

/// Parses `38;5;n`, `38;2;r;g;b` and the colon forms. Returns (color, extra params consumed).
fn parse_extended_color(ps: &[&[u16]], i: usize) -> (Option<u32>, usize) {
    let p = ps[i];
    if p.len() > 1 {
        // Colon form: 38:5:n  38:2:r:g:b  38:2::r:g:b
        match p.get(1).copied() {
            Some(5) => (p.get(2).map(|&n| color_indexed(n as u8)), 0),
            Some(2) => {
                let rest = &p[2..];
                let rgb = if rest.len() >= 4 { &rest[1..4] } else if rest.len() == 3 { rest } else { return (None, 0) };
                (Some(color_rgb(rgb[0] as u8, rgb[1] as u8, rgb[2] as u8)), 0)
            }
            _ => (None, 0),
        }
    } else {
        let get = |k: usize| ps.get(i + k).and_then(|x| x.first()).copied();
        match get(1) {
            Some(5) => match get(2) {
                Some(n) => (Some(color_indexed(n as u8)), 2),
                None => (None, 1),
            },
            Some(2) => match (get(2), get(3), get(4)) {
                (Some(r), Some(g), Some(b)) => (Some(color_rgb(r as u8, g as u8, b as u8)), 4),
                _ => (None, ps.len() - i - 1),
            },
            _ => (None, 0),
        }
    }
}

fn resize_grid(g: &mut Grid, new_cols: usize, new_rows: usize, crow: &mut usize, primary: bool) -> u64 {
    let mut popped = 0u64;
    if new_cols != g.cols {
        let base = g.base();
        let shrinking = new_cols < g.cols;
        for (i, line) in g.lines.iter_mut().enumerate() {
            let in_history = i < base;
            // History lines are stored trimmed: they never need padding, and only need work when shrinking.
            if in_history && !shrinking {
                continue;
            }
            if new_cols < line.cells.len() {
                line.cells.truncate(new_cols);
                if let Some(last) = line.cells.last_mut() {
                    if last.flags & flags::WIDE != 0 {
                        *last = Cell::BLANK;
                    }
                }
            } else if !in_history {
                line.cells.resize(new_cols, Cell::BLANK);
            }
        }
        g.cols = new_cols;
    }
    if new_rows < g.rows {
        let remove = g.rows - new_rows;
        let base = g.base();
        let mut drop_bottom = 0;
        while drop_bottom < remove
            && g.rows - 1 - drop_bottom > *crow
            && g.lines[base + g.rows - 1 - drop_bottom].is_blank()
        {
            drop_bottom += 1;
        }
        for _ in 0..drop_bottom {
            g.lines.pop_back();
        }
        let pushed = remove - drop_bottom;
        *crow = crow.saturating_sub(pushed);
        g.rows = new_rows;
        let limit = g.rows + g.max_sb;
        while g.lines.len() > limit {
            g.lines.pop_front();
            popped += 1;
        }
        let _ = primary;
    } else if new_rows > g.rows {
        let add = new_rows - g.rows;
        let avail = g.lines.len() - g.rows;
        let pull = add.min(avail);
        g.rows = new_rows;
        for _ in 0..(add - pull) {
            g.lines.push_back(Line::blank(new_cols, Cell::BLANK));
        }
        // Lines pulled back out of (trimmed) history must be full width again.
        let base = g.base();
        for line in g.lines.iter_mut().skip(base) {
            if line.cells.len() != new_cols {
                line.cells.resize(new_cols, Cell::BLANK);
            }
        }
        *crow += pull;
    }
    if primary { popped } else { 0 }
}

// ==================================================================== Perform

impl Perform for Screen {
    fn print(&mut self, c: char) {
        self.put_char(c);
    }

    fn execute(&mut self, byte: u8) {
        self.last_zwj = false;
        match byte {
            0x07 => self.bell_count += 1,
            0x08 => {
                if self.ccol > 0 {
                    self.ccol -= 1;
                }
                self.pending_wrap = false;
            }
            0x09 => {
                self.ccol = self.next_tab(self.ccol);
                self.pending_wrap = false;
            }
            0x0A | 0x0B | 0x0C => {
                self.linefeed();
                self.pending_wrap = false;
            }
            0x0D => {
                self.ccol = 0;
                self.pending_wrap = false;
            }
            0x0E => self.gl = 1,
            0x0F => self.gl = 0,
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, inter: &[u8], ignore: bool, action: char) {
        if ignore {
            return;
        }
        self.last_zwj = false;
        let mut buf: [&[u16]; 32] = [&[]; 32];
        let mut n = 0;
        for p in params.iter() {
            if n < 32 {
                buf[n] = p;
                n += 1;
            }
        }
        let ps = &buf[..n];
        let p = |i: usize, def: usize| -> usize {
            match ps.get(i).and_then(|x| x.first()) {
                Some(&v) if v != 0 => v as usize,
                _ => def,
            }
        };
        let raw = |i: usize| -> usize { ps.get(i).and_then(|x| x.first()).copied().unwrap_or(0) as usize };
        let private = inter.first() == Some(&b'?');
        let rows = self.rows;
        let cols = self.cols;

        // Sequences with an intermediate other than '?' have their own meanings.
        if !inter.is_empty() && !private {
            match (inter, action) {
                ([b' '], 'q') => self.cursor_style = (raw(0).min(6)) as u8,
                ([b'!'], 'p') => self.soft_reset(),
                ([b'>'], 'c') => self.respond(b"\x1b[>1;10;0c"),
                ([b'>'], 'q') => self.respond(b"\x1bP>|aleno 0.1\x1b\\"),
                ([b'$'], 'p') => {
                    let m = raw(0) as u16;
                    let st = if m == 4 { if self.insert_mode { 1 } else { 2 } } else { 0 };
                    self.respond(format!("\x1b[{};{}$y", m, st).as_bytes());
                }
                _ => {}
            }
            return;
        }
        if private {
            match (inter, action) {
                ([b'?'], 'h') | ([b'?'], 'l') => {
                    for q in ps {
                        if let Some(&m) = q.first() {
                            self.set_dec_mode(m, action == 'h');
                        }
                    }
                }
                ([b'?', b'$'], 'p') => {
                    let m = raw(0) as u16;
                    let st = self.dec_mode_state(m);
                    self.respond(format!("\x1b[?{};{}$y", m, st).as_bytes());
                }
                _ => {}
            }
            return;
        }

        match action {
            'A' => {
                let top = if self.crow >= self.top { self.top } else { 0 };
                self.crow = self.crow.saturating_sub(p(0, 1)).max(top);
                self.pending_wrap = false;
            }
            'B' | 'e' => {
                let bot = if self.crow <= self.bottom { self.bottom } else { rows - 1 };
                self.crow = (self.crow + p(0, 1)).min(bot);
                self.pending_wrap = false;
            }
            'C' | 'a' => {
                self.ccol = (self.ccol + p(0, 1)).min(cols - 1);
                self.pending_wrap = false;
            }
            'D' => {
                self.ccol = self.ccol.saturating_sub(p(0, 1));
                self.pending_wrap = false;
            }
            'E' => {
                self.crow = (self.crow + p(0, 1)).min(self.bottom.max(self.crow));
                self.ccol = 0;
                self.pending_wrap = false;
            }
            'F' => {
                self.crow = self.crow.saturating_sub(p(0, 1)).max(if self.crow >= self.top { self.top } else { 0 });
                self.ccol = 0;
                self.pending_wrap = false;
            }
            'G' | '`' => {
                self.ccol = (p(0, 1) - 1).min(cols - 1);
                self.pending_wrap = false;
            }
            'H' | 'f' => self.goto(p(0, 1) - 1, p(1, 1) - 1),
            'd' => {
                let c = self.ccol;
                self.goto(p(0, 1) - 1, c);
            }
            'I' => {
                for _ in 0..p(0, 1) {
                    self.ccol = self.next_tab(self.ccol);
                }
                self.pending_wrap = false;
            }
            'Z' => {
                for _ in 0..p(0, 1) {
                    self.ccol = self.prev_tab(self.ccol);
                }
                self.pending_wrap = false;
            }
            'J' => self.erase_display(raw(0)),
            'K' => self.erase_line(raw(0)),
            'L' => {
                if self.crow >= self.top && self.crow <= self.bottom {
                    let n = p(0, 1);
                    self.scroll_down_region(self.crow, self.bottom, n);
                    self.ccol = 0;
                    self.pending_wrap = false;
                }
            }
            'M' => {
                if self.crow >= self.top && self.crow <= self.bottom {
                    let n = p(0, 1);
                    self.scroll_up_region_in(self.crow, self.bottom, n);
                    self.ccol = 0;
                    self.pending_wrap = false;
                }
            }
            '@' => self.insert_chars(p(0, 1)),
            'P' => self.delete_chars(p(0, 1)),
            'X' => {
                let (r, c) = (self.crow, self.ccol);
                self.erase_cells(r, c, c + p(0, 1));
            }
            'S' => self.scroll_up_region(self.top, self.bottom, p(0, 1)),
            'T' => self.scroll_down_region(self.top, self.bottom, p(0, 1)),
            'b' => {
                if let Some(c) = self.last_char {
                    for _ in 0..p(0, 1).min(65535) {
                        self.put_char(c);
                    }
                }
            }
            'g' => match raw(0) {
                0 => {
                    let c = self.ccol;
                    self.tabs[c] = false;
                }
                3 => self.tabs.iter_mut().for_each(|t| *t = false),
                _ => {}
            },
            'm' => self.sgr(ps),
            'h' | 'l' => {
                for q in ps {
                    if q.first() == Some(&4) {
                        self.insert_mode = action == 'h';
                    }
                }
            }
            'r' => {
                let t = p(0, 1) - 1;
                let b = p(1, rows) - 1;
                if t < b && b < rows {
                    self.top = t;
                    self.bottom = b;
                    self.goto(0, 0);
                }
            }
            's' => self.save_cursor(),
            'u' => self.restore_cursor(),
            'n' => match raw(0) {
                5 => self.respond(b"\x1b[0n"),
                6 => {
                    let r = if self.has_mode(mode::ORIGIN) { self.crow.saturating_sub(self.top) } else { self.crow };
                    self.respond(format!("\x1b[{};{}R", r + 1, self.ccol + 1).as_bytes());
                }
                _ => {}
            },
            'c' => {
                if raw(0) == 0 {
                    self.respond(b"\x1b[?62;22c");
                }
            }
            't' => {
                if raw(0) == 18 {
                    self.respond(format!("\x1b[8;{};{}t", rows, cols).as_bytes());
                }
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, inter: &[u8], ignore: bool, byte: u8) {
        if ignore {
            return;
        }
        self.last_zwj = false;
        match (inter, byte) {
            ([], b'7') => self.save_cursor(),
            ([], b'8') => self.restore_cursor(),
            ([], b'D') => {
                self.linefeed();
                self.pending_wrap = false;
            }
            ([], b'E') => {
                self.linefeed();
                self.ccol = 0;
                self.pending_wrap = false;
            }
            ([], b'M') => {
                self.reverse_index();
                self.pending_wrap = false;
            }
            ([], b'H') => {
                let c = self.ccol;
                self.tabs[c] = true;
            }
            ([], b'c') => self.full_reset(),
            ([], b'=') => self.modes |= mode::APP_KEYPAD,
            ([], b'>') => self.modes &= !mode::APP_KEYPAD,
            ([b'('], b) => self.g0 = if b == b'0' { Charset::Graphics } else { Charset::Ascii },
            ([b')'], b) => self.g1 = if b == b'0' { Charset::Graphics } else { Charset::Ascii },
            ([b'#'], b'8') => {
                let e = Cell { ch: b'E' as u32, ..Cell::BLANK };
                for r in 0..self.rows {
                    self.g().row_mut(r).cells.iter_mut().for_each(|c| *c = e);
                }
                self.all_dirty = true;
            }
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell: bool) {
        self.last_zwj = false;
        let Some(cmd) = params.first() else { return };
        match *cmd {
            b"0" | b"1" | b"2" => {
                let mut t = String::new();
                for (i, p) in params[1..].iter().enumerate() {
                    if i > 0 {
                        t.push(';');
                    }
                    t.push_str(&String::from_utf8_lossy(p));
                }
                t.retain(|c| !c.is_control());
                t.truncate(MAX_TITLE.min(t.len()));
                while !t.is_char_boundary(t.len()) {
                    t.pop();
                }
                if *cmd != b"1" {
                    self.title = t;
                    self.title_dirty = true;
                }
            }
            b"7" => {
                if let Some(p) = params.get(1) {
                    self.cwd = String::from_utf8_lossy(p).chars().take(MAX_URI).collect();
                }
            }
            b"8" => {
                let uri = params.get(2).map(|u| String::from_utf8_lossy(u).into_owned()).unwrap_or_default();
                if uri.is_empty() || uri.len() > MAX_URI {
                    self.cur_link = 0;
                } else if let Some(&id) = self.link_map.get(&uri) {
                    self.cur_link = id;
                } else if self.links.len() < MAX_LINKS {
                    let id = self.links.len() as u16;
                    self.links.push(uri.clone());
                    self.link_map.insert(uri, id);
                    self.cur_link = id;
                }
            }
            b"9" => {
                // ConEmu / Windows Terminal progress: `OSC 9 ; 4 ; state ; percent`.
                if params.get(1).map(|p| *p) == Some(b"4") {
                    let num = |i: usize| params.get(i).and_then(|p| std::str::from_utf8(p).ok()).and_then(|t| t.trim().parse::<u32>().ok());
                    let state = num(2).unwrap_or(0);
                    let value = num(3).map_or(0, |v| v.min(100)) as u8;
                    let next = match state {
                        0 => Some((0, 0)),
                        1 | 2 | 4 => Some((state as u8, value)),
                        3 => Some((3, 0)),
                        _ => None,
                    };
                    if let Some(n) = next {
                        self.set_progress(n);
                    }
                }
            }
            b"133" => self.osc_133(params),
            // OSC 52 (clipboard) is deliberately ignored: terminal output is untrusted.
            _ => {}
        }
    }

    // DCS sequences are parsed (so they cannot corrupt state) but not acted upon.
    fn hook(&mut self, _p: &Params, _i: &[u8], _ignore: bool, _c: char) {}
    fn put(&mut self, _b: u8) {}
    fn unhook(&mut self) {}
}

impl Screen {
    /// Absolute id of the cursor's line in the primary grid (None on the alternate screen).
    fn cursor_abs(&self) -> Option<u64> {
        if self.alt {
            return None;
        }
        Some(self.evicted + (self.grids[0].base() + self.crow) as u64)
    }

    fn set_progress(&mut self, p: (u8, u8)) {
        if self.progress != p {
            self.progress = p;
            self.progress_dirty = true;
        }
    }

    /// OSC 133 semantic prompt marks: A = prompt start, C = output start, D = command finished.
    fn osc_133(&mut self, params: &[&[u8]]) {
        let Some(abs) = self.cursor_abs() else { return };
        let Some(kind) = params.get(1).and_then(|p| p.first()).copied() else { return };
        while self.blocks.front().map_or(false, |b| b.prompt < self.evicted) {
            self.blocks.pop_front();
        }
        match kind {
            b'A' => {
                if let Some(last) = self.blocks.back_mut().filter(|b| b.output.is_none()) {
                    last.prompt = abs; // prompt redrawn, or a new prompt after an empty command
                } else {
                    if self.blocks.len() >= MAX_BLOCKS {
                        self.blocks.pop_front();
                    }
                    self.blocks.push_back(Block { prompt: abs, output: None, end: None, exit: None, dur_ms: None, started: None, cmd: None });
                }
            }
            b'C' => {
                let at = if self.ccol > 0 { abs + 1 } else { abs };
                if let Some(last) = self.blocks.back_mut().filter(|b| b.output.is_none()) {
                    last.output = Some(at);
                    last.started = Some(std::time::Instant::now());
                    // `133;C;cmd=<text>`: the command line (the text may itself contain ';').
                    if params.len() > 2 {
                        let joined = params[2..].iter().map(|p| String::from_utf8_lossy(p).into_owned()).collect::<Vec<_>>().join(";");
                        if let Some(c) = joined.strip_prefix("cmd=") {
                            let c = c.trim();
                            if !c.is_empty() {
                                last.cmd = Some(c.chars().take(1024).collect());
                            }
                        }
                    }
                }
            }
            b'D' => {
                self.set_progress((0, 0));
                let at = if self.ccol > 0 { abs + 1 } else { abs };
                if let Some(last) = self.blocks.back_mut().filter(|b| b.output.is_some() && b.end.is_none()) {
                    last.end = Some(at);
                    let mut exit = None;
                    let mut dur = last.started.map(|t| t.elapsed().as_millis().min(u32::MAX as u128 - 1) as u32);
                    let mut replayed = false;
                    for (i, p) in params.iter().enumerate().skip(2) {
                        let t = String::from_utf8_lossy(p);
                        if let Some(d) = t.strip_prefix("dur=") {
                            replayed = true; // only snapshots carry an explicit duration
                            if let Ok(v) = d.parse::<u32>() {
                                dur = Some(v);
                            }
                        } else if i == 2 {
                            exit = t.parse::<i32>().ok();
                        }
                    }
                    last.exit = exit;
                    last.dur_ms = dur;
                    if !replayed {
                        let ev = (last.prompt, last.output.unwrap_or(last.prompt), exit, dur.unwrap_or(0));
                        if self.finished.len() >= 64 {
                            self.finished.remove(0);
                        }
                        self.finished.push(ev);
                    }
                }
            }
            _ => {}
        }
    }

    /// OSC 133 bytes to replay, keyed by absolute line, in emission order.
    fn mark_bytes(&self) -> HashMap<u64, Vec<u8>> {
        let mut m: HashMap<u64, Vec<u8>> = HashMap::new();
        for b in &self.blocks {
            m.entry(b.prompt).or_default().extend_from_slice(b"\x1b]133;A\x07");
            if let Some(o) = b.output {
                let mut t = String::from("\x1b]133;C");
                if let Some(c) = &b.cmd {
                    // Keep it a single well-formed OSC: no control characters.
                    t += &format!(";cmd={}", c.chars().filter(|ch| !ch.is_control()).collect::<String>());
                }
                t.push('\x07');
                m.entry(o).or_default().extend_from_slice(t.as_bytes());
            }
            if let Some(e) = b.end {
                let mut t = String::from("\x1b]133;D;");
                if let Some(x) = b.exit {
                    t += &x.to_string();
                }
                if let Some(d) = b.dur_ms {
                    t += &format!(";dur={}", d);
                }
                t.push('\x07');
                m.entry(e).or_default().extend_from_slice(t.as_bytes());
            }
        }
        m
    }

    // IL/DL helper: delete lines within [top, bottom] shifting up, same as scroll_up_region
    // but never touching scrollback.
    fn scroll_up_region_in(&mut self, top: usize, bottom: usize, n: usize) {
        let n = n.min(bottom + 1 - top);
        let blank = self.blank();
        let g = self.g();
        let b = g.base();
        for _ in 0..n {
            let mut l = g.lines.remove(b + top).unwrap();
            l.cells.fill(blank);
            l.wrapped = false;
            g.lines.insert(b + bottom, l);
        }
        self.mark_range(top, bottom);
    }
}
