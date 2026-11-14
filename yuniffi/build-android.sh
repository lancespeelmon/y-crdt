#!/bin/sh
# Builds the Android libraries and the Kotlin bindings into target/yuniffi-android.
#
# UniFFI's Kotlin bindings load the library through JNA, so an app needs the shared libraries
# (jniLibs/<abi>/libyuniffi.so). Linking those needs the Android NDK: set ANDROID_NDK_HOME, and
# the script uses its clang for each ABI. Without it, the script builds the static libraries
# only, which checks that the code compiles for every ABI.
set -eu
cd "$(dirname "$0")/.."
out=target/yuniffi-android
rm -rf "$out" && mkdir -p "$out"
api=24

abi_of() {
    case "$1" in
        aarch64-linux-android) echo arm64-v8a ;;
        armv7-linux-androideabi) echo armeabi-v7a ;;
        x86_64-linux-android) echo x86_64 ;;
        i686-linux-android) echo x86 ;;
    esac
}

clang_of() {
    case "$1" in
        armv7-linux-androideabi) echo "armv7a-linux-androideabi$api-clang" ;;
        *) echo "$1$api-clang" ;;
    esac
}

for target in aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android; do
    abi=$(abi_of "$target")
    mkdir -p "$out/jniLibs/$abi" "$out/static/$abi"
    if [ -n "${ANDROID_NDK_HOME:-}" ]; then
        bin="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$(uname -s | tr A-Z a-z)-x86_64/bin"
        linker_var="CARGO_TARGET_$(echo "$target" | tr a-z- A-Z_)_LINKER"
        env "$linker_var=$bin/$(clang_of "$target")" \
            cargo rustc -q -p yuniffi --lib --release --target "$target" --crate-type cdylib
        cp "target/$target/release/libyuniffi.so" "$out/jniLibs/$abi/"
    fi
    cargo rustc -q -p yuniffi --lib --release --target "$target" --crate-type staticlib
    cp "target/$target/release/libyuniffi.a" "$out/static/$abi/"
done

cargo build -q -p yuniffi --release
host_lib=target/release/libyuniffi.dylib
[ -f "$host_lib" ] || host_lib=target/release/libyuniffi.so
cargo run -q -p yuniffi --features cli --bin uniffi-bindgen -- generate \
    --library "$host_lib" --language kotlin --out-dir "$out/kotlin"

if [ -n "${ANDROID_NDK_HOME:-}" ]; then
    echo "built $out/jniLibs and $out/kotlin"
else
    echo "built $out/static and $out/kotlin (set ANDROID_NDK_HOME for the jniLibs .so files)"
fi
