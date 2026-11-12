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
- **Breaking:** `TypeRef::XmlHook` is now `TypeRef::XmlHook(Arc<str>)` and carries the hook
  name, like `TypeRef::XmlElement` carries its tag. `XmlHookRef::hook_name` returns it.

### Added
- `Update::decode_v2_with_block_limit`, which decodes a v2 update from untrusted input while
  refusing ones that would decode into more than a given number of blocks.
- `Any::MAX_DECODE_DEPTH`: decoding an `Any` value nested deeper than this returns an error.
- `SECURITY.md` describing how to report vulnerabilities privately.

### Fixed
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
- Deleting and garbage-collecting deeply nested shared types no longer recurses, so it cannot
  overflow the stack.
- Decoding reserves memory in proportion to the remaining input rather than to lengths read
  from it.
- `ItemPtr`'s `Hash` and `Eq` now agree (both compare addresses).
- `IdSet` squashing no longer computes an out-of-range index for an empty block list.
- yffi: `yinput_null`/`yinput_undefined` and the matching `YOutput` constructors no longer
  return uninitialized memory in the value field.
- The C header `libyrs.h` now includes `ERR_MALFORMED`.

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
