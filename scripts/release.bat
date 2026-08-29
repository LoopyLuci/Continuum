@echo off
REM Release script for Windows
REM Usage: scripts\release.bat <version>
REM Example: scripts\release.bat 0.3.0

setlocal enabledelayedexpansion

if "%~1"=="" (
    echo Usage: %~0 ^<version^>
    echo Example: %~0 0.3.0
    exit /b 1
)

set VERSION=%~1
echo Preparing release v%VERSION%

REM Validate version format
echo %VERSION% | findstr /r "^[0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*$" >nul
if errorlevel 1 (
    echo Error: Version must be in format X.Y.Z
    exit /b 1
)

REM Run tests
echo Running tests...
cargo test --workspace --quiet --tests -- --skip continuum_ai

REM Run clippy
echo Running clippy...
cargo clippy --workspace --quiet -- -D warnings

REM Run formatter check
echo Checking formatting...
cargo fmt -- --check

REM Build release binaries
echo Building release binaries...
cargo build --release --workspace --exclude continuum-ai --exclude continuum-plugin-sdk

REM Run deny check
echo Running cargo deny...
cargo deny check

echo Release v%VERSION% prepared successfully!
echo Next steps:
echo   git push origin main
echo   git tag -a v%VERSION% -m "Release v%VERSION%"
echo   git push origin v%VERSION%
echo   Create GitHub release with artifacts from target\release\

endlocal
