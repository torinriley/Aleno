//! Session registry and the per-connection request loop.

use crate::pane::{Pane, SpawnOptions};
use aleno_protocol::{err, read_msg, write_msg, Msg, SessionInfo, PROTOCOL_VERSION};
use std::collections::HashMap;
use std::io::{self, BufWriter, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_SESSIONS: usize = 256;
const MAX_PANES_PER_SESSION: usize = 256;
const MAX_LAYOUT: usize = 1 << 20;

pub struct Session {
    pub id: u32,
    name: Mutex<String>,
    pub created: u64,
    panes: Mutex<HashMap<u32, Arc<Pane>>>,
    layout: Mutex<String>,
    next_pane: AtomicU32,
}

impl Session {
    pub fn name(&self) -> String {
        self.name.lock().unwrap().clone()
    }
    pub fn pane(&self, id: u32) -> Option<Arc<Pane>> {
        self.panes.lock().unwrap().get(&id).cloned()
    }
    pub fn pane_ids(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.panes.lock().unwrap().keys().copied().collect();
        v.sort();
        v
    }
}

pub struct Server {
    sessions: Mutex<HashMap<u32, Arc<Session>>>,
    next_session: AtomicU32,
    pub shutdown: AtomicBool,
    connections: AtomicU32,
    next_conn: std::sync::atomic::AtomicU64,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Server {
    pub fn new() -> Arc<Server> {
        Arc::new(Server {
            sessions: Mutex::new(HashMap::new()),
            next_session: AtomicU32::new(1),
            shutdown: AtomicBool::new(false),
            connections: AtomicU32::new(0),
            next_conn: std::sync::atomic::AtomicU64::new(1),
        })
    }

    pub fn session(&self, id: u32) -> Option<Arc<Session>> {
        self.sessions.lock().unwrap().get(&id).cloned()
    }

    pub fn session_count(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }

    pub fn connection_count(&self) -> u32 {
        self.connections.load(Ordering::Relaxed)
    }

    pub fn create_session(&self, name: &str) -> Result<Arc<Session>, String> {
        let mut map = self.sessions.lock().unwrap();
        if map.len() >= MAX_SESSIONS {
            return Err("too many sessions".into());
        }
        let id = self.next_session.fetch_add(1, Ordering::Relaxed);
        let name: String = if name.is_empty() { format!("session-{}", id) } else { name.chars().take(128).collect() };
        let s = Arc::new(Session {
            id,
            name: Mutex::new(name),
            created: now(),
            panes: Mutex::new(HashMap::new()),
            layout: Mutex::new(String::new()),
            next_pane: AtomicU32::new(1),
        });
        map.insert(id, s.clone());
        drop(map);
        Ok(s)
    }

    pub fn kill_session(&self, id: u32) -> bool {
        let s = self.sessions.lock().unwrap().remove(&id);
        match s {
            Some(s) => {
                for p in s.panes.lock().unwrap().values() {
                    p.kill();
                }
                true
            }
            None => false,
        }
    }

    pub fn kill_all(&self) {
        let ids: Vec<u32> = self.sessions.lock().unwrap().keys().copied().collect();
        for id in ids {
            self.kill_session(id);
        }
    }

    /// Drop sessions whose panes have all exited and that nobody is attached to.
    pub fn reap_orphans(&self) {
        let mut map = self.sessions.lock().unwrap();
        map.retain(|_, session| {
            let panes = session.panes.lock().unwrap();
            panes.is_empty() || panes.values().any(|pane| pane.is_alive() || pane.client_count() > 0)
        });
    }

    pub fn list(&self) -> Vec<SessionInfo> {
        let map = self.sessions.lock().unwrap();
        let mut v: Vec<SessionInfo> = map
            .values()
            .map(|s| {
                let panes = s.panes.lock().unwrap();
                SessionInfo {
                    id: s.id,
                    name: s.name(),
                    created_unix: s.created,
                    panes: panes.len() as u32,
                    clients: panes.values().map(|p| p.client_count() as u32).sum(),
                }
            })
            .collect();
        v.sort_by_key(|s| s.id);
        v
    }

    /// Accept loop. Returns when `shutdown` is set (the listener must be non-blocking-with-poll or woken by a connect).
    pub fn serve(self: &Arc<Self>, listener: UnixListener) {
        listener.set_nonblocking(true).ok();
        let uid = unsafe { libc::getuid() };
        let lfd = listener.as_raw_fd();
        while !self.shutdown.load(Ordering::Acquire) {
            // Sleep in poll() so a new connection wakes us immediately (no fixed-interval polling latency),
            // while still noticing `shutdown` within half a second.
            let mut pfd = libc::pollfd { fd: lfd, events: libc::POLLIN, revents: 0 };
            let r = unsafe { libc::poll(&mut pfd, 1, 500) };
            if r <= 0 {
                continue;
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    // Only the owning user may talk to the daemon.
                    let mut peer_uid: libc::uid_t = 0;
                    let mut peer_gid: libc::gid_t = 0;
                    let r = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut peer_uid, &mut peer_gid) };
                    if r != 0 || peer_uid != uid {
                        continue;
                    }
                    stream.set_nonblocking(false).ok();
                    let me = self.clone();
                    let _ = std::thread::Builder::new().name("conn".into()).spawn(move || {
                        me.connections.fetch_add(1, Ordering::Relaxed);
                        let _ = me.handle(stream);
                        me.connections.fetch_sub(1, Ordering::Relaxed);
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }

    fn handle(&self, stream: UnixStream) -> io::Result<()> {
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut rd = stream.try_clone()?;
        let mut wr = BufWriter::new(stream.try_clone()?);

        match read_msg(&mut rd)? {
            Msg::Hello { version, .. } if version == PROTOCOL_VERSION && !self.shutdown.load(Ordering::Acquire) => {
                write_msg(&mut wr, &Msg::Welcome { version: PROTOCOL_VERSION, pid: std::process::id() })?;
                wr.flush()?;
            }
            Msg::Hello { .. } => {
                write_msg(&mut wr, &Msg::Err { code: err::VERSION, message: "unsupported protocol version".into() })?;
                wr.flush()?;
                return Ok(());
            }
            _ => return Ok(()),
        }
        stream.set_read_timeout(None)?;

        let conn_id = self.next_conn.fetch_add(1, Ordering::Relaxed);
        let mut attached: Option<(Arc<Pane>, u64)> = None;
        // Pane whose pty this connection currently owns (direct mode).
        let mut direct: Option<Arc<Pane>> = None;
        let result = (|| -> io::Result<()> {
            loop {
                let msg = match read_msg(&mut rd) {
                    Ok(m) => m,
                    Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
                    Err(e) => return Err(e),
                };
                // Direct-mode owner: the pty lives in the window; only state hand-back messages arrive here.
                if let Some(pane) = &direct {
                    match msg {
                        Msg::Checkpoint { cols, rows, snapshot } => pane.set_checkpoint(conn_id, cols, rows, snapshot),
                        Msg::Return { cols, rows, snapshot } => {
                            let ok = pane.reclaim_with_state(conn_id, cols, rows, &snapshot);
                            direct = None;
                            write_msg(&mut wr, &if ok { Msg::Ok } else { Msg::Err { code: err::BAD_REQUEST, message: "not the owner".into() } })?;
                            wr.flush()?;
                        }
                        Msg::Detach => return Ok(()),
                        _ => {}
                    }
                    continue;
                }
                // Once attached, this connection is a dedicated pane stream.
                if let Some((pane, _)) = &attached {
                    match msg {
                        Msg::Input { data } => {
                            if pane.write_input(&data).is_err() {
                                return Ok(());
                            }
                        }
                        Msg::Resize { cols, rows } => pane.resize(cols, rows),
                        Msg::Detach => return Ok(()),
                        _ => {}
                    }
                    continue;
                }
                let reply = match msg {
                    Msg::CreateSession { name } => match self.create_session(&name) {
                        Ok(s) => Msg::SessionCreated { id: s.id },
                        Err(m) => Msg::Err { code: err::LIMIT, message: m },
                    },
                    Msg::ListSessions => Msg::Sessions { list: self.list() },
                    Msg::RenameSession { session, name } => match self.session(session) {
                        Some(s) => {
                            let name: String = name.chars().filter(|c| !c.is_control()).take(128).collect();
                            if !name.trim().is_empty() {
                                *s.name.lock().unwrap() = name;
                            }
                            Msg::Ok
                        }
                        None => Msg::Err { code: err::NO_SUCH_SESSION, message: "no such session".into() },
                    },
                    Msg::KillSession { session } => {
                        if self.kill_session(session) {
                            Msg::Ok
                        } else {
                            Msg::Err { code: err::NO_SUCH_SESSION, message: "no such session".into() }
                        }
                    }
                    Msg::SetLayout { session, json } => match self.session(session) {
                        Some(s) if json.len() <= MAX_LAYOUT => {
                            *s.layout.lock().unwrap() = json;
                            Msg::Ok
                        }
                        Some(_) => Msg::Err { code: err::LIMIT, message: "layout too large".into() },
                        None => Msg::Err { code: err::NO_SUCH_SESSION, message: "no such session".into() },
                    },
                    Msg::GetLayout { session } => match self.session(session) {
                        Some(s) => Msg::Layout { json: s.layout.lock().unwrap().clone() },
                        None => Msg::Err { code: err::NO_SUCH_SESSION, message: "no such session".into() },
                    },
                    Msg::CreatePane { session, cols, rows, shell, cwd, scrollback, seed, welcome } => match self.session(session) {
                        None => Msg::Err { code: err::NO_SUCH_SESSION, message: "no such session".into() },
                        Some(s) => {
                            if s.panes.lock().unwrap().len() >= MAX_PANES_PER_SESSION {
                                Msg::Err { code: err::LIMIT, message: "too many panes".into() }
                            } else {
                                let id = s.next_pane.fetch_add(1, Ordering::Relaxed);
                                let opts = SpawnOptions { cols, rows, shell, cwd, scrollback: scrollback as usize, seed, welcome };
                                match Pane::spawn(id, &opts) {
                                    Ok(p) => {
                                        s.panes.lock().unwrap().insert(id, p);
                                        Msg::PaneCreated { pane: id }
                                    }
                                    Err(e) => Msg::Err { code: err::SPAWN_FAILED, message: e.to_string() },
                                }
                            }
                        }
                    },
                    Msg::KillPane { session, pane } => match self.session(session) {
                        None => Msg::Err { code: err::NO_SUCH_SESSION, message: "no such session".into() },
                        Some(s) => {
                            let p = s.panes.lock().unwrap().remove(&pane);
                            match p {
                                Some(p) => {
                                    p.kill();
                                    Msg::Ok
                                }
                                None => Msg::Err { code: err::NO_SUCH_PANE, message: "no such pane".into() },
                            }
                        }
                    },
                    Msg::ListPanes { session } => match self.session(session) {
                        None => Msg::Err { code: err::NO_SUCH_SESSION, message: "no such session".into() },
                        Some(s) => {
                            let mut list: Vec<_> = s.panes.lock().unwrap().values().map(|p| p.info()).collect();
                            list.sort_by_key(|p| p.id);
                            Msg::Panes { list }
                        }
                    },
                    Msg::AttachDirect { session, pane, cols, rows } => {
                        match self.session(session).and_then(|s| s.pane(pane)) {
                            None => Msg::Err { code: err::NO_SUCH_PANE, message: "no such pane".into() },
                            Some(p) => match p.grant_direct(conn_id, cols, rows, stream.try_clone()?) {
                                None => Msg::Err { code: err::DIRECT_UNAVAILABLE, message: "direct attach unavailable".into() },
                                Some((snap, fd)) => {
                                    write_msg(&mut wr, &Msg::Attached)?;
                                    write_msg(&mut wr, &Msg::Snapshot { data: snap })?;
                                    wr.flush()?;
                                    // The fd rides along with the `Granted` frame.
                                    aleno_protocol::send_frame_with_fd(stream.as_raw_fd(), &Msg::Granted.encode(), fd)?;
                                    direct = Some(p);
                                    continue;
                                }
                            },
                        }
                    }
                    Msg::AttachPane { session, pane, cols, rows } => {
                        match self.session(session).and_then(|s| s.pane(pane)) {
                            None => Msg::Err { code: err::NO_SUCH_PANE, message: "no such pane".into() },
                            Some(p) => {
                                write_msg(&mut wr, &Msg::Attached)?;
                                wr.flush()?;
                                let (sink_id, rx) = p.attach(cols, rows);
                                attached = Some((p, sink_id));
                                // Dedicated writer thread: only it writes from now on.
                                let mut w = wr.get_ref().try_clone()?;
                                let sock = stream.try_clone()?;
                                std::thread::Builder::new().name("conn-writer".into()).spawn(move || {
                                    crate::pane::set_interactive_qos();
                                    let mut bw = BufWriter::with_capacity(128 * 1024, &mut w);
                                    'outer: while let Ok(m) = rx.recv() {
                                        if write_msg(&mut bw, &m).is_err() {
                                            break;
                                        }
                                        // Opportunistically batch queued messages before flushing.
                                        while let Ok(m) = rx.try_recv() {
                                            if write_msg(&mut bw, &m).is_err() {
                                                break 'outer;
                                            }
                                        }
                                        if bw.flush().is_err() {
                                            break;
                                        }
                                    }
                                    let _ = sock.shutdown(std::net::Shutdown::Both);
                                })?;
                                continue;
                            }
                        }
                    }
                    Msg::Shutdown => {
                        self.shutdown.store(true, Ordering::Release);
                        write_msg(&mut wr, &Msg::Ok)?;
                        wr.flush()?;
                        self.shutdown.store(true, Ordering::Release);
                        return Ok(());
                    }
                    Msg::Detach => return Ok(()),
                    _ => Msg::Err { code: err::BAD_REQUEST, message: "unexpected message".into() },
                };
                write_msg(&mut wr, &reply)?;
                wr.flush()?;
            }
        })();
        if let Some((p, id)) = attached {
            p.detach(id);
        }
        // The owner vanished without returning the pty: take it back from its last checkpoint.
        if let Some(p) = direct {
            p.release_owner(conn_id);
        }
        result
    }
}
