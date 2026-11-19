//! Client side of the Aleno protocol: control connection, pane streams with a local terminal
//! mirror, and the C ABI consumed by the Swift app.

pub mod ffi;

use aleno_protocol::*;
use aleno_terminal::Terminal;
use std::io::{self, BufWriter, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

fn handshake(path: &Path, name: &str) -> io::Result<UnixStream> {
    let mut s = UnixStream::connect(path)?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    write_msg(&mut s, &Msg::Hello { version: PROTOCOL_VERSION, client: name.into() })?;
    match read_msg(&mut s)? {
        Msg::Welcome { .. } => {
            s.set_read_timeout(None)?;
            Ok(s)
        }
        Msg::Err { message, .. } => Err(io::Error::new(io::ErrorKind::Other, message)),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "bad handshake")),
    }
}

fn rpc_err(m: Msg) -> io::Error {
    match m {
        Msg::Err { message, .. } => io::Error::new(io::ErrorKind::Other, message),
        _ => io::Error::new(io::ErrorKind::InvalidData, "unexpected reply"),
    }
}

/// Control connection: session/pane management, request/response.
pub struct Ctl {
    path: std::path::PathBuf,
    stream: Mutex<UnixStream>,
}

impl Ctl {
    pub fn connect(path: &Path) -> io::Result<Ctl> {
        Ok(Ctl { path: path.to_path_buf(), stream: Mutex::new(handshake(path, "aleno-gui")?) })
    }
    pub fn socket(&self) -> &Path {
        &self.path
    }
    fn call(&self, m: Msg) -> io::Result<Msg> {
        let mut s = self.stream.lock().unwrap();
        write_msg(&mut *s, &m)?;
        read_msg(&mut *s)
    }
    pub fn create_session(&self, name: &str) -> io::Result<u32> {
        match self.call(Msg::CreateSession { name: name.into() })? {
            Msg::SessionCreated { id } => Ok(id),
            m => Err(rpc_err(m)),
        }
    }
    pub fn list_sessions(&self) -> io::Result<Vec<SessionInfo>> {
        match self.call(Msg::ListSessions)? {
            Msg::Sessions { list } => Ok(list),
            m => Err(rpc_err(m)),
        }
    }
    pub fn rename_session(&self, session: u32, name: &str) -> io::Result<()> {
        match self.call(Msg::RenameSession { session, name: name.into() })? {
            Msg::Ok => Ok(()),
            m => Err(rpc_err(m)),
        }
    }
    pub fn kill_session(&self, session: u32) -> io::Result<()> {
        match self.call(Msg::KillSession { session })? {
            Msg::Ok => Ok(()),
            m => Err(rpc_err(m)),
        }
    }
    pub fn set_layout(&self, session: u32, json: &str) -> io::Result<()> {
        match self.call(Msg::SetLayout { session, json: json.into() })? {
            Msg::Ok => Ok(()),
            m => Err(rpc_err(m)),
        }
    }
    pub fn get_layout(&self, session: u32) -> io::Result<String> {
        match self.call(Msg::GetLayout { session })? {
            Msg::Layout { json } => Ok(json),
            m => Err(rpc_err(m)),
        }
    }
    pub fn create_pane(&self, session: u32, cols: u16, rows: u16, shell: &str, cwd: &str, scrollback: u32) -> io::Result<u32> {
        self.create_pane_seeded(session, cols, rows, shell, cwd, scrollback, &[])
    }
    /// Like `create_pane`, replaying `seed` (a `snapshot` of an earlier pane) above the new shell's first prompt.
    pub fn create_pane_seeded(&self, session: u32, cols: u16, rows: u16, shell: &str, cwd: &str, scrollback: u32, seed: &[u8]) -> io::Result<u32> {
        self.create_pane_full(session, cols, rows, shell, cwd, scrollback, seed, false)
    }
    /// Like `create_pane_seeded`; with `welcome` the pane first plays the first-run welcome animation.
    pub fn create_pane_full(&self, session: u32, cols: u16, rows: u16, shell: &str, cwd: &str, scrollback: u32, seed: &[u8], welcome: bool) -> io::Result<u32> {
        match self.call(Msg::CreatePane { session, cols, rows, shell: shell.into(), cwd: cwd.into(), scrollback, seed: seed.to_vec(), welcome })? {
            Msg::PaneCreated { pane } => Ok(pane),
            m => Err(rpc_err(m)),
        }
    }
    pub fn kill_pane(&self, session: u32, pane: u32) -> io::Result<()> {
        match self.call(Msg::KillPane { session, pane })? {
            Msg::Ok => Ok(()),
            m => Err(rpc_err(m)),
        }
    }
    pub fn list_panes(&self, session: u32) -> io::Result<Vec<PaneInfo>> {
        match self.call(Msg::ListPanes { session })? {
            Msg::Panes { list } => Ok(list),
            m => Err(rpc_err(m)),
        }
    }
}

