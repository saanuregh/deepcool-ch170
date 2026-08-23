# Convenience installer for `mise run install`.
# Orchestration only: refreshes C:\bin from the release build, stops any running
# instance, then delegates the actual task registration to the CLI (`--install`).
# Self-elevates because the task runs with highest privileges.
param([switch]$Elevated)
$ErrorActionPreference = 'Stop'

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host 'Requesting elevation...'
    Start-Process pwsh -Verb RunAs -Wait -ArgumentList `
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $PSCommandPath, '-Elevated'
    exit $LASTEXITCODE
}

$binDir = 'C:\bin'
$binExe = Join-Path $binDir 'deepcool-ch170.exe'
$src = (Resolve-Path (Join-Path $PSScriptRoot '..\target\release\deepcool-ch170.exe')).Path

Write-Host 'Stopping any running instance...'
Stop-ScheduledTask -TaskName 'DeepCool CH170' -ErrorAction SilentlyContinue
Get-Process deepcool-ch170 -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 800

Write-Host "Installing to $binExe"
New-Item -ItemType Directory -Force -Path $binDir | Out-Null
Copy-Item $src $binExe -Force

Write-Host 'Registering autostart via the CLI...'
Start-Process -FilePath $binExe -ArgumentList '--install' -Wait

Write-Host 'Starting...'
Start-ScheduledTask -TaskName 'DeepCool CH170' -ErrorAction SilentlyContinue

Write-Host "Done. Autostart installed at $binExe; it will start at each logon."
if ($Elevated) { Start-Sleep -Seconds 3 }
