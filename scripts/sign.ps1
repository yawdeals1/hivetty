<#
.SYNOPSIS
  Authenticode-sign one file. Tauri calls this for the app and the installers (bundle.windows
  .signCommand, set by release.ps1), and release.ps1 calls it for the bundled helper programs.

.DESCRIPTION
  Picks the first configured method:
    HIVETTY_SIGN_COMMAND      a command line to run; {file} is replaced by the quoted path
                              (e.g. for Azure Trusted Signing or a cloud HSM tool)
    HIVETTY_SIGN_THUMBPRINT   SHA-1 thumbprint of a code-signing cert in the Windows cert store
    HIVETTY_SIGN_PFX          path to a .pfx file (password in HIVETTY_SIGN_PFX_PASSWORD)
  With none set the file is left unsigned, unless HIVETTY_SIGN_REQUIRED=1, which fails the build.
  HIVETTY_SIGN_TIMESTAMP_URL overrides the RFC 3161 timestamp server.
#>
param([Parameter(Mandatory = $true)][string]$File)

$ErrorActionPreference = 'Stop'
$timestamp = if ($env:HIVETTY_SIGN_TIMESTAMP_URL) { $env:HIVETTY_SIGN_TIMESTAMP_URL } else { 'http://timestamp.digicert.com' }

function Find-SignTool {
    $kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
    if (Test-Path $kits) {
        $found = Get-ChildItem $kits -Directory | Where-Object { $_.Name -match '^\d+\.' } |
            Sort-Object { [version]$_.Name } -Descending |
            ForEach-Object { Join-Path $_.FullName 'x64\signtool.exe' } |
            Where-Object { Test-Path $_ } | Select-Object -First 1
        if ($found) { return $found }
    }
    $cmd = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    throw 'signtool.exe not found; install the Windows SDK'
}

if ($env:HIVETTY_SIGN_COMMAND) {
    $line = $env:HIVETTY_SIGN_COMMAND.Replace('{file}', '"' + $File + '"')
    Write-Host "sign: $line"
    & cmd.exe /d /c $line
    if ($LASTEXITCODE -ne 0) { throw "signing command failed ($LASTEXITCODE)" }
    exit 0
}

$auth = $null
if ($env:HIVETTY_SIGN_THUMBPRINT) {
    $auth = @('/sha1', $env:HIVETTY_SIGN_THUMBPRINT)
} elseif ($env:HIVETTY_SIGN_PFX) {
    $auth = @('/f', $env:HIVETTY_SIGN_PFX)
    if ($env:HIVETTY_SIGN_PFX_PASSWORD) { $auth += @('/p', $env:HIVETTY_SIGN_PFX_PASSWORD) }
}

if (-not $auth) {
    if ($env:HIVETTY_SIGN_REQUIRED -eq '1') { throw "no signing certificate configured (HIVETTY_SIGN_REQUIRED=1): $File" }
    Write-Host "sign: skipped, no certificate configured ($([IO.Path]::GetFileName($File)))"
    exit 0
}

$signtool = Find-SignTool
$args = @('sign') + $auth + @('/fd', 'sha256', '/tr', $timestamp, '/td', 'sha256', '/d', 'Hivetty', $File)
& $signtool @args
if ($LASTEXITCODE -ne 0) { throw "signtool failed ($LASTEXITCODE) for $File" }
& $signtool verify /pa /q $File
if ($LASTEXITCODE -ne 0) { throw "signature on $File doesn't verify" }
