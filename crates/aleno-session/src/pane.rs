//! A pane = one PTY-backed shell process plus the authoritative terminal state for it.

use aleno_protocol::{write_msg, Msg};
use aleno_terminal::Terminal;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Bounded per-client queue (messages, each <= 64 KiB). A client that falls behind first applies
/// brief backpressure to the PTY reader (like a real terminal pacing `cat`); if it stays stalled
/// for `CLIENT_STALL_LIMIT` it is disconnected and can simply re-attach for a fresh snapshot.
const CLIENT_QUEUE: usize = 256;
/// How long the PTY reader will wait for a lagging client before disconnecting it.
const CLIENT_STALL_LIMIT: Duration = Duration::from_secs(2);
const READ_CHUNK: usize = 64 * 1024;
/// Largest history replayed to an attaching client.
const SNAPSHOT_HISTORY: usize = 2000;

pub struct SpawnOptions {
    pub cols: u16,
    pub rows: u16,
    pub shell: String,
    pub cwd: String,
    pub scrollback: usize,
    /// Output to replay onto the screen before the shell starts; it ends up in scrollback above the new prompt.
    pub seed: Vec<u8>,
}

#[derive(Clone)]
struct Sink {
    id: u64,
    tx: SyncSender<Msg>,
}

struct Inner {
    term: Terminal,
    sinks: Vec<Sink>,
    cols: u16,
    rows: u16,
}

/// State of a pane whose pty has been handed to a window ("direct mode").
#[derive(Default)]
struct DirectState {
    owner: Option<u64>,
    /// The owner's connection, used to send `Release` / `Exited`.
    stream: Option<UnixStream>,
    /// Latest screen state the owner stored; used if the owner disappears without returning the pty.
    checkpoint: Option<(u16, u16, Vec<u8>)>,
}

pub struct Pane {
    pub id: u32,
    master: OwnedFd,
    pid: u32,
    scrollback: usize,
    inner: Mutex<Inner>,
    /// Held by the pty reader for one read+parse cycle; `grant_direct` takes it to hand over at a clean boundary.
    gate: Mutex<()>,
    direct: Mutex<DirectState>,
    direct_cv: Condvar,
    direct_flag: AtomicBool,
    alive: AtomicBool,
    exit_code: AtomicI32,
    next_sink: AtomicU64,
}

/// Resolve the shell to run. Only absolute paths (or bare names found on a fixed set of system
/// directories) are accepted; the value is exec'd directly, never interpreted by a shell.
pub fn resolve_shell(requested: &str) -> String {
    let ok = |p: &str| {
        use std::os::unix::fs::PermissionsExt;
        std::path::Path::new(p).is_absolute()
            && std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    };
    if !requested.is_empty() && requested != "default" {
        if requested.contains('/') {
            if ok(requested) {
                return requested.to_string();
            }
        } else {
            for dir in ["/bin", "/usr/bin", "/usr/local/bin", "/opt/homebrew/bin"] {
                let p = format!("{}/{}", dir, requested);
                if ok(&p) {
                    return p;
                }
            }
        }
    }
    if let Ok(s) = std::env::var("SHELL") {
        if ok(&s) {
            return s;
        }
    }
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() && !(*pw).pw_shell.is_null() {
            let s = std::ffi::CStr::from_ptr((*pw).pw_shell).to_string_lossy().into_owned();
            if ok(&s) {
                return s;
            }
        }
    }
    "/bin/zsh".to_string()
}

/// Terminal I/O is latency-critical: run these threads at user-interactive QoS so macOS schedules them promptly
/// (and on performance cores) even though the daemon itself is a headless background process.
pub fn set_interactive_qos() {
    #[cfg(target_os = "macos")]
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
}

fn set_nonblocking(fd: RawFd) {
    unsafe {
        let fl = libc::fcntl(fd, libc::F_GETFL);
        libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK);
        libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
    }
}

