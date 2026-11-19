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

Then publish `dist/Aleno.dmg` on the GitHub releases page:

```bash
gh release create v0.1.0 dist/Aleno.dmg --title "Aleno 0.1.0" --notes "..."
```

Bump `CFBundleShortVersionString` in `project.yml` first.