pub const EVENT_OUTPUT: u32 = 0;
pub const EVENT_TITLE: u32 = 1;
pub const EVENT_EXITED: u32 = 2;
pub const EVENT_BELL: u32 = 3;
pub const EVENT_DISCONNECTED: u32 = 4;
/// A shell command finished (see `aleno_pane_take_finished`).
pub const EVENT_COMMAND_FINISHED: u32 = 5;
/// The pane's terminal progress (OSC 9;4) changed.
pub const EVENT_PROGRESS: u32 = 6;

pub type Notify = Box<dyn Fn(u32) + Send + Sync>;
type SharedNotify = Arc<dyn Fn(u32) + Send + Sync>;

pub struct PaneShared {
    pub term: Mutex<Terminal>,
    pub wake_pending: AtomicBool,
    pub exited: AtomicBool,
    pub exit_code: AtomicI32,
    pub disconnected: AtomicBool,
    pub title: Mutex<String>,
    /// Set while the UI thread wants the terminal lock; readers back off briefly so frames are not starved.
    pub ui_waiting: AtomicBool,
}

/// How the pane is being fed.
enum Mode {
    /// The daemon parses and streams output to us over the socket.
    Streamed { tx: Sender<Msg>, sock: UnixStream },
    /// The daemon handed us the pty master: we read and write it ourselves (one parse, no socket in the data path).
    Direct(DirectConn),
    Closed,
}

struct DirectConn {
    master: Arc<OwnedFd>,
    /// Connection to the daemon (Return / Checkpoint go here).
    ctl: Arc<Mutex<UnixStream>>,
    input: Sender<Vec<u8>>,
    stop: Arc<AtomicBool>,
    /// Write end of the self-pipe that wakes the reader out of poll().
    wake_w: Arc<OwnedFd>,
    /// (reader has stopped consuming the pty, condvar)
    parked: Arc<(Mutex<bool>, Condvar)>,
}

struct Core {
    shared: Arc<PaneShared>,
    path: PathBuf,
    session: u32,
    pane: u32,
    notify: SharedNotify,
    mode: Mutex<Mode>,
    /// Signalled when the daemon acknowledges `Return`.
    ack: (Mutex<bool>, Condvar),
    /// True while switching from direct to streamed; input typed in that gap is queued, not dropped.
    transitioning: AtomicBool,
    pending_input: Mutex<Vec<u8>>,
}

pub struct PaneClient {
    pub shared: Arc<PaneShared>,
    core: Arc<Core>,
}

fn new_shared(cols: u16, rows: u16, scrollback: usize, respond: bool) -> Arc<PaneShared> {
    let mut term = Terminal::new(cols as usize, rows as usize, scrollback);
    term.set_respond(respond);
    Arc::new(PaneShared {
        term: Mutex::new(term),
        wake_pending: AtomicBool::new(false),
        exited: AtomicBool::new(false),
        exit_code: AtomicI32::new(0),
        disconnected: AtomicBool::new(false),
        title: Mutex::new(String::new()),
        ui_waiting: AtomicBool::new(false),
    })
}

fn wake(sh: &PaneShared, notify: &SharedNotify, ev: u32) {
    if ev != EVENT_OUTPUT || !sh.wake_pending.swap(true, Ordering::AcqRel) {
        notify(ev);
    }
}

fn set_qos() {
    #[cfg(target_os = "macos")]
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
}

