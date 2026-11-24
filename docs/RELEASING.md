# Releasing Hivetty

## One-time setup

- **Updater key.** Updates are verified against the public key in `src-tauri/tauri.conf.json`
  (`plugins.updater.pubkey`). The matching private key is `~/.tauri/hivetty-updater.key` on the
  release machine. Back it up somewhere safe: without it, installed copies can't be updated.
  For CI, put its contents in `TAURI_SIGNING_PRIVATE_KEY` (and the password, if any, in
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`).
- **Code signing (optional, recommended).** Set one of these before building (see
  `scripts/sign.ps1`):
  - `HIVETTY_SIGN_THUMBPRINT`: a code-signing certificate in the Windows certificate store
  - `HIVETTY_SIGN_PFX` + `HIVETTY_SIGN_PFX_PASSWORD`: a .pfx file
  - `HIVETTY_SIGN_COMMAND`: any signing command; `{file}` is replaced with the file path
    (e.g. Azure Trusted Signing)

  Set `HIVETTY_SIGN_REQUIRED=1` to fail the build instead of shipping unsigned files.
- **Hosting.** Pick where installers and the update manifest are served from (HTTPS). The
  manifest URL is baked into each build with `-UpdateEndpoint`; `{{channel}}` in it becomes
  `stable` or `nightly`. Users can override it with `updates.endpoint` in hivetty.json.

## Cutting a release

1. Bump `version` in `src-tauri/tauri.conf.json` (and `Cargo.toml`'s workspace version).
2. Build:

   ```powershell
   .\scripts\release.ps1 -Channel stable `
       -DownloadBase   'https://example.com/hivetty/{{channel}}/{{version}}' `
       -UpdateEndpoint 'https://example.com/hivetty/{{channel}}/latest.json' `
       -Notes 'What changed'
   ```

   Nightly: `-Channel nightly` (version becomes `<version>-nightly.<UTC timestamp>`, NSIS only).
3. Upload `dist\<channel>\<version>\*` to the download location, **then** replace
   `<channel>/latest.json` with the new `latest.json`. Running copies pick it up within hours
   (or at once with Settings → Updates → Check now, or `hivetty update`).
4. Stable only: submit `dist\stable\<version>\winget\` to `microsoft/winget-pkgs` under
   `manifests\h\Hivetty\Hivetty\<version>\` (the script already ran `winget validate`).

## What the installers do

- **NSIS** (`*-setup.exe`, per-user, `%LOCALAPPDATA%\Hivetty`; also what the updater installs):
  puts `bin\` (only `hivetty.exe`) on the user PATH; uninstall stops the session host, removes
  the PATH entry and `%LOCALAPPDATA%\Hivetty\runtime`. Settings and sessions in
  `%APPDATA%\Hivetty` are kept. Hooks: `src-tauri/windows/installer-hooks.nsh`.
- **MSI** (per-machine, Program Files): adds `bin\` to the machine PATH
  (`src-tauri/windows/path-fragment.wxs`).
- In-app updates start the NSIS installer with `/P /UPDATE /R /WAITPID=<pid>`. The installer
  waits for the old process to end, then waits until nothing (e.g. an antivirus scan) holds the
  old exe, so it never aborts with "insufficient permissions to close Hivetty".

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
