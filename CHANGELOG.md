# Changelog

All notable changes to this fork are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0, a minor version bump
(0.x → 0.x+1) may contain breaking changes; a patch release must not.

History before 0.28.1 is in the upstream [y-crdt/y-crdt](https://github.com/y-crdt/y-crdt)
repository and its release notes.

## [Unreleased]

### Changed
- Subdocument references are written like Yjs' `ContentDoc`: `gc` only when it is off,
  `autoLoad` only when it is on, and never `shouldLoad`, so Yjs peers no longer load every
  yrs-created subdocument eagerly. On read, `shouldLoad` is `shouldLoad || autoLoad`, as in Yjs.
- **Breaking:** `Options` has a new `meta: Option<Any>` field (Yjs' `Doc.meta`, kept across
  re-encodes and returned by `Doc::meta`), and no longer implements `Eq` since `Any` doesn't.
- "v2" state vectors and snapshots now use Yjs' formats: `StateVector::encode_v2` writes the
  same bytes as `encode_v1` (Yjs has one state vector format), and `Snapshot::encode_v2` writes
  Yjs' `encodeSnapshotV2` layout. `encode_state_vector_from_update_v2` returns a plain state
  vector, and `diff_updates_v2` accepts the one Yjs sends. The decoders still accept the
  whole-v2-update frame yrs wrote before.
- `XmlElementRef::get_string` renders like Yjs' `YXmlElement.toString()`: attributes sorted by
  name and the tag name lowercased. Attribute order used to follow hash-map order.
  `XmlTextRef::get_string` sorts the attributes of each format the same way.
- **Breaking:** `TypeRef::XmlHook` is now `TypeRef::XmlHook(Arc<str>)` and carries the hook
  name, like `TypeRef::XmlElement` carries its tag. `XmlHookRef::hook_name` returns it.

- **Breaking (yffi):** `ERR_INVALID_PARENT` is now 11. It was 9, the same value as
  `ERR_CUSTOM`, so callers couldn't tell the two errors apart.

### Added
- `Update::decode_v2_with_block_limit`, which decodes a v2 update from untrusted input while
  refusing ones that would decode into more than a given number of blocks.
- `Any::MAX_DECODE_DEPTH`: decoding an `Any` value nested deeper than this returns an error.
- `SECURITY.md` describing how to report vulnerabilities privately.
- Property tests for CRDT behaviour (`yrs/tests/crdt_properties.rs`): convergence, apply in any
  order, merge ≡ sequential apply, diffs, and encode/decode stability.
- Tests that sync and awareness messages match y-protocols byte for byte.
- A CI coverage job with a ratchet: line coverage per module may not drop below
  `.github/coverage/floor.json`.

### Fixed
- Updates received out of order could stay pending forever, so a peer never showed content that
  had arrived. A pending update recorded only which client it waited for, sometimes the wrong
  one (one whose blocks it held itself), and compared clocks, which can't see a hole that yrs
  fills with a Skip. It now records the exact IDs it waits for and is retried once they arrive.
- A v1 update whose embed or format JSON held an integer too large for an i64 (Yjs writes
  `1e19` as twenty digits) was rejected as a whole. Such numbers now read as floats, as
  `JSON.parse` reads them.
- Clocks of 2^31 and above (left and right origins, map key clocks) were encoded wrongly in v2,
  and Yjs' v2 encoding of them failed to decode: the run-length column encoder used 32-bit
  differences. It now uses 64-bit ones, as Yjs' numbers allow.
- Relative positions: `sticky_index(len, Assoc::After)` (a caret at the end of a sequence)
  returned `None`; it now returns the end-of-type position Yjs creates. Positions without a
  trailing assoc (written by Yjs before it had one) decode as "after" instead of failing, and
  any non-negative assoc means "after", as in Yjs.
- A v2 update in which Yjs split a surrogate pair across two text items (each holding a lone
  surrogate) decoded into the wrong item lengths and clocks. Each half now reads as U+FFFD,
  keeping lengths and clocks as in Yjs.
- `YXmlHook` nodes were encoded and decoded without their hook name. A v1 update holding one
  failed to decode, and in v2 every key after it (element tags, attribute names) was read from
  the wrong position.
- `ContentJSON` (what Yjs 12 and early 13 wrote for array and map values) could not be decoded:
  the decoder read one entry more than Yjs writes. Its values are now also returned parsed, as
  Yjs does (`undefined` as `Any::Undefined`), instead of as raw JSON text.
- V2 encoding wrote binary content (`Uint8Array` values from Yjs) with a second length prefix,
  so any v2 re-encode, merge or diff of such a document produced bytes that neither Yjs nor yrs
  could decode.
- `encode_state_from_snapshot` panicked when the snapshot listed a client at clock 0. Such
  clients are now skipped, as in Yjs.
- Deleting and garbage-collecting deeply nested shared types no longer recurses, so it cannot
  overflow the stack.
- Decoding reserves memory in proportion to the remaining input rather than to lengths read
  from it.
- `ItemPtr`'s `Hash` and `Eq` now agree (both compare addresses).
- `IdSet` squashing no longer computes an out-of-range index for an empty block list.
- yffi: `yinput_null`/`yinput_undefined` and the matching `YOutput` constructors no longer
  return uninitialized memory in the value field.
- The C header `libyrs.h` now includes `ERR_MALFORMED`.
- yffi: `ychunks_destroy`, `yundo_manager_destroy`, `ysticky_index_destroy`, `yweak_destroy` and
  `yweak_iter_destroy` accept NULL, like the other destroy functions.
- yffi: `ybranch_get` with a root name that isn't valid UTF-8 returns NULL instead of reading it
  as UTF-8 unchecked (undefined behaviour).

- ywasm: transaction origins that aren't strings (objects, numbers, `null`) were stored as raw
  wasm-bindgen heap slot indices: a second read got a freed slot, some transactions threw, and
  `trackedOrigins` never matched an object. They are now kept in a registry and compared with
  `===`, as in Yjs. The registry holds origin objects weakly, so it doesn't keep them alive. An
  origin set from Rust reads as a `Uint8Array` of its bytes.
- ywasm: passing a ywasm object other than a transaction (or a freed one) where a transaction
  is expected is now an error instead of a cast of its pointer. Using a freed shared type is
  also an error.
- ywasm: a `BigInt` value panicked (and trapped the instance); it is now stored as an integer,
  or rejected if it doesn't fit 64 bits. A `Uint8Array` is now stored as binary, as in Yjs,
  instead of as an object with numbered keys.

### Removed
- ywasm: the unused, unmaintained optional `wee_alloc` dependency (and its implicit feature).

### Development
- CI runs on `next` as well as `main`, with actions pinned by SHA.
- New CI gates: `cargo fmt --check`, `cargo clippy -D warnings` (pre-existing lints listed in
  `[workspace.lints]`), `cargo deny check`, a check that the C header is up to date, the C tests
  under AddressSanitizer/UndefinedBehaviorSanitizer, and every benchmark run once in test mode.
- Releases now publish only after the full CI suite passes.
- Benchmarks use a fresh document per iteration; B4 benchmarks find their input files again.

## [0.28.1] - upstream

Last upstream release this fork is based on.
