# Convenience remover for `mise run uninstall`.
# Stops any running instance and delegates removal to the CLI (`--uninstall`).
# Leaves the installed binary in place. Self-elevates.
param([switch]$Elevated)
$ErrorActionPreference = 'Stop'

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host 'Requesting elevation...'
    Start-Process pwsh -Verb RunAs -Wait -ArgumentList `
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $PSCommandPath, '-Elevated'
    exit $LASTEXITCODE
}

Write-Host 'Stopping any running instance...'
Stop-ScheduledTask -TaskName 'DeepCool CH170' -ErrorAction SilentlyContinue
Get-Process deepcool-ch170 -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 500

$binExe = 'C:\bin\deepcool-ch170.exe'
if (Test-Path $binExe) {
    Start-Process -FilePath $binExe -ArgumentList '--uninstall' -Wait
}
else {
    schtasks /Delete /TN 'DeepCool CH170' /F
}

Write-Host 'Done. Autostart removed (binary left in place).'
if ($Elevated) { Start-Sleep -Seconds 3 }
