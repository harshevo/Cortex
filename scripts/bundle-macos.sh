#!/bin/sh
# Builds the desktop app in release mode and wraps it as Cortex.app.
#
#     scripts/bundle-macos.sh              leaves it in target/release/Cortex.app
#     scripts/bundle-macos.sh --install    also copies it to /Applications
#
# The app still needs this checkout where it is: it starts the embedding
# service from `ml/` and the mind service from `../brain`. If the checkout
# moves, build again.
set -eu

cd "$(dirname "$0")/.."

cargo build --release -p cortex-gui

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
app=target/release/Cortex.app

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/cortex-gui "$app/Contents/MacOS/cortex-gui"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Cortex</string>
    <key>CFBundleDisplayName</key>
    <string>Cortex</string>
    <key>CFBundleIdentifier</key>
    <string>dev.cortex.app</string>
    <key>CFBundleExecutable</key>
    <string>cortex-gui</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$version</string>
    <key>CFBundleVersion</key>
    <string>$version</string>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST

# Signed for this machine only, so macOS will open it without complaint.
codesign --force --sign - "$app"

echo "built $app"

if [ "${1:-}" = "--install" ]; then
    rm -rf /Applications/Cortex.app
    cp -R "$app" /Applications/Cortex.app
    echo "installed /Applications/Cortex.app"
fi