fn write_all_pty(fd: i32, mut data: &[u8]) {
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
        if n < 0 && errno == libc::EAGAIN && Instant::now() < deadline {
            let mut pfd = libc::pollfd { fd, events: libc::POLLOUT, revents: 0 };
            unsafe { libc::poll(&mut pfd, 1, 100) };
            continue;
        }
        return;
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Two-stage pty pipeline (the design used by Ghostty and WezTerm): a *gather* thread drains the kernel pty into
// rotating buffers while a *parse* thread feeds the terminal, so reading never waits for parsing. The macOS pty hands
// over at most ~1 KB per read; a gather loop that spins briefly on EAGAIN bridges the writer's microsecond refill gaps
// on saturated streams, while small interactive output is delivered immediately with no added latency.
// ---------------------------------------------------------------------------------------------------------------------
const RING_BUFFERS: usize = 4;
const RING_CAPACITY: usize = 64 * 1024;
/// Bytes gathered in one go that mark a stream as saturated (a full kernel queue), worth briefly waiting on.
const BRIDGE_THRESHOLD: usize = 1024;
const BRIDGE_SPIN_MAX: u32 = 16;
const BRIDGE_POLL_MS: i32 = 1;
/// Longest one batch may spend bridging gaps before it is delivered regardless (well under one display frame).
const GATHER_BUDGET: Duration = Duration::from_millis(3);

struct RingState {
    free: Vec<Vec<u8>>,
    ready: VecDeque<(Vec<u8>, usize)>,
    done: bool,
    /// Set while the gather stage sleeps in a bridge poll; the parse stage only pokes the idle pipe then.
    bridging: bool,
}

struct Ring {
    st: Mutex<RingState>,
    batch_ready: Condvar,
    slot_free: Condvar,
}

impl Ring {
    fn new() -> Ring {
        Ring {
            st: Mutex::new(RingState {
                free: (0..RING_BUFFERS).map(|_| vec![0u8; RING_CAPACITY]).collect(),
                ready: VecDeque::new(),
                done: false,
                bridging: false,
            }),
            batch_ready: Condvar::new(),
            slot_free: Condvar::new(),
        }
    }
}

fn poll_fds(fds: &mut [libc::pollfd], timeout_ms: i32) -> i32 {
    unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout_ms) }
}

fn drain_pipe(fd: i32) {
    let mut trash = [0u8; 16];
    loop {
        let n = unsafe { libc::read(fd, trash.as_mut_ptr() as *mut _, trash.len()) };
        if n < trash.len() as isize {
            break;
        }
    }
}

impl PaneClient {
    /// Attach to a pane. Tries direct mode first (the window reads the pty itself); falls back to a streamed
    /// attach when the daemon cannot hand the pty over (another client is attached, older daemon, or
    /// `ALENO_NO_DIRECT` is set).
    pub fn attach(path: &Path, session: u32, pane: u32, cols: u16, rows: u16, scrollback: usize, notify: Notify) -> io::Result<PaneClient> {
        let notify: SharedNotify = Arc::from(notify);
        if std::env::var_os("ALENO_NO_DIRECT").is_none() {
            // Any failure (e.g. an older daemon that drops the connection on an unknown request) falls back to the
            // streamed protocol on a fresh connection.
            if let Ok(Some(c)) = Self::try_direct(path, session, pane, cols, rows, scrollback, &notify) {
                return Ok(c);
            }
        }
        Self::attach_streamed_with(path, session, pane, cols, rows, scrollback, notify)
    }

    /// Always use the streamed protocol (tests, and the fallback).
    pub fn attach_streamed(path: &Path, session: u32, pane: u32, cols: u16, rows: u16, scrollback: usize, notify: Notify) -> io::Result<PaneClient> {
        Self::attach_streamed_with(path, session, pane, cols, rows, scrollback, Arc::from(notify))
    }

    fn make_core(shared: &Arc<PaneShared>, path: &Path, session: u32, pane: u32, _scrollback: usize, notify: SharedNotify) -> Arc<Core> {
        Arc::new(Core {
            shared: shared.clone(),
            path: path.to_path_buf(),
            session,
            pane,
            notify,
            mode: Mutex::new(Mode::Closed),
            ack: (Mutex::new(false), Condvar::new()),
            transitioning: AtomicBool::new(false),
            pending_input: Mutex::new(Vec::new()),
        })
    }

    fn attach_streamed_with(path: &Path, session: u32, pane: u32, cols: u16, rows: u16, scrollback: usize, notify: SharedNotify) -> io::Result<PaneClient> {
        let mut s = handshake(path, "aleno-gui-pane")?;
        write_msg(&mut s, &Msg::AttachPane { session, pane, cols, rows })?;
        match read_msg(&mut s)? {
            Msg::Attached => {}
            m => return Err(rpc_err(m)),
        }
        let shared = new_shared(cols, rows, scrollback, false); // the daemon's emulator answers device queries
        let core = Self::make_core(&shared, path, session, pane, scrollback, notify);
        Core::start_streamed(&core, s)?;
        Ok(PaneClient { shared, core })
    }

