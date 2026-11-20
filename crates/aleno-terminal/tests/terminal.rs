use aleno_terminal::*;

fn t(cols: usize, rows: usize) -> Terminal {
    Terminal::new(cols, rows, 1000)
}

#[test]
fn prints_and_wraps() {
    let mut term = t(5, 3);
    term.feed(b"abcdefgh");
    assert_eq!(term.row_text(0), "abcde");
    assert_eq!(term.row_text(1), "fgh");
}

#[test]
fn crlf_and_scroll_into_history() {
    let mut term = t(10, 3);
    term.feed(b"1\r\n2\r\n3\r\n4\r\n5");
    assert_eq!(term.row_text(0), "3");
    assert_eq!(term.row_text(2), "5");
    assert_eq!(term.scrollback_len(), 2);
    assert_eq!(term.text_range(0, 0, 1, 5), "1\n2");
}

#[test]
fn cursor_movement_and_erase() {
    let mut term = t(10, 5);
    term.feed(b"hello\x1b[1;3H**\x1b[2;1Hworld\x1b[1;1H\x1b[K");
    assert_eq!(term.row_text(0), "");
    assert_eq!(term.row_text(1), "world");
    term.feed(b"\x1b[2J");
    assert_eq!(term.screen_text().trim(), "");
}

#[test]
fn sgr_colors() {
    let mut term = t(20, 2);
    term.feed(b"\x1b[1;31mA\x1b[38;5;200mB\x1b[38;2;1;2;3mC\x1b[48:2::9:8:7mD\x1b[0mE");
    assert_eq!(term.cell(0, 0).fg, color_indexed(1));
    assert!(term.cell(0, 0).flags & flags::BOLD != 0);
    assert_eq!(term.cell(0, 1).fg, color_indexed(200));
    assert_eq!(term.cell(0, 2).fg, color_rgb(1, 2, 3));
    assert_eq!(term.cell(0, 3).bg, color_rgb(9, 8, 7));
    assert_eq!(term.cell(0, 4).fg, COLOR_DEFAULT);
    assert_eq!(term.cell(0, 4).flags, 0);
}

#[test]
fn scroll_region() {
    let mut term = t(5, 5);
    term.feed(b"1\r\n2\r\n3\r\n4\r\n5");
    term.feed(b"\x1b[2;4r\x1b[4;1H\n");
    assert_eq!(term.row_text(0), "1");
    assert_eq!(term.row_text(1), "3");
    assert_eq!(term.row_text(2), "4");
    assert_eq!(term.row_text(3), "");
    assert_eq!(term.row_text(4), "5");
    assert_eq!(term.scrollback_len(), 0);
}

#[test]
fn alt_screen_preserves_primary() {
    let mut term = t(10, 3);
    term.feed(b"main\x1b[?1049h");
    assert!(term.is_alt_screen());
    assert_eq!(term.row_text(0), "");
    term.feed(b"alt\x1b[?1049l");
    assert!(!term.is_alt_screen());
    assert_eq!(term.row_text(0), "main");
    assert_eq!(term.cursor(), (0, 4));
}

#[test]
fn wide_and_combining() {
    let mut term = t(10, 2);
    term.feed("日本e\u{301}x".as_bytes());
    assert!(term.cell(0, 0).flags & flags::WIDE != 0);
    assert!(term.cell(0, 1).flags & flags::WIDE_CONT != 0);
    assert_eq!(term.row_text(0), "日本e\u{301}x");
    assert_eq!(term.cursor().1, 6);
    // Cluster occupies a single cell.
    assert_eq!(term.cell(0, 4).ch & CLUSTER_FLAG, CLUSTER_FLAG);
    assert_eq!(term.cell(0, 5).ch, 'x' as u32);
}

#[test]
fn wide_wraps_at_margin() {
    let mut term = t(3, 3);
    term.feed("ab日".as_bytes());
    assert_eq!(term.row_text(0), "ab");
    assert_eq!(term.row_text(1), "日");
}

#[test]
fn zwj_emoji_sequence_is_one_cluster() {
    let mut term = t(10, 2);
    term.feed("👨\u{200D}👩x".as_bytes());
    assert_eq!(term.cell(0, 2).ch, 'x' as u32);
}

