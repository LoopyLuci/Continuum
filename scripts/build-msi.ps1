# Build MSI installer for Continuum
# Requires: cargo wix (cargo install cargo-wix)

Write-Host "Building Continuum MSI installer..." -ForegroundColor Cyan

# Build release binary
cargo build --release --bin continuum
if ($LASTEXITCODE -ne 0) {
    Write-Host "Build failed!" -ForegroundColor Red
    exit 1
}

# Create WiX manifest if it doesn't exist
if (-not (Test-Path "wix\main.wxs")) {
    Write-Host "Creating WiX manifest..." -ForegroundColor Yellow
    cargo wix init
}

# Build MSI
cargo wix --no-build --output target\release\continuum-installer.msi
if ($LASTEXITCODE -ne 0) {
    Write-Host "MSI build failed! Trying manual approach..." -ForegroundColor Yellow

    # Manual MSI creation using WiX Toolset
    $wixPath = Get-Command candle.exe -ErrorAction SilentlyContinue
    if ($wixPath) {
        candle.exe -dSourceDir="target\release" -dOutputName="continuum.msi" wix\main.wxs -o wix\main.wixobj
        light.exe wix\main.wixobj -o target\release\continuum-installer.msi
    } else {
        Write-Host "WiX Toolset not found. Install from https://wixtoolset.org/" -ForegroundColor Red
        Write-Host "Or install cargo-wix: cargo install cargo-wix" -ForegroundColor Yellow
    }
}

Write-Host "Done! MSI at target\release\continuum-installer.msi" -ForegroundColor Green
