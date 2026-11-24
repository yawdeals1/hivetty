# Hivetty shell integration for PowerShell 7 / Windows PowerShell.
# Emits: OSC 9;9 (cwd), OSC 133 A/B (prompt), C (command executed), D;<exit> (command finished).
if ($global:__HivettyLoaded) { return }
$global:__HivettyLoaded = $true
$global:__HivettyOrigPrompt = $function:prompt
$global:__HivettyCmdRan = $false

function global:prompt {
    $ok = $?
    $code = $global:LASTEXITCODE
    $e = [char]27; $st = "$e\"
    $out = ''
    if ($global:__HivettyCmdRan) {
        $exit = if ($ok) { 0 } elseif ($code) { $code } else { 1 }
        $out += "$e]133;D;$exit$st"
        $global:__HivettyCmdRan = $false
    }
    $loc = $executionContext.SessionState.Path.CurrentLocation
    if ($loc.Provider.Name -eq 'FileSystem') { $out += "$e]9;9;`"$($loc.ProviderPath)`"$st" }
    $out += "$e]133;A$st"
    $orig = if ($global:__HivettyOrigPrompt) { & $global:__HivettyOrigPrompt } else { "PS $($loc)> " }
    $global:LASTEXITCODE = $code
    "$out$orig$e]133;B$st"
}

# Mark command execution right after the line is read (same hook point VS Code uses).
if (Test-Path Function:\PSConsoleHostReadLine) {
    $global:__HivettyOrigReadLine = $function:PSConsoleHostReadLine
    function global:PSConsoleHostReadLine {
        $line = $global:__HivettyOrigReadLine.Invoke()
        if (-not [string]::IsNullOrWhiteSpace($line)) {
            $global:__HivettyCmdRan = $true
            [Console]::Write("$([char]27)]133;C$([char]7)")
        }
        $line
    }
}