#[test]
fn malformed_utf8_and_escapes_are_safe() {
    let mut term = t(10, 3);
    term.feed(&[0xff, 0xfe, 0xc3, 0x28, 0x1b, 0x5b, 0x1b, b'[', b'9', b'9', b'9', b'9', b'9', b'Z']);
    term.feed(b"\x1b]0;unterminated");
    term.feed(b"\x1b[38;2;1m\x1b[38;5m\x1b[;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;m");
    term.feed(b"ok");
    // Still functional.
    assert!(term.screen_text().contains("ok") || term.rows() == 3);
}

#[test]
fn title_and_hyperlinks() {
    let mut term = t(20, 2);
    term.feed(b"\x1b]2;my title\x07");
    assert_eq!(term.title(), "my title");
    assert!(term.take_title_changed());
    term.feed(b"\x1b]8;;http://example.com\x1b\\link\x1b]8;;\x1b\\ plain");
    let id = term.cell(0, 0).link;
    assert!(id != 0);
    assert_eq!(term.link(id), "http://example.com");
    assert_eq!(term.cell(0, 5).link, 0);
}

#[test]
fn modes_toggle() {
    let mut term = t(10, 3);
    term.feed(b"\x1b[?1h\x1b[?2004h\x1b[?1000h\x1b[?1006h\x1b[?25l");
    let m = term.modes();
    assert!(m & mode::APP_CURSOR != 0);
    assert!(m & mode::BRACKETED_PASTE != 0);
    assert!(m & mode::MOUSE_PRESS != 0);
    assert!(m & mode::MOUSE_SGR != 0);
    assert!(m & mode::CURSOR_VISIBLE == 0);
    term.feed(b"\x1b[?1l\x1b[?2004l");
    assert!(term.modes() & (mode::APP_CURSOR | mode::BRACKETED_PASTE) == 0);
}

#[test]
fn device_reports() {
    let mut term = t(80, 24);
    term.feed(b"\x1b[5;7H\x1b[6n\x1b[c");
    let r = String::from_utf8(term.take_responses()).unwrap();
    assert!(r.starts_with("\x1b[5;7R"));
    assert!(r.contains("\x1b[?62"));
    term.set_respond(false);
    term.feed(b"\x1b[6n");
    assert!(term.take_responses().is_empty());
}

#[test]
fn insert_delete_chars_and_lines() {
    let mut term = t(10, 4);
    term.feed(b"abcdef\x1b[1;3H\x1b[2@");
    assert_eq!(term.row_text(0), "ab  cdef");
    term.feed(b"\x1b[3P");
    assert_eq!(term.row_text(0), "abdef");
    term.feed(b"\x1b[2;1Hxx\x1b[L");
    assert_eq!(term.row_text(1), "");
    assert_eq!(term.row_text(2), "xx");
}

#[test]
fn resize_keeps_content_and_cursor() {
    let mut term = t(10, 4);
    term.feed(b"a\r\nb\r\nc\r\nd\r\ne");
    assert_eq!(term.scrollback_len(), 1);
    term.resize(10, 2);
    assert_eq!(term.rows(), 2);
    assert_eq!(term.row_text(1), "e");
    assert_eq!(term.row_text(0), "d");
    term.resize(6, 6);
    assert_eq!(term.cols(), 6);
    assert_eq!(term.row_text(term.cursor().0), "e");
    term.feed("\x1b[2J".as_bytes());
}

#[test]
fn scrollback_is_bounded() {
    let mut term = Terminal::new(10, 3, 50);
    for i in 0..1000 {
        term.feed(format!("line {}\r\n", i).as_bytes());
    }
    assert_eq!(term.scrollback_len(), 50);
    assert!(term.text_range(0, 0, 0, 20).starts_with("line "));
}

#[test]
fn search_finds_matches() {
    let mut term = t(20, 3);
    term.feed(b"Hello World\r\nfoo hello\r\n");
    let m = term.search("hello", true, 100);
    assert_eq!(m.len(), 2);
    assert_eq!(m[0], Match { row: 0, col_start: 0, col_end: 4 });
    assert_eq!(term.search("hello", false, 100).len(), 1);
}

#[test]
fn line_drawing_charset() {
    let mut term = t(10, 2);
    term.feed(b"\x1b(0lqk\x1b(B");
    assert_eq!(term.row_text(0), "┌─┐");
}

