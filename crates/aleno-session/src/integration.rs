//! Shell integration: scripts that make the shell report prompts, command boundaries and the working directory
//! (OSC 133 / OSC 7). They are embedded in the daemon and written next to its socket on first use, so the feature
//! works from any install location and needs nothing from the app bundle.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

const ZSHENV: &str = include_str!("../shell/zshenv");
const BASH_RC: &str = include_str!("../shell/bash.rc");
const FISH: &str = include_str!("../shell/fish.fish");

fn write_if_changed(path: &Path, content: &str) -> std::io::Result<()> {
    if std::fs::read_to_string(path).map(|c| c == content).unwrap_or(false) {
        return Ok(());
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(tmp, path)
}

/// Directory holding the integration files (created and refreshed on demand).
pub fn prepare() -> Option<PathBuf> {
    let dir = aleno_protocol::socket_path().parent()?.join("shell-integration");
    write_if_changed(&dir.join("zsh/.zshenv"), ZSHENV).ok()?;
    write_if_changed(&dir.join("bash/aleno.bash"), BASH_RC).ok()?;
    write_if_changed(&dir.join("fish/vendor_conf.d/aleno-integration.fish"), FISH).ok()?;
    Some(dir)
}

/// Arrange for `shell` to load the integration. Returns false when the shell is not supported. May change how the
/// shell is invoked (bash gets `--rcfile`).
pub fn apply(cmd: &mut Command, shell: &str) -> bool {
    if std::env::var_os("ALENO_SHELL_INTEGRATION").map_or(false, |v| v == "0") {
        return false;
    }
    let name = Path::new(shell).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let Some(dir) = prepare() else { return false };
    match name.as_str() {
        "zsh" => {
            if let Some(z) = std::env::var_os("ZDOTDIR") {
                cmd.env("ALENO_ZDOTDIR", z);
            }
            cmd.env("ZDOTDIR", dir.join("zsh"));
            true
        }
        "bash" => {
            // --rcfile only applies to non-login shells, so emulate the login startup inside the rcfile.
            cmd.env("ALENO_BASH_LOGIN", "1");
            cmd.arg("--rcfile").arg(dir.join("bash/aleno.bash"));
            true
        }
        "fish" => {
            let mut xdg = OsString::from(dir.as_os_str());
            xdg.push(":");
            xdg.push(std::env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into()));
            cmd.env("XDG_DATA_DIRS", xdg);
            true
        }
        _ => false,
    }
}
