#!/bin/bash
# Build AppImage for Continuum
# Requires: linuxdeploy

set -e

echo "Building Continuum AppImage..."

# Build release binary
cargo build --release --bin continuum

# Create AppDir structure
mkdir -p AppDir/usr/bin
mkdir -p AppDir/usr/share/applications
mkdir -p AppDir/usr/share/icons/hicolor/256x256/apps

# Copy binary
cp target/release/continuum AppDir/usr/bin/

# Create desktop file
cat > AppDir/usr/share/applications/continuum.desktop << EOF
[Desktop Entry]
Type=Application
Name=Continuum
Comment=Remote access for everyone
Exec=continuum
Icon=continuum
Categories=Network;RemoteAccess;
Terminal=false
EOF

# Copy desktop file to AppDir root
cp AppDir/usr/share/applications/continuum.desktop AppDir/

# Create a simple icon (placeholder)
# In production, use a real icon file
convert -size 256x256 xc:steelblue -fill white -pointsize 72 -gravity center -annotate 0 "C" AppDir/usr/share/icons/hicolor/256x256/apps/continuum.png 2>/dev/null || true

# Build AppImage
if command -v linuxdeploy &> /dev/null; then
    linuxdeploy --appdir AppDir --output appimage
    echo "AppImage built: Continuum-x86_64.AppImage"
else
    echo "linuxdeploy not found. Install from https://github.com/linuxdeploy/linuxdeploy"
    echo "Or use: wget https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage"
fi