#[test]
fn copy_view_tracks_dirty_rows() {
    let mut term = t(4, 3);
    let mut cells = vec![Cell::BLANK; 12];
    let mut dirty = vec![0u8; 3];
    let info = term.copy_view(4, 3, &mut cells, &mut dirty);
    assert_eq!(dirty, vec![1, 1, 1]);
    assert_eq!(info.needs_realloc, 0);
    term.copy_view(4, 3, &mut cells, &mut dirty);
    assert_eq!(dirty, vec![0, 0, 0]);
    term.feed(b"\x1b[2;1Hx");
    term.copy_view(4, 3, &mut cells, &mut dirty);
    assert_eq!(dirty, vec![0, 1, 0]);
    assert_eq!(cells[4].ch, 'x' as u32);
    let info = term.copy_view(5, 3, &mut cells, &mut dirty);
    assert_eq!(info.needs_realloc, 1);
}

fn same_screen(a: &Terminal, b: &Terminal) -> bool {
    for r in 0..a.rows() {
        for c in 0..a.cols() {
            let (x, y) = (a.cell(r, c), b.cell(r, c));
            if a.cluster(x.ch) != b.cluster(y.ch) || x.fg != y.fg || x.bg != y.bg || (x.flags & 0x7ff) != (y.flags & 0x7ff) {
                return false;
            }
        }
    }
    true
}

#[test]
fn snapshot_roundtrip_primary() {
    let mut a = t(30, 8);
    a.feed(b"\x1b[1;32muser@host\x1b[0m:~$ ls\r\n");
    for i in 0..20 {
        a.feed(format!("\x1b[3{}mrow {} \x1b[0m日本語\r\n", i % 8, i).as_bytes());
    }
    a.feed(b"\x1b[?2004h\x1b]2;hi\x07\x1b[2;4r\x1b[5;5H");
    let snap = a.snapshot_vt(1000);
    let mut b = t(30, 8);
    b.set_respond(false);
    b.feed(&snap);
    assert!(same_screen(&a, &b), "\nA:\n{}\nB:\n{}", a.screen_text(), b.screen_text());
    assert_eq!(a.cursor(), b.cursor());
    assert_eq!(a.modes() & mode::BRACKETED_PASTE, b.modes() & mode::BRACKETED_PASTE);
    assert_eq!(b.title(), "hi");
    assert_eq!(a.scrollback_len(), b.scrollback_len());
    assert_eq!(a.text_range(0, 0, 3, 10), b.text_range(0, 0, 3, 10));
}

#[test]
fn snapshot_roundtrip_alt_screen() {
    let mut a = t(20, 6);
    a.feed(b"primary text\x1b[?1049h\x1b[2;3Halt content\x1b[7mINV\x1b[0m\x1b[?1000h\x1b[?25l");
    let snap = a.snapshot_vt(100);
    let mut b = t(20, 6);
    b.feed(&snap);
    assert!(b.is_alt_screen());
    assert!(same_screen(&a, &b));
    assert!(b.modes() & mode::MOUSE_PRESS != 0);
    assert!(b.modes() & mode::CURSOR_VISIBLE == 0);
    b.feed(b"\x1b[?1049l");
    assert_eq!(b.row_text(0), "primary text");
}

#[test]
fn large_random_stream_does_not_panic() {
    let mut term = t(40, 12);
    let mut x: u64 = 0x1234_5678_9abc_def0;
    let mut buf = vec![0u8; 1 << 16];
    for _ in 0..40 {
        for b in buf.iter_mut() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = (x >> 24) as u8;
        }
        term.feed(&buf);
        if x % 7 == 0 {
            term.resize(20 + (x % 50) as usize, 5 + (x % 20) as usize);
        }
    }
    let snap = term.snapshot_vt(100);
    let mut other = t(term.cols(), term.rows());
    other.feed(&snap);
}

fn screens_equal(a: &Terminal, b: &Terminal) -> bool {
    if a.cols() != b.cols() || a.rows() != b.rows() || a.cursor() != b.cursor() || a.scrollback_len() != b.scrollback_len() {
        return false;
    }
    for r in 0..a.rows() {
        for c in 0..a.cols() {
            let (x, y) = (a.cell(r, c), b.cell(r, c));
            if a.cluster(x.ch) != b.cluster(y.ch) || x.fg != y.fg || x.bg != y.bg || x.flags != y.flags {
                return false;
            }
        }
    }
    true
}

