//! Daemon pipeline ceiling: attach with a raw socket (no terminal parsing on the client) and count bytes.
use aleno_client::*;
use aleno_protocol::*;
use std::os::unix::net::UnixStream;
use std::process::Command;
use std::time::{Duration, Instant};
fn main() {
    let dir = std::env::temp_dir().join(format!("aleno-raw-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("d.sock");
    let exe = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/release/aleno-daemon");
    let mut d = Command::new(&exe).arg("run").env("ALENO_SOCKET", &sock).env("ALENO_IDLE_EXIT_SECS", "0").spawn().unwrap();
    std::thread::sleep(Duration::from_millis(400));
    let ctl = Ctl::connect(&sock).unwrap();
    let s = ctl.create_session("raw").unwrap();
    let p = ctl.create_pane(s, 200, 60, "/bin/sh", "/tmp", 10_000).unwrap();
    let mut c0 = UnixStream::connect(&sock).unwrap();
    let mut w = c0.try_clone().unwrap();
    let mut c = std::io::BufReader::with_capacity(256 * 1024, c0.try_clone().unwrap());
    let _ = &mut c0;
    write_msg(&mut w, &Msg::Hello { version: PROTOCOL_VERSION, client: "raw".into() }).unwrap();
    read_msg(&mut c).unwrap();
    write_msg(&mut w, &Msg::AttachPane { session: s, pane: p, cols: 200, rows: 60 }).unwrap();
    read_msg(&mut c).unwrap();
    write_msg(&mut w, &Msg::Input { data: b"cat /tmp/ag/big.txt; echo RAWDONE$((1+1))\n".to_vec() }).unwrap();
    let st = Instant::now();
    let (mut bytes, mut msgs, mut tail) = (0usize, 0usize, Vec::new());
    loop {
        match read_msg(&mut c).unwrap() {
            Msg::Output { data } => { bytes += data.len(); msgs += 1; tail.extend_from_slice(&data); if tail.len() > 64 { tail.drain(..tail.len() - 64); } if tail.windows(9).any(|w| w == b"RAWDONE2\r") || String::from_utf8_lossy(&tail).contains("RAWDONE2") && bytes > 100_000_000 { break; } }
            _ => {}
        }
    }
    let secs = st.elapsed().as_secs_f64();
    println!("raw socket client: {:.0} MB in {:.2}s = {:.0} MB/s; {} messages (avg {:.1} KB each)", bytes as f64 / 1e6, secs, bytes as f64 / 1e6 / secs, msgs, bytes as f64 / msgs as f64 / 1024.0);
    let _ = Command::new(&exe).arg("stop").env("ALENO_SOCKET", &sock).status();
    let _ = d.wait();
}