    fn try_direct(path: &Path, session: u32, pane: u32, cols: u16, rows: u16, scrollback: usize, notify: &SharedNotify) -> io::Result<Option<PaneClient>> {
        let mut s = handshake(path, "aleno-gui-pane")?;
        write_msg(&mut s, &Msg::AttachDirect { session, pane, cols, rows })?;
        match read_msg(&mut s)? {
            Msg::Attached => {}
            // Unavailable right now, or an older daemon that does not know the request: use the streamed protocol.
            Msg::Err { .. } => return Ok(None),
            m => return Err(rpc_err(m)),
        }
        let snapshot = match read_msg(&mut s)? {
            Msg::Snapshot { data } => data,
            m => return Err(rpc_err(m)),
        };
        let (m, fd) = read_msg_with_fd(&s)?;
        if !matches!(m, Msg::Granted) {
            return Err(rpc_err(m));
        }
        let master = Arc::new(fd.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no fd with Granted"))?);
        let shared = new_shared(cols, rows, scrollback, true); // no daemon parser in this mode: answer queries ourselves
        {
            let mut t = shared.term.lock().unwrap();
            t.feed(&snapshot);
            let _ = t.take_responses();
        }
        let core = Self::make_core(&shared, path, session, pane, scrollback, notify.clone());
        Core::start_direct(&core, s, master)?;
        wake(&shared, notify, EVENT_OUTPUT);
        Ok(Some(PaneClient { shared, core }))
    }

    pub fn send_input(&self, data: &[u8]) {
        match &*self.core.mode.lock().unwrap() {
            Mode::Streamed { tx, .. } => {
                for chunk in data.chunks(MAX_INPUT / 2) {
                    let _ = tx.send(Msg::Input { data: chunk.to_vec() });
                }
            }
            Mode::Direct(d) => {
                let _ = d.input.send(data.to_vec());
            }
            Mode::Closed => {
                if self.core.transitioning.load(Ordering::Acquire) {
                    self.core.pending_input.lock().unwrap().extend_from_slice(data);
                }
            }
        }
    }

    /// Resize both the local mirror and the PTY.
    pub fn resize(&self, cols: u16, rows: u16) {
        self.shared.term.lock().unwrap().resize(cols as usize, rows as usize);
        match &*self.core.mode.lock().unwrap() {
            Mode::Streamed { tx, .. } => {
                let _ = tx.send(Msg::Resize { cols, rows });
            }
            Mode::Direct(d) => {
                let ws = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
                unsafe { libc::ioctl(d.master.as_raw_fd(), libc::TIOCSWINSZ as _, &ws) };
            }
            Mode::Closed => {}
        }
    }

    /// True when this client currently owns the pty (direct mode).
    pub fn is_direct(&self) -> bool {
        matches!(&*self.core.mode.lock().unwrap(), Mode::Direct(_))
    }

    /// Test hook: vanish without handing the pty back, as a crashed process would.
    pub fn abandon(self) {
        let old = std::mem::replace(&mut *self.core.mode.lock().unwrap(), Mode::Closed);
        if let Mode::Direct(d) = &old {
            d.stop.store(true, Ordering::Release);
            let b = 1u8;
            unsafe { libc::write(d.wake_w.as_raw_fd(), &b as *const _ as *const _, 1) };
            let _ = d.ctl.lock().unwrap().shutdown(std::net::Shutdown::Both);
        }
    }
}

impl Drop for PaneClient {
    fn drop(&mut self) {
        let direct = matches!(&*self.core.mode.lock().unwrap(), Mode::Direct(_));
        if direct {
            let _ = self.core.return_to_daemon(); // hand the pty and the exact screen state back
        }
        let old = std::mem::replace(&mut *self.core.mode.lock().unwrap(), Mode::Closed);
        match old {
            Mode::Streamed { tx, sock } => {
                let _ = tx.send(Msg::Detach);
                std::thread::sleep(Duration::from_millis(5));
                let _ = sock.shutdown(std::net::Shutdown::Both);
            }
            Mode::Direct(d) => {
                let _ = d.ctl.lock().unwrap().shutdown(std::net::Shutdown::Both);
            }
            Mode::Closed => {}
        }
    }
}

impl Core {
    // ------------------------------------------------------------------ streamed

