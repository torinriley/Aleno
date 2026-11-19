//! Aleno IPC protocol: versioned, length-framed binary messages over a Unix socket.
//!
//! Frame: `u32 LE length` (tag + payload) | `u8 tag` | payload. All integers are little-endian.
//! Strings and byte blobs are `u32 length` + data. Every decode is bounds-checked and frames
//! larger than [`MAX_FRAME`] are rejected before any allocation.

use std::io::{self, Read, Write};

pub const PROTOCOL_VERSION: u32 = 1;
/// Upper bound for one frame (snapshots are the largest messages).
pub const MAX_FRAME: usize = 16 << 20;
/// Upper bound for one `Input` message payload.
pub const MAX_INPUT: usize = 1 << 20;
const MAX_ITEMS: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: u32,
    pub name: String,
    pub created_unix: u64,
    pub panes: u32,
    pub clients: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneInfo {
    pub id: u32,
    pub alive: bool,
    pub title: String,
    pub cols: u16,
    pub rows: u16,
    pub cwd: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    // client -> daemon
    Hello { version: u32, client: String },
    CreateSession { name: String },
    ListSessions,
    KillSession { session: u32 },
    SetLayout { session: u32, json: String },
    GetLayout { session: u32 },
    /// `seed`: terminal output to replay into the new pane's screen before the shell starts (reopened tabs).
    CreatePane { session: u32, cols: u16, rows: u16, shell: String, cwd: String, scrollback: u32, seed: Vec<u8> },
    KillPane { session: u32, pane: u32 },
    ListPanes { session: u32 },
    AttachPane { session: u32, pane: u32, cols: u16, rows: u16 },
    Input { data: Vec<u8> },
    Resize { cols: u16, rows: u16 },
    Detach,
    Shutdown,
    /// Like `AttachPane`, but ask the daemon to hand the pty master fd to this client (SCM_RIGHTS) so it can read
    /// and write the pty directly. Answered by `Attached`, `Snapshot`, then `Granted` carrying the fd; or by
    /// `Err{DIRECT_UNAVAILABLE}` (use `AttachPane` instead).
    AttachDirect { session: u32, pane: u32, cols: u16, rows: u16 },
    /// Direct-mode owner gives the pty back, with its current screen state. Answered by `Ok`.
    Return { cols: u16, rows: u16, snapshot: Vec<u8> },
    /// Direct-mode owner periodically stores its screen state in the daemon (used if the owner dies).
    Checkpoint { cols: u16, rows: u16, snapshot: Vec<u8> },
    RenameSession { session: u32, name: String },
    // daemon -> client
    Welcome { version: u32, pid: u32 },
    Ok,
    Err { code: u32, message: String },
    SessionCreated { id: u32 },
    Sessions { list: Vec<SessionInfo> },
    Layout { json: String },
    PaneCreated { pane: u32 },
    Panes { list: Vec<PaneInfo> },
    Attached,
    Snapshot { data: Vec<u8> },
    Output { data: Vec<u8> },
    Title { title: String },
    Exited { code: i32 },
    /// Another client resized the pane; mirrors must follow.
    Resized { cols: u16, rows: u16 },
    /// The pty fd accompanies this frame as ancillary data.
    Granted,
    /// Another client wants this pane: the direct owner must send `Return`.
    Release,
}

pub mod err {
    pub const BAD_REQUEST: u32 = 1;
    pub const NO_SUCH_SESSION: u32 = 2;
    pub const NO_SUCH_PANE: u32 = 3;
    pub const SPAWN_FAILED: u32 = 4;
    pub const VERSION: u32 = 5;
    pub const LIMIT: u32 = 6;
    pub const DIRECT_UNAVAILABLE: u32 = 7;
}

struct Enc(Vec<u8>);
impl Enc {
    fn u8(&mut self, v: u8) {
        self.0.push(v)
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes())
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes())
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes())
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
    }
    fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes())
    }
}

