# Bindings for Swift and Kotlin: UniFFI or the C ABI

Plan item 7.7 asks whether to generate Swift and Kotlin bindings with
[UniFFI](https://mozilla.github.io/uniffi-rs/) or keep building them on yffi's C ABI. This is
the evaluation. Nothing here is built or published yet.

## Recommendation

Use **UniFFI for Swift and Kotlin**, in a new `yuniffi` crate on top of a shared
`ybindings-core`. Keep **yffi's C ABI** for C, C++, and languages with a C FFI but no UniFFI
generator (Go, Ruby, Python through cffi, .NET through P/Invoke).

## Why

yffi hands out raw pointers (`Branch*`, `YTransaction*`) and leaves lifetimes to the caller.
Every Swift and Kotlin wrapper over it has to rebuild the same things by hand: reference
ownership, freeing in the right order, mapping the `YOutput` tagged union to native enums,
callback trampolines, and error codes. Each of those is a place to get memory safety wrong.

UniFFI generates those layers from Rust definitions:

| Concern | C ABI (yffi) | UniFFI |
|---|---|---|
| Object lifetime | Raw pointers; caller frees, in order | `Arc`-backed objects; the host's GC or ARC frees them |
| Use after the document is gone | Undefined behavior | An object holds its `Arc<Doc>`, so it can't happen |
| Values | `YOutput` tagged union, manual `youtput_destroy` | Generated native enums and records |
| Errors | Status codes, NULL, or a panic (abort) | Thrown native errors from `Result` |
| Callbacks | C function pointer plus `void*` state | Native closures through callback interfaces |
| Async | Not available | `async fn` maps to Swift `async` and Kotlin coroutines |
| Header | cbindgen, checked into the repo | Generated per language at build time |

The costs:

- **Per-call overhead.** UniFFI copies arguments through a serialized buffer and goes through an
  `Arc` for every object call. For yrs this is small next to the work of a transaction, but
  calls that move many small values one at a time (reading a long array element by element)
  should get batch methods (`to_json`, `values()`).
- **Transactions don't map to borrows.** Rust's `TransactionMut<'doc>` can't cross the
  boundary. `yuniffi` would expose a `Transaction` object owning a `TransactionMut<'static>`
  over an `Arc<Doc>` it holds, made safe by that ownership, and closing on `commit()` or drop.
  Using it after commit is an error, not undefined behavior. This is the same model plan item
  7.2 asks yffi to adopt for read transactions.
- **Generated API shape.** The bindings follow UniFFI's conventions, not hand-written idioms. A
  thin hand-written layer per language can add conveniences (Swift `Codable` for `Any`, Kotlin
  `Flow` for observers) where they matter.
- **One more crate to release.** `yuniffi` would build XCFrameworks for Apple platforms and AARs
  for Android in CI. Those artifacts need signing and publishing accounts, which are out of scope
  for now.

## Shape of the work

1. **`ybindings-core`** (plan item 7.7, first half): move the conversions yffi and ywasm both
   write by hand into one crate. That covers values (`Any` and `Out` to a binding-neutral enum),
   events and deltas, origins, and undo options. yffi and ywasm then depend on it, so the three
   bindings can't drift apart.
2. **`yuniffi`**: UDL-free (proc-macro) UniFFI definitions over `ybindings-core`:
   - `Doc`, `Transaction`, and the shared types as `#[uniffi::Object]`s holding `Arc<Doc>`;
   - values and events as `#[uniffi::Enum]`/`#[uniffi::Record]`;
   - errors as `#[uniffi::Error]`;
   - observers as callback interfaces whose subscription object unsubscribes on drop.
3. **Tests in each language**: port the C suite's cases (`tests-ffi/main.cpp`) to Swift (XCTest)
   and Kotlin (JUnit), run against the generated bindings in CI on macOS and Linux.
4. **Artifacts**: XCFramework (iOS, iOS simulator, macOS) and Android AARs (arm64-v8a,
   armeabi-v7a, x86_64), built in CI. Publishing to SwiftPM and Maven Central comes after.

## What stays on the C ABI

yffi stays the stable, lowest-common-denominator interface, with the hardening the plan already
calls for:
- 7.1: status codes and `ylast_error()` instead of panics;
- 7.3: generational handles instead of raw pointers;
- 7.5: an ISO C header with an ABI version.

Languages without a UniFFI generator keep using it, and so do hosts that need to embed yrs
without a runtime.
