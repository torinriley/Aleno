#!/bin/bash
# The app version lives in project.yml (CFBundleShortVersionString) and the Rust workspace version in Cargo.toml.
# They must match; the About window shows the app version and the shell reports the crate version in TERM_PROGRAM_VERSION.
set -euo pipefail
cd "$(dirname "$0")/.."
app=$(grep -E '^\s+CFBundleShortVersionString:' project.yml | head -1 | sed -E 's/.*"([^"]+)".*/\1/')
rust=$(awk '/^\[workspace.package\]/{f=1;next} f&&/^version/{gsub(/[" ]/,"",$3); print $3; exit}' Cargo.toml)
if [ "$app" != "$rust" ]; then
  echo "version mismatch: project.yml says $app, Cargo.toml says $rust" >&2
  exit 1
fi
echo "version $app"
