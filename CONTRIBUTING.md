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

## Pull requests

- One change per pull request, with a short description of what and why.
- Run `cargo test --workspace` and make sure the app builds.
- Performance matters here. If a change touches the render path, the parser or the pty path, run the relevant benchmark from [docs/PERFORMANCE.md](docs/PERFORMANCE.md) before and after, and include the numbers.
- Keep the interface minimal. A feature that needs a lot of new UI should start as an issue.
- Match the style of the surrounding code.

## Reporting bugs

Open an issue with your macOS version, what you did, what you expected and what happened. For anything security related, read [SECURITY.md](SECURITY.md) first.
