//! Shell integration end to end: a real zsh/bash must report command blocks through OSC 133.
//! Own test binary: it sets HOME / ALENO_SOCKET for the whole process.

use aleno_client::*;
use aleno_session::Server;
use std::os::unix::net::UnixListener;
use std::time::{Duration, Instant};

fn run(shell: &str, rc_name: &str, rc: &str) {
    let dir = std::env::temp_dir().join(format!("aleno-blk-{}-{}", shell.replace('/', "_"), std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(rc_name), rc).unwrap();
    std::env::set_var("HOME", &dir);
    let sock = dir.join("d.sock");
    std::env::set_var("ALENO_SOCKET", &sock);
    let listener = UnixListener::bind(&sock).unwrap();
    let server = Server::new();
    let s2 = server.clone();
    std::thread::spawn(move || s2.serve(listener));

    let ctl = Ctl::connect(&sock).unwrap();
    let s = ctl.create_session("t").unwrap();
    let p = ctl.create_pane(s, 100, 30, shell, dir.to_str().unwrap(), 5000).unwrap();
    let finished_events = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let fe = finished_events.clone();
    let c = PaneClient::attach(&sock, s, p, 100, 30, 5000, Box::new(move |ev| {
        if ev == EVENT_COMMAND_FINISHED {
            fe.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }))
    .unwrap();

    let wait = |what: &str, f: &dyn Fn(&aleno_terminal::Terminal) -> bool| {
        let end = Instant::now() + Duration::from_secs(15);
        loop {
            {
                let t = c.shared.term.lock().unwrap();
                if f(&t) {
                    return;
                }
                if Instant::now() > end {
                    panic!("timeout waiting for {}: blocks={:?}\n{}", what, t.blocks(), t.screen_text());
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    };
    wait("first prompt", &|t| !t.blocks().is_empty());
    c.send_input(b"echo block-one; echo second-line\n");
    wait("block 1 done", &|t| t.blocks().iter().any(|b| b.end_row >= 0));
    c.send_input(b"(exit 3)\n");
    wait("block 2 done", &|t| t.blocks().iter().filter(|b| b.end_row >= 0).count() >= 2);

    assert!(finished_events.load(std::sync::atomic::Ordering::SeqCst) >= 2, "command-finished events were delivered");
    let mut t = c.shared.term.lock().unwrap();
    let fin = t.take_finished();
    assert_eq!(fin.len(), 2, "{:?}", fin);
    assert_eq!((fin[0].exit, fin[1].exit), (0, 3));
    let b: Vec<_> = t.blocks().into_iter().filter(|b| b.end_row >= 0).collect();
    assert_eq!(b[0].exit, 0, "{:?}", b);
    assert_eq!(b[1].exit, 3, "{:?}", b);
    // The shell integration reports the command line that was typed.
    assert_eq!(t.block_command(b[0].prompt_row as usize).as_deref(), Some("echo block-one; echo second-line"), "{:?}", b);
    assert_eq!(t.block_command(b[1].prompt_row as usize).as_deref(), Some("(exit 3)"), "{:?}", b);
    let out = t.text_range(b[0].output_row as usize, 0, b[0].end_row as usize - 1, usize::MAX);
    assert_eq!(out, "block-one\nsecond-line", "{:?}", b);
    assert!(t.cwd().contains(dir.file_name().unwrap().to_str().unwrap()), "cwd reported via OSC 7: {:?}", t.cwd());
    drop(t);
    server.kill_all();
    server.shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
}

#[test]
fn zsh_reports_blocks() {
    run("/bin/zsh", ".zshrc", "PROMPT='%~ %# '\nunsetopt zle 2>/dev/null; true\n");
}
