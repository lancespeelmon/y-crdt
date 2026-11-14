#!/bin/sh
# Builds Yrs.xcframework (iOS devices, the iOS simulator on Apple silicon and Intel, and macOS)
# and the Swift bindings that go with it, into target/yuniffi-apple. Needs Xcode and the Rust
# targets aarch64-apple-ios, aarch64-apple-ios-sim, x86_64-apple-ios and aarch64-apple-darwin.
set -eu
cd "$(dirname "$0")/.."
out=target/yuniffi-apple
rm -rf "$out" && mkdir -p "$out/headers" "$out/ios-sim"

for target in aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios aarch64-apple-darwin; do
    cargo rustc -q -p yuniffi --lib --release --target "$target" --crate-type staticlib
done

# Bindings and the C header, from the macOS build (the interface is the same on every target).
cargo build -q -p yuniffi --release
cargo run -q -p yuniffi --features cli --bin uniffi-bindgen -- generate \
    --library target/release/libyuniffi.dylib --language swift --out-dir "$out/swift"
cp "$out/swift/yuniffiFFI.h" "$out/headers/"
cp "$out/swift/yuniffiFFI.modulemap" "$out/headers/module.modulemap"

# One simulator library for both architectures.
lipo -create \
    target/aarch64-apple-ios-sim/release/libyuniffi.a \
    target/x86_64-apple-ios/release/libyuniffi.a \
    -output "$out/ios-sim/libyuniffi.a"

xcodebuild -create-xcframework \
    -library target/aarch64-apple-ios/release/libyuniffi.a -headers "$out/headers" \
    -library "$out/ios-sim/libyuniffi.a" -headers "$out/headers" \
    -library target/aarch64-apple-darwin/release/libyuniffi.a -headers "$out/headers" \
    -output "$out/Yrs.xcframework" >/dev/null

echo "built $out/Yrs.xcframework and $out/swift/yuniffi.swift"
