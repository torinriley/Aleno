//! aleno-daemon: per-user background process that owns PTYs and terminal sessions.
//!
//! Commands:
//!   run               run in the foreground (used by launchd and by `start`)
//!   start             ensure a daemon is running; detach it into its own session if not
//!   stop              ask the daemon to shut down (terminates all sessions)
//!   status | list     print daemon / session information
//!   install-launchd   install a LaunchAgent that starts the daemon at login
//!   uninstall-launchd remove it

use aleno_protocol::*;
use aleno_session::Server;
use std::fs::{self, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static SIGNALLED: AtomicBool = AtomicBool::new(false);

extern "C" {
    /// macOS: return freed-but-retained heap pages to the OS.
    fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
}
extern "C" fn on_signal(_: libc::c_int) {
    SIGNALLED.store(true, Ordering::SeqCst);
}

fn connect() -> std::io::Result<UnixStream> {
    let mut s = UnixStream::connect(socket_path())?;
    s.set_read_timeout(Some(Duration::from_secs(3)))?;
    write_msg(&mut s, &Msg::Hello { version: PROTOCOL_VERSION, client: "aleno-daemon-cli".into() })?;
    match read_msg(&mut s)? {
        Msg::Welcome { .. } => Ok(s),
        m => Err(std::io::Error::new(std::io::ErrorKind::Other, format!("unexpected reply {:?}", m))),
    }
}

fn ensure_dir() -> std::io::Result<()> {
    let dir = socket_path().parent().map(|p| p.to_path_buf()).unwrap_or_else(support_dir);
    if !dir.exists() {
        fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
    } else if dir == support_dir() {
        // Only tighten permissions on Aleno's own directory, never on a shared one such as /tmp.
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn run(app_owned: bool) -> i32 {
    if let Err(e) = ensure_dir() {
        eprintln!("aleno-daemon: cannot create {:?}: {}", support_dir(), e);
        return 1;
    }
    let sock = socket_path();
    let lock_path = sock.with_extension("lock");
    let lock = match OpenOptions::new().create(true).write(true).truncate(false).mode_600().open(&lock_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("aleno-daemon: lock file: {}", e);
            return 1;
        }
    };
    // Single-instance coordination: the advisory lock lives as long as the process does.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        eprintln!("aleno-daemon: already running");
        return 0;
    }
    let _ = fs::remove_file(&sock);
    let old = unsafe { libc::umask(0o177) };
    let listener = UnixListener::bind(&sock);
    unsafe { libc::umask(old) };
    let listener = match listener {
        Ok(l) => l,
        Err(e) => {
            eprintln!("aleno-daemon: bind {:?}: {}", sock, e);
            return 1;
        }
    };
    let _ = fs::set_permissions(&sock, fs::Permissions::from_mode(0o600));

    unsafe {
        libc::signal(libc::SIGTERM, on_signal as usize);
        libc::signal(libc::SIGINT, on_signal as usize);
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }
    let server = Server::new();
    if app_owned {
        let shutdown_server = server.clone();
        std::thread::spawn(move || {
            // The GUI holds the only write end. EOF also handles crashes and force quits.
            let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
            shutdown_server.shutdown.store(true, Ordering::Release);
            let _ = UnixStream::connect(socket_path()); // wake the accept loop
        });
    }
    let idle_limit: u64 = std::env::var("ALENO_IDLE_EXIT_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(600);
    {
        let server = server.clone();
        std::thread::spawn(move || {
            let mut idle_since: Option<Instant> = None;
            let mut ticks = 0u32;
            loop {
                std::thread::sleep(Duration::from_millis(1000));
                if SIGNALLED.load(Ordering::SeqCst) {
                    server.shutdown.store(true, Ordering::SeqCst);
                    return;
                }
                if server.shutdown.load(Ordering::SeqCst) {
                    return;
                }
                server.reap_orphans();
                ticks += 1;
                // Give memory back after bursts of output (every ~10 s; cheap when there is nothing to free).
                if ticks % 10 == 0 {
                    unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) };
                }
                if idle_limit > 0 && server.session_count() == 0 && server.connection_count() == 0 {
                    let t = *idle_since.get_or_insert_with(Instant::now);
                    if t.elapsed().as_secs() >= idle_limit {
                        server.shutdown.store(true, Ordering::SeqCst);
                        return;
                    }
                } else {
                    idle_since = None;
                }
            }
        });
    }
    eprintln!("aleno-daemon {} listening on {:?} (pid {})", env!("CARGO_PKG_VERSION"), sock, std::process::id());
    server.serve(listener);
    // Graceful shutdown: hang up every shell, remove the socket.
    server.kill_all();
    std::thread::sleep(Duration::from_millis(200));
    let _ = fs::remove_file(&sock);
    if app_owned {
        if let Some(dir) = sock.parent() {
            for name in ["zsh/.zshenv", "bash/aleno.bash", "fish/vendor_conf.d/aleno-integration.fish"] {
                let _ = fs::remove_file(dir.join("shell-integration").join(name));
            }
            for name in ["fish/vendor_conf.d", "fish", "bash", "zsh", ""] {
                let _ = fs::remove_dir(dir.join("shell-integration").join(name));
            }
            let _ = fs::remove_file(&lock_path);
            let _ = fs::remove_dir(dir);
        }
    }
    eprintln!("aleno-daemon: shut down");
    0
}