#[test]
fn fast_paths_match_the_general_parser() {
    // Whole-chunk feeding uses the fast paths (ASCII runs, C0 controls, inline SGR); feeding one byte at a time forces
    // every sequence through the general parser. Both must produce the identical screen.
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut rnd = move || { x ^= x << 13; x ^= x >> 7; x ^= x << 17; x };
    for round in 0..300 {
        let mut data: Vec<u8> = Vec::new();
        for _ in 0..(50 + rnd() % 400) {
            match rnd() % 12 {
                0 | 1 | 2 | 3 => { for _ in 0..(1 + rnd() % 30) { data.push(0x20 + (rnd() % 95) as u8); } }
                4 => data.extend_from_slice(b"\r\n"),
                5 => data.push(b'\n'),
                6 => data.push(b'\t'),
                7 | 8 | 9 => {
                    data.extend_from_slice(b"\x1b[");
                    for k in 0..(rnd() % 20) {
                        if k > 0 { data.push(if rnd() % 25 == 0 { b':' } else { b';' }); }
                        data.extend_from_slice(format!("{}", rnd() % 300).as_bytes());
                    }
                    data.push(if rnd() % 15 == 0 { b'K' } else { b'm' });
                }
                10 => data.extend_from_slice(format!("\x1b[{};{}H", 1 + rnd() % 30, 1 + rnd() % 90).as_bytes()),
                _ => data.extend_from_slice("日本é".as_bytes()),
            }
        }
        let (cols, rows) = (20 + (round % 80), 5 + (round % 25));
        let mut whole = Terminal::new(cols, rows, 500);
        let mut bytewise = Terminal::new(cols, rows, 500);
        whole.feed(&data);
        for b in &data { bytewise.feed(std::slice::from_ref(b)); }
        assert!(screens_equal(&whole, &bytewise), "round {}: fast path diverged from the general parser", round);
        // Also split at a random point (a sequence cut across two feeds).
        let cut = (rnd() as usize) % data.len().max(1);
        let mut split = Terminal::new(cols, rows, 500);
        split.feed(&data[..cut]);
        split.feed(&data[cut..]);
        assert!(screens_equal(&whole, &split), "round {}: split feed diverged", round);
    }
}

// ------------------------------------------------------------------ OSC 133 command blocks

fn session(t: &mut Terminal, cmd: &str, out: &str, exit: i32) {
    t.feed(b"\x1b]133;A\x07$ ");
    t.feed(cmd.as_bytes());
    t.feed(b"\r\n\x1b]133;C\x07");
    t.feed(out.as_bytes());
    t.feed(format!("\x1b]133;D;{}\x07", exit).as_bytes());
}

#[test]
fn osc133_tracks_blocks_with_exit_status() {
    let mut t = Terminal::new(40, 10, 100);
    session(&mut t, "ls", "a\r\nb\r\n", 0);
    session(&mut t, "false", "", 1);
    t.feed(b"\x1b]133;A\x07$ ");
    let b = t.blocks();
    assert_eq!(b.len(), 3);
    assert_eq!((b[0].prompt_row, b[0].output_row, b[0].end_row, b[0].exit), (0, 1, 3, 0));
    assert_eq!((b[1].prompt_row, b[1].output_row, b[1].end_row, b[1].exit), (3, 4, 4, 1));
    assert_eq!((b[2].prompt_row, b[2].output_row, b[2].end_row), (4, -1, -1));
    assert!(b[0].dur_ms >= 0 && b[2].dur_ms == -1);
    assert_eq!(t.text_range(1, 0, 2, usize::MAX), "a\nb");
}

#[test]
fn osc133_redrawn_prompt_does_not_duplicate_and_blocks_follow_scrolling() {
    let mut t = Terminal::new(20, 4, 100);
    t.feed(b"\x1b]133;A\x07$ \x1b]133;A\x07$ ");
    assert_eq!(t.blocks().len(), 1);
    t.feed(b"x\r\n\x1b]133;C\x07one\r\ntwo\r\nthree\r\n\x1b]133;D;0\x07");
    let b = t.blocks();
    assert_eq!(b.len(), 1);
    // Output scrolled into history; row indexes still address the same text.
    assert_eq!(t.text_range(b[0].output_row as usize, 0, b[0].end_row as usize - 1, usize::MAX), "one\ntwo\nthree");
}