    fn start_streamed(core: &Arc<Core>, s: UnixStream) -> io::Result<()> {
        let (tx, rx) = channel::<Msg>();
        let mut ws = s.try_clone()?;
        std::thread::Builder::new().name("pane-writer".into()).spawn(move || {
            let mut w = BufWriter::new(&mut ws);
            while let Ok(m) = rx.recv() {
                if write_msg(&mut w, &m).is_err() {
                    return;
                }
                while let Ok(m) = rx.try_recv() {
                    if write_msg(&mut w, &m).is_err() {
                        return;
                    }
                }
                if w.flush().is_err() {
                    return;
                }
            }
        })?;
        // Buffered: messages are often ~1 KB, so unbuffered reads would cost two syscalls each.
        let mut rs = std::io::BufReader::with_capacity(256 * 1024, s.try_clone()?);
        let c = core.clone();
        std::thread::Builder::new().name("pane-reader".into()).spawn(move || {
            let sh = &c.shared;
            let mut last_bells = 0;
            loop {
                match read_msg(&mut rs) {
                    Ok(Msg::Snapshot { data }) | Ok(Msg::Output { data }) => {
                        if sh.ui_waiting.load(Ordering::Acquire) {
                            std::thread::sleep(Duration::from_micros(150));
                        }
                        let bell = {
                            let mut t = sh.term.lock().unwrap();
                            t.feed(&data);
                            let b = t.bell_count();
                            let r = b != last_bells;
                            last_bells = b;
                            (r, t.has_finished(), t.take_progress_changed())
                        };
                        let (bell, finished, progress) = (bell.0, bell.1, bell.2);
                        wake(sh, &c.notify, EVENT_OUTPUT);
                        if progress {
                            wake(sh, &c.notify, EVENT_PROGRESS);
                        }
                        if bell {
                            wake(sh, &c.notify, EVENT_BELL);
                        }
                        if finished {
                            wake(sh, &c.notify, EVENT_COMMAND_FINISHED);
                        }
                    }
                    Ok(Msg::Title { title }) => {
                        *sh.title.lock().unwrap() = title;
                        wake(sh, &c.notify, EVENT_TITLE);
                    }
                    Ok(Msg::Resized { cols, rows }) => {
                        sh.term.lock().unwrap().resize(cols as usize, rows as usize);
                        wake(sh, &c.notify, EVENT_OUTPUT);
                    }
                    Ok(Msg::Exited { code }) => {
                        sh.exit_code.store(code, Ordering::Release);
                        sh.exited.store(true, Ordering::Release);
                        wake(sh, &c.notify, EVENT_EXITED);
                    }
                    Ok(_) => {}
                    Err(_) => {
                        sh.disconnected.store(true, Ordering::Release);
                        wake(sh, &c.notify, EVENT_DISCONNECTED);
                        return;
                    }
                }
            }
        })?;
        *core.mode.lock().unwrap() = Mode::Streamed { tx, sock: s };
        Ok(())
    }

    // ------------------------------------------------------------------ direct

