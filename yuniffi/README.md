# yuniffi

Swift and Kotlin bindings for [yrs](../yrs), generated with
[UniFFI](https://mozilla.github.io/uniffi-rs/) 0.32. The crate is not published; build it
from this repository.

It exposes a small, document-level API:

- `YDoc`: create a document (optionally with a client ID), get its root `text`, `map` and
  `array`, encode its state vector and state as an update, apply updates, and observe the
  updates it produces (`observeUpdates(key:observer:)` / `unobserveUpdates(key:)`).
- `YText`, `YMap`, `YArray`: edit and read the root shared types. Values are `YValue`:
  null, bool, `Int` (i64), `Number` (f64), `BigInt`, string, bytes, or `Shared` for a nested
  shared type (read-only; nested types aren't exposed yet).
- Free functions on V1 updates: `mergeUpdates`, `diffUpdate`, `stateVectorFromUpdate` and
  `validateUpdate`.

Errors are thrown as `YError`: an invalid or unapplicable update, an index out of range, a
client ID above 2^53 − 1 (Yjs's limit), or a value the binding can't represent (for example a
non-finite number).

Text indices and lengths count UTF-16 code units, as in Yjs (Swift's `utf16` view, Kotlin's
`String` indices). Updates use the V1 encoding, so they interoperate with Yjs and the other bindings.

## Swift

```sh
./yuniffi/test-swift.sh      # macOS: builds the dylib, generates Swift, runs a smoke test
./yuniffi/build-apple.sh     # builds target/yuniffi-apple/Yrs.xcframework
```

`build-apple.sh` needs Xcode and the Rust targets `aarch64-apple-ios`,
`aarch64-apple-ios-sim`, `x86_64-apple-ios` and `aarch64-apple-darwin`. It writes:

- `target/yuniffi-apple/Yrs.xcframework`: static libraries for iOS, the iOS simulator
  (arm64 and x86_64) and macOS (arm64), with the C header and module map;
- `target/yuniffi-apple/swift/yuniffi.swift`: the Swift bindings. Add it to the target that
  links the XCFramework.

```swift
let doc = try YDoc.withClientId(clientId: 1)
let text = doc.text(name: "content")
try text.insert(index: 0, chunk: "hello")
let update = doc.encodeStateAsUpdate(stateVector: nil)

let other = YDoc()
try other.applyUpdate(update: update)
print(other.text(name: "content").getString())  // "hello"
```

## Kotlin (Android)

```sh
./yuniffi/build-android.sh
```

It needs the Rust targets `aarch64-linux-android`, `armv7-linux-androideabi`,
`i686-linux-android` and `x86_64-linux-android`, and writes:

- `target/yuniffi-android/kotlin/uniffi/yuniffi/yuniffi.kt`: the Kotlin bindings, which load
  the library through [JNA](https://github.com/java-native-access/jna);
- `target/yuniffi-android/static/<abi>/libyuniffi.a`: static libraries per ABI;
- with `ANDROID_NDK_HOME` set, `target/yuniffi-android/jniLibs/<abi>/libyuniffi.so`: the
  shared libraries an Android app loads. Copy `jniLibs` into `src/main/` of the app module and
  add `net.java.dev.jna:jna` (the `@aar` artifact) as a dependency.

## Observers

`observeUpdates` calls the observer on the thread that commits the transaction, with the V1
update the transaction produced. The observer must not edit the same document from inside the
callback. On Swift, an observer that may be called from several threads should guard its state
(see `tests/swift/main.swift`).

## Bindings generator

The generator is built from the same UniFFI version as the library:

```sh
cargo run -p yuniffi --features cli --bin uniffi-bindgen -- \
  generate --library target/release/libyuniffi.dylib --language swift --out-dir out
```

The `cli` feature needs a newer Rust than the library's `rust-version` (1.85).