#[test]
fn osc133_blocks_survive_snapshot_replay() {
    let mut t = Terminal::new(30, 6, 200);
    for i in 0..8 {
        session(&mut t, &format!("echo {}", i), &format!("{}\r\n", i), i % 3);
    }
    t.feed(b"\x1b]133;A\x07$ ");
    let want: Vec<_> = t.blocks().iter().map(|b| (b.exit, b.end_row - b.output_row)).collect();
    let snap = t.snapshot_vt(1000);
    let mut u = Terminal::new(30, 6, 200);
    u.feed(&snap);
    let got: Vec<_> = u.blocks().iter().map(|b| (b.exit, b.end_row - b.output_row)).collect();
    assert_eq!(got, want);
    assert_eq!(u.blocks().len(), 9);
    let b = u.blocks();
    assert_eq!(u.text_range(b[3].output_row as usize, 0, b[3].end_row as usize - 1, usize::MAX), "3");
}

#[test]
fn osc133_old_blocks_are_dropped_with_evicted_history() {
    let mut t = Terminal::new(20, 3, 5);
    for i in 0..20 {
        session(&mut t, "c", &format!("{}\r\n", i), 0);
    }
    let b = t.blocks();
    assert!(!b.is_empty() && b.len() <= 8);
    for x in &b {
        assert!(x.prompt_row < 8);
    }
}

#[test]
fn osc133_finish_events_are_live_only() {
    let mut t = Terminal::new(40, 6, 100);
    session(&mut t, "sleep 1", "", 0);
    session(&mut t, "false", "x\r\n", 2);
    let f = t.take_finished();
    assert_eq!(f.len(), 2);
    assert_eq!(f[1].exit, 2);
    assert!(f[1].cmd_row >= 0);
    assert!(t.take_finished().is_empty());
    // A snapshot replay (which carries explicit durations) must not look like new completions.
    let mut u = Terminal::new(40, 6, 100);
    u.feed(&t.snapshot_vt(100));
    assert!(u.take_finished().is_empty());
    assert_eq!(u.blocks().len(), 2);
}

// ------------------------------------------------------------------ reflow

fn all_text(t: &Terminal) -> String {
    t.text_range(0, 0, t.total_lines() - 1, usize::MAX)
}

#[test]
fn reflow_joins_and_splits_wrapped_lines() {
    let mut t = Terminal::new(10, 8, 100);
    t.feed(b"abcdefghijklmnopqrstuvwxyz");
    assert_eq!((t.row_text(0).as_str(), t.row_text(1).as_str(), t.row_text(2).as_str()), ("abcdefghij", "klmnopqrst", "uvwxyz"));
    t.resize(20, 8);
    assert_eq!((t.row_text(0).as_str(), t.row_text(1).as_str()), ("abcdefghijklmnopqrst", "uvwxyz"));
    assert_eq!(t.cursor(), (1, 6));
    t.resize(5, 8);
    assert_eq!(t.row_text(0), "abcde");
    assert_eq!(t.row_text(5), "z");
    // Round trip restores the original layout.
    t.resize(10, 8);
    assert_eq!((t.row_text(0).as_str(), t.row_text(1).as_str(), t.row_text(2).as_str()), ("abcdefghij", "klmnopqrst", "uvwxyz"));
    assert_eq!(t.cursor(), (2, 6));
}

#[test]
fn reflow_keeps_hard_breaks_and_cursor_character() {
    let mut t = Terminal::new(12, 6, 100);
    t.feed(b"one\r\ntwo\r\n$ echo hello");
    assert_eq!(t.cursor(), (2, 12 - 1 + 0).min((2, 11)));
    t.resize(8, 6);
    assert_eq!(t.row_text(0), "one");
    assert_eq!(t.row_text(1), "two");
    assert_eq!(t.row_text(2), "$ echo h");
    assert_eq!(t.row_text(3), "ello");
    assert_eq!(t.cursor(), (3, 4)); // right after "hello"
    t.feed(b"!");
    assert_eq!(t.row_text(3), "ello!");
}

