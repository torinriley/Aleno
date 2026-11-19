# Contributing to Aleno

Thanks for helping. Aleno is a small codebase and changes are easy to review, so the bar is simple: keep it fast, keep it native, keep it minimal.

## Set up

You need an Apple Silicon Mac on macOS 15 or later, Xcode 26 or later, [Rust](https://rustup.rs) (stable) and [XcodeGen](https://github.com/yonaskolb/XcodeGen).

```bash
brew install xcodegen
git clone https://github.com/torinriley/Aleno.git
cd Aleno
scripts/generate-project.sh      # always use this, not plain `xcodegen generate`
xcodebuild -project Aleno.xcodeproj -scheme Aleno -configuration Debug -derivedDataPath build build
open build/Build/Products/Debug/Aleno.app
```

`scripts/generate-project.sh` regenerates `Aleno.xcodeproj` from `project.yml` and patches a file type that XcodeGen gets wrong for the Icon Composer app icon. The project file is generated, so it is not committed. Run the script again after adding or removing source files.

The Xcode build runs `scripts/build-rust.sh`, which builds the Rust crates in release mode and embeds the daemon.

### Code signing

Builds are ad-hoc signed by default, which is fine for development. If you want a stable signing identity (so macOS remembers permissions such as notifications between builds), create `config/Signing.local.xcconfig`; it is git-ignored:

```
CODE_SIGN_IDENTITY = Apple Development
DEVELOPMENT_TEAM = YOURTEAMID
```

## Test

```bash
cargo build --workspace          # the integration tests start the real daemon binary
cargo test --workspace
```

The tests cover the parser, reflow, command blocks and snapshots, the protocol, and end-to-end sessions with real shells. Please add a test with any change to the terminal engine. For UI changes, say in the pull request how you checked them.

## Where things live

| Area | Where |
|---|---|
| Terminal behavior (escape sequences, grid, scrollback, reflow) | `crates/aleno-terminal` |
| Panes, sessions, shell integration scripts | `crates/aleno-session` |
| App to daemon messages | `crates/aleno-protocol` |
| C interface the app calls | `crates/aleno-client/src/ffi.rs` and `app/Application/aleno.h` |
| Windows, tabs, splits, tab bar | `app/Workspace` |
| The terminal view, input and mouse | `app/TerminalView` |
| Text drawing | `app/Rendering` and `shaders/Terminal.metal` |
| Settings and keybindings | `app/Configuration` and `schemas/settings.schema.json` |

When you add a setting, add it to `Settings.swift` (with validation), to `schemas/settings.schema.json`, and to [docs/CONFIGURATION.md](docs/CONFIGURATION.md). Keep `aleno.h` in sync with `ffi.rs`.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how the parts fit together.

## Branches and commits

`main` is protected: it cannot be deleted or force-pushed, and all changes arrive through pull requests. Never commit to it directly.

Make each change on its own branch, cut from an up-to-date `main`, named `<type>/<short-description>` in lowercase with hyphens:

| Prefix | For |
|---|---|
| `feat/` | A new feature or behavior, for example `feat/quick-terminal` |
| `fix/` | A bug fix, for example `fix/find-bar-overlap` |
| `docs/` | Documentation only |
| `perf/` | A performance improvement |
| `refactor/` | A change that does not alter behavior |
| `test/` | Tests only |
| `chore/` | Build, tooling, CI and dependency updates |

Write commit messages as `<type>: <summary>`, using the same types without the slash. Keep the summary in the imperative mood and under about 70 characters, and add a body when the why is not obvious:

```
fix: keep the find bar below the tab strip

The bar was anchored to the top of the view, so it overlapped the tabs
in multi-tab windows.
```

Keep a branch to one change, rebase it on `main` before opening the pull request, and give the pull request the same `<type>: <summary>` title as its main commit.

## Pull requests

- One change per pull request, with a short description of what and why.
- Run `cargo test --workspace` and make sure the app builds.
- Performance matters here. If a change touches the render path, the parser or the pty path, run the relevant benchmark from [docs/PERFORMANCE.md](docs/PERFORMANCE.md) before and after, and include the numbers.
- Keep the interface minimal. A feature that needs a lot of new UI should start as an issue.
- Match the style of the surrounding code.

## Reporting bugs

Open an issue with your macOS version, what you did, what you expected and what happened. For anything security related, read [SECURITY.md](SECURITY.md) first.
