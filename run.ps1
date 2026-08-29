# Continuum — Self-managing launcher
# ====================================
# Usage:
#   .\run.ps1              # Start server + client
#   .\run.ps1 server       # Start server only
#   .\run.ps1 client       # Start client only
#   .\run.ps1 stop         # Stop all
#   .\run.ps1 status       # Check status
#   .\run.ps1 watch        # Start server + CI watcher
# ====================================

param(
    [Parameter(Position=0)]
    [ValidateSet("server", "client", "stop", "status", "watch", "")]
    [string]$Action = ""
)

$ErrorActionPreference = "SilentlyContinue"
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$serverPid = $null

function Stop-All {
    Get-Process continuum-server -ErrorAction SilentlyContinue | Stop-Process -Force
    Get-Process continuum-client -ErrorAction SilentlyContinue | Stop-Process -Force
    Write-Host "All Continuum processes stopped." -ForegroundColor Green
}

function Get-Status {
    $servers = Get-Process continuum-server -ErrorAction SilentlyContinue
    $clients = Get-Process continuum-client -ErrorAction SilentlyContinue

    Write-Host ""
    Write-Host "  Continuum Status" -ForegroundColor Cyan
    Write-Host "  ================="

    if ($servers) {
        Write-Host "  Server:   RUNNING  (PID $($servers[0].Id))" -ForegroundColor Green
        Write-Host "  Address:  127.0.0.1:4433"
        Write-Host "  Memory:   $([math]::Round($servers[0].WorkingSet64/1MB, 1)) MB"
    } else {
        Write-Host "  Server:   STOPPED" -ForegroundColor Red
    }

    if ($clients) {
        Write-Host "  Client:   RUNNING  (PID $($clients[0].Id))" -ForegroundColor Green
    } else {
        Write-Host "  Client:   STOPPED" -ForegroundColor Red
    }

    Write-Host "  ================="
    Write-Host ""
}

function Start-Server {
    $exe = Join-Path $root "continuum-server.exe"
    if (-not (Test-Path $exe)) {
        Write-Host "Server not found. Building..." -ForegroundColor Yellow
        cargo build --release --bin continuum-server
        $exe = Join-Path $root "continuum-server.exe"
    }

    Write-Host "Starting Continuum server on 127.0.0.1:4433..." -ForegroundColor Cyan
    Start-Process -FilePath $exe -ArgumentList "--listen", "0.0.0.0:4433" -WorkingDirectory $root -PassThru | Select-Object -First 1 Id | ForEach-Object {
        $script:serverPid = $_.Id
    }
}

function Start-Client {
    $exe = Join-Path $root "continuum-client.exe"
    if (-not (Test-Path $exe)) {
        Write-Host "Client not found. Building..." -ForegroundColor Yellow
        cargo build --release --bin continuum-client
        $exe = Join-Path $root "continuum-client.exe"
    }

    Write-Host "Starting Continuum client..." -ForegroundColor Cyan
    Start-Process -FilePath $exe -WorkingDirectory $root
}

switch ($Action) {
    "stop"   { Stop-All }
    "status" { Get-Status }
    "server" {
        Start-Server
        Start-Sleep -Seconds 2
        Get-Status
    }
    "client" {
        Start-Client
        Start-Sleep -Seconds 2
        Get-Status
    }
    "watch" {
        Start-Server
        Start-Sleep -Seconds 3
        Get-Status
        Write-Host "Starting CI watcher..." -ForegroundColor Cyan
        cargo run --release -p continuum-ci -- watch
    }
    "" {
        Start-Server
        Start-Sleep -Seconds 3
        Get-Status
        Start-Client
        Write-Host ""
        Write-Host "Both server and client are running." -ForegroundColor Green
        Write-Host "Use .\run.ps1 status to check status" -ForegroundColor Gray
        Write-Host "Use .\run.ps1 stop to stop all" -ForegroundColor Gray
    }
}
