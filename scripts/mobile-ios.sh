#!/bin/sh
# Builds the phone core for iOS and writes the Swift that calls it.
#
#   scripts/mobile-ios.sh            the simulator, an iPhone, and this Mac
#   scripts/mobile-ios.sh --quick    this Mac and the simulator only
#
# Leaves apps/ios/Core/CortexCore.xcframework and apps/ios/Core/cortex_mobile.swift,
# which the Xcode project in apps/ios builds against. Needs full Xcode, and
# the Rust targets: rustup target add aarch64-apple-ios aarch64-apple-ios-sim
set -eu

cd "$(dirname "$0")/.."

# Xcode's command line tools alone have no iOS SDK.
if [ -z "${DEVELOPER_DIR:-}" ] && [ -d /Applications/Xcode.app ]; then
    export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
fi

# The C in the core (SQLite, the code parsers) is built for the oldest
# systems the app supports, as in apps/ios/project.yml: otherwise it is
# built for the SDK's own and the app cannot start on older ones.
export IPHONEOS_DEPLOYMENT_TARGET=17.0
export MACOSX_DEPLOYMENT_TARGET=14.0

targets="aarch64-apple-darwin aarch64-apple-ios-sim"

if [ "${1:-}" != "--quick" ]; then
    targets="$targets aarch64-apple-ios"
fi

for target in $targets; do
    cargo build -p cortex-mobile --release --target "$target"
done

core=apps/ios/Core
generated=target/mobile/swift

rm -rf "$generated" "$core/CortexCore.xcframework"
mkdir -p "$generated/headers" "$core"

cargo run -q -p cortex-mobile --features bindgen --bin uniffi-bindgen -- \
    generate --library target/aarch64-apple-darwin/release/libcortex_mobile.dylib \
    --language swift --out-dir "$generated"

# A framework's module map has to be called just that.
mv "$generated/cortex_mobileFFI.h" "$generated/headers/"
mv "$generated/cortex_mobileFFI.modulemap" "$generated/headers/module.modulemap"
cp "$generated/cortex_mobile.swift" "$core/"

libraries=""

for target in $targets; do
    libraries="$libraries -library target/$target/release/libcortex_mobile.a -headers $generated/headers"
done

# shellcheck disable=SC2086
xcodebuild -create-xcframework $libraries -output "$core/CortexCore.xcframework" >/dev/null

echo "wrote $core/CortexCore.xcframework and $core/cortex_mobile.swift"