#[test]
fn reflow_never_splits_wide_characters() {
    let mut t = Terminal::new(6, 4, 10);
    t.feed("ab你好c".as_bytes());
    t.resize(3, 4);
    let rows: Vec<String> = (0..4).map(|r| t.row_text(r)).collect();
    assert_eq!(rows, vec!["ab", "你", "好c", ""]);
    t.resize(8, 4);
    assert_eq!(t.row_text(0), "ab你好c");
}

#[test]
fn reflow_preserves_history_and_text() {
    let mut t = Terminal::new(20, 4, 500);
    for i in 0..40 {
        t.feed(format!("line {} {}\r\n", i, "x".repeat(i % 30)).as_bytes());
    }
    let before = all_text(&t).replace('\n', "");
    for w in [7, 33, 12, 80, 20, 3, 20] {
        t.resize(w, 4);
        assert_eq!(all_text(&t).replace('\n', ""), before, "width {}", w);
    }
    assert_eq!(t.cols(), 20);
}

#[test]
fn reflow_moves_command_blocks_with_their_lines() {
    let mut t = Terminal::new(30, 8, 200);
    session(&mut t, "echo", &format!("{}\r\nshort\r\n", "L".repeat(70)), 0);
    session(&mut t, "false", "", 1);
    t.feed(b"\x1b]133;A\x07$ ");
    let text = |t: &Terminal, b: &BlockInfo| t.text_range(b.output_row as usize, 0, b.end_row as usize - 1, usize::MAX);
    let b = t.blocks();
    let want = text(&t, &b[0]);
    for w in [10, 25, 90, 30] {
        t.resize(w, 8);
        let b = t.blocks();
        // The live prompt is wiped on resize (the shell repaints it) but its block stays for the repaint to fill.
        assert_eq!(b.len(), 3, "width {}", w);
        assert_eq!(text(&t, &b[0]), want, "width {}", w);
        assert_eq!(b[1].exit, 1);
    }
}

/// What zsh + Powerlevel10k send on SIGWINCH with a two-line prompt: the cursor is on the input line, so they go up
/// one row, clear below and repaint both lines. The terminal must leave the cursor where the shell expects it.
fn p10k_prompt(t: &mut Terminal, width: usize) {
    let gap = " ".repeat(width - 17); // "> PROMPT-TOP" + gap + "right" fills the row exactly
    t.feed(format!("\x1b[0m\x1b[?2004l\r\n\x1b[A> PROMPT-TOP{gap}right\r\n\\_ ").as_bytes());
}

#[test]
fn shell_repaint_after_resize_does_not_leave_stray_prompts() {
    let mut t = Terminal::new(60, 12, 200);
    t.feed(b"out-1\r\nout-2\r\nout-3\r\n");
    t.feed(b"\x1b]133;A\x07");
    p10k_prompt(&mut t, 60);
    let mut seed = 7u64;
    for _ in 0..40 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let w = 30 + ((seed >> 33) % 70) as usize;
        t.resize(w, 12);
        // The shell notices SIGWINCH and repaints from the input line.
        t.feed(b"\r\r\x1b[A\x1b[0m\x1b[J");
        p10k_prompt(&mut t, w);
    }
    t.resize(60, 12);
    t.feed(b"\r\r\x1b[A\x1b[0m\x1b[J");
    p10k_prompt(&mut t, 60);
    let screen = t.screen_text();
    assert_eq!(screen.matches("PROMPT-TOP").count(), 1, "stray prompts:\n{}", screen);
    assert_eq!(screen.matches("\\_").count(), 1, "stray prompts:\n{}", screen);
    for l in ["out-1", "out-2", "out-3"] {
        assert!(screen.contains(l), "{} was eaten:\n{}", l, screen);
    }
}

#[test]
fn reflow_with_alternate_screen_and_tiny_sizes_is_safe() {
    let mut t = Terminal::new(20, 6, 50);
    t.feed(b"hello world this is a long primary line\r\nprompt$ ");
    t.feed(b"\x1b[?1049h\x1b[Halt screen");
    t.resize(9, 4);
    t.resize(1, 1);
    t.resize(30, 7);
    t.feed(b"\x1b[?1049l");
    assert!(all_text(&t).contains("hello"));
    let (r, c) = t.cursor();
    assert!(r < t.rows() && c < t.cols());
}

