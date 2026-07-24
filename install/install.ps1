# Full install: builds release binaries, registers the PreToolUse hook +
# statusLine in Claude Code's settings.json, and registers a Scheduled Task
# so specd (the tray daemon) starts at login on its own -- the Windows
# equivalent of spec-fedora's systemd --user service. Idempotent: safe to
# re-run after a `git pull` to pick up new binaries.

param(
    [string]$SettingsPath = "$env:USERPROFILE\.claude\settings.json"
)

$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent $PSScriptRoot

Write-Output "Compilando binarios release..."
Push-Location $RepoRoot
try {
    cargo build --release --workspace
    if ($LASTEXITCODE -ne 0) { throw "cargo build falhou" }
} finally {
    Pop-Location
}

$HookBin = Join-Path $RepoRoot "target\release\spec-hook.exe"
$StatusLineBin = Join-Path $RepoRoot "target\release\spec-statusline.exe"
$DaemonBin = Join-Path $RepoRoot "target\release\specd.exe"

& (Join-Path $PSScriptRoot "merge_settings.ps1") -SettingsPath $SettingsPath -HookBin $HookBin -StatusLineBin $StatusLineBin

Write-Output "Registrando tarefa agendada 'SpecWindowsTray' (inicia no logon, reinicia se cair)..."
$action = New-ScheduledTaskAction -Execute $DaemonBin
$trigger = New-ScheduledTaskTrigger -AtLogOn -User "$env:USERDOMAIN\$env:USERNAME"
$settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable `
    -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) `
    -ExecutionTimeLimit (New-TimeSpan -Seconds 0) -MultipleInstances IgnoreNew
$principal = New-ScheduledTaskPrincipal -UserId "$env:USERDOMAIN\$env:USERNAME" -LogonType Interactive -RunLevel Limited
Register-ScheduledTask -TaskName "SpecWindowsTray" -Action $action -Trigger $trigger `
    -Settings $settings -Principal $principal -Force `
    -Description "Spec tray daemon (permission approvals + usage bars) for Claude Code" | Out-Null

$running = Get-Process specd -ErrorAction SilentlyContinue
if (-not $running) {
    Write-Output "Iniciando specd agora (nao precisa esperar o proximo logon)..."
    Start-ScheduledTask -TaskName "SpecWindowsTray"
} else {
    Write-Output "specd ja estava rodando -- pare o processo antigo manualmente se quiser trocar para o build novo."
}

Write-Output "Instalacao concluida."
