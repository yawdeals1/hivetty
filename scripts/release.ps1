<#
.SYNOPSIS
  Build a signed Hivetty release: installers, updater manifest and (stable) winget manifests.

.EXAMPLE
  # Stable release, uploaded to https://downloads.example.com/hivetty/stable/0.2.0/
  .\scripts\release.ps1 -Channel stable `
      -DownloadBase 'https://downloads.example.com/hivetty/{{channel}}/{{version}}' `
      -UpdateEndpoint 'https://downloads.example.com/hivetty/{{channel}}/latest.json'

.EXAMPLE
  # Nightly (version becomes <base>-nightly.<yyyyMMddHHmm>, NSIS only)
  .\scripts\release.ps1 -Channel nightly -DownloadBase ... -UpdateEndpoint ...

.DESCRIPTION
  Output goes to dist\<channel>\<version>\:
    Hivetty_<version>_x64-setup.exe (+ .sig)   per-user NSIS installer, also used by the updater
    Hivetty_<version>_x64_en-US.msi (+ .sig)   per-machine MSI (stable only)
    latest.json                                updater manifest; publish it at -UpdateEndpoint
    winget\                                    winget manifests (stable only)
  Upload the installers to -DownloadBase, then replace latest.json at the update endpoint.

  Code signing: see scripts\sign.ps1 (HIVETTY_SIGN_THUMBPRINT / HIVETTY_SIGN_PFX /
  HIVETTY_SIGN_COMMAND). Updater signing: TAURI_SIGNING_PRIVATE_KEY (key text or path; defaults to
  ~\.tauri\hivetty-updater.key) and TAURI_SIGNING_PRIVATE_KEY_PASSWORD.
#>
param(
    [ValidateSet('stable', 'nightly')][string]$Channel = 'stable',
    # Version to release (stable: defaults to tauri.conf.json's; nightly: the base for -nightly.N)
    [string]$Version,
    # Where the installers will be downloadable from; {{channel}} and {{version}} are filled in.
    [Parameter(Mandatory = $true)][string]$DownloadBase,
    # Update manifest URL baked into the build; {{channel}} is filled in at runtime.
    [Parameter(Mandatory = $true)][string]$UpdateEndpoint,
    [string]$Notes = '',
    [switch]$SkipMsi,
    # LOCAL TESTING ONLY: let the updater use plain http (e.g. a 127.0.0.1 test server).
    [switch]$InsecureUpdates
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$conf = Get-Content src-tauri\tauri.conf.json -Raw | ConvertFrom-Json
if (-not $Version) { $Version = $conf.version }
if ($Channel -eq 'nightly') {
    $Version = "$($Version -replace '-.*$', '')-nightly.$((Get-Date).ToUniversalTime().ToString('yyyyMMddHHmm'))"
}
$buildMsi = ($Channel -eq 'stable') -and -not $SkipMsi
$targets = @('nsis')
if ($buildMsi) { $targets += 'msi' }
Write-Host "== Hivetty $Version ($Channel): $($targets -join ', ')"

# Updater signing key
if (-not $env:TAURI_SIGNING_PRIVATE_KEY) {
    $default = Join-Path $env:USERPROFILE '.tauri\hivetty-updater.key'
    if (-not (Test-Path $default)) { throw "no updater key: set TAURI_SIGNING_PRIVATE_KEY or create $default (pnpm tauri signer generate -w $default)" }
    $env:TAURI_SIGNING_PRIVATE_KEY = $default
}
if ($null -eq $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD) { $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = '' }

$sign = Join-Path $PSScriptRoot 'sign.ps1'

# UTF-8 without a BOM (Windows PowerShell's -Encoding utf8 adds one, which JSON readers reject).
function Write-Utf8([string]$Path, [string]$Text) {
    [IO.File]::WriteAllText($Path, $Text, (New-Object Text.UTF8Encoding $false))
}

# 1. Helper programs, staged where the release config's resources point.
cargo build --release --locked -p hivetty-cli -p hivetty-host
if ($LASTEXITCODE -ne 0) { throw 'building helper programs failed' }
$stage = Join-Path $root 'src-tauri\binaries'
New-Item -ItemType Directory -Force $stage | Out-Null
foreach ($exe in 'hivetty.exe', 'tmux.exe', 'hivetty-host.exe') {
    Copy-Item (Join-Path $root "target\release\$exe") (Join-Path $stage $exe) -Force
    & $sign (Join-Path $stage $exe)
    if ($LASTEXITCODE -ne 0) { throw "signing $exe failed" }
}

# 2. Release-only Tauri config, merged over tauri.conf.json.
# Structured form: Tauri runs it without a shell, so no quoting (it would pass quotes literally).
$signCmd = [ordered]@{ cmd = 'powershell'; args = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $sign, '%1') }
$release = [ordered]@{
    version = $Version
    bundle  = [ordered]@{
        targets                = $targets
        createUpdaterArtifacts = $true
        resources              = [ordered]@{
            '../shell-integration/*'    = 'shell-integration/'
            'binaries/hivetty.exe'      = 'bin/hivetty.exe'
            'binaries/tmux.exe'         = 'tmux.exe'
            'binaries/hivetty-host.exe' = 'hivetty-host.exe'
        }
        windows                = @{ signCommand = $signCmd }
    }
}
if ($InsecureUpdates) {
    Write-Warning 'InsecureUpdates: this build accepts updates over plain http. Never publish it.'
    $release['plugins'] = @{ updater = @{ dangerousInsecureTransportProtocol = $true } }
}
$releaseConf = Join-Path $root 'target\tauri.release.conf.json'
New-Item -ItemType Directory -Force (Split-Path $releaseConf) | Out-Null
Write-Utf8 $releaseConf ($release | ConvertTo-Json -Depth 8)

