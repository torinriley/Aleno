//! C ABI for the Swift app. All pointers are owned by the caller until the matching `*_free`.
//! Strings returned by this module are malloc'd by Rust and must be released with
//! `aleno_string_free`. Functions never unwind across the boundary (panic = abort in release).

use crate::*;
use aleno_terminal::{BlockInfo, Cell, FinishedInfo, ViewInfo};
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::PathBuf;
use std::sync::atomic::Ordering;

unsafe fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

fn to_c(s: String) -> *mut c_char {
    CString::new(s.replace('\0', "")).unwrap().into_raw()
}

#[no_mangle]
pub unsafe extern "C" fn aleno_string_free(p: *mut c_char) {
    if !p.is_null() {
        drop(CString::from_raw(p));
    }
}

/// Default daemon socket path (`ALENO_SOCKET` or Application Support).
#[no_mangle]
pub extern "C" fn aleno_socket_path() -> *mut c_char {
    to_c(aleno_protocol::socket_path().to_string_lossy().into_owned())
}

thread_local! {
    static CONTROL_ERROR: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// Last control error on this thread. Caller frees the returned string.
#[no_mangle]
pub extern "C" fn aleno_ctl_last_error() -> *mut c_char {
    CONTROL_ERROR.with(|e| to_c(e.borrow().clone()))
}

// ---------------------------------------------------------------- control

#[no_mangle]
pub unsafe extern "C" fn aleno_ctl_connect(path: *const c_char) -> *mut Ctl {
    let p = PathBuf::from(cstr(path));
    match Ctl::connect(&p) {
        Ok(c) => Box::into_raw(Box::new(c)),
        Err(e) => { CONTROL_ERROR.with(|v| *v.borrow_mut() = e.to_string()); std::ptr::null_mut() },
    }
}

#[no_mangle]
pub unsafe extern "C" fn aleno_ctl_free(c: *mut Ctl) {
    if !c.is_null() {
        drop(Box::from_raw(c));
    }
}

#[no_mangle]
pub unsafe extern "C" fn aleno_ctl_create_session(c: &Ctl, name: *const c_char) -> i64 {
    match c.create_session(&cstr(name)) {
        Ok(id) => id as i64,
        Err(e) => { CONTROL_ERROR.with(|v| *v.borrow_mut() = e.to_string()); -1 },
    }
}

#[no_mangle]
pub unsafe extern "C" fn aleno_ctl_rename_session(c: &Ctl, session: u32, name: *const c_char) -> i32 {
    c.rename_session(session, &cstr(name)).is_ok() as i32
}

#[no_mangle]
pub unsafe extern "C" fn aleno_ctl_kill_session(c: &Ctl, session: u32) -> i32 {
    c.kill_session(session).is_ok() as i32
}

/// JSON: `[{"id":1,"name":"..","created":123,"panes":2,"clients":0}, ...]`, or NULL on error.
#[no_mangle]
pub unsafe extern "C" fn aleno_ctl_list_sessions_json(c: &Ctl) -> *mut c_char {
    match c.list_sessions() {
        Ok(list) => {
            let v: Vec<serde_json::Value> = list
                .iter()
                .map(|s| serde_json::json!({"id": s.id, "name": s.name, "created": s.created_unix, "panes": s.panes, "clients": s.clients}))
                .collect();
            to_c(serde_json::Value::Array(v).to_string())
        }
        Err(e) => { CONTROL_ERROR.with(|v| *v.borrow_mut() = e.to_string()); std::ptr::null_mut() },
    }
}

#[no_mangle]
pub unsafe extern "C" fn aleno_ctl_list_panes_json(c: &Ctl, session: u32) -> *mut c_char {
    match c.list_panes(session) {
        Ok(list) => {
            let v: Vec<serde_json::Value> = list
                .iter()
                .map(|p| serde_json::json!({"id": p.id, "alive": p.alive, "title": p.title, "cols": p.cols, "rows": p.rows, "cwd": p.cwd}))
                .collect();
            to_c(serde_json::Value::Array(v).to_string())
        }
        Err(e) => { CONTROL_ERROR.with(|v| *v.borrow_mut() = e.to_string()); std::ptr::null_mut() },
    }
}

#[no_mangle]
pub unsafe extern "C" fn aleno_ctl_set_layout(c: &Ctl, session: u32, json: *const c_char) -> i32 {
    c.set_layout(session, &cstr(json)).is_ok() as i32
}

#[no_mangle]
pub unsafe extern "C" fn aleno_ctl_get_layout(c: &Ctl, session: u32) -> *mut c_char {
    match c.get_layout(session) {
        Ok(j) => to_c(j),
        Err(e) => { CONTROL_ERROR.with(|v| *v.borrow_mut() = e.to_string()); std::ptr::null_mut() },
    }
}

#[no_mangle]
pub unsafe extern "C" fn aleno_ctl_create_pane(
    c: &Ctl,
    session: u32,
    cols: u16,
    rows: u16,
    shell: *const c_char,
    cwd: *const c_char,
    scrollback: u32,
) -> i64 {
    match c.create_pane(session, cols, rows, &cstr(shell), &cstr(cwd), scrollback) {
        Ok(id) => id as i64,
        Err(e) => { CONTROL_ERROR.with(|v| *v.borrow_mut() = e.to_string()); -1 },
    }
}

/// Like `aleno_ctl_create_pane`, replaying `seed` (NUL-terminated output of `aleno_pane_snapshot`) onto the new pane first.
#[no_mangle]
pub unsafe extern "C" fn aleno_ctl_create_pane_seeded(
    c: &Ctl,
    session: u32,
    cols: u16,
    rows: u16,
    shell: *const c_char,
    cwd: *const c_char,
    scrollback: u32,
    seed: *const c_char,
    welcome: i32,
) -> i64 {
    let seed = if seed.is_null() { Vec::new() } else { std::ffi::CStr::from_ptr(seed).to_bytes().to_vec() };
    match c.create_pane_full(session, cols, rows, &cstr(shell), &cstr(cwd), scrollback, &seed, welcome != 0) {
        Ok(id) => id as i64,
        Err(e) => { CONTROL_ERROR.with(|v| *v.borrow_mut() = e.to_string()); -1 },
    }
}

#[no_mangle]
pub unsafe extern "C" fn aleno_ctl_kill_pane(c: &Ctl, session: u32, pane: u32) -> i32 {
    c.kill_pane(session, pane).is_ok() as i32
}

// ---------------------------------------------------------------- pane stream

pub type AlenoNotify = extern "C" fn(*mut c_void, u32);

struct SendPtr(*mut c_void);
unsafe impl Send for SendPtr {}
unsafe impl Sync for SendPtr {}

#[no_mangle]
pub unsafe extern "C" fn aleno_pane_attach(
    path: *const c_char,
    session: u32,
    pane: u32,
    cols: u16,
    rows: u16,
    scrollback: u32,
    notify: AlenoNotify,
    userdata: *mut c_void,
) -> *mut PaneClient {
    let ud = SendPtr(userdata);
    let cb: Notify = Box::new(move |ev| {
        let _ = &ud;
        notify(ud.0, ev)
    });
    match PaneClient::attach(&PathBuf::from(cstr(path)), session, pane, cols, rows, scrollback as usize, cb) {
        Ok(p) => Box::into_raw(Box::new(p)),
        Err(e) => { CONTROL_ERROR.with(|v| *v.borrow_mut() = e.to_string()); std::ptr::null_mut() },
    }
}

#[no_mangle]
pub unsafe extern "C" fn aleno_pane_free(p: *mut PaneClient) {
    if !p.is_null() {
        drop(Box::from_raw(p));
    }
}

#[no_mangle]
pub unsafe extern "C" fn aleno_pane_input(p: &PaneClient, data: *const u8, len: usize) {
    if !data.is_null() {
        p.send_input(std::slice::from_raw_parts(data, len));
    }
}

#[no_mangle]
pub unsafe extern "C" fn aleno_pane_resize(p: &PaneClient, cols: u16, rows: u16) {
    p.resize(cols, rows);
}

/// Copy dirty rows into caller-owned buffers. See `ViewInfo::needs_realloc`.
#[no_mangle]
pub unsafe extern "C" fn aleno_pane_copy_view(
    p: &PaneClient,
    expect_cols: u32,
    expect_rows: u32,
    cells: *mut Cell,
    cells_cap: usize,
    dirty: *mut u8,
    dirty_cap: usize,
    info: *mut ViewInfo,
) {
    let sh = &p.shared;
    sh.ui_waiting.store(true, Ordering::Release);
    let mut t = sh.term.lock().unwrap();
    sh.ui_waiting.store(false, Ordering::Release);
    sh.wake_pending.store(false, Ordering::Release);
    let cells = std::slice::from_raw_parts_mut(cells, cells_cap);
    let dirty = std::slice::from_raw_parts_mut(dirty, dirty_cap);
    *info = t.copy_view(expect_cols as usize, expect_rows as usize, cells, dirty);
}

#[no_mangle]
pub unsafe extern "C" fn aleno_pane_scroll(p: &PaneClient, delta: i32) {
    p.shared.term.lock().unwrap().scroll_display(delta as isize);
    p.shared.wake_pending.store(false, Ordering::Release);
}

#[no_mangle]
pub unsafe extern "C" fn aleno_pane_set_display_offset(p: &PaneClient, off: u32) {
    p.shared.term.lock().unwrap().set_display_offset(off as usize);
}

#[no_mangle]
pub unsafe extern "C" fn aleno_pane_set_scrollback_limit(p: &PaneClient, n: u32) {
    p.shared.term.lock().unwrap().set_scrollback_limit(n as usize);
}

/// Text between absolute positions (inclusive). Caller frees with `aleno_string_free`.
#[no_mangle]
pub unsafe extern "C" fn aleno_pane_text(p: &PaneClient, r0: u32, c0: u32, r1: u32, c1: u32) -> *mut c_char {
    to_c(p.shared.term.lock().unwrap().text_range(r0 as usize, c0 as usize, r1 as usize, c1 as usize))
}

#[no_mangle]
pub unsafe extern "C" fn aleno_pane_cluster(p: &PaneClient, id: u32) -> *mut c_char {
    to_c(p.shared.term.lock().unwrap().cluster(id))
}

#[no_mangle]
pub unsafe extern "C" fn aleno_pane_link(p: &PaneClient, id: u16) -> *mut c_char {
    to_c(p.shared.term.lock().unwrap().link(id).to_string())
}

#[no_mangle]
pub unsafe extern "C" fn aleno_pane_title(p: &PaneClient) -> *mut c_char {
    let t = p.shared.term.lock().unwrap();
    to_c(t.title().to_string())
}

#[no_mangle]
pub unsafe extern "C" fn aleno_pane_cwd(p: &PaneClient) -> *mut c_char {
    let t = p.shared.term.lock().unwrap();
    to_c(t.cwd().to_string())
}

#[no_mangle]
pub unsafe extern "C" fn aleno_pane_is_exited(p: &PaneClient) -> i32 {
    p.shared.exited.load(Ordering::Acquire) as i32
}

#[no_mangle]
pub unsafe extern "C" fn aleno_pane_exit_code(p: &PaneClient) -> i32 {
    p.shared.exit_code.load(Ordering::Acquire)
}

#[no_mangle]
pub unsafe extern "C" fn aleno_pane_is_disconnected(p: &PaneClient) -> i32 {
    p.shared.disconnected.load(Ordering::Acquire) as i32
}

/// Search the whole buffer. Returns the number of matches (capped at 100000); results are
/// written to `out` as triples (abs_row, col_start, col_end) up to `cap` matches.
#[no_mangle]
pub unsafe extern "C" fn aleno_pane_search(
    p: &PaneClient,
    query: *const c_char,
    case_insensitive: i32,
    out: *mut u32,
    cap: usize,
) -> usize {
    let q = cstr(query);
    let t = p.shared.term.lock().unwrap();
    let m = t.search(&q, case_insensitive != 0, 100_000);
    for (i, x) in m.iter().take(cap).enumerate() {
        *out.add(i * 3) = x.row as u32;
        *out.add(i * 3 + 1) = x.col_start as u32;
        *out.add(i * 3 + 2) = x.col_end as u32;
    }
    m.len()
}

/// Command blocks (OSC 133) overlapping line indexes `[r0, r1)`; returns how many were written (<= cap).
#[no_mangle]
pub unsafe extern "C" fn aleno_pane_blocks(p: &PaneClient, r0: u32, r1: u32, out: *mut BlockInfo, cap: usize) -> usize {
    let t = p.shared.term.lock().unwrap();
    let v = t.blocks_in(r0 as usize, r1 as usize);
    // Keep the newest when more overlap than fit.
    let skip = v.len().saturating_sub(cap);
    for (i, b) in v.iter().skip(skip).enumerate() {
        *out.add(i) = *b;
    }
    v.len() - skip
}

/// The pane's screen and recent history as terminal output that rebuilds it (owned string; release with `aleno_string_free`).
#[no_mangle]
pub unsafe extern "C" fn aleno_pane_snapshot(p: &PaneClient, max_history: u32) -> *mut c_char {
    to_c(String::from_utf8_lossy(&p.shared.term.lock().unwrap().snapshot_vt(max_history as usize)).into_owned())
}

/// The command line of the block whose prompt is on line `prompt_row` (owned string, NULL when unknown).
#[no_mangle]
pub unsafe extern "C" fn aleno_pane_block_command(p: &PaneClient, prompt_row: u32) -> *mut c_char {
    match p.shared.term.lock().unwrap().block_command(prompt_row as usize) {
        Some(c) => to_c(c),
        None => std::ptr::null_mut(),
    }
}

/// The pane's terminal-reported progress (OSC 9;4): `state` 0 none, 1 normal, 2 error, 3 indeterminate, 4 paused; `percent` 0-100.
#[no_mangle]
pub unsafe extern "C" fn aleno_pane_progress(p: &PaneClient, state: *mut u32, percent: *mut u32) {
    let (s, v) = p.shared.term.lock().unwrap().progress();
    *state = s as u32;
    *percent = v as u32;
}

/// Nearest prompt line above (`dir < 0`) or below (`dir > 0`) `from`, or -1.
#[no_mangle]
pub unsafe extern "C" fn aleno_pane_prompt_nav(p: &PaneClient, from: u32, dir: i32) -> i64 {
    p.shared.term.lock().unwrap().prompt_nav(from as usize, dir).map_or(-1, |r| r as i64)
}

/// Latest block that produced output. Returns 1 and fills `out`, or 0 when there is none.
#[no_mangle]
pub unsafe extern "C" fn aleno_pane_last_block(p: &PaneClient, finished_only: i32, out: *mut BlockInfo) -> i32 {
    match p.shared.term.lock().unwrap().last_block(finished_only != 0) {
        Some(b) => {
            *out = b;
            1
        }
        None => 0,
    }
}

/// Take commands that finished since the last call (see `EVENT_COMMAND_FINISHED`). Returns how many were written.
#[no_mangle]
pub unsafe extern "C" fn aleno_pane_take_finished(p: &PaneClient, out: *mut FinishedInfo, cap: usize) -> usize {
    let v = p.shared.term.lock().unwrap().take_finished();
    let skip = v.len().saturating_sub(cap);
    for (i, f) in v.iter().skip(skip).enumerate() {
        *out.add(i) = *f;
    }
    v.len() - skip
}

/// Number of cells per `AlenoCell` for ABI sanity checks.
#[no_mangle]
pub extern "C" fn aleno_cell_size() -> usize {
    std::mem::size_of::<Cell>()
}
#[no_mangle]
pub extern "C" fn aleno_view_info_size() -> usize {
    std::mem::size_of::<ViewInfo>()
}