struct Dec<'a>(&'a [u8]);
fn bad(m: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, m.to_string())
}
impl<'a> Dec<'a> {
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        if self.0.len() < n {
            return Err(bad("truncated message"));
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn bytes(&mut self) -> io::Result<Vec<u8>> {
        let n = self.u32()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    fn str(&mut self) -> io::Result<String> {
        let n = self.u32()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| bad("invalid utf-8"))
    }
    fn count(&mut self) -> io::Result<usize> {
        let n = self.u32()? as usize;
        if n > MAX_ITEMS {
            return Err(bad("too many items"));
        }
        Ok(n)
    }
}

impl Msg {
    fn tag(&self) -> u8 {
        use Msg::*;
        match self {
            Hello { .. } => 1,
            CreateSession { .. } => 2,
            ListSessions => 3,
            KillSession { .. } => 4,
            SetLayout { .. } => 5,
            GetLayout { .. } => 6,
            CreatePane { .. } => 7,
            KillPane { .. } => 8,
            ListPanes { .. } => 9,
            AttachPane { .. } => 10,
            Input { .. } => 11,
            Resize { .. } => 12,
            Detach => 13,
            Shutdown => 14,
            AttachDirect { .. } => 15,
            Return { .. } => 16,
            Checkpoint { .. } => 17,
            RenameSession { .. } => 18,
            Welcome { .. } => 32,
            Ok => 33,
            Err { .. } => 34,
            SessionCreated { .. } => 35,
            Sessions { .. } => 36,
            Layout { .. } => 37,
            PaneCreated { .. } => 38,
            Panes { .. } => 39,
            Attached => 40,
            Snapshot { .. } => 41,
            Output { .. } => 42,
            Title { .. } => 43,
            Exited { .. } => 44,
            Resized { .. } => 45,
            Granted => 46,
            Release => 47,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        use Msg::*;
        let mut e = Enc(Vec::with_capacity(32));
        e.0.extend_from_slice(&[0, 0, 0, 0]); // length placeholder
        e.u8(self.tag());
        match self {
            Hello { version, client } => {
                e.u32(*version);
                e.str(client);
            }
            CreateSession { name } => e.str(name),
            RenameSession { session, name } => {
                e.u32(*session);
                e.str(name);
            }
            ListSessions | Detach | Shutdown | Ok | Attached | Granted | Release => {}
            KillSession { session } | GetLayout { session } | ListPanes { session } => e.u32(*session),
            SetLayout { session, json } => {
                e.u32(*session);
                e.str(json);
            }
            CreatePane { session, cols, rows, shell, cwd, scrollback, seed } => {
                e.u32(*session);
                e.u16(*cols);
                e.u16(*rows);
                e.str(shell);
                e.str(cwd);
                e.u32(*scrollback);
                e.bytes(seed);
            }
            KillPane { session, pane } => {
                e.u32(*session);
                e.u32(*pane);
            }
            AttachPane { session, pane, cols, rows } => {
                e.u32(*session);
                e.u32(*pane);
                e.u16(*cols);
                e.u16(*rows);
            }
            Input { data } | Snapshot { data } | Output { data } => e.bytes(data),
            AttachDirect { session, pane, cols, rows } => {
                e.u32(*session);
                e.u32(*pane);
                e.u16(*cols);
                e.u16(*rows);
            }
            Return { cols, rows, snapshot } | Checkpoint { cols, rows, snapshot } => {
                e.u16(*cols);
                e.u16(*rows);
                e.bytes(snapshot);
            }
            Resize { cols, rows } => {
                e.u16(*cols);
                e.u16(*rows);
            }
            Welcome { version, pid } => {
                e.u32(*version);
                e.u32(*pid);
            }
            Err { code, message } => {
                e.u32(*code);
                e.str(message);
            }
            SessionCreated { id } => e.u32(*id),
            Sessions { list } => {
                e.u32(list.len() as u32);
                for s in list {
                    e.u32(s.id);
                    e.str(&s.name);
                    e.u64(s.created_unix);
                    e.u32(s.panes);
                    e.u32(s.clients);
                }
            }
            Layout { json } => e.str(json),
            PaneCreated { pane } => e.u32(*pane),
            Panes { list } => {
                e.u32(list.len() as u32);
                for p in list {
                    e.u32(p.id);
                    e.u8(p.alive as u8);
                    e.str(&p.title);
                    e.u16(p.cols);
                    e.u16(p.rows);
                    e.str(&p.cwd);
                }
            }
            Title { title } => e.str(title),
            Exited { code } => e.u32(*code as u32),
            Resized { cols, rows } => {
                e.u16(*cols);
                e.u16(*rows);
            }
        }
        let len = (e.0.len() - 4) as u32;
        e.0[0..4].copy_from_slice(&len.to_le_bytes());
        e.0
    }

    pub fn decode(buf: &[u8]) -> io::Result<Msg> {
        use Msg::*;
        let mut d = Dec(buf);
        let tag = d.u8()?;
        let m = match tag {
            1 => Hello { version: d.u32()?, client: d.str()? },
            2 => CreateSession { name: d.str()? },
            3 => ListSessions,
            4 => KillSession { session: d.u32()? },
            5 => SetLayout { session: d.u32()?, json: d.str()? },
            6 => GetLayout { session: d.u32()? },
            7 => CreatePane {
                session: d.u32()?,
                cols: d.u16()?,
                rows: d.u16()?,
                shell: d.str()?,
                cwd: d.str()?,
                scrollback: d.u32()?,
                seed: d.bytes()?,
            },
            8 => KillPane { session: d.u32()?, pane: d.u32()? },
            9 => ListPanes { session: d.u32()? },
            10 => AttachPane { session: d.u32()?, pane: d.u32()?, cols: d.u16()?, rows: d.u16()? },
            11 => {
                let data = d.bytes()?;
                if data.len() > MAX_INPUT {
                    return Err(bad("input too large"));
                }
                Input { data }
            }
            12 => Resize { cols: d.u16()?, rows: d.u16()? },
            13 => Detach,
            14 => Shutdown,
            15 => AttachDirect { session: d.u32()?, pane: d.u32()?, cols: d.u16()?, rows: d.u16()? },
            16 => Return { cols: d.u16()?, rows: d.u16()?, snapshot: d.bytes()? },
            17 => Checkpoint { cols: d.u16()?, rows: d.u16()?, snapshot: d.bytes()? },
            18 => RenameSession { session: d.u32()?, name: d.str()? },
            32 => Welcome { version: d.u32()?, pid: d.u32()? },
            33 => Ok,
            34 => Err { code: d.u32()?, message: d.str()? },
            35 => SessionCreated { id: d.u32()? },
            36 => {
                let n = d.count()?;
                let mut list = Vec::with_capacity(n);
                for _ in 0..n {
                    list.push(SessionInfo {
                        id: d.u32()?,
                        name: d.str()?,
                        created_unix: d.u64()?,
                        panes: d.u32()?,
                        clients: d.u32()?,
                    });
                }
                Sessions { list }
            }
            37 => Layout { json: d.str()? },
            38 => PaneCreated { pane: d.u32()? },
            39 => {
                let n = d.count()?;
                let mut list = Vec::with_capacity(n);
                for _ in 0..n {
                    list.push(PaneInfo {
                        id: d.u32()?,
                        alive: d.u8()? != 0,
                        title: d.str()?,
                        cols: d.u16()?,
                        rows: d.u16()?,
                        cwd: d.str()?,
                    });
                }
                Panes { list }
            }
            40 => Attached,
            41 => Snapshot { data: d.bytes()? },
            42 => Output { data: d.bytes()? },
            43 => Title { title: d.str()? },
            44 => Exited { code: d.u32()? as i32 },
            45 => Resized { cols: d.u16()?, rows: d.u16()? },
            46 => Granted,
            47 => Release,
            _ => return Result::Err(bad("unknown message tag")),
        };
        if !d.0.is_empty() {
            return Result::Err(bad("trailing bytes"));
        }
        Result::Ok(m)
    }
}

pub fn write_msg<W: Write>(w: &mut W, m: &Msg) -> io::Result<()> {
    let buf = m.encode();
    if buf.len() - 4 > MAX_FRAME {
        return Err(bad("frame too large"));
    }
    w.write_all(&buf)
}

/// Reads one frame. `Ok(None)` means clean EOF at a frame boundary.
pub fn read_msg_opt<R: Read>(r: &mut R) -> io::Result<Option<Msg>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let n = u32::from_le_bytes(len) as usize;
    if n == 0 || n > MAX_FRAME {
        return Err(bad("invalid frame length"));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf)?;
    Msg::decode(&buf).map(Some)
}

pub fn read_msg<R: Read>(r: &mut R) -> io::Result<Msg> {
    read_msg_opt(r)?.ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed"))
}

/// Send one already-framed message and pass `fd` along as SCM_RIGHTS ancillary data.
pub fn send_frame_with_fd(sock: std::os::fd::RawFd, frame: &[u8], fd: std::os::fd::RawFd) -> io::Result<()> {
    unsafe {
        let mut iov = libc::iovec { iov_base: frame.as_ptr() as *mut _, iov_len: frame.len() };
        let space = libc::CMSG_SPACE(std::mem::size_of::<std::os::fd::RawFd>() as u32) as usize;
        let mut cbuf = vec![0u8; space];
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cbuf.as_mut_ptr() as *mut _;
        msg.msg_controllen = space as _;
        let cm = libc::CMSG_FIRSTHDR(&msg);
        (*cm).cmsg_level = libc::SOL_SOCKET;
        (*cm).cmsg_type = libc::SCM_RIGHTS;
        (*cm).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<std::os::fd::RawFd>() as u32) as _;
        std::ptr::copy_nonoverlapping(&fd as *const _ as *const u8, libc::CMSG_DATA(cm), std::mem::size_of::<std::os::fd::RawFd>());
        let n = libc::sendmsg(sock, &msg, 0);
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        // A tiny frame is sent whole; finish defensively if the kernel took only part of it.
        let mut sent = n as usize;
        while sent < frame.len() {
            let m = libc::send(sock, frame[sent..].as_ptr() as *const _, frame.len() - sent, 0);
            if m < 0 {
                return Err(io::Error::last_os_error());
            }
            sent += m as usize;
        }
    }
    Ok(())
}