impl Pane {
    /// A fresh terminal, preloaded with `opts.seed` pushed up into scrollback so the shell's first prompt lands on a clean screen.
    fn seeded_terminal(cols: u16, rows: u16, opts: &SpawnOptions) -> Terminal {
        let mut t = Terminal::new(cols as usize, rows as usize, opts.scrollback.clamp(0, 1_000_000));
        if !opts.seed.is_empty() {
            t.feed(&opts.seed);
            let _ = t.take_responses();
            t.feed(format!("\x1b[0m\x1b[?25h\x1b[{}S\x1b[H", rows).as_bytes());
        }
        t
    }

    pub fn spawn(id: u32, opts: &SpawnOptions) -> io::Result<Arc<Pane>> {
        let cols = opts.cols.clamp(1, 1000);
        let rows = opts.rows.clamp(1, 1000);
        let mut master: libc::c_int = -1;
        let mut slave: libc::c_int = -1;
        let ws = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
        let r = unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), &ws as *const _ as *mut _) };
        if r != 0 {
            return Err(io::Error::last_os_error());
        }
        let master = unsafe { OwnedFd::from_raw_fd(master) };
        let slave = unsafe { OwnedFd::from_raw_fd(slave) };
        // UTF-8 aware line discipline so that backspace erases whole characters.
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(slave.as_raw_fd(), &mut t) == 0 {
                t.c_iflag |= libc::IUTF8;
                libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &t);
            }
        }
        set_nonblocking(master.as_raw_fd());

        let shell = resolve_shell(&opts.shell);
        let name = std::path::Path::new(&shell).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let mut cmd = Command::new(&shell);
        cmd.arg0(format!("-{}", name)); // login shell
        cmd.env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .env("TERM_PROGRAM", "Aleno")
            .env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"))
            .env("SHELL", &shell);
        if std::env::var_os("LANG").is_none() && std::env::var_os("LC_ALL").is_none() {
            cmd.env("LANG", "en_US.UTF-8");
        }
        for k in ["ALENO_SOCKET", "__CFBundleIdentifier"] {
            cmd.env_remove(k);
        }
        if crate::integration::apply(&mut cmd, &shell) && name == "bash" {
            cmd.arg0("bash"); // --rcfile is ignored by login shells; the rcfile emulates the login startup itself
        }
        let cwd = if !opts.cwd.is_empty() && std::path::Path::new(&opts.cwd).is_dir() {
            opts.cwd.clone()
        } else {
            std::env::var("HOME").unwrap_or_else(|_| "/".into())
        };
        cmd.current_dir(cwd);
        cmd.stdin(Stdio::from(slave.try_clone()?)).stdout(Stdio::from(slave.try_clone()?)).stderr(Stdio::from(slave));
        unsafe {
            cmd.pre_exec(|| {
                // The shell must start with default signal handling regardless of how the daemon was started: a
                // daemon launched as a background job inherits "ignore SIGINT/SIGQUIT", and ignored signals survive
                // exec, which would make Ctrl-C do nothing in every shell. (The daemon also ignores SIGHUP itself.)
                for sig in [
                    libc::SIGHUP, libc::SIGINT, libc::SIGQUIT, libc::SIGILL, libc::SIGTRAP, libc::SIGABRT, libc::SIGFPE,
                    libc::SIGBUS, libc::SIGSEGV, libc::SIGSYS, libc::SIGPIPE, libc::SIGALRM, libc::SIGTERM, libc::SIGTSTP,
                    libc::SIGTTIN, libc::SIGTTOU, libc::SIGCHLD, libc::SIGWINCH, libc::SIGUSR1, libc::SIGUSR2,
                ] {
                    libc::signal(sig, libc::SIG_DFL);
                }
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
                // New session, and make the pty (already fd 0) the controlling terminal.
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn()?;
        let pid = child.id();
        // Dropping `cmd` closes our copies of the slave fds.
        drop(cmd);

        let pane = Arc::new(Pane {
            id,
            master,
            pid,
            scrollback: opts.scrollback.clamp(0, 1_000_000),
            inner: Mutex::new(Inner {
                term: Self::seeded_terminal(cols, rows, opts),
                sinks: Vec::new(),
                cols,
                rows,
            }),
            gate: Mutex::new(()),
            direct: Mutex::new(DirectState::default()),
            direct_cv: Condvar::new(),
            direct_flag: AtomicBool::new(false),
            alive: AtomicBool::new(true),
            exit_code: AtomicI32::new(0),
            next_sink: AtomicU64::new(1),
        });
        let p2 = pane.clone();
        std::thread::Builder::new().name(format!("pane-{}-reader", id)).spawn(move || p2.reader(child))?;
        Ok(pane)
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }
    pub fn exit_code(&self) -> i32 {
        self.exit_code.load(Ordering::Acquire)
    }

    pub fn info(&self) -> aleno_protocol::PaneInfo {
        let g = self.inner.lock().unwrap();
        aleno_protocol::PaneInfo {
            id: self.id,
            alive: self.is_alive(),
            title: g.term.title().to_string(),
            cols: g.cols,
            rows: g.rows,
            cwd: g.term.cwd().to_string(),
        }
    }

    pub fn client_count(&self) -> usize {
        self.inner.lock().unwrap().sinks.len() + self.direct_flag.load(Ordering::Acquire) as usize
    }

    pub fn is_direct(&self) -> bool {
        self.direct_flag.load(Ordering::Acquire)
    }

    /// Plain-text dump of the visible screen (diagnostics and tests).
    pub fn screen_text(&self) -> String {
        self.inner.lock().unwrap().term.screen_text()
    }

    /// Non-blocking fan-out. Returns clients whose queue was full (they still need delivery).
    fn broadcast(sinks: &mut Vec<Sink>, msg: &Msg) -> Vec<Sink> {
        let mut full = Vec::new();
        sinks.retain(|s| match s.tx.try_send(msg.clone()) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                full.push(s.clone());
                true
            }
            Err(TrySendError::Disconnected(_)) => false,
        });
        full
    }

    fn deliver_to_laggards(&self, laggards: Vec<Sink>, msg: &Msg) {
        for s in laggards {
            let deadline = Instant::now() + CLIENT_STALL_LIMIT;
            loop {
                match s.tx.try_send(msg.clone()) {
                    Ok(()) => break,
                    Err(TrySendError::Disconnected(_)) => {
                        self.detach(s.id);
                        break;
                    }
                    Err(TrySendError::Full(_)) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(1))
                    }
                    Err(TrySendError::Full(_)) => {
                        self.detach(s.id); // dropping the sender closes the client's stream
                        break;
                    }
                }
            }
        }
    }

    fn handle_output(&self, data: &[u8]) {
        let msg = Msg::Output { data: data.to_vec() };
        let (responses, laggards) = {
            let mut g = self.inner.lock().unwrap();
            g.term.feed(data);
            let responses = g.term.take_responses();
            let title = if g.term.take_title_changed() { Some(g.term.title().to_string()) } else { None };
            let mut laggards = Vec::new();
            if !g.sinks.is_empty() {
                laggards = Self::broadcast(&mut g.sinks, &msg);
                if let Some(t) = &title {
                    // Titles are tiny; a full queue here is already covered by the Output retry.
                    Self::broadcast(&mut g.sinks, &Msg::Title { title: t.clone() });
                }
            }
            (responses, laggards)
        };
        if !laggards.is_empty() {
            self.deliver_to_laggards(laggards, &msg);
        }
        if !responses.is_empty() {
            let _ = self.write_input(&responses);
        }
    }

    /// Read everything currently available from the pty (non-blocking) into `buf`.
    fn drain_pty(fd: RawFd, buf: &mut [u8], filled: &mut usize) -> bool {
        // returns true on EOF/error (slave closed)
        while *filled < buf.len() {
            let n = unsafe { libc::read(fd, buf[*filled..].as_mut_ptr() as *mut _, buf.len() - *filled) };
            if n > 0 {
                *filled += n as usize;
                continue;
            }
            let errno = io::Error::last_os_error().raw_os_error().unwrap_or(0);
            return n == 0 || (errno != libc::EAGAIN && errno != libc::EINTR);
        }
        false
    }

    fn reader(self: Arc<Self>, mut child: Child) {
        set_interactive_qos();
        let fd = self.master.as_raw_fd();
        let mut buf = vec![0u8; READ_CHUNK];
        // During a flood the pty only hands over ~1 KB per read, so the writer and this thread ping-pong through the
        // kernel. After a large read we stay on-CPU for a short spin instead of sleeping in poll(), which halves the
        // hand-off latency. Interactive output (tiny reads) never spins.
        let mut hot = false;
        loop {
            if self.direct_flag.load(Ordering::Acquire) {
                // A window owns the pty and reads it itself: only watch for the child exiting.
                if let Ok(Some(_)) = child.try_wait() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            let guard = self.gate.lock().unwrap();
            if self.direct_flag.load(Ordering::Acquire) {
                drop(guard);
                continue;
            }
            let mut filled = 0usize;
            let mut eof = Self::drain_pty(fd, &mut buf, &mut filled);
            if filled == 0 && !eof && hot {
                let until = Instant::now() + Duration::from_micros(80);
                while filled == 0 && !eof && Instant::now() < until {
                    std::hint::spin_loop();
                    eof = Self::drain_pty(fd, &mut buf, &mut filled);
                }
            }
            if filled > 0 {
                self.handle_output(&buf[..filled]);
                hot = filled >= 512;
                if !eof {
                    continue;
                }
            }
            if eof {
                break; // EOF / EIO: slave side fully closed
            }
            hot = false;
            drop(guard); // never sleep while holding the gate
            let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            let r = unsafe { libc::poll(&mut pfd, 1, 200) };
            if r > 0 {
                continue;
            }
            if let Ok(Some(_)) = child.try_wait() {
                // Drain whatever is still buffered in the pty.
                let _g = self.gate.lock().unwrap();
                loop {
                    let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len()) };
                    if n <= 0 {
                        break;
                    }
                    self.handle_output(&buf[..n as usize]);
                }
                break;
            }
        }
        // The pty closed; reap the child (bounded wait, then SIGKILL the group).
        let deadline = Instant::now() + Duration::from_millis(1500);
        let status = loop {
            match child.try_wait() {
                Ok(Some(st)) => break Some(st),
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
                _ => {
                    unsafe { libc::killpg(self.pid as i32, libc::SIGKILL) };
                    break child.wait().ok();
                }
            }
        };
        use std::os::unix::process::ExitStatusExt;
        let code = status.map(|s| s.code().unwrap_or_else(|| 128 + s.signal().unwrap_or(0))).unwrap_or(-1);
        self.exit_code.store(code, Ordering::Release);
        self.alive.store(false, Ordering::Release);
        {
            let mut g = self.inner.lock().unwrap();
            Self::broadcast(&mut g.sinks, &Msg::Exited { code });
        }
        // A direct-mode owner reads the pty itself; tell it the exit status.
        let owner_stream = self.direct.lock().unwrap().stream.as_ref().and_then(|s| s.try_clone().ok());
        if let Some(mut st) = owner_stream {
            let _ = write_msg(&mut st, &Msg::Exited { code });
        }
    }

    /// Write bytes to the child's terminal input. Bounded wait if the child is not reading.
    pub fn write_input(&self, mut data: &[u8]) -> io::Result<()> {
        let fd = self.master.as_raw_fd();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !data.is_empty() {
            let n = unsafe { libc::write(fd, data.as_ptr() as *const _, data.len()) };
            if n > 0 {
                data = &data[n as usize..];
                continue;
            }
            let errno = io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if n < 0 && errno == libc::EINTR {
                continue;
            }
            if n < 0 && errno == libc::EAGAIN {
                if Instant::now() > deadline {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "pty input blocked"));
                }
                let mut pfd = libc::pollfd { fd, events: libc::POLLOUT, revents: 0 };
                unsafe { libc::poll(&mut pfd, 1, 100) };
                continue;
            }
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let cols = cols.clamp(1, 1000);
        let rows = rows.clamp(1, 1000);
        let mut g = self.inner.lock().unwrap();
        if g.cols == cols && g.rows == rows {
            return;
        }
        g.cols = cols;
        g.rows = rows;
        g.term.resize(cols as usize, rows as usize);
        let ws = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
        unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ as _, &ws) };
        Self::broadcast(&mut g.sinks, &Msg::Resized { cols, rows });
    }

    /// Atomically snapshot the screen and register for subsequent output. The returned receiver
    /// yields `Snapshot` first, then ordered `Output`/`Title`/`Resized`/`Exited` messages.
    pub fn attach(&self, cols: u16, rows: u16) -> (u64, Receiver<Msg>) {
        // A window that owns the pty must give it back before anyone can be streamed to.
        let (tx, rx) = sync_channel(CLIENT_QUEUE);
        let mut g = loop {
            if self.direct_flag.load(Ordering::Acquire) {
                self.recall_direct();
            }
            let g = self.inner.lock().unwrap();
            if !self.direct_flag.load(Ordering::Acquire) {
                break g;
            }
        };
        if (g.cols != cols || g.rows != rows) && cols > 0 && rows > 0 {
            let (c, r) = (cols.clamp(1, 1000), rows.clamp(1, 1000));
            g.cols = c;
            g.rows = r;
            g.term.resize(c as usize, r as usize);
            let ws = libc::winsize { ws_row: r, ws_col: c, ws_xpixel: 0, ws_ypixel: 0 };
            unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ as _, &ws) };
            Self::broadcast(&mut g.sinks, &Msg::Resized { cols: c, rows: r });
        }
        let snap = g.term.snapshot_vt(SNAPSHOT_HISTORY);
        let _ = tx.send(Msg::Snapshot { data: snap });
        if !g.term.title().is_empty() {
            let _ = tx.send(Msg::Title { title: g.term.title().to_string() });
        }
        if !self.is_alive() {
            let _ = tx.send(Msg::Exited { code: self.exit_code() });
        }
        let id = self.next_sink.fetch_add(1, Ordering::Relaxed);
        g.sinks.push(Sink { id, tx });
        (id, rx)
    }

    // ------------------------------------------------------------------ direct mode

    /// Hand the pty to one window. Only possible when nobody is streamed to and nobody owns it. Returns the screen
    /// snapshot and the pty master fd (the caller passes it over the socket).
    pub fn grant_direct(&self, owner: u64, cols: u16, rows: u16, stream: UnixStream) -> Option<(Vec<u8>, RawFd)> {
        let _gate = self.gate.lock().unwrap();
        let mut d = self.direct.lock().unwrap();
        let mut inner = self.inner.lock().unwrap();
        if d.owner.is_some() || !inner.sinks.is_empty() || !self.is_alive() {
            return None;
        }
        if cols > 0 && rows > 0 && (inner.cols != cols || inner.rows != rows) {
            let (c, r) = (cols.clamp(1, 1000), rows.clamp(1, 1000));
            inner.cols = c;
            inner.rows = r;
            inner.term.resize(c as usize, r as usize);
            let ws = libc::winsize { ws_row: r, ws_col: c, ws_xpixel: 0, ws_ypixel: 0 };
            unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ as _, &ws) };
        }
        let snap = inner.term.snapshot_vt(SNAPSHOT_HISTORY);
        d.owner = Some(owner);
        d.stream = Some(stream);
        d.checkpoint = None;
        self.direct_flag.store(true, Ordering::Release);
        Some((snap, self.master.as_raw_fd()))
    }

    /// Owner stores its current screen state (used if it later vanishes without calling `Return`).
    pub fn set_checkpoint(&self, owner: u64, cols: u16, rows: u16, snapshot: Vec<u8>) {
        let mut d = self.direct.lock().unwrap();
        if d.owner == Some(owner) {
            d.checkpoint = Some((cols, rows, snapshot));
        }
    }

    fn restore_from(&self, inner: &mut Inner, cols: u16, rows: u16, snapshot: &[u8]) {
        let mut t = Terminal::new(cols as usize, rows as usize, self.scrollback);
        t.feed(snapshot);
        let _ = t.take_responses();
        inner.term = t;
        inner.cols = cols;
        inner.rows = rows;
    }

    /// The owner returned the pty together with its exact screen state.
    pub fn reclaim_with_state(&self, owner: u64, cols: u16, rows: u16, snapshot: &[u8]) -> bool {
        let _gate = self.gate.lock().unwrap();
        let mut d = self.direct.lock().unwrap();
        if d.owner != Some(owner) {
            return false;
        }
        let mut inner = self.inner.lock().unwrap();
        self.restore_from(&mut inner, cols.clamp(1, 1000), rows.clamp(1, 1000), snapshot);
        d.owner = None;
        d.stream = None;
        d.checkpoint = None;
        self.direct_flag.store(false, Ordering::Release);
        self.direct_cv.notify_all();
        true
    }

    /// The owner is gone (or unresponsive): take the pty back using its last checkpoint, if any.
    pub fn reclaim_from_checkpoint(&self, only_owner: Option<u64>) {
        let _gate = self.gate.lock().unwrap();
        let mut d = self.direct.lock().unwrap();
        if d.owner.is_none() || (only_owner.is_some() && d.owner != only_owner) {
            return;
        }
        if let Some((c, r, snap)) = d.checkpoint.take() {
            let mut inner = self.inner.lock().unwrap();
            self.restore_from(&mut inner, c.clamp(1, 1000), r.clamp(1, 1000), &snap);
            let ws = libc::winsize { ws_row: inner.rows, ws_col: inner.cols, ws_xpixel: 0, ws_ypixel: 0 };
            unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ as _, &ws) };
        }
        d.owner = None;
        d.stream = None;
        self.direct_flag.store(false, Ordering::Release);
        self.direct_cv.notify_all();
    }

    pub fn release_owner(&self, owner: u64) {
        self.reclaim_from_checkpoint(Some(owner));
    }

    /// Ask the owner to return the pty and wait (bounded) for it; fall back to its last checkpoint.
    pub fn recall_direct(&self) {
        let stream = {
            let d = self.direct.lock().unwrap();
            if d.owner.is_none() {
                return;
            }
            d.stream.as_ref().and_then(|s| s.try_clone().ok())
        };
        if let Some(mut s) = stream {
            let _ = write_msg(&mut s, &Msg::Release);
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut d = self.direct.lock().unwrap();
        while d.owner.is_some() {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            d = self.direct_cv.wait_timeout(d, deadline - now).unwrap().0;
        }
        let still_owned = d.owner.is_some();
        drop(d);
        if still_owned {
            self.reclaim_from_checkpoint(None);
        }
    }

    pub fn detach(&self, sink_id: u64) {
        self.inner.lock().unwrap().sinks.retain(|s| s.id != sink_id);
    }

    /// Hang up the shell's process group; escalate to SIGKILL if it lingers.
    pub fn kill(&self) {
        if !self.is_alive() {
            return;
        }
        let pid = self.pid as i32;
        unsafe { libc::killpg(pid, libc::SIGHUP) };
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(2));
            unsafe { libc::killpg(pid, libc::SIGKILL) };
        });
    }
}
