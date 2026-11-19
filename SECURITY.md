# Security

## Reporting a vulnerability

Please report security problems privately, using GitHub's [private vulnerability reporting](https://github.com/torinriley/Aleno/security/advisories/new) on this repository. Do not open a public issue for something that could be exploited.

Include what you found, how to reproduce it, and which version you tested. You will get an answer as soon as possible, and a fix will be released before the details are made public.

## What Aleno protects

- **Terminal output is treated as untrusted.** Escape sequences that could read or write your clipboard (OSC 52) are ignored. Titles and links are length-limited and stripped of control characters.
- **The daemon is private to you.** It listens on a Unix socket in a directory only your user can access, and the message format has hard size limits checked before any allocation.
- **Pastes are safe.** Bracketed paste cannot be ended early by pasted text.