trait Mode600 {
    fn mode_600(&mut self) -> &mut Self;
}
impl Mode600 for OpenOptions {
    fn mode_600(&mut self) -> &mut Self {
        std::os::unix::fs::OpenOptionsExt::mode(self, 0o600)
    }
}

fn start() -> i32 {
    if connect().is_ok() {
        return 0;
    }
    if ensure_dir().is_err() {
        return 1;
    }
    let exe = std::env::current_exe().expect("current_exe");
    let log_dir = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default().join("Library/Logs/Aleno");
    let _ = fs::create_dir_all(&log_dir);
    let log = File::options().create(true).append(true).open(log_dir.join("daemon.log"));
    let mut cmd = Command::new(exe);
    cmd.arg("run").stdin(Stdio::null());
    match log {
        Ok(f) => {
            cmd.stdout(Stdio::from(f.try_clone().unwrap())).stderr(Stdio::from(f));
        }
        Err(_) => {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid(); // survive the launching GUI
            Ok(())
        });
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => { eprintln!("aleno-daemon: spawn: {}", e); return 1; }
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if connect().is_ok() {
            return 0;
        }
        // A previous daemon may still hold the instance lock while shutting down.
        // Reap the losing starter and retry, rather than waiting five seconds for
        // a child that has already exited and can never open the socket.
        if matches!(child.try_wait(), Ok(Some(_))) {
            std::thread::sleep(Duration::from_millis(20));
            child = match cmd.spawn() {
                Ok(child) => child,
                Err(e) => { eprintln!("aleno-daemon: spawn: {}", e); return 1; }
            };
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    eprintln!("aleno-daemon: failed to start");
    1
}

fn plist_path() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Library/LaunchAgents/com.aleno.daemon.plist")
}

fn install_launchd() -> i32 {
    let exe = std::env::current_exe().expect("current_exe");
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.aleno.daemon</string>
  <key>ProgramArguments</key><array><string>{}</string><string>run</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ProcessType</key><string>Interactive</string>
  <key>EnvironmentVariables</key><dict><key>ALENO_IDLE_EXIT_SECS</key><string>0</string></dict>
  <key>StandardErrorPath</key><string>{}/Library/Logs/Aleno/daemon.log</string>
</dict>
</plist>
"#,
        exe.display(),
        std::env::var("HOME").unwrap_or_default()
    );
    let _ = fs::create_dir_all(std::env::var("HOME").unwrap_or_default() + "/Library/Logs/Aleno");
    if fs::write(plist_path(), plist).is_err() {
        return 1;
    }
    let uid = unsafe { libc::getuid() };
    let st = Command::new("launchctl").args(["bootstrap", &format!("gui/{}", uid)]).arg(plist_path()).status();
    println!("installed {}", plist_path().display());
    if matches!(st, Ok(s) if s.success()) { 0 } else { 1 }
}

fn uninstall_launchd() -> i32 {
    let uid = unsafe { libc::getuid() };
    let _ = Command::new("launchctl").args(["bootout", &format!("gui/{}/com.aleno.daemon", uid)]).status();
    let _ = fs::remove_file(plist_path());
    0
}

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| "run".into());
    let code = match arg.as_str() {
        "run" => run(false),
        "run-app" => run(true),
        "start" => start(),
        "stop" => match connect() {
            Ok(mut s) => {
                let _ = write_msg(&mut s, &Msg::Shutdown);
                let _ = read_msg(&mut s);
                0
            }
            Err(_) => {
                eprintln!("daemon not running");
                1
            }
        },
        "status" | "list" => match connect() {
            Ok(mut s) => {
                write_msg(&mut s, &Msg::ListSessions).unwrap();
                if let Ok(Msg::Sessions { list }) = read_msg(&mut s) {
                    println!("daemon running at {:?}, {} session(s)", socket_path(), list.len());
                    for i in list {
                        println!("  session {} \"{}\": {} pane(s), {} client(s)", i.id, i.name, i.panes, i.clients);
                    }
                }
                0
            }
            Err(_) => {
                println!("daemon not running");
                1
            }
        },
        "install-launchd" => install_launchd(),
        "uninstall-launchd" => uninstall_launchd(),
        _ => {
            eprintln!("usage: aleno-daemon [run|start|stop|status|install-launchd|uninstall-launchd]");
            2
        }
    };
    std::process::exit(code);
}
