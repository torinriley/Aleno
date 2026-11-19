<p align="center">
  <img src="docs/assets/icon.png" width="128" alt="Aleno app icon">
</p>

<h1 align="center">Aleno</h1>

<p align="center">
  A fast, minimal, truly native terminal for macOS.<br>
  Rust engine, Swift and AppKit interface, Metal renderer.
</p>

<p align="center">
  <img alt="macOS 15+" src="https://img.shields.io/badge/macOS-15%2B-black">
  <img alt="Apple Silicon" src="https://img.shields.io/badge/Apple%20Silicon-arm64-black">
  <img alt="Rust and Swift" src="https://img.shields.io/badge/Rust%20%2B%20Swift-black">
  <img alt="MIT license" src="https://img.shields.io/badge/license-MIT-black">
</p>

---

Aleno looks and behaves like a Mac app, because it is one. Real AppKit windows, a GPU-drawn text grid, and a small Rust core that does the terminal work. It starts in about 150 ms, sits at 0% CPU when idle, and keeps pace with the fastest terminals on a firehose of output.

## Why Aleno

- **Native, not wrapped.** AppKit windows, a translucent tab strip, Finder integration and system notifications. No web view, no cross-platform toolkit.
- **Fast where it counts.** About 150 ms from launch to first frame, 0.8 ms of GPU time per frame, and `cat` of a 135 MB file in about half a second, level with Ghostty. See [Performance](docs/PERFORMANCE.md).
- **It knows what your commands did.** Failed commands get a red bar and an exit-code badge. `⌥⌘C` copies the last command's output. A command that runs for a while notifies you when it finishes, and tools like `brew` and `cargo` show live progress on the tab and the Dock icon.
- **Tabs and splits done properly.** Drag a tab out to make a window. Reopen a closed tab with its history (`⌘⇧T`). Split any pane, as deep as you like.
- **Beautiful by default.** Translucent windows with an adjustable blur or macOS Liquid Glass, programming ligatures, and a theme that follows the system.
- **Text that behaves.** Lines re-wrap when you resize, `⌘`-click a folder to open it in Finder, drag a file in to paste its path, and Find searches the whole scrollback.
- **Part of macOS.** Right-click a folder in Finder for "New Aleno Window Here", or drop it on the Dock icon.
- **Configured with one JSON file.** Live reload, a schema for autocomplete, and a bad file never breaks your terminal.

## Install

Download the latest notarized `Aleno.dmg` from the [Releases](https://github.com/torinriley/Aleno/releases) page, open it, and drag Aleno to Applications.

Requires macOS 15 or later on Apple Silicon.

## Build from source

You need Xcode 26 or later, [Rust](https://rustup.rs) and [XcodeGen](https://github.com/yonaskolb/XcodeGen) (`brew install xcodegen`).

```bash
git clone https://github.com/torinriley/Aleno.git
cd Aleno
scripts/generate-project.sh
xcodebuild -project Aleno.xcodeproj -scheme Aleno -configuration Release -derivedDataPath build build
open build/Build/Products/Release/Aleno.app
```

The Xcode build compiles the Rust crates for you. More in [CONTRIBUTING.md](CONTRIBUTING.md).

## Configure

Press `⌘,` to open `~/.config/aleno/settings.json`. Changes apply as soon as you save.

```json
{
  "$schema": "./settings.schema.json",
  "font": { "family": "SF Mono", "size": 13, "ligatures": true },
  "appearance": { "theme": "system", "opacity": 0.85, "blur": true, "blurRadius": 25 },
  "terminal": { "cursorStyle": "bar", "notifyAfterSeconds": 10 },
  "keybindings": { "splitRight": "cmd+d", "splitDown": "cmd+shift+d" }
}
```

Every option is listed in [docs/CONFIGURATION.md](docs/CONFIGURATION.md).

## Documentation

| | |
|---|---|
| [Features](docs/FEATURES.md) | Command blocks, notifications, progress, reflow, tabs and more, in detail |
| [Configuration](docs/CONFIGURATION.md) | Every setting and keybinding |
| [Architecture](docs/ARCHITECTURE.md) | How the engine, daemon, protocol and renderer fit together |
| [Performance](docs/PERFORMANCE.md) | Benchmarks and how to reproduce them |
| [Contributing](CONTRIBUTING.md) | Building, testing and sending changes |
| [Releasing](docs/RELEASING.md) | Signing, notarizing and packaging a build |
| [Security](SECURITY.md) | Reporting a vulnerability |

## Project layout

```
app/          Swift + AppKit: windows, tabs, splits, Metal renderer, settings
crates/       Rust: terminal engine, IPC protocol, session daemon, client library
shaders/      Metal shaders for the text grid
schemas/      JSON schema for settings.json
scripts/      Project generation, release and build helpers
tests/        Benchmarks and integration scripts
```

## License

[MIT](LICENSE). The bundled terminal parser is a lightly patched copy of [vte](https://github.com/alacritty/vte) (MIT or Apache-2.0), and the bundled Symbols Nerd Font is under its own license (`app/Resources/Fonts/LICENSE-SymbolsNerdFont.txt`).