#[test]
fn reflow_random_resizes_do_not_panic() {
    let mut seed = 0x1234_5678_9abc_def0u64;
    let mut rnd = |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    let mut t = Terminal::new(40, 12, 80);
    let pieces: [&[u8]; 8] = [b"hello ", "世界 ".as_bytes(), b"\r\n", b"\x1b[31mred\x1b[0m ", b"\x1b[2K", b"\x1b[?1049h", b"\x1b[?1049l", b"\x1b]133;A\x07"];
    for _ in 0..3000 {
        match rnd(4) {
            0 => t.resize(1 + rnd(120) as usize, 1 + rnd(30) as usize),
            _ => {
                for _ in 0..rnd(20) {
                    t.feed(pieces[rnd(8) as usize]);
                }
            }
        }
        let (r, c) = t.cursor();
        assert!(r < t.rows() && c < t.cols());
        let _ = t.blocks();
    }
}

#[test]
fn clearing_the_screen_drops_blocks_on_it_but_keeps_scrolled_ones() {
    let mut t = Terminal::new(30, 4, 100);
    for i in 0..6 {
        session(&mut t, "cmd", &format!("{}\r\n", i), 1); // scrolls: older blocks end up in history
    }
    assert!(t.blocks().len() >= 5);
    t.feed(b"\x1b]133;A\x07$ clear");
    t.feed(b"\r\n\x1b]133;C\x07\x1b[H\x1b[2J");
    let b = t.blocks();
    assert!(b.iter().all(|x| x.end_row >= 0 && (x.end_row as usize) < t.bottom_base()), "{:?}", b);
    t.feed(b"\x1b]133;D;0\x07\x1b]133;A\x07$ ");
    assert_eq!(t.blocks().last().unwrap().output_row, -1); // only the fresh prompt remains on screen
    t.feed(b"\x1b[3J");
    assert!(t.blocks().iter().all(|x| x.end_row >= 0));
}

// ------------------------------------------------------------------ command text and folding

#[test]
fn osc133_c_carries_the_command_text() {
    let mut t = Terminal::new(40, 10, 100);
    t.feed(b"\x1b]133;A\x07$ ls -la; echo hi\r\n\x1b]133;C;cmd=ls -la; echo hi\x07out\r\n\x1b]133;D;0\x07");
    let b = t.blocks();
    assert_eq!(t.block_command(b[0].prompt_row as usize).as_deref(), Some("ls -la; echo hi"));
    // The command survives a snapshot round trip.
    let mut t2 = Terminal::new(40, 10, 100);
    t2.feed(&t.snapshot_vt(100));
    let b2 = t2.blocks();
    assert_eq!(t2.block_command(b2[0].prompt_row as usize).as_deref(), Some("ls -la; echo hi"));
}

#[test]
fn osc_9_4_reports_progress_and_clears_it() {
    let mut t = Terminal::new(40, 10, 100);
    assert_eq!(t.progress(), (0, 0));
    t.feed(b"\x1b]9;4;1;42\x07");
    assert_eq!(t.progress(), (1, 42));
    assert!(t.take_progress_changed());
    assert!(!t.take_progress_changed(), "reported once");
    t.feed(b"\x1b]9;4;1;42\x07");
    assert!(!t.take_progress_changed(), "unchanged value is not a change");
    t.feed(b"\x1b]9;4;1;250\x1b\\");
    assert_eq!(t.progress(), (1, 100), "percent is clamped");
    t.feed(b"\x1b]9;4;3\x07");
    assert_eq!(t.progress(), (3, 0));
    t.feed(b"\x1b]9;4;2;70\x07");
    assert_eq!(t.progress(), (2, 70));
    t.feed(b"\x1b]9;4;9;5\x07");
    assert_eq!(t.progress(), (2, 70), "unknown states are ignored");
    t.feed(b"\x1b]9;4;0\x07");
    assert_eq!(t.progress(), (0, 0));
    // A notification OSC 9 is not progress, and a finished command clears a dangling bar.
    t.feed(b"\x1b]9;hello\x07");
    assert_eq!(t.progress(), (0, 0));
    session(&mut t, "brew", "", 0);
    t.feed(b"\x1b]9;4;1;10\x07");
    t.feed(b"\x1b]133;A\x07$ x\r\n\x1b]133;C\x07\x1b]9;4;1;50\x07\x1b]133;D;0\x07");
    assert_eq!(t.progress(), (0, 0));
}
