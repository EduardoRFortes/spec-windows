# Conservatively merges Spec's PreToolUse hook into ~/.claude/settings.json:
# backs up the original file, never touches unrelated keys, and is
# idempotent (safe to re-run). Windows port of spec-fedora's
# install/merge_settings.py — same shape, same hook timeout.

param(
    [Parameter(Mandatory = $true)][string]$SettingsPath,
    [Parameter(Mandatory = $true)][string]$HookBin
)

$ErrorActionPreference = "Stop"

$HookTimeoutSeconds = 60

function Add-PropertyIfMissing {
    param($Object, [string]$Name, $Value)
    if (-not (Get-Member -InputObject $Object -Name $Name -MemberType NoteProperty)) {
        $Object | Add-Member -NotePropertyName $Name -NotePropertyValue $Value
    }
}

$settingsDir = Split-Path -Parent $SettingsPath
if (-not (Test-Path $settingsDir)) {
    New-Item -ItemType Directory -Force -Path $settingsDir | Out-Null
}

if (Test-Path $SettingsPath) {
    $backup = "$SettingsPath.bak.$([DateTimeOffset]::UtcNow.ToUnixTimeSeconds())"
    Copy-Item $SettingsPath $backup
    Write-Output "Backup salvo em $backup"
    $raw = (Get-Content $SettingsPath -Raw).Trim()
    if ($raw) {
        $data = $raw | ConvertFrom-Json
    } else {
        $data = [PSCustomObject]@{}
    }
} else {
    $data = [PSCustomObject]@{}
}

Add-PropertyIfMissing -Object $data -Name "hooks" -Value ([PSCustomObject]@{})
Add-PropertyIfMissing -Object $data.hooks -Name "PreToolUse" -Value @()

$preToolUse = @($data.hooks.PreToolUse)
$alreadyInstalled = $false
foreach ($group in $preToolUse) {
    foreach ($h in @($group.hooks)) {
        if ($h.type -eq "command" -and $h.command -eq $HookBin) {
            $alreadyInstalled = $true
        }
    }
}

if ($alreadyInstalled) {
    Write-Output "Hook já registrado em settings.json, nada a fazer."
} else {
    # No "matcher": omitting it means "all tools" per Claude Code's docs.
    # "args" (even empty) is what switches Windows from shell form (Git
    # Bash/PowerShell trying to interpret our backslash-heavy exe path as a
    # shell command, and failing silently) to exec form (direct process
    # launch) — see docs.claude.com/en/docs/claude-code/hooks.
    $newGroup = [PSCustomObject]@{
        hooks = @(
            [PSCustomObject]@{
                type    = "command"
                command = $HookBin
                args    = @()
                timeout = $HookTimeoutSeconds
            }
        )
    }
    $data.hooks.PreToolUse = @($preToolUse) + @($newGroup)
    Write-Output "Hook de permissão registrado ($HookBin)."
}

$json = $data | ConvertTo-Json -Depth 10
# Set-Content -Encoding utf8 writes a BOM on Windows PowerShell 5.1, which
# Claude Code's (Node) JSON parser rejects — it fails the whole file
# silently rather than just ignoring the BOM, so the hook never fires.
[System.IO.File]::WriteAllText($SettingsPath, $json, (New-Object System.Text.UTF8Encoding($false)))
Write-Output "$SettingsPath atualizado."