    fn start_direct(core: &Arc<Core>, s: UnixStream, master: Arc<OwnedFd>) -> io::Result<()> {
        let ctl = Arc::new(Mutex::new(s.try_clone()?));
        let (itx, irx) = channel::<Vec<u8>>();
        let stop = Arc::new(AtomicBool::new(false));
        let parked = Arc::new((Mutex::new(false), Condvar::new()));
        // Self-pipe: lets `stop` interrupt the reader's poll() immediately.
        let mut fds = [0i32; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let (wake_r, wake_w) = unsafe { (OwnedFd::from_raw_fd(fds[0]), Arc::new(OwnedFd::from_raw_fd(fds[1]))) };
        unsafe {
            libc::fcntl(fds[0], libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(fds[1], libc::F_SETFD, libc::FD_CLOEXEC);
        }

        // Input writer: never block the UI thread on a full pty buffer.
        let mfd = master.clone();
        std::thread::Builder::new().name("pty-writer".into()).spawn(move || {
            set_qos();
            while let Ok(mut data) = irx.recv() {
                while let Ok(more) = irx.try_recv() {
                    data.extend_from_slice(&more);
                }
                write_all_pty(mfd.as_raw_fd(), &data);
            }
        })?;

        // Output pipeline: gather thread (pty -> ring) and parse thread (ring -> local terminal).
        let mut ifds = [0i32; 2];
        if unsafe { libc::pipe(ifds.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let (idle_r, idle_w) = unsafe { (OwnedFd::from_raw_fd(ifds[0]), OwnedFd::from_raw_fd(ifds[1])) };
        unsafe {
            for f in ifds {
                libc::fcntl(f, libc::F_SETFD, libc::FD_CLOEXEC);
                libc::fcntl(f, libc::F_SETFL, libc::fcntl(f, libc::F_GETFL) | libc::O_NONBLOCK);
            }
        }
        let ring = Arc::new(Ring::new());
        {
            let (m, st, r) = (master.clone(), stop.clone(), ring.clone());
            let wake_r_dup = unsafe { OwnedFd::from_raw_fd(libc::dup(wake_r.as_raw_fd())) };
            let idle_r2 = unsafe { OwnedFd::from_raw_fd(libc::dup(idle_r.as_raw_fd())) };
            std::thread::Builder::new().name("pty-gather".into()).spawn(move || gather_thread(m, wake_r_dup, idle_r2, st, r))?;
        }
        {
            let (c, pk, ct, it, r) = (core.clone(), parked.clone(), ctl.clone(), itx.clone(), ring.clone());
            let _keep = (&idle_r, &wake_r);
            std::thread::Builder::new().name("pty-parse".into()).spawn(move || c.parse_thread(pk, ct, it, r, idle_w))?;
        }

        // Daemon messages (Release / Exited / Ok-for-Return).
        let c = core.clone();
        let mut rs = s;
        std::thread::Builder::new().name("pane-ctl".into()).spawn(move || loop {
            match read_msg(&mut rs) {
                Ok(Msg::Release) => {
                    // Hand the pty back from another thread: this one must keep reading to receive the Ok.
                    let c2 = c.clone();
                    std::thread::spawn(move || c2.release_and_restream());
                }
                Ok(Msg::Ok) => {
                    let (m, cv) = &c.ack;
                    *m.lock().unwrap() = true;
                    cv.notify_all();
                }
                Ok(Msg::Exited { code }) => {
                    c.shared.exit_code.store(code, Ordering::Release);
                    c.shared.exited.store(true, Ordering::Release);
                    wake(&c.shared, &c.notify, EVENT_EXITED);
                }
                Ok(_) => {}
                Err(_) => {
                    let closed = matches!(&*c.mode.lock().unwrap(), Mode::Closed);
                    if !closed && !c.shared.exited.load(Ordering::Acquire) {
                        c.shared.disconnected.store(true, Ordering::Release);
                        wake(&c.shared, &c.notify, EVENT_DISCONNECTED);
                    }
                    return;
                }
            }
        })?;
        *core.mode.lock().unwrap() = Mode::Direct(DirectConn { master, ctl, input: itx, stop, wake_w, parked });
        Ok(())
    }

    fn checkpoint(&self, ctl: &Arc<Mutex<UnixStream>>) {
        let (cols, rows, snapshot) = {
            let t = self.shared.term.lock().unwrap();
            (t.cols() as u16, t.rows() as u16, t.snapshot_vt(2000))
        };
        let mut s = ctl.lock().unwrap();
        let _ = write_msg(&mut *s, &Msg::Checkpoint { cols, rows, snapshot });
    }

    /// Parse stage: consumes batches in order and feeds the local terminal.
    fn parse_thread(self: Arc<Self>, parked: Arc<(Mutex<bool>, Condvar)>, ctl: Arc<Mutex<UnixStream>>, input: Sender<Vec<u8>>, ring: Arc<Ring>, idle_w: OwnedFd) {
        set_qos();
        let sh = &self.shared;
        let mut dirty = false;
        let mut last_ckpt = Instant::now();
        let mut last_bells = 0;
        loop {
            let batch = {
                let mut st = ring.st.lock().unwrap();
                loop {
                    if let Some(b) = st.ready.pop_front() {
                        break Some(b);
                    }
                    if st.done {
                        break None;
                    }
                    let timeout = if dirty { Duration::from_millis(300) } else { Duration::from_secs(1) };
                    let (g, to) = ring.batch_ready.wait_timeout(st, timeout).unwrap();
                    st = g;
                    if to.timed_out() && dirty && last_ckpt.elapsed() >= Duration::from_millis(300) {
                        // Output went quiet: store the settled screen so a crash loses (almost) nothing.
                        drop(st);
                        self.checkpoint(&ctl);
                        last_ckpt = Instant::now();
                        dirty = false;
                        st = ring.st.lock().unwrap();
                    }
                }
            };
            let Some((buf, len)) = batch else { break };
            if sh.ui_waiting.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_micros(150)); // let the renderer take the lock between batches
            }
            let (responses, title, bell, finished, progress) = {
                let mut t = sh.term.lock().unwrap();
                t.feed(&buf[..len]);
                let b = t.bell_count();
                let bell = b != last_bells;
                last_bells = b;
                (t.take_responses(), t.take_title_changed(), bell, t.has_finished(), t.take_progress_changed())
            };
            if !responses.is_empty() {
                let _ = input.send(responses);
            }
            wake(sh, &self.notify, EVENT_OUTPUT);
            if title {
                wake(sh, &self.notify, EVENT_TITLE);
            }
            if bell {
                wake(sh, &self.notify, EVENT_BELL);
            }
            if finished {
                wake(sh, &self.notify, EVENT_COMMAND_FINISHED);
            }
            if progress {
                wake(sh, &self.notify, EVENT_PROGRESS);
            }
            dirty = true;
            if last_ckpt.elapsed() >= Duration::from_secs(1) {
                self.checkpoint(&ctl);
                last_ckpt = Instant::now();
                dirty = false;
            }
            // Give the buffer back; if we just ran dry while the gather stage is bridging a refill gap, wake it so it
            // delivers what it has instead of sleeping out its poll (an idle parser means delivery must not wait).
            let wake_gather = {
                let mut st = ring.st.lock().unwrap();
                st.free.push(buf);
                st.ready.is_empty() && st.bridging
            };
            ring.slot_free.notify_one();
            if wake_gather {
                let b = 1u8;
                unsafe { libc::write(idle_w.as_raw_fd(), &b as *const _ as *const _, 1) };
            }
        }
        let (m, cv) = &*parked;
        *m.lock().unwrap() = true;
        cv.notify_all();
    }

    /// Stop reading the pty and hand it back to the daemon together with the exact screen state.
    fn return_to_daemon(&self) -> io::Result<()> {
        let (ctl, stop, wake_w, parked) = match &*self.mode.lock().unwrap() {
            Mode::Direct(d) => (d.ctl.clone(), d.stop.clone(), d.wake_w.clone(), d.parked.clone()),
            _ => return Ok(()),
        };
        stop.store(true, Ordering::Release);
        let b = 1u8;
        unsafe { libc::write(wake_w.as_raw_fd(), &b as *const _ as *const _, 1) };
        {
            let (m, cv) = &*parked;
            let mut g = m.lock().unwrap();
            let end = Instant::now() + Duration::from_millis(500);
            while !*g {
                let now = Instant::now();
                if now >= end {
                    break;
                }
                g = cv.wait_timeout(g, end - now).unwrap().0;
            }
        }
        *self.ack.0.lock().unwrap() = false;
        let (cols, rows, snapshot) = {
            let t = self.shared.term.lock().unwrap();
            (t.cols() as u16, t.rows() as u16, t.snapshot_vt(2000))
        };
        write_msg(&mut *ctl.lock().unwrap(), &Msg::Return { cols, rows, snapshot })?;
        let (m, cv) = &self.ack;
        let mut g = m.lock().unwrap();
        let end = Instant::now() + Duration::from_secs(2);
        while !*g {
            let now = Instant::now();
            if now >= end {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "daemon did not acknowledge Return"));
            }
            g = cv.wait_timeout(g, end - now).unwrap().0;
        }
        Ok(())
    }

