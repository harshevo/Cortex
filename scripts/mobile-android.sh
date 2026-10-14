#!/bin/sh
# Builds the phone core for Android and writes the Kotlin that calls it.
#
#   scripts/mobile-android.sh            arm64 phones and the arm64 emulator
#   scripts/mobile-android.sh --all      also x86_64, for emulators on Intel
#
# Leaves the libraries in apps/android/app/src/main/jniLibs and the Kotlin
# in apps/android/app/src/main/java/uniffi, which the Gradle project in
# apps/android builds against. Needs the Android NDK (ANDROID_NDK_HOME, or
# the newest one under the SDK), `cargo install cargo-ndk`, and the Rust
# targets: rustup target add aarch64-linux-android x86_64-linux-android
set -eu

cd "$(dirname "$0")/.."

sdk="${ANDROID_HOME:-$HOME/Library/Android/sdk}"

if [ -z "${ANDROID_NDK_HOME:-}" ]; then
    ANDROID_NDK_HOME="$(ls -d "$sdk"/ndk/* 2>/dev/null | sort -V | tail -1)"
    export ANDROID_NDK_HOME
fi

if [ -z "$ANDROID_NDK_HOME" ]; then
    echo "error: no Android NDK found under $sdk/ndk" >&2
    exit 1
fi

abis="-t arm64-v8a"

if [ "${1:-}" = "--all" ]; then
    abis="$abis -t x86_64"
fi

app=apps/android/app/src/main

# shellcheck disable=SC2086
cargo ndk $abis --platform 26 -o "$app/jniLibs" build -p cortex-mobile --release

# Two of the core's dependencies also build as libraries of their own. The
# core already holds them, so the copies only add weight to the app.
find "$app/jniLibs" -name 'lib*.so' ! -name 'libcortex_mobile.so' -delete

# The Kotlin is the same for every phone, so it is read off this machine's
# own build of the core.
cargo build -p cortex-mobile --release

library=target/release/libcortex_mobile.dylib
[ -f "$library" ] || library=target/release/libcortex_mobile.so

rm -rf "$app/java/uniffi"

cargo run -q -p cortex-mobile --features bindgen --bin uniffi-bindgen -- \
    generate --library "$library" --language kotlin --no-format --out-dir "$app/java"

echo "wrote $app/jniLibs and $app/java/uniffi"
