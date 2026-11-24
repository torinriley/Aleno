//! How fast can anything read a file's output from a pty? `cat <file>` runs on a pty and this process just reads
//! and discards, so the time is the floor for any terminal. usage: ptyfloor <file> [block|spin|spinpoll] [runs]
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let file = std::ffi::CString::new(a[1].clone()).unwrap();
    let mode = a.get(2).map(|s| s.as_str()).unwrap_or("spin").to_string();
    let runs: usize = a.get(3).map(|s| s.parse().unwrap()).unwrap_or(6);
    // READER_QOS=interactive|utility: the QoS the reading thread runs with (Aleno's gather thread uses user-interactive).
    match std::env::var("READER_QOS").as_deref() {
        Ok("interactive") => unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0); },
        Ok("initiated") => unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INITIATED, 0); },
        Ok("utility") => unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0); },
        _ => {}
    }
    let backoff: usize = std::env::var("BACKOFF").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut best = f64::MAX;
    let mut times = Vec::new();
    let mut cat_cpu = (0.0f64, 0.0f64);
    let mut eagains = 0usize;
    let (mut total, mut reads) = (0usize, 0usize);
    for _ in 0..runs {
        unsafe {
            let mut master: libc::c_int = 0;
            let mut ws = libc::winsize { ws_row: 34, ws_col: 124, ws_xpixel: 0, ws_ypixel: 0 };
            let pid = libc::forkpty(&mut master, std::ptr::null_mut(), std::ptr::null_mut(), &mut ws);
            if pid == 0 {
                if std::env::var_os("NO_OPOST").is_some() {
                    // Experiment: turn off the kernel's output processing (newline to CR LF) on the slave side.
                    let mut t: libc::termios = std::mem::zeroed();
                    libc::tcgetattr(0, &mut t);
                    t.c_oflag &= !libc::OPOST;
                    libc::tcsetattr(0, libc::TCSANOW, &t);
                }
                // CHILD_QOS=utility|background|default|interactive: the QoS the writer (cat) runs with.
                match std::env::var("CHILD_QOS").as_deref() {
                    Ok("utility") => { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0); }
                    Ok("background") => { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_BACKGROUND, 0); }
                    Ok("interactive") => { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0); }
                    _ => {}
                }
                if std::env::var_os("VIA_ZSH").is_some() {
                    // Run it the way a terminal does: inside an interactive zsh, which sets up the tty its own way.
                    let z = std::ffi::CString::new("/bin/zsh").unwrap();
                    let (f, i, c) = (std::ffi::CString::new("-f").unwrap(), std::ffi::CString::new("-i").unwrap(), std::ffi::CString::new("-c").unwrap());
                    let cmd = std::ffi::CString::new(format!("cat {}", a[1])).unwrap();
                    let args = [z.as_ptr(), f.as_ptr(), i.as_ptr(), c.as_ptr(), cmd.as_ptr(), std::ptr::null()];
                    libc::execv(z.as_ptr(), args.as_ptr());
                }
                if let Ok(prefix) = std::env::var("CHILD_PREFIX") {
                    // e.g. CHILD_PREFIX="taskpolicy -b": run cat under a wrapper command
                    let mut words: Vec<std::ffi::CString> = prefix.split_whitespace().map(|w| std::ffi::CString::new(w).unwrap()).collect();
                    words.push(std::ffi::CString::new("cat").unwrap());
                    words.push(file.clone());
                    let mut ptrs: Vec<*const libc::c_char> = words.iter().map(|w| w.as_ptr()).collect();
                    ptrs.push(std::ptr::null());
                    libc::execvp(words[0].as_ptr(), ptrs.as_ptr());
                }
                let cat = std::ffi::CString::new("cat").unwrap();
                let args = [cat.as_ptr(), file.as_ptr(), std::ptr::null()];
                libc::execvp(cat.as_ptr(), args.as_ptr());
                libc::_exit(127);
            }
            if mode != "block" {
                let fl = libc::fcntl(master, libc::F_GETFL);
                libc::fcntl(master, libc::F_SETFL, fl | libc::O_NONBLOCK);
            }
            // DEST=hot (default): every read goes to the start of one buffer, as a trivial reader would.
            // DEST=ring: reads advance through 64 KB buffers that rotate (four of them), like Aleno's gather stage.
            // DEST=ringtouch: the same, but the lines about to be written are pulled into the cache while we wait.
            let dest = std::env::var("DEST").unwrap_or_else(|_| "hot".into());
            let mut bufs: Vec<Vec<u8>> = (0..4).map(|_| vec![0u8; 64 * 1024]).collect();
            let (mut cur, mut off) = (0usize, 0usize);
            let mut buf = vec![0u8; 64 * 1024];
            let t0 = Instant::now();
            total = 0;
            reads = 0;
            let mut eagain_run = 0usize;
            loop {
                let n = if dest == "hot" {
                    libc::read(master, buf.as_mut_ptr() as *mut _, buf.len())
                } else {
                    let b = &mut bufs[cur];
                    libc::read(master, b[off..].as_mut_ptr() as *mut _, 64 * 1024 - off)
                };
                if n > 0 {
                    total += n as usize;
                    reads += 1;
                    if dest != "hot" {
                        off += n as usize;
                        if off >= 64 * 1024 - 1024 { cur = (cur + 1) % 4; off = 0; }
                    }
                    continue;
                }
                if n == 0 {
                    break;
                }
                let e = *libc::__error();
                if e == libc::EAGAIN {
                    eagain_run += 1;
                    if dest == "ringtouch" {
                        // Touch the next 2 KB of the destination (one byte per 64-byte line) so the lines are already
                        // here, writable, when the next read lands.
                        let b = &mut bufs[cur];
                        let end = (off + 2048).min(64 * 1024);
                        let mut k = off;
                        while k < end { unsafe { std::ptr::write_volatile(&mut b[k], 0) }; k += 64; }
                    }
                    if backoff > 0 {
                        // Pause in user space instead of hitting the tty again at once: fewer empty reads fighting
                        // the writer for the tty lock.
                        for _ in 0..backoff { std::hint::spin_loop(); }
                    }
                    if mode == "spinpoll" {
                        let mut p = libc::pollfd { fd: master, events: libc::POLLIN, revents: 0 };
                        libc::poll(&mut p, 1, 1);
                    }
                    continue; // spin: read again
                }
                break; // EIO: the child closed its side
            }
            let el = t0.elapsed().as_secs_f64();
            eagains = eagain_run;
            times.push(el);
            best = best.min(el);
            let mut st = 0;
            let mut ru: libc::rusage = std::mem::zeroed();
            libc::wait4(pid, &mut st, 0, &mut ru);
            cat_cpu = (ru.ru_utime.tv_sec as f64 + ru.ru_utime.tv_usec as f64 / 1e6, ru.ru_stime.tv_sec as f64 + ru.ru_stime.tv_usec as f64 / 1e6);
            libc::close(master);
        }
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = times[times.len() / 2];
    println!("last run: cat cpu user {:.3}s sys {:.3}s, {} empty reads", cat_cpu.0, cat_cpu.1, eagains);
    println!("{:<9} best {:.3}s median {:.3}s  {} MB in {} reads ({} B/read)", mode, best, med, total / 1_000_000, reads, total / reads.max(1));
}
