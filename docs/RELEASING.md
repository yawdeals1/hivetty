# Releasing Hivetty

Releases live on GitHub (`yawdeals1/hivetty`). Installed copies check for updates at startup and
every few hours:

- **Stable** reads `https://github.com/yawdeals1/hivetty/releases/latest/download/latest.json`,
  the `latest.json` attached to the newest published (non-pre-)release.
- **Nightly** reads `https://github.com/yawdeals1/hivetty/releases/download/nightly/latest.json`,
  the rolling `nightly` pre-release (`.github/workflows/nightly.yml`, daily when `main` changed).

So **publishing a GitHub release is what rolls out an update.** `.github/workflows/release.yml`
builds the installers for every published release and attaches them with `latest.json`.

## One-time setup

- **Updater key (required).** Updates are verified against the public key in
  `src-tauri/tauri.conf.json` (`plugins.updater.pubkey`). The private key is
  `~/.tauri/hivetty-updater.key`; back it up. Without it, installed copies can never be updated.
  Give it to the workflow as a repository secret:

  ```bash
  # from Git Bash: redirect the file itself. Piping it through Windows PowerShell adds a UTF-8 BOM,
  # which makes signing fail in CI ("Invalid symbol 239, offset 0").
  gh secret set TAURI_SIGNING_PRIVATE_KEY --repo yawdeals1/hivetty < ~/.tauri/hivetty-updater.key
  ```

  (If the key has a password, also set `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.)
- **Code signing (optional, recommended).** Unsigned installers trigger SmartScreen warnings.
  Locally, set one of `HIVETTY_SIGN_THUMBPRINT`, `HIVETTY_SIGN_PFX` (+ `HIVETTY_SIGN_PFX_PASSWORD`)
  or `HIVETTY_SIGN_COMMAND` (see `scripts/sign.ps1`). In CI, add the secrets
  `HIVETTY_SIGN_PFX_BASE64` (the .pfx, base64) and `HIVETTY_SIGN_PFX_PASSWORD`.

## Cutting a release

1. Bump `version` in `src-tauri/tauri.conf.json` and the workspace `Cargo.toml`, commit, push.
2. Create the release (this publishes it and starts the build):

   ```powershell
   gh release create v0.2.0 --title "Hivetty 0.2.0" --notes "What changed"
   ```

   The workflow attaches `Hivetty_<v>_x64-setup.exe`, the MSI, their `.sig` files,
   `latest.json` and `winget-manifests.zip` (~20 min). Installed copies offer the update once
   `latest.json` is attached. A pre-release is never "latest", so it doesn't reach stable users.
3. Optional: submit `winget-manifests.zip`'s contents to `microsoft/winget-pkgs` under
   `manifests\h\Hivetty\Hivetty\<version>\`.

To build a release by hand instead, run `scripts/release.ps1` (see its header) and upload
`dist\stable\<version>\*` with `gh release upload`. A release that already has `latest.json`
attached when it's published is left alone by the workflow (run the workflow manually with the
tag to rebuild it).

## What the installers do

- **NSIS** (`*-setup.exe`; also what the updater installs) installs for all users in
  `C:\Program Files\Hivetty` and asks for administrator permission (UAC), on install and on each
  update. It puts `bin\` (only `hivetty.exe`) on the machine PATH; uninstall stops the session
  host, removes the PATH entry and `%LOCALAPPDATA%\Hivetty\runtime`. Settings and sessions in
  `%APPDATA%\Hivetty` are kept. Hooks: `src-tauri/windows/installer-hooks.nsh`.
- **MSI**: the same, per-machine (`src-tauri/windows/path-fragment.wxs`).
- In-app updates start the NSIS installer elevated with `/P /UPDATE /R /WAITPID=<pid>`; if the
  UAC prompt is declined, Hivetty keeps running. The installer waits for the old process to end,
  then until nothing (e.g. an antivirus scan) holds the old exe, so it never aborts with
  "insufficient permissions to close Hivetty". If something else (seen: McAfee) still holds the
  old `hivetty-app.exe` after ~30 s, the installer renames it aside and installs the new one
  instead of failing with "Failed to kill Hivetty"; the old copy is deleted later.

## Testing an update locally

Never publish these builds (they accept updates over plain http):

```powershell
.\scripts\release.ps1 -Version 0.1.0 -SkipMsi -InsecureUpdates `
    -DownloadBase 'http://127.0.0.1:8787/{{channel}}/{{version}}' `
    -UpdateEndpoint 'http://127.0.0.1:8787/{{channel}}/latest.json'
# install it, build 0.1.1 the same way, copy dist\stable\0.1.1\latest.json to dist\stable\,
# serve dist\ (python -m http.server 8787 --bind 127.0.0.1 --directory dist), then
# `hivetty update install` in the installed copy.
```
