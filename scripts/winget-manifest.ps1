<#
.SYNOPSIS
  Write winget manifests (schema 1.9.0) for a Hivetty release, ready for a PR to
  microsoft/winget-pkgs under manifests\h\Hivetty\Hivetty\<version>\.
  The static fields live in packaging\winget\*.yaml; this fills in version, URL and hash.
#>
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$InstallerUrl,
    [Parameter(Mandatory = $true)][string]$InstallerPath,
    [Parameter(Mandatory = $true)][string]$OutDir
)

$ErrorActionPreference = 'Stop'
$templates = Join-Path (Split-Path -Parent $PSScriptRoot) 'packaging\winget'
$sha = (Get-FileHash -Algorithm SHA256 $InstallerPath).Hash.ToUpperInvariant()
$date = (Get-Date).ToUniversalTime().ToString('yyyy-MM-dd')
New-Item -ItemType Directory -Force $OutDir | Out-Null

$vars = @{
    '{{version}}'      = $Version
    '{{installerUrl}}' = $InstallerUrl
    '{{sha256}}'       = $sha
    '{{releaseDate}}'  = $date
}
foreach ($t in Get-ChildItem (Join-Path $templates '*.yaml')) {
    $text = Get-Content $t.FullName -Raw
    foreach ($k in $vars.Keys) { $text = $text.Replace($k, $vars[$k]) }
    [IO.File]::WriteAllText((Join-Path $OutDir $t.Name), $text, (New-Object Text.UTF8Encoding $false))
}

if (Get-Command winget -ErrorAction SilentlyContinue) {
    winget validate --manifest $OutDir
    if ($LASTEXITCODE -ne 0) { throw 'winget validate failed' }
}
Write-Host "winget manifests: $OutDir (sha256 $sha)"
