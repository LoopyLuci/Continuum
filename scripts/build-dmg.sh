#!/bin/bash
# Build DMG for Continuum
set -e

echo "Building Continuum DMG..."

# Build release binary
cargo build --release --bin continuum

# Create .app bundle
mkdir -p Continuum.app/Contents/MacOS
mkdir -p Continuum.app/Contents/Resources

cp target/release/continuum Continuum.app/Contents/MacOS/

cat > Continuum.app/Contents/Info.plist << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>continuum</string>
    <key>CFBundleIdentifier</key>
    <string>com.continuum.remote</string>
    <key>CFBundleName</key>
    <string>Continuum</string>
    <key>CFBundleVersion</key>
    <string>1.1.0</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
</dict>
</plist>
EOF

# Ad-hoc code sign
codesign --force --deep --sign - Continuum.app 2>/dev/null || true

# Create DMG
hdiutil create -volname "Continuum" -srcfolder Continuum.app -ov -format UDZO Continuum.dmg

echo "DMG built: Continuum.dmg"
