# Features

## Welcome

The first time Aleno runs, the logo materializes in dot-matrix lettering with a soft shadow, followed by a short introduction: the essential shortcuts, what command blocks and notifications do, and where the settings file lives. Then your shell starts as usual. Press any key to skip the animation.

It runs as the first thing in the new pane, in front of your shell, so it adds nothing to your shell's startup and leaves nothing in your history. See it again any time with Help, Welcome to Aleno.

`workspace.welcome` controls it: `"first-launch"` (the default), `"always"` or `"never"`. In narrow windows the logo is left out and the text is shortened.

## About

Aleno, About Aleno shows the icon, the exact version and build number (read from the running app, so it is always right), a one-line description, and two quiet links: the website and the GitHub page. Esc closes it.

## Command blocks

Aleno's shells report where each prompt starts, when a command begins and ends, its exit status, and the working directory (the OSC 133 and OSC 7 sequences). Aleno turns that into:

- A red bar in the margin and a `✗ code · duration` badge on failed commands. Slow commands get a duration badge. Clicking a badge copies that command's output.
- `⌥⌘C` copies the output of the last command. "Select Last Command Output" (`⌥⌘A`) highlights it.
- The typed command line is kept with each block.

The integration is injected automatically: zsh through `ZDOTDIR`, bash 4.4 and later through `--rcfile`, fish through `XDG_DATA_DIRS`. Set `ALENO_SHELL_INTEGRATION=0` to turn it off. Other shells work normally, just without blocks. macOS's built-in bash 3.2 is too old for command blocks.

## Notifications

A command that ran at least `terminal.notifyAfterSeconds` seconds (default 10, `0` disables) and finished in a pane you are not looking at (another tab, another window, or Aleno in the background) posts a macOS notification with the command, exit status and duration. Clicking it brings that pane forward. macOS asks for permission the first time.

## Progress

Programs that report their own progress (the OSC 9;4 sequence, used by recent `brew` and `cargo`) show a thin bar along the bottom of their tab, or under the title in a single-tab window, and on the Dock icon. Red means an error, yellow means paused, and a sweeping bar means the amount is unknown. The bar clears when the program says it is done or when the command finishes.

Try it:

```bash
for i in 10 30 50 70 90; do printf '\e]9;4;1;%d\a' $i; sleep 0.7; done; printf '\e]9;4;0\a'
```

## Tabs, windows and splits

- A translucent tab strip with `⌘1` to `⌘9` hints, and animated opening and closing.
- Drag a tab out of the strip to turn it into its own window. A preview of the window follows your cursor. Drop it back on the strip to put it back.
- `⌘⇧T` reopens the most recently closed tab (up to ten), with its splits, folders and its old screen and scrollback above a fresh shell. Programs that were running are not restored.
- Splits nest to any depth. `⌘D` splits right, `⌘⇧D` splits down, `⌘⌥` and the arrow keys move between panes, `⌘⌃` and the arrow keys resize.
- While you resize a window or divider, a small badge shows the grid size, such as `121 × 35`.

## Text

- **Reflow.** Resizing re-wraps soft-wrapped lines in the screen and the scrollback, keeps the cursor on the same character and keeps command marks attached to their lines. Hard line breaks and full-screen programs (vim, htop) are left alone. A prompt you are sitting at is cleared and redrawn by your shell instead of being reflowed.
- **Ligatures** (`font.ligatures`, on by default). Runs of symbols are shaped together when the font has a ligature or contextual alternate for them (Fira Code, JetBrains Mono, Cascadia Code, CommitMono and others). Fonts without ligatures, such as SF Mono and Menlo, are unaffected.
- **Find.** `⌘F` searches the whole scrollback with highlighted matches and a counter.
- **Folders.** `⌘`-click a folder name in output to open it in Finder. Drag a file into the window to paste its path, quoted for the shell.

## Look and feel

- Translucent windows with `appearance.opacity`, plus either an adjustable blur (`blurRadius`), the standard macOS frosted blur, or Liquid Glass (`blurStyle`: `"glass"`).
- Block, bar or underline cursor with optional blinking. Programs can change it with DECSCUSR.
- Light, dark or system theme, with optional color overrides and a custom 16-color palette.
- Block elements (`▀ ▄ █ ▌ ▐ ░ ▒ ▓` and the quadrants) and braille patterns are drawn by the renderer itself, filling the whole cell with no gaps at any line height, so charts, progress bars and dot-matrix graphics from tools like `btop` and `bat` look solid.
- A bundled Nerd Font symbols fallback, so icons in prompts such as Powerlevel10k render without extra setup.

## macOS integration

- Right-click a folder or file in Finder, then Services, then "New Aleno Window Here" or "New Aleno Tab Here". Folders also appear under Open With, and you can drop a folder on the Dock icon.
- Notifications, Dock progress and the system appearance all follow the standard macOS behavior.
