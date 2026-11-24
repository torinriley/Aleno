# Changelog

Each release gets a section headed `## <version>`, newest first, with one short bullet per change. The newest
section is what Aleno shows in the terminal after an update (see `crates/aleno-daemon/src/whatsnew.rs`), so keep
the bullets to plain sentences. `scripts/check-versions.sh` fails when the current version has no section.

## 0.1.7
- Unicode, emoji and wide-character output is about twice as fast.
- A zero-width joiner followed by plain text no longer swallows the next character.

## 0.1.6
- Aleno checks for updates every time it launches, and asks before installing.

## 0.1.5
- After an update, Aleno shows the new version and what changed in the terminal.

## 0.1.4
- A styled install window for the disk image, with an arrow from Aleno to the Applications folder.

## 0.1.3
- Aleno now updates itself. It checks in the background, installs on quit, and has Check for Updates in the Aleno menu.

## 0.1.2
- The first tab's title no longer jitters while a new tab slides in.
- The size badge only shows while you are resizing the window.
- Resizing keeps the cursor row and the prompt block.

## 0.1.1
- A dot-matrix welcome animation on first launch.
- A custom About window with the exact version and links.
- A confirm-on-quit setting, and faint text stays legible on translucent windows.
- Powerline tips and glyph art keep their exact colours.

## 0.1.0
- The first release.