# 3. Build and bundle.
$env:HIVETTY_UPDATE_ENDPOINT = $UpdateEndpoint
$bundleDir = Join-Path $root 'target\release\bundle'
if (Test-Path $bundleDir) { Remove-Item -Recurse -Force $bundleDir }
pnpm tauri build --config $releaseConf
if ($LASTEXITCODE -ne 0) { throw 'tauri build failed' }

# 4. Collect artifacts.
$out = Join-Path $root "dist\$Channel\$Version"
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force $out | Out-Null
$nsis = Get-ChildItem "$bundleDir\nsis\*-setup.exe" | Select-Object -First 1
if (-not $nsis) { throw 'NSIS installer missing' }
Copy-Item $nsis.FullName, "$($nsis.FullName).sig" $out
if ($buildMsi) {
    $msi = Get-ChildItem "$bundleDir\msi\*.msi" | Select-Object -First 1
    Copy-Item $msi.FullName, "$($msi.FullName).sig" $out
}

$base = $DownloadBase.Replace('{{channel}}', $Channel).Replace('{{version}}', $Version).TrimEnd('/')
$nsisUrl = "$base/$([uri]::EscapeDataString($nsis.Name))"
$nsisSig = (Get-Content "$($nsis.FullName).sig" -Raw).Trim()
$platforms = [ordered]@{
    'windows-x86_64'      = [ordered]@{ signature = $nsisSig; url = $nsisUrl }
    'windows-x86_64-nsis' = [ordered]@{ signature = $nsisSig; url = $nsisUrl }
}
if ($buildMsi) {
    $platforms['windows-x86_64-msi'] = [ordered]@{
        signature = (Get-Content "$($msi.FullName).sig" -Raw).Trim()
        url       = "$base/$([uri]::EscapeDataString($msi.Name))"
    }
}
$manifest = [ordered]@{
    version   = $Version
    notes     = $Notes
    pub_date  = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    platforms = $platforms
}
Write-Utf8 (Join-Path $out 'latest.json') ($manifest | ConvertTo-Json -Depth 6)

# 5. winget manifests for stable releases.
if ($Channel -eq 'stable') {
    & (Join-Path $PSScriptRoot 'winget-manifest.ps1') -Version $Version -InstallerUrl $nsisUrl `
        -InstallerPath (Join-Path $out $nsis.Name) -OutDir (Join-Path $out 'winget')
}

Write-Host "== done: $out"
Get-ChildItem $out -Recurse | ForEach-Object { Write-Host "   $($_.FullName.Substring($out.Length + 1))" }
