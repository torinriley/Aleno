//! Cell, color and line primitives shared by the emulator and the FFI layer.

/// Color encoding stored in `Cell::fg` / `Cell::bg`:
/// `0` = terminal default, `0x01_0000_ii` = palette index, `0x02_rr_gg_bb` = true color.
pub const COLOR_DEFAULT: u32 = 0;
pub const COLOR_TAG_MASK: u32 = 0xFF00_0000;
pub const COLOR_TAG_PALETTE: u32 = 0x0100_0000;
pub const COLOR_TAG_RGB: u32 = 0x0200_0000;

pub const fn color_indexed(i: u8) -> u32 {
    COLOR_TAG_PALETTE | i as u32
}
pub const fn color_rgb(r: u8, g: u8, b: u8) -> u32 {
    COLOR_TAG_RGB | ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

/// Attribute flags in `Cell::flags`.
pub mod flags {
    pub const BOLD: u16 = 1 << 0;
    pub const DIM: u16 = 1 << 1;
    pub const ITALIC: u16 = 1 << 2;
    pub const UNDERLINE: u16 = 1 << 3;
    pub const BLINK: u16 = 1 << 4;
    pub const INVERSE: u16 = 1 << 5;
    pub const HIDDEN: u16 = 1 << 6;
    pub const STRIKE: u16 = 1 << 7;
    /// First half of a double-width character.
    pub const WIDE: u16 = 1 << 8;
    /// Spacer following a WIDE cell. Carries no glyph.
    pub const WIDE_CONT: u16 = 1 << 9;
    pub const DOUBLE_UNDERLINE: u16 = 1 << 10;
    /// Mask of attributes that change how an *empty* cell looks.
    pub const VISIBLE_ON_BLANK: u16 = UNDERLINE | INVERSE | STRIKE | DOUBLE_UNDERLINE;
}

/// High bit of `Cell::ch`: the low bits index the terminal's grapheme-cluster table.
pub const CLUSTER_FLAG: u32 = 0x8000_0000;

/// One terminal cell. `repr(C)` because this exact layout is copied to Swift.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    pub ch: u32,
    pub fg: u32,
    pub bg: u32,
    pub flags: u16,
    /// OSC 8 hyperlink id (0 = none).
    pub link: u16,
}

/// `Cell` is `repr(C)`, 16 bytes with no padding, so equality with BLANK is a single 128-bit compare.
const BLANK_BITS: [u64; 2] = unsafe { std::mem::transmute::<Cell, [u64; 2]>(Cell { ch: b' ' as u32, fg: COLOR_DEFAULT, bg: COLOR_DEFAULT, flags: 0, link: 0 }) };

impl Cell {
    /// Exactly `Cell::BLANK` (fast bitwise compare; used when trimming history lines).
    #[inline(always)]
    pub fn is_exactly_blank(&self) -> bool {
        let bits: [u64; 2] = unsafe { std::mem::transmute::<Cell, [u64; 2]>(*self) };
        bits == BLANK_BITS
    }

    pub const BLANK: Cell = Cell { ch: b' ' as u32, fg: COLOR_DEFAULT, bg: COLOR_DEFAULT, flags: 0, link: 0 };

    #[inline]
    pub fn is_blank(&self) -> bool {
        self.ch == b' ' as u32
            && self.bg == COLOR_DEFAULT
            && self.flags & flags::VISIBLE_ON_BLANK == 0
            && self.link == 0
    }
}

impl Default for Cell {
    fn default() -> Self {
        Cell::BLANK
    }
}

#[derive(Clone, Debug)]
pub struct Line {
    pub cells: Vec<Cell>,
    /// True when this line soft-wrapped into the next one.
    pub wrapped: bool,
}

impl Line {
    pub fn blank(cols: usize, fill: Cell) -> Line {
        Line { cells: vec![fill; cols], wrapped: false }
    }
    pub fn is_blank(&self) -> bool {
        self.cells.iter().all(|c| c.is_blank())
    }
}