    /// Another client wants this pane: give the pty back, then keep following the pane as a streamed client.
    fn release_and_restream(self: Arc<Self>) {
        self.transitioning.store(true, Ordering::Release);
        let ok = self.return_to_daemon().is_ok() && {
            let old = std::mem::replace(&mut *self.mode.lock().unwrap(), Mode::Closed);
            if let Mode::Direct(d) = &old {
                let _ = d.ctl.lock().unwrap().shutdown(std::net::Shutdown::Both);
            }
            let (cols, rows) = {
                let mut t = self.shared.term.lock().unwrap();
                t.set_respond(false); // the daemon's emulator answers again
                (t.cols() as u16, t.rows() as u16)
            };
            (|| -> io::Result<()> {
                let mut s = handshake(&self.path, "aleno-gui-pane")?;
                write_msg(&mut s, &Msg::AttachPane { session: self.session, pane: self.pane, cols, rows })?;
                match read_msg(&mut s)? {
                    Msg::Attached => {}
                    m => return Err(rpc_err(m)),
                }
                Core::start_streamed(&self, s)
            })()
            .is_ok()
        };
        // Deliver whatever was typed during the hand-off, then resume normal routing.
        let queued = std::mem::take(&mut *self.pending_input.lock().unwrap());
        if ok && !queued.is_empty() {
            if let Mode::Streamed { tx, .. } = &*self.mode.lock().unwrap() {
                for chunk in queued.chunks(MAX_INPUT / 2) {
                    let _ = tx.send(Msg::Input { data: chunk.to_vec() });
                }
            }
        }
        self.transitioning.store(false, Ordering::Release);
        if !ok {
            self.shared.disconnected.store(true, Ordering::Release);
            wake(&self.shared, &self.notify, EVENT_DISCONNECTED);
        }
    }
}

/// Gather stage: drain the pty into ring buffers, bridging the writer's refill gaps on saturated streams.
fn gather_thread(master: Arc<OwnedFd>, wake_r: OwnedFd, idle_r: OwnedFd, stop: Arc<AtomicBool>, ring: Arc<Ring>) {
    set_qos();
    let fd = master.as_raw_fd();
    'outer: loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        // Claim a free buffer. This only blocks when the parse stage is a full ring behind, which is exactly when we
        // should stop reading and let the kernel queue push back on the child.
        let mut buf = {
            let mut st = ring.st.lock().unwrap();
            loop {
                if let Some(b) = st.free.pop() {
                    break b;
                }
                if stop.load(Ordering::Acquire) {
                    break 'outer;
                }
                st = ring.slot_free.wait_timeout(st, Duration::from_millis(100)).unwrap().0;
            }
        };
        let mut total = 0usize;
        let mut spins = 0u32;
        let mut bridge_start: Option<Instant> = None;
        let mut fatal = false;
        'gather: while total < RING_CAPACITY {
            let n = unsafe { libc::read(fd, buf[total..].as_mut_ptr() as *mut _, RING_CAPACITY - total) };
            if n > 0 {
                total += n as usize;
                spins = 0;
                continue;
            }
            if n == 0 {
                break; // macOS returns 0 when the child dies; deliver what we have
            }
            let errno = io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if errno == libc::EINTR {
                continue;
            }
            if errno != libc::EAGAIN {
                fatal = true; // EIO: pty closed
                break;
            }
            // The kernel queue is momentarily dry.
            if total < BRIDGE_THRESHOLD {
                break 'gather; // interactive trickle: deliver immediately, add no latency
            }
            if spins < BRIDGE_SPIN_MAX {
                spins += 1;
                std::hint::spin_loop();
                continue;
            }
            let now = Instant::now();
            match bridge_start {
                Some(t) if now.duration_since(t) >= GATHER_BUDGET => break 'gather,
                None => bridge_start = Some(now),
                _ => {}
            }
            {
                let mut st = ring.st.lock().unwrap();
                if st.ready.is_empty() {
                    break 'gather; // parser is idle: waiting would add straight to latency
                }
                st.bridging = true;
            }
            let mut pfds = [
                libc::pollfd { fd, events: libc::POLLIN, revents: 0 },
                libc::pollfd { fd: wake_r.as_raw_fd(), events: libc::POLLIN, revents: 0 },
                libc::pollfd { fd: idle_r.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            ];
            let r = poll_fds(&mut pfds, BRIDGE_POLL_MS);
            ring.st.lock().unwrap().bridging = false;
            if r <= 0 || pfds[1].revents & libc::POLLIN != 0 {
                break 'gather; // quiet for a full timeout (burst over) or asked to stop
            }
            if pfds[2].revents & libc::POLLIN != 0 {
                drain_pipe(idle_r.as_raw_fd());
                break 'gather; // parser went idle: deliver now
            }
            if pfds[0].revents & libc::POLLIN == 0 {
                break 'gather;
            }
            spins = 0;
        }
        if total > 0 {
            ring.st.lock().unwrap().ready.push_back((buf, total));
            ring.batch_ready.notify_one();
        } else {
            ring.st.lock().unwrap().free.push(buf);
            // Nothing available: sleep until the pty is readable (or we are told to stop).
            let mut pfds = [
                libc::pollfd { fd, events: libc::POLLIN, revents: 0 },
                libc::pollfd { fd: wake_r.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            ];
            if !fatal && poll_fds(&mut pfds, 1000) > 0 && pfds[1].revents & libc::POLLIN != 0 {
                break;
            }
        }
        if fatal {
            break;
        }
    }
    let mut st = ring.st.lock().unwrap();
    st.done = true;
    drop(st);
    ring.batch_ready.notify_all();
}
