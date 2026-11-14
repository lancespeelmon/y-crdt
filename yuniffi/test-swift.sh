#!/bin/sh
# Builds yuniffi for this Mac, generates the Swift bindings and runs the smoke test.
set -eu
cd "$(dirname "$0")/.."
out=target/yuniffi-swift
cargo build -p yuniffi --release
cargo run -q -p yuniffi --features cli --bin uniffi-bindgen -- generate \
    --library target/release/libyuniffi.dylib --language swift --out-dir "$out"
# swiftc finds the C module through its module map, which UniFFI names after the crate.
cp "$out/yuniffiFFI.modulemap" "$out/module.modulemap"
swiftc -O -module-name YuniffiTest \
    -I "$out" -L target/release -lyuniffi \
    -Xcc -fmodule-map-file="$out/module.modulemap" \
    "$out/yuniffi.swift" yuniffi/tests/swift/main.swift \
    -o "$out/smoke"
DYLD_LIBRARY_PATH=target/release "$out/smoke"
