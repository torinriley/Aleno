//! The what's-new banner: after an update, the first pane shows the logo, the version and a short changelog
//! instead of the full first-run introduction (see `welcome.rs`).
//!
//! The changelog is `CHANGELOG.md`, compiled into the binary, so it always describes the version that is running.
//! `~/.config/aleno/.version` remembers the last version that was shown. A banner is due when the user has already
//! seen the first-run welcome (`.welcomed`) and `.version` is missing or differs from this build.

use std::path::PathBuf;

const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASES_URL: &str = "github.com/torinriley/Aleno/releases";

/// One `## <version>` section of the changelog.
#[derive(Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub notes: Vec<String>,
}

/// Sections in file order (newest first). A bullet may continue on following indented lines.
pub fn parse(changelog: &str) -> Vec<Release> {
    let mut out: Vec<Release> = Vec::new();
    for line in changelog.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            let v = h.split_whitespace().next().unwrap_or("").trim_start_matches('v');
            out.push(Release { version: v.to_string(), notes: Vec::new() });
        } else if let Some(cur) = out.last_mut() {
            if let Some(b) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
                cur.notes.push(b.trim().to_string());
            } else if line.starts_with(' ') && !line.trim().is_empty() {
                if let Some(last) = cur.notes.last_mut() {
                    last.push(' ');
                    last.push_str(line.trim());
                }
            }
        }
    }
    out
}

/// The notes to show for the update to `current`: every section after `from` up to and including `current`, or
/// just the `current` section when `from` is unknown or not in the changelog.
pub fn notes_since(releases: &[Release], current: &str, from: Option<&str>) -> Vec<String> {
    let start = releases.iter().position(|r| r.version == current);
    let Some(start) = start else { return Vec::new() };
    let end = from
        .and_then(|f| releases.iter().position(|r| r.version == f))
        .filter(|&e| e > start)
        .unwrap_or(start + 1);
    releases[start..end].iter().flat_map(|r| r.notes.iter().cloned()).collect()
}

/// The notes for this build's version, given the version the user last ran.
pub fn current_notes(from: Option<&str>) -> Vec<String> {
    notes_since(&parse(CHANGELOG), VERSION, from)
}

fn state_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".config/aleno")
}

fn last_version() -> Option<String> {
    std::fs::read_to_string(state_dir().join(".version")).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// `Some(previous version, if known)` when an update banner is due.
pub fn pending_update() -> Option<Option<String>> {
    if let Ok(from) = std::env::var("ALENO_WHATSNEW_FROM") {
        return Some(Some(from).filter(|f| !f.is_empty())); // preview: ALENO_WHATSNEW_FROM=0.1.2 aleno-daemon welcome ...
    }
    if !state_dir().join(".welcomed").exists() {
        return None;
    }
    match last_version() {
        Some(v) if v == VERSION => None,
        other => Some(other),
    }
}

/// Remember that this version's welcome or banner has been shown. Preview runs leave the state alone.
pub fn record_seen() {
    if std::env::var_os("ALENO_WHATSNEW_FROM").is_some() {
        return;
    }
    let dir = state_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(".version"), VERSION);
}

/// Greedy word wrap on `width` visible columns.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if !cur.is_empty() && cur.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// The text under the logo. Styling uses the terminal's own palette (blue accent, bold, dim), so it follows the theme.
pub fn banner_lines(cols: usize, from: Option<&str>) -> Vec<String> {
    const MAX_NOTES: usize = 6;
    let b = |s: &str| format!("\x1b[1m{}\x1b[22m", s);
    let d = |s: &str| format!("\x1b[2m{}\x1b[22m", s);
    let accent = |s: &str| format!("\x1b[1;34m{}\x1b[22;39m", s);

    let mut v = vec![String::new()];
    let mut title = format!("{} {}", b("Aleno"), accent(VERSION));
    if let Some(f) = from {
        title.push_str(&d(&format!("  \u{b7}  updated from {}", f)));
    } else {
        title.push_str(&d("  \u{b7}  updated"));
    }
    v.push(title);
    v.push(String::new());

    let notes = current_notes(from);
    if notes.is_empty() {
        v.push(d("See the release notes for what changed."));
    } else {
        v.push(b("What's new"));
        let text_w = cols.saturating_sub(4 + 4).clamp(20, 72);
        for n in notes.iter().take(MAX_NOTES) {
            for (i, l) in wrap(n, text_w).iter().enumerate() {
                v.push(format!("{}{}", if i == 0 { format!("{}  ", accent("\u{203a}")) } else { "   ".to_string() }, l));
            }
        }
        if notes.len() > MAX_NOTES {
            v.push(d(&format!("   and {} more", notes.len() - MAX_NOTES)));
        }
    }
    v.push(String::new());
    let url = format!("{}/tag/v{}", RELEASES_URL, VERSION);
    if cols >= url.chars().count() + 20 {
        v.push(format!("{}  {}", b("Release notes"), url));
    } else {
        v.push(b("Release notes"));
        v.push(RELEASES_URL.trim_end_matches("/releases").to_string());
    }
    v.push(String::new());
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Changelog\n\nintro\n\n## 0.3.0\n- Third thing that is long enough to continue\n  on a second line.\n- Another\n\n## 0.2.0\n- Second\n\n## 0.1.0\n- First\n";

    #[test]
    fn parses_sections_and_continuations() {
        let r = parse(SAMPLE);
        assert_eq!(r.iter().map(|r| r.version.as_str()).collect::<Vec<_>>(), ["0.3.0", "0.2.0", "0.1.0"]);
        assert_eq!(r[0].notes, ["Third thing that is long enough to continue on a second line.", "Another"]);
    }

    #[test]
    fn notes_cover_the_versions_you_skipped() {
        let r = parse(SAMPLE);
        assert_eq!(notes_since(&r, "0.3.0", Some("0.2.0")).len(), 2, "one version ahead");
        assert_eq!(notes_since(&r, "0.3.0", Some("0.1.0")).len(), 3, "two versions ahead");
        assert_eq!(notes_since(&r, "0.3.0", None).len(), 2, "unknown previous version: just this one");
        assert_eq!(notes_since(&r, "0.3.0", Some("9.9.9")).len(), 2, "previous version not in the file");
        assert!(notes_since(&r, "0.9.0", None).is_empty());
    }

    #[test]
    fn the_real_changelog_has_this_version() {
        let r = parse(CHANGELOG);
        assert!(r.iter().any(|r| r.version == VERSION && !r.notes.is_empty()), "CHANGELOG.md needs a '## {}' section with bullets", VERSION);
    }

    #[test]
    fn wrap_respects_the_width() {
        for l in wrap("one two three four five six seven eight nine ten", 12) {
            assert!(l.chars().count() <= 12, "{:?}", l);
        }
        assert_eq!(wrap("", 10), Vec::<String>::new());
    }

    #[test]
    fn banner_fits_narrow_and_wide_terminals() {
        let visible = |s: &str| {
            let mut n = 0;
            let mut it = s.chars();
            while let Some(c) = it.next() {
                if c == '\x1b' {
                    for x in it.by_ref() {
                        if x.is_ascii_alphabetic() {
                            break;
                        }
                    }
                } else {
                    n += 1;
                }
            }
            n
        };
        for cols in [40usize, 60, 80, 120] {
            for l in banner_lines(cols, Some("0.1.0")) {
                assert!(visible(&l) + 4 <= cols, "line too wide for {} columns: {:?}", cols, l);
            }
        }
    }
}