/// Read exactly `buf.len()` bytes from a stream socket, returning a file descriptor if one arrived as SCM_RIGHTS.
pub fn recv_exact_with_fd(sock: std::os::fd::RawFd, buf: &mut [u8]) -> io::Result<Option<std::os::fd::OwnedFd>> {
    use std::os::fd::FromRawFd;
    let mut got = 0usize;
    let mut fd_out = None;
    while got < buf.len() {
        unsafe {
            let mut iov = libc::iovec { iov_base: buf[got..].as_mut_ptr() as *mut _, iov_len: buf.len() - got };
            let space = libc::CMSG_SPACE(std::mem::size_of::<std::os::fd::RawFd>() as u32) as usize;
            let mut cbuf = vec![0u8; space];
            let mut msg: libc::msghdr = std::mem::zeroed();
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            msg.msg_control = cbuf.as_mut_ptr() as *mut _;
            msg.msg_controllen = space as _;
            let n = libc::recvmsg(sock, &mut msg, 0);
            if n < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed"));
            }
            got += n as usize;
            let mut cm = libc::CMSG_FIRSTHDR(&msg);
            while !cm.is_null() {
                if (*cm).cmsg_level == libc::SOL_SOCKET && (*cm).cmsg_type == libc::SCM_RIGHTS {
                    let mut fd: std::os::fd::RawFd = -1;
                    std::ptr::copy_nonoverlapping(libc::CMSG_DATA(cm), &mut fd as *mut _ as *mut u8, std::mem::size_of::<std::os::fd::RawFd>());
                    if fd >= 0 {
                        libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                        if fd_out.is_none() {
                            fd_out = Some(std::os::fd::OwnedFd::from_raw_fd(fd));
                        }
                    }
                }
                cm = libc::CMSG_NXTHDR(&msg, cm);
            }
        }
    }
    Ok(fd_out)
}

