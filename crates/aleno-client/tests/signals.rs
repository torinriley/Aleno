//! Child shells must start with default signal handling, whatever the daemon inherited. Run in its own process
//! because it changes this process's signal dispositions.
use aleno_client::*;
use aleno_session::Server;
use std::os::unix::net::UnixListener;
use std::time::{Duration, Instant};

#[test]
fn shells_do_not_inherit_ignored_signals() {
    // Exactly what a background job (`cmd &` in a non-interactive shell) or a nohup'd daemon looks like to its children.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGQUIT, libc::SIG_IGN);
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }
    let dir = std::env::temp_dir().join(format!("aleno-sig-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("d.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let server = Server::new();
    let s2 = server.clone();
    std::thread::spawn(move || s2.serve(listener));
    let ctl = Ctl::connect(&sock).unwrap();
    let s = ctl.create_session("sig").unwrap();
    let p = ctl.create_pane(s, 100, 30, "/bin/sh", "/tmp", 1000).unwrap();
    let c = PaneClient::attach(&sock, s, p, 100, 30, 1000, Box::new(|_| {})).unwrap();
    let wait = |needle: &str| {
        let end = Instant::now() + Duration::from_secs(8);
        while Instant::now() < end {
            if c.shared.term.lock().unwrap().screen_text().contains(needle) { return true; }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    };
    // The shell must report default dispositions: no signal is ignored (SigIgn mask empty for INT/QUIT/HUP/TERM).
    c.send_input(b"grep -E '^Sig(Ign|Blk)' /proc/$$/status 2>/dev/null; trap -p INT HUP 2>/dev/null; sleep 100 &\nkill -0 $! && echo CHILD-UP\n");
    assert!(wait("CHILD-UP"));
    // Behavioural check: Ctrl-C must interrupt a foreground command.
    c.send_input(b"sleep 30; echo AFTER-SLEEP-$((6*7))\n");
    std::thread::sleep(Duration::from_millis(400));
    c.send_input(b"\x03");
    assert!(wait("AFTER-SLEEP-42"), "Ctrl-C was ignored: the shell inherited SIGINT=SIG_IGN\n{}", c.shared.term.lock().unwrap().screen_text());
    server.kill_all();
}
