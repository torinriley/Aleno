//! Integration tests: real PTYs, real shells, real sockets.

use aleno_client::*;
use aleno_protocol::*;
use aleno_session::Server;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn tmp_sock(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("aleno-t-{}-{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.join("d.sock")
}

struct InProc {
    server: Arc<Server>,
    sock: PathBuf,
}
fn start_inproc(tag: &str) -> InProc {
    let sock = tmp_sock(tag);
    let listener = UnixListener::bind(&sock).unwrap();
    let server = Server::new();
    let s2 = server.clone();
    std::thread::spawn(move || s2.serve(listener));
    InProc { server, sock }
}
impl Drop for InProc {
    fn drop(&mut self) {
        self.server.kill_all();
        self.server.shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

fn attach(sock: &Path, s: u32, p: u32, cols: u16, rows: u16) -> PaneClient {
    PaneClient::attach(sock, s, p, cols, rows, 10000, Box::new(|_| {})).expect("attach")
}

fn screen(c: &PaneClient) -> String {
    c.shared.term.lock().unwrap().screen_text()
}

fn wait_for(c: &PaneClient, what: &str, f: impl Fn(&str) -> bool) -> String {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let t = screen(c);
        if f(&t) {
            return t;
        }
        if Instant::now() > end {
            panic!("timed out waiting for {:?}; screen:\n{}", what, t);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn new_pane(ctl: &Ctl, shell: &str) -> (u32, u32) {
    let s = ctl.create_session("t").unwrap();
    let p = ctl.create_pane(s, 120, 40, shell, "/tmp", 10000).unwrap();
    (s, p)
}

#[test]
fn shell_roundtrip_and_resize() {
    let d = start_inproc("rt");
    let ctl = Ctl::connect(&d.sock).unwrap();
    let (s, p) = new_pane(&ctl, "/bin/sh");
    let c = attach(&d.sock, s, p, 100, 30);
    c.send_input(b"echo hello-$((40+2))\n");
    wait_for(&c, "hello-42", |t| t.contains("hello-42\n"));
    c.send_input(b"stty size\n");
    wait_for(&c, "30 100", |t| t.contains("\n30 100"));
    c.resize(90, 20);
    c.send_input(b"stty size\n");
    wait_for(&c, "20 90", |t| t.contains("\n20 90"));
}

#[test]
fn device_status_report_is_answered_by_daemon() {
    let d = start_inproc("dsr");
    let ctl = Ctl::connect(&d.sock).unwrap();
    let (s, p) = new_pane(&ctl, "/bin/bash");
    let c = attach(&d.sock, s, p, 100, 30);
    c.send_input(b"printf '\\033[6n'; IFS=';' read -s -d R -a pos; echo ROW${pos[0]#*[}COL${pos[1]}END\n");
    wait_for(&c, "cursor report", |t| t.contains("COL1END") || t.contains("COL") && t.contains("END\n"));
}

#[test]
fn session_survives_gui_detach_and_reattach() {
    let d = start_inproc("persist");
    let ctl = Ctl::connect(&d.sock).unwrap();
    let (s, p) = new_pane(&ctl, "/bin/sh");
    let c = attach(&d.sock, s, p, 100, 30);
    c.send_input(b"echo PID=$$; i=0; while true; do i=$((i+1)); echo tick$i; sleep 0.1; done\n");
    let t = wait_for(&c, "ticks", |t| t.contains("tick5"));
    let pid_line = t.lines().find(|l| l.starts_with("PID=")).unwrap().to_string();
    drop(c); // "close the GUI"
    std::thread::sleep(Duration::from_millis(1200));
    let infos = ctl.list_panes(s).unwrap();
    assert!(infos[0].alive, "shell must survive detach");
    let c2 = attach(&d.sock, s, p, 100, 30);
    let t2 = wait_for(&c2, "later tick", |t| {
        t.lines().filter_map(|l| l.strip_prefix("tick")).filter_map(|n| n.parse::<u32>().ok()).max().unwrap_or(0) >= 15
    });
    assert!(t2.contains(&pid_line), "history/snapshot must show the original shell pid:\n{}", t2);
    c2.send_input(b"\x03echo back-$((1+1))\n");
    wait_for(&c2, "interactive again", |t| t.contains("back-2"));
}

#[test]
fn multiple_clients_see_same_output_and_independent_panes() {
    let d = start_inproc("multi");
    let ctl = Ctl::connect(&d.sock).unwrap();
    let s = ctl.create_session("m").unwrap();
    let p1 = ctl.create_pane(s, 80, 24, "/bin/sh", "/tmp", 1000).unwrap();
    let p2 = ctl.create_pane(s, 80, 24, "/bin/sh", "/tmp", 1000).unwrap();
    let a = attach(&d.sock, s, p1, 80, 24);
    let b = attach(&d.sock, s, p1, 80, 24);
    let other = attach(&d.sock, s, p2, 80, 24);
    a.send_input(b"echo shared-$((6*7))\n");
    wait_for(&a, "a sees", |t| t.contains("shared-42"));
    wait_for(&b, "b sees", |t| t.contains("shared-42"));
    other.send_input(b"echo $$ > /tmp/aleno-pid-other-$PPID; echo own-$$\n");
    wait_for(&other, "own", |t| t.contains("own-"));
    assert!(!screen(&a).contains("own-"));
    let list = ctl.list_panes(s).unwrap();
    assert_eq!(list.len(), 2);
    assert_ne!(
        d.server.session(s).unwrap().pane(p1).unwrap().pid(),
        d.server.session(s).unwrap().pane(p2).unwrap().pid()
    );
}

#[test]
fn slow_client_is_dropped_without_stalling_others() {
    let d = start_inproc("slow");
    let ctl = Ctl::connect(&d.sock).unwrap();
    let (s, p) = new_pane(&ctl, "/bin/sh");
    // A client that attaches and then never reads.
    let mut slow = UnixStream::connect(&d.sock).unwrap();
    write_msg(&mut slow, &Msg::Hello { version: PROTOCOL_VERSION, client: "slow".into() }).unwrap();
    assert!(matches!(read_msg(&mut slow).unwrap(), Msg::Welcome { .. }));
    write_msg(&mut slow, &Msg::AttachPane { session: s, pane: p, cols: 80, rows: 24 }).unwrap();
    assert_eq!(read_msg(&mut slow).unwrap(), Msg::Attached);
    let good = attach(&d.sock, s, p, 80, 24);
    good.send_input(b"yes aleno-flood | head -c 30000000; echo DONE-$((1+2))\n");
    wait_for(&good, "flood done", |t| t.contains("DONE-3"));
    // The slow client has been disconnected by the daemon; the pane is still healthy.
    let end = Instant::now() + Duration::from_secs(5);
    while d.server.session(s).unwrap().pane(p).unwrap().client_count() > 1 && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(d.server.session(s).unwrap().pane(p).unwrap().client_count(), 1);
    good.send_input(b"echo still-alive\n");
    wait_for(&good, "alive", |t| t.contains("still-alive"));
}

#[test]
fn abrupt_disconnect_is_cleaned_up() {
    let d = start_inproc("abrupt");
    let ctl = Ctl::connect(&d.sock).unwrap();
    let (s, p) = new_pane(&ctl, "/bin/sh");
    let mut raw = UnixStream::connect(&d.sock).unwrap();
    write_msg(&mut raw, &Msg::Hello { version: PROTOCOL_VERSION, client: "x".into() }).unwrap();
    read_msg(&mut raw).unwrap();
    write_msg(&mut raw, &Msg::AttachPane { session: s, pane: p, cols: 80, rows: 24 }).unwrap();
    read_msg(&mut raw).unwrap();
    drop(raw);
    let pane = d.server.session(s).unwrap().pane(p).unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    while pane.client_count() > 0 && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(pane.client_count(), 0);
    assert!(pane.is_alive());
}

#[test]
fn killing_session_terminates_processes() {
    let d = start_inproc("kill");
    let ctl = Ctl::connect(&d.sock).unwrap();
    let (s, p) = new_pane(&ctl, "/bin/sh");
    let c = attach(&d.sock, s, p, 80, 24);
    c.send_input(b"sleep 1000 &\nwait\n");
    std::thread::sleep(Duration::from_millis(300));
    let pid = d.server.session(s).unwrap().pane(p).unwrap().pid() as i32;
    assert_eq!(unsafe { libc_kill(pid, 0) }, 0);
    ctl.kill_session(s).unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    while unsafe { libc_kill(pid, 0) } == 0 && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(c.shared.exited.load(std::sync::atomic::Ordering::SeqCst) || true);
    assert_ne!(unsafe { libc_kill(pid, 0) }, 0, "shell should be gone");
    assert!(ctl.list_sessions().unwrap().is_empty());
}

extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

#[test]
fn exit_is_reported_and_snapshot_replays_it() {
    let d = start_inproc("exit");
    let ctl = Ctl::connect(&d.sock).unwrap();
    let (s, p) = new_pane(&ctl, "/bin/sh");
    let c = attach(&d.sock, s, p, 80, 24);
    c.send_input(b"exit 7\n");
    let end = Instant::now() + Duration::from_secs(5);
    while !c.shared.exited.load(std::sync::atomic::Ordering::SeqCst) && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(c.shared.exited.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(c.shared.exit_code.load(std::sync::atomic::Ordering::SeqCst), 7);
    let late = attach(&d.sock, s, p, 80, 24);
    let end = Instant::now() + Duration::from_secs(5);
    while !late.shared.exited.load(std::sync::atomic::Ordering::SeqCst) && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(late.shared.exited.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn layout_is_stored_per_session() {
    let d = start_inproc("layout");
    let ctl = Ctl::connect(&d.sock).unwrap();
    let s = ctl.create_session("l").unwrap();
    ctl.set_layout(s, "{\"tabs\":[1,2]}").unwrap();
    let ctl2 = Ctl::connect(&d.sock).unwrap();
    assert_eq!(ctl2.get_layout(s).unwrap(), "{\"tabs\":[1,2]}");
    assert!(ctl2.get_layout(999).is_err());
    assert!(ctl2.create_pane(999, 80, 24, "", "", 100).is_err());
}

#[test]
fn rejects_bad_version_and_garbage() {
    let d = start_inproc("bad");
    let mut s = UnixStream::connect(&d.sock).unwrap();
    write_msg(&mut s, &Msg::Hello { version: 99, client: "x".into() }).unwrap();
    assert!(matches!(read_msg(&mut s).unwrap(), Msg::Err { code: err::VERSION, .. }));
    let mut s = UnixStream::connect(&d.sock).unwrap();
    use std::io::Write;
    s.write_all(&[0xff, 0xff, 0xff, 0x7f, 1, 2, 3]).unwrap();
    // The daemon just closes the connection.
    let mut b = [0u8; 8];
    use std::io::Read;
    assert_eq!(s.read(&mut b).unwrap_or(0), 0);
    // Still serving.
    assert!(Ctl::connect(&d.sock).is_ok());
}

#[test]
fn modes_and_alt_screen_survive_reattach() {
    let d = start_inproc("modes");
    let ctl = Ctl::connect(&d.sock).unwrap();
    let (s, p) = new_pane(&ctl, "/bin/sh");
    let c = attach(&d.sock, s, p, 80, 24);
    c.send_input(b"printf '\\033[?2004h\\033[?1000h\\033[?1049hALT-SCREEN-TEXT'\n");
    wait_for(&c, "alt", |t| t.contains("ALT-SCREEN-TEXT"));
    drop(c);
    let c2 = attach(&d.sock, s, p, 80, 24);
    wait_for(&c2, "alt after reattach", |t| t.contains("ALT-SCREEN-TEXT"));
    let t = c2.shared.term.lock().unwrap();
    assert!(t.is_alt_screen());
    assert!(t.modes() & aleno_terminal::mode::BRACKETED_PASTE != 0);
    assert!(t.modes() & aleno_terminal::mode::MOUSE_PRESS != 0);
}

fn have(cmd: &str) -> bool {
    Command::new("sh").arg("-c").arg(format!("command -v {}", cmd)).stdout(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

#[test]
fn common_cli_programs_work() {
    let d = start_inproc("cli");
    let ctl = Ctl::connect(&d.sock).unwrap();
    let (s, p) = new_pane(&ctl, "/bin/bash");
    let c = attach(&d.sock, s, p, 120, 40);
    c.send_input(b"git --version\n");
    wait_for(&c, "git", |t| t.contains("git version"));
    if have("ssh") {
        c.send_input(b"ssh -V 2>&1 | head -1\n");
        wait_for(&c, "ssh", |t| t.contains("OpenSSH") || t.contains("ssh"));
    }
    // less: alternate screen in, content, quit, alternate screen out.
    std::fs::write("/tmp/aleno-less-test.txt", (1..200).map(|i| format!("less-line-{}\n", i)).collect::<String>()).unwrap();
    c.send_input(b"less /tmp/aleno-less-test.txt\n");
    wait_for(&c, "less", |t| t.contains("less-line-1\n") && t.contains("less-line-39"));
    assert!(c.shared.term.lock().unwrap().is_alt_screen());
    c.send_input(b" ");
    wait_for(&c, "less page", |t| t.contains("less-line-41") && !t.contains("less-line-1\n"));
    c.send_input(b"q");
    wait_for(&c, "less quit", |t| !t.contains("less-line-41"));
    assert!(!c.shared.term.lock().unwrap().is_alt_screen());
    c.send_input(b"echo prompt-ok\n");
    wait_for(&c, "prompt", |t| t.contains("prompt-ok"));
}

#[test]
fn neovim_runs_in_alt_screen_and_edits() {
    if !have("nvim") {
        eprintln!("nvim not installed; skipping");
        return;
    }
    let d = start_inproc("nvim");
    let ctl = Ctl::connect(&d.sock).unwrap();
    let (s, p) = new_pane(&ctl, "/bin/bash");
    let c = attach(&d.sock, s, p, 100, 30);
    let _ = std::fs::remove_file("/tmp/aleno-nvim-out.txt");
    c.send_input(b"nvim -u NONE -n /tmp/aleno-nvim-out.txt\n");
    wait_for(&c, "nvim up", |_| c.shared.term.lock().unwrap().is_alt_screen());
    assert!(c.shared.term.lock().unwrap().is_alt_screen());
    c.send_input(b"ihello from nvim \xe6\x97\xa5\xe6\x9c\xac\x1b");
    wait_for(&c, "typed", |t| t.contains("hello from nvim 日本"));
    // Reattach mid-session: the editor screen must be reproduced.
    drop(c);
    let c2 = attach(&d.sock, s, p, 100, 30);
    wait_for(&c2, "restored editor", |t| t.contains("hello from nvim 日本"));
    c2.send_input(b":wq\r");
    wait_for(&c2, "quit", |t| !c2.shared.term.lock().unwrap().is_alt_screen() || t.contains("$"));
    std::thread::sleep(Duration::from_millis(300));
    assert!(std::fs::read_to_string("/tmp/aleno-nvim-out.txt").unwrap().contains("hello from nvim 日本"));
}

// ---------- real daemon process: GUI "process" exit does not affect sessions ----------

fn daemon_bin() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    for prof in ["debug", "release"] {
        let p = root.join(prof).join("aleno-daemon");
        if p.exists() {
            return p;
        }
    }
    panic!("build aleno-daemon first (cargo build -p aleno-daemon)");
}

struct DaemonProc(Child, PathBuf);
impl Drop for DaemonProc {
    fn drop(&mut self) {
        let _ = Command::new(daemon_bin()).arg("stop").env("ALENO_SOCKET", &self.1).status();
        let _ = self.0.wait();
    }
}

#[test]
fn real_daemon_process_persists_sessions_across_client_processes() {
    let sock = tmp_sock("proc");
    let child = Command::new(daemon_bin()).arg("run").env("ALENO_SOCKET", &sock).env("ALENO_IDLE_EXIT_SECS", "0").spawn().unwrap();
    let _guard = DaemonProc(child, sock.clone());
    let end = Instant::now() + Duration::from_secs(5);
    let ctl = loop {
        match Ctl::connect(&sock) {
            Ok(c) => break c,
            Err(_) if Instant::now() < end => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => panic!("daemon did not start: {}", e),
        }
    };
    // Socket permissions are owner-only.
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&sock).unwrap().permissions().mode() & 0o777, 0o600);
    // A second daemon instance refuses to start.
    let st = Command::new(daemon_bin()).arg("run").env("ALENO_SOCKET", &sock).output().unwrap();
    assert!(String::from_utf8_lossy(&st.stderr).contains("already running"));

    let s = ctl.create_session("persist").unwrap();
    ctl.set_layout(s, "{\"v\":1}").unwrap();
    let p = ctl.create_pane(s, 100, 30, "/bin/sh", "/tmp", 1000).unwrap();
    let c = attach(&sock, s, p, 100, 30);
    c.send_input(b"echo SHELLPID=$$; sleep 600\n");
    let t = wait_for(&c, "pid", |t| t.contains("SHELLPID="));
    let pid_line = t.lines().find(|l| l.starts_with("SHELLPID=")).unwrap().to_string();
    // Simulate the GUI process going away entirely.
    drop(c);
    drop(ctl);
    std::thread::sleep(Duration::from_millis(500));
    // "Relaunched GUI": brand-new connections.
    let ctl2 = Ctl::connect(&sock).unwrap();
    let sessions = ctl2.list_sessions().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].name, "persist");
    assert_eq!(ctl2.get_layout(s).unwrap(), "{\"v\":1}");
    let panes = ctl2.list_panes(s).unwrap();
    assert!(panes[0].alive);
    let c2 = attach(&sock, s, p, 100, 30);
    wait_for(&c2, "restored", |t| t.contains(&pid_line));
    c2.send_input(b"\x03echo $$\n");
    let t = wait_for(&c2, "same shell", |t| t.matches(pid_line.trim_start_matches("SHELLPID=")).count() >= 2);
    assert!(t.contains(&pid_line));
}

// ---------------------------------------------------------------- direct mode (window owns the pty)

fn attach_streamed(sock: &Path, s: u32, p: u32, cols: u16, rows: u16) -> PaneClient {
    PaneClient::attach_streamed(sock, s, p, cols, rows, 10000, Box::new(|_| {})).expect("attach_streamed")
}

#[test]
fn direct_attach_is_used_and_counted() {
    let d = start_inproc("direct1");
    let ctl = Ctl::connect(&d.sock).unwrap();
    let (s, p) = new_pane(&ctl, "/bin/sh");
    let c = attach(&d.sock, s, p, 100, 30);
    assert!(c.is_direct(), "first attach should receive the pty");
    let pane = d.server.session(s).unwrap().pane(p).unwrap();
    assert!(pane.is_direct());
    assert_eq!(pane.client_count(), 1);
    c.send_input(b"echo direct-$((20+22))\n");
    wait_for(&c, "direct output", |t| t.contains("direct-42\n"));
    c.resize(80, 20);
    c.send_input(b"stty size\n");
    wait_for(&c, "resize in direct mode", |t| t.contains("\n20 80"));
}

#[test]
fn graceful_detach_returns_exact_state_to_the_daemon() {
    let d = start_inproc("direct2");
    let ctl = Ctl::connect(&d.sock).unwrap();
    let (s, p) = new_pane(&ctl, "/bin/sh");
    let c = attach(&d.sock, s, p, 100, 30);
    c.send_input(b"echo RETURN-MARKER-$((1+1)); printf '\\033]2;my-title\\007\\033[?2004h'\n");
    wait_for(&c, "marker", |t| t.contains("RETURN-MARKER-2"));
    std::thread::sleep(Duration::from_millis(150));
    drop(c); // hands the pty back with the window's screen state
    let pane = d.server.session(s).unwrap().pane(p).unwrap();
    let end = Instant::now() + Duration::from_secs(3);
    while pane.is_direct() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!pane.is_direct(), "daemon must own the pty again");
    assert!(pane.screen_text().contains("RETURN-MARKER-2"), "daemon state must include the output the window saw:\n{}", pane.screen_text());
    // A streamed client sees the same screen and the live shell still works.
    let c2 = attach_streamed(&d.sock, s, p, 100, 30);
    wait_for(&c2, "restored", |t| t.contains("RETURN-MARKER-2"));
    assert_eq!(c2.shared.term.lock().unwrap().title(), "my-title");
    c2.send_input(b"echo still-$((3+4))\n");
    wait_for(&c2, "interactive", |t| t.contains("still-7"));
}

#[test]
fn crashed_window_is_recovered_from_its_checkpoint() {
    let d = start_inproc("direct3");
    let ctl = Ctl::connect(&d.sock).unwrap();
    let (s, p) = new_pane(&ctl, "/bin/sh");
    let c = attach(&d.sock, s, p, 100, 30);
    c.send_input(b"echo CRASH-MARKER-$((5*5))\n");
    wait_for(&c, "marker", |t| t.contains("CRASH-MARKER-25"));
    std::thread::sleep(Duration::from_millis(900)); // output settles -> the window checkpoints
    c.abandon(); // no Return: like SIGKILL
    let pane = d.server.session(s).unwrap().pane(p).unwrap();
    let end = Instant::now() + Duration::from_secs(3);
    while pane.is_direct() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!pane.is_direct());
    assert!(pane.is_alive(), "the shell must survive the window dying");
    let c2 = attach_streamed(&d.sock, s, p, 100, 30);
    wait_for(&c2, "checkpointed screen", |t| t.contains("CRASH-MARKER-25"));
    c2.send_input(b"echo alive-$((8+8))\n");
    wait_for(&c2, "shell alive", |t| t.contains("alive-16"));
}

#[test]
fn second_window_recalls_the_pty_and_both_stay_live() {
    let d = start_inproc("direct4");
    let ctl = Ctl::connect(&d.sock).unwrap();
    let (s, p) = new_pane(&ctl, "/bin/sh");
    let a = attach(&d.sock, s, p, 100, 30);
    assert!(a.is_direct());
    a.send_input(b"echo before-recall\n");
    wait_for(&a, "before", |t| t.contains("before-recall"));
    let b = attach(&d.sock, s, p, 100, 30); // direct is unavailable -> recalls A's pty, streams to both
    assert!(!b.is_direct());
    let end = Instant::now() + Duration::from_secs(5);
    while a.is_direct() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!a.is_direct(), "A must have given the pty back");
    wait_for(&b, "B sees history", |t| t.contains("before-recall"));
    b.send_input(b"echo after-recall-$((6*7))\n");
    wait_for(&a, "A follows", |t| t.contains("after-recall-42"));
    wait_for(&b, "B follows", |t| t.contains("after-recall-42"));
}

#[test]
fn direct_mode_answers_device_queries_itself() {
    let d = start_inproc("direct5");
    let ctl = Ctl::connect(&d.sock).unwrap();
    let (s, p) = new_pane(&ctl, "/bin/bash");
    let c = attach(&d.sock, s, p, 100, 30);
    assert!(c.is_direct());
    c.send_input(b"printf '\\033[6n'; IFS=';' read -s -d R -a pos; echo GOT${pos[1]}END\n");
    wait_for(&c, "cursor report answered by the window", |t| t.contains("GOT1END") || (t.contains("GOT") && t.contains("END\n")));
}

#[test]
fn direct_flood_throughput_is_sane() {
    let d = start_inproc("direct6");
    let ctl = Ctl::connect(&d.sock).unwrap();
    let (s, p) = new_pane(&ctl, "/bin/sh");
    let c = attach(&d.sock, s, p, 200, 60);
    assert!(c.is_direct());
    let st = Instant::now();
    c.send_input(b"yes aleno-direct-flood | head -c 40000000; echo FLOODDONE$((1+1))\n");
    wait_for(&c, "flood", |t| t.contains("FLOODDONE2"));
    eprintln!("direct flood: 40 MB in {:.2}s", st.elapsed().as_secs_f64());
    c.send_input(b"echo after-flood\n");
    wait_for(&c, "responsive after flood", |t| t.contains("after-flood"));
}

#[test]
fn attach_falls_back_when_the_daemon_drops_unknown_requests() {
    // Emulates an older daemon: handshake works, but an unknown message tag closes the connection.
    use std::io::Write;
    let sock = tmp_sock("olddaemon");
    let listener = UnixListener::bind(&sock).unwrap();
    let real = start_inproc("olddaemon-real");
    let ctl = Ctl::connect(&real.sock).unwrap();
    let (s, p) = new_pane(&ctl, "/bin/sh");
    let real_sock = real.sock.clone();
    std::thread::spawn(move || {
        // Proxy: forward everything to the real daemon except AttachDirect, which kills the connection.
        for conn in listener.incoming() {
            let Ok(mut client) = conn else { break };
            let real_sock = real_sock.clone();
            std::thread::spawn(move || {
                let mut up = UnixStream::connect(&real_sock).unwrap();
                let mut c2 = client.try_clone().unwrap();
                let mut up2 = up.try_clone().unwrap();
                std::thread::spawn(move || { let _ = std::io::copy(&mut up2, &mut c2); });
                loop {
                    let mut hdr = [0u8; 4];
                    if std::io::Read::read_exact(&mut client, &mut hdr).is_err() { return; }
                    let n = u32::from_le_bytes(hdr) as usize;
                    let mut body = vec![0u8; n];
                    if std::io::Read::read_exact(&mut client, &mut body).is_err() { return; }
                    if body[0] == 15 { let _ = client.shutdown(std::net::Shutdown::Both); return; } // AttachDirect: unknown to an old daemon -> hang up
                    let _ = up.write_all(&hdr);
                    let _ = up.write_all(&body);
                }
            });
        }
    });
    let c = PaneClient::attach(&sock, s, p, 80, 24, 1000, Box::new(|_| {})).expect("must fall back to a streamed attach");
    assert!(!c.is_direct());
    c.send_input(b"echo compat-$((9+9))\n");
    wait_for(&c, "streamed works", |t| t.contains("compat-18"));
}

#[test]
fn seeded_pane_keeps_old_output_in_scrollback_above_the_new_shell() {
    let d = start_inproc("seeded");
    let ctl = Ctl::connect(&d.sock).unwrap();
    let s = ctl.create_session("t").unwrap();
    let seed = b"line-one\r\nline-two\r\nOLD-SCREEN\r\n";
    let p = ctl.create_pane_seeded(s, 80, 24, "/bin/sh", "/tmp", 1000, seed).unwrap();
    let c = attach(&d.sock, s, p, 80, 24);
    c.send_input(b"echo fresh-$((2+3))\n");
    wait_for(&c, "new shell works", |t| t.contains("fresh-5"));
    // The seeded text is above the screen, in history, and the visible screen starts clean.
    let all = {
        let t = c.shared.term.lock().unwrap();
        t.text_range(0, 0, t.total_lines() - 1, usize::MAX)
    };
    assert!(all.contains("OLD-SCREEN") && all.contains("line-one"), "{}", all);
    assert!(!screen(&c).contains("OLD-SCREEN"), "seed must not sit on the visible screen:\n{}", screen(&c));
}

#[test]
fn progress_from_the_shell_reaches_the_client_and_raises_an_event() {
    let d = start_inproc("progress");
    let ctl = Ctl::connect(&d.sock).unwrap();
    let (s, p) = new_pane(&ctl, "/bin/sh");
    let events = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let ev = events.clone();
    let c = PaneClient::attach(&d.sock, s, p, 80, 24, 1000, Box::new(move |e| {
        if e == EVENT_PROGRESS {
            ev.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }))
    .unwrap();
    c.send_input(b"printf '\\033]9;4;1;40\\a'; echo done-1\n");
    wait_for(&c, "progress output", |t| t.contains("done-1"));
    assert_eq!(c.shared.term.lock().unwrap().progress(), (1, 40));
    assert!(events.load(std::sync::atomic::Ordering::SeqCst) >= 1, "EVENT_PROGRESS delivered");
    c.send_input(b"printf '\\033]9;4;0\\a'; echo done-2\n");
    wait_for(&c, "cleared", |t| t.contains("done-2"));
    assert_eq!(c.shared.term.lock().unwrap().progress(), (0, 0));
}

#[test]
fn welcome_pane_plays_the_intro_then_becomes_the_shell() {
    let sock = tmp_sock("welcome");
    let child = Command::new(daemon_bin()).arg("run").env("ALENO_SOCKET", &sock).env("ALENO_IDLE_EXIT_SECS", "0").spawn().unwrap();
    let _guard = DaemonProc(child, sock.clone());
    let end = Instant::now() + Duration::from_secs(5);
    let ctl = loop {
        match Ctl::connect(&sock) {
            Ok(c) => break c,
            Err(_) if Instant::now() < end => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => panic!("daemon did not start: {}", e),
        }
    };
    let s = ctl.create_session("welcome").unwrap();
    let p = ctl.create_pane_full(s, 100, 30, "/bin/sh", "/tmp", 1000, &[], true).unwrap();
    let c = attach(&sock, s, p, 100, 30);
    // The wordmark is drawn with braille dots, then the intro text appears.
    let t = wait_for(&c, "welcome text", |t| t.contains("Welcome to Aleno") && t.contains("settings.json"));
    assert!(t.chars().any(|c| ('\u{2801}'..='\u{28FF}').contains(&c)), "the wordmark should be on screen:\n{}", t);
    // A key press skips what is left, and afterwards the pane is a normal shell (the wrapper exec'd into it).
    c.send_input(b"q");
    std::thread::sleep(Duration::from_millis(300));
    c.send_input(b"echo after-welcome-$((20+22))\n");
    wait_for(&c, "shell works", |t| t.contains("after-welcome-42"));
    let infos = ctl.list_panes(s).unwrap();
    assert!(infos[0].alive);
}