/// Read one framed message, also returning a file descriptor that arrived with it (if any).
pub fn read_msg_with_fd(sock: &std::os::unix::net::UnixStream) -> io::Result<(Msg, Option<std::os::fd::OwnedFd>)> {
    use std::os::fd::AsRawFd;
    let mut len = [0u8; 4];
    let fd = recv_exact_with_fd(sock.as_raw_fd(), &mut len)?;
    let n = u32::from_le_bytes(len) as usize;
    if n == 0 || n > MAX_FRAME {
        return Err(bad("invalid frame length"));
    }
    let mut body = vec![0u8; n];
    let fd2 = recv_exact_with_fd(sock.as_raw_fd(), &mut body)?;
    Ok((Msg::decode(&body)?, fd.or(fd2)))
}

/// Default per-user socket directory: `~/Library/Application Support/Aleno`.
pub fn support_dir() -> std::path::PathBuf {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_else(|| "/tmp".into());
    home.join("Library/Application Support/Aleno")
}

pub fn socket_path() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("ALENO_SOCKET") {
        return p.into();
    }
    support_dir().join("daemon.sock")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples() -> Vec<Msg> {
        vec![
            Msg::Hello { version: 1, client: "t".into() },
            Msg::CreateSession { name: "main".into() },
            Msg::ListSessions,
            Msg::RenameSession { session: 2, name: "work".into() },
            Msg::SetLayout { session: 3, json: "{\"a\":1}".into() },
            Msg::CreatePane { session: 1, cols: 80, rows: 24, shell: "".into(), cwd: "/tmp".into(), scrollback: 1000, seed: b"old\r\nscreen".to_vec() },
            Msg::AttachPane { session: 1, pane: 2, cols: 100, rows: 40 },
            Msg::Input { data: vec![1, 2, 3, 0xff] },
            Msg::Resize { cols: 1, rows: 2 },
            Msg::Err { code: 3, message: "nope".into() },
            Msg::Sessions {
                list: vec![SessionInfo { id: 1, name: "a".into(), created_unix: 99, panes: 2, clients: 1 }],
            },
            Msg::Panes {
                list: vec![PaneInfo { id: 1, alive: true, title: "t".into(), cols: 80, rows: 24, cwd: "/".into() }],
            },
            Msg::Snapshot { data: vec![7; 1000] },
            Msg::Output { data: vec![] },
            Msg::Exited { code: -1 },
            Msg::Ok,
        ]
    }

    #[test]
    fn roundtrip() {
        for m in samples() {
            let enc = m.encode();
            let mut cur = std::io::Cursor::new(enc);
            assert_eq!(read_msg(&mut cur).unwrap(), m);
        }
    }

    #[test]
    fn stream_of_frames() {
        let mut buf = Vec::new();
        for m in samples() {
            write_msg(&mut buf, &m).unwrap();
        }
        let mut cur = std::io::Cursor::new(buf);
        for m in samples() {
            assert_eq!(read_msg(&mut cur).unwrap(), m);
        }
        assert!(read_msg_opt(&mut cur).unwrap().is_none());
    }

    #[test]
    fn rejects_oversize_and_garbage() {
        let mut big = (MAX_FRAME as u32 + 1).to_le_bytes().to_vec();
        big.push(1);
        assert!(read_msg(&mut std::io::Cursor::new(big)).is_err());
        assert!(read_msg(&mut std::io::Cursor::new(vec![0, 0, 0, 0])).is_err());
        assert!(read_msg(&mut std::io::Cursor::new(vec![1, 0, 0, 0, 250])).is_err());
        // truncated string length claims more than available
        let mut e = Msg::CreateSession { name: "abc".into() }.encode();
        e[5] = 200;
        assert!(read_msg(&mut std::io::Cursor::new(e)).is_err());
        // truncated stream
        let e = Msg::Snapshot { data: vec![1; 10] }.encode();
        assert!(read_msg(&mut std::io::Cursor::new(e[..e.len() - 3].to_vec())).is_err());
    }

    #[test]
    fn fuzz_decode_never_panics() {
        let mut x: u64 = 88172645463325252;
        for _ in 0..20000 {
            let n = (x % 64) as usize;
            let mut b = Vec::new();
            for _ in 0..n {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                b.push(x as u8);
            }
            let _ = Msg::decode(&b);
        }
    }
}
