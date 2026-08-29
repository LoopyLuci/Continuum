# Continuum Server — Windows Service Installation Script
# ======================================================
# Run as Administrator:
#   powershell -ExecutionPolicy Bypass -File install-continuum-service.ps1
#
# No Docker or Kubernetes required — runs as a native Windows service.
#
# Uninstall:
#   Stop-Service ContinuumServer
#   sc.exe delete ContinuumServer

$ServiceName = "ContinuumServer"
$DisplayName = "Continuum Remote Desktop Server"
$Description = "Remote desktop host server using QUIC transport"
$BinaryPath = "C:\Program Files\Continuum\continuum-server.exe"

Write-Host "Installing Continuum Server as a Windows service..." -ForegroundColor Cyan

# Check if running as admin
$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator
)
if (-not $isAdmin) {
    Write-Host "ERROR: This script must be run as Administrator." -ForegroundColor Red
    exit 1
}

# Check if binary exists
if (-not (Test-Path $BinaryPath)) {
    Write-Host "Binary not found at $BinaryPath" -ForegroundColor Yellow
    Write-Host "Building from source..."
    Set-Location ..
    cargo build --release --bin continuum-server
    $BinaryPath = "..\target\release\continuum-server.exe"
}

# Create service
sc.exe create $ServiceName binPath= "`"$BinaryPath`" --listen 0.0.0.0:4433" displayName= $DisplayName

if ($LASTEXITCODE -eq 0) {
    sc.exe description $ServiceName $Description
    sc.exe failure $ServiceName reset= 86400 actions= restart/5000/restart/10000/restart/30000
    sc.exe start $ServiceName

    Write-Host "Service '$ServiceName' installed and started successfully." -ForegroundColor Green
    Write-Host ""
    Write-Host "Manage with:"
    Write-Host "  sc.exe stop $ServiceName"
    Write-Host "  sc.exe start $ServiceName"
    Write-Host "  sc.exe delete $ServiceName"
    Write-Host ""
    Write-Host "View logs:"
    Write-Host "  Get-Content \"$env:PROGRAMDATA\continuum\server.log\" -Tail 50 -Wait"
} else {
    Write-Host "Failed to create service. Try running as Administrator." -ForegroundColor Red
    exit 1
}
