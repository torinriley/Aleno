# Releasing

`scripts/release.sh` builds Aleno, signs it with your Developer ID certificate and the hardened runtime, notarizes it with Apple, staples the ticket, and produces `dist/Aleno.dmg`. It takes about five minutes, most of it waiting for Apple.

## One-time setup

1. Get a **Developer ID Application** certificate from your Apple Developer account and install it in your login keychain. Check that it shows up with `security find-identity -v -p codesigning`.
2. Save notarization credentials. Create an app-specific password at appleid.apple.com, then run:

   ```bash
   xcrun notarytool store-credentials aleno-notary --apple-id <your Apple ID> --team-id <your team ID>
   ```

3. Tell the script which identity to use, either in the environment or in `config/release.local.env` (git-ignored):

   ```bash
   ALENO_SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)"
   ALENO_NOTARY_PROFILE="aleno-notary"
   ```

## Make a release

```bash
scripts/release.sh
```

The script:

1. Builds the Release configuration.
2. Signs the bundled daemon, then the app, with the hardened runtime and a secure timestamp.
3. Submits the app to Apple's notary service and waits, then staples the ticket.
4. Builds a disk image with an Applications shortcut, signs it, notarizes it and staples it.
5. Checks both with `spctl`, which must report `Notarized Developer ID`.

The script produces, in `dist/`:

- `Aleno.dmg`, the notarized disk image
- `Aleno-<version>-arm64.zip`, the notarized app bundle
- `SHA256SUMS`, checksums for both

`scripts/release.sh --unsigned` builds the same package with an ad-hoc signature and no notarization. That is what CI uses when it has no secrets.

Versions: the app version is `CFBundleShortVersionString` in `project.yml`, and the Rust crates share the version in the `[workspace.package]` table of `Cargo.toml`. `scripts/check-versions.sh` (run by CI) fails when they differ. The build number shown in the About window, as in `Version 0.1.0 (24)`, is the number of commits and is set automatically on every build.

Bump both versions before a release, then publish by hand with:

```bash
gh release create v0.1.0 dist/Aleno.dmg dist/Aleno-0.1.0-arm64.zip dist/SHA256SUMS --title "Aleno 0.1.0" --generate-notes
```

## Releasing from GitHub Actions

`.github/workflows/ci.yml` builds and tests every push and pull request.

`.github/workflows/release.yml` runs when you push a tag such as `v0.1.0` (or when started by hand). It runs the tests, builds, signs, notarizes and publishes the release with the same script. The tag must match `CFBundleShortVersionString` in `project.yml`.

For signing it needs these repository secrets (Settings, Secrets and variables, Actions):

| Secret | What it is |
|---|---|
| `APPLE_CERT_P12_BASE64` | Your Developer ID Application certificate and its private key, exported from Keychain Access as a `.p12` and encoded with `base64 -i cert.p12` |
| `APPLE_CERT_PASSWORD` | The password you set when exporting the `.p12` |
| `APPLE_SIGN_IDENTITY` | The identity name, for example `Developer ID Application: Your Name (TEAMID)` |
| `NOTARY_APPLE_ID` | The Apple ID used for notarization |
| `NOTARY_PASSWORD` | An app-specific password for that Apple ID |
| `NOTARY_TEAM_ID` | Your Apple developer team ID |

Without these the workflow still builds and uploads an unsigned package as a workflow artifact, and never publishes a release.
