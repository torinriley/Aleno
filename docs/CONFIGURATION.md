# Configuration

Aleno reads `~/.config/aleno/settings.json`. Press `⌘,` to open it. Changes apply as soon as you save, and a file with an error is rejected whole, so a typo never leaves you with a broken terminal. Add `"$schema": "./settings.schema.json"` for autocomplete in editors that support JSON schemas.

```json
{
  "$schema": "./settings.schema.json",
  "font": { "family": "SF Mono", "size": 13, "lineHeight": 1.2, "ligatures": true },
  "appearance": { "theme": "system", "opacity": 0.85, "blur": true, "blurRadius": 25, "padding": 12 },
  "terminal": { "shell": "default", "scrollback": 10000, "cursorStyle": "bar", "notifyAfterSeconds": 10 },
  "workspace": { "confirmOnClose": false, "welcome": "first-launch" },
  "keybindings": { "newTab": "cmd+t", "splitRight": "cmd+d", "splitDown": "cmd+shift+d" }
}
```

## font

| Key | Default | Description |
|---|---|---|
| `family` | `SF Mono` | Font family. It must be installed; a monospaced family is recommended. Falls back to the system monospaced font. |
| `size` | `13` | Size in points. |
| `lineHeight` | `1.2` | Multiple of the font's natural line height. |
| `characterSpacing` | `0` | Extra horizontal space per cell in points; may be negative. |
| `ligatures` | `true` | Draw programming ligatures when the font has them (Fira Code, JetBrains Mono, Cascadia Code, CommitMono). |

## appearance

| Key | Default | Description |
|---|---|---|
| `theme` | `system` | `system`, `light` or `dark`. `system` follows macOS live. |
| `opacity` | `1` | Background opacity from `0` (fully clear) to `1`. Text stays opaque. |
| `blur` | `true` | Blur the desktop behind the window when `opacity` is below 1. |
| `blurStyle` | `blur` | `blur` (alias `vibrancy`) is Aleno's frosted blur. `glass` uses macOS Liquid Glass (macOS 26 and later). |
| `blurRadius` | unset | Strength of the blur: `0` is none, around `20` is subtle, `60` and up is heavy. When unset, the standard macOS frosted material is used. Ignored with `glass`. |
| `padding` | `12` | Space between the pane edge and the text grid, in points. |
| `colors.foreground`, `colors.background`, `colors.cursor`, `colors.selection` | theme | Color overrides, as `#rrggbb`. |
| `colors.palette` | theme | The 16 ANSI colors: 0-7 normal, 8-15 bright. |

## terminal

| Key | Default | Description |
|---|---|---|
| `shell` | `default` | `default` uses your login shell. Otherwise an absolute path or a program name found in the system bin directories. |
| `scrollback` | `10000` | Lines of history kept per pane. |
| `cursorStyle` | `bar` | `block`, `bar` or `underline`. Programs can override it. |
| `cursorBlink` | `true` | Blink the cursor while the pane is focused. |
| `optionAsMeta` | `true` | Treat `⌥` as Meta (ESC-prefixed keys) instead of composing special characters. |
| `notifyAfterSeconds` | `10` | Notify when a command that ran at least this long finishes in a pane you are not looking at. `0` turns it off. |

## workspace

| Key | Default | Description |
|---|---|---|
| `confirmOnClose` | `false` | Ask before closing a window. Quitting with open windows always asks. |
| `welcome` | `first-launch` | When to play the welcome animation: `first-launch`, `always` or `never`. Help, Welcome to Aleno shows it on demand. |

## keybindings

Each key is an action and each value a shortcut such as `"cmd+shift+d"`. Modifiers are `cmd`, `shift`, `alt` and `ctrl`. Keys are a single character or one of `left`, `right`, `up`, `down`, `pageup`, `pagedown`, `home`, `end`, `tab`, `enter`, `space`, `escape`. Use an empty string to unbind an action.

| Action | Default | | Action | Default |
|---|---|---|---|---|
| `newTab` | `cmd+t` | | `find` | `cmd+f` |
| `newWindow` | `cmd+n` | | `findNext` | `cmd+g` |
| `closePane` | `cmd+w` | | `findPrevious` | `cmd+shift+g` |
| `closeTab` | `cmd+shift+w` | | `copy` | `cmd+c` |
| `reopenClosedTab` | `cmd+shift+t` | | `paste` | `cmd+v` |
| `nextTab` | `cmd+shift+]` | | `selectAll` | `cmd+a` |
| `previousTab` | `cmd+shift+[` | | `clearScrollback` | `cmd+k` |
| `splitRight` | `cmd+d` | | `copyLastOutput` | `cmd+alt+c` |
| `splitDown` | `cmd+shift+d` | | `selectLastOutput` | `cmd+alt+a` |
| `focusLeft` / `focusRight` | `cmd+alt+left` / `right` | | `scrollPageUp` / `scrollPageDown` | `shift+pageup` / `pagedown` |
| `focusUp` / `focusDown` | `cmd+alt+up` / `down` | | `scrollToTop` / `scrollToBottom` | `cmd+home` / `cmd+end` |
| `resizeLeft` / `resizeRight` | `cmd+ctrl+left` / `right` | | `increaseFontSize` | `cmd+=` |
| `resizeUp` / `resizeDown` | `cmd+ctrl+up` / `down` | | `decreaseFontSize` | `cmd+-` |
| `openSettings` | `cmd+,` | | `resetFontSize` | `cmd+0` |
| `toggleFullScreen` | `cmd+ctrl+f` | | | |

## Environment variables

| Variable | Effect |
|---|---|
| `ALENO_SHELL_INTEGRATION=0` | Turn off the shell integration (command blocks, notifications, command text). |
