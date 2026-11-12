# Changelog

All notable changes to this fork are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0, a minor version bump
(0.x → 0.x+1) may contain breaking changes; a patch release must not.

History before 0.28.1 is in the upstream [y-crdt/y-crdt](https://github.com/y-crdt/y-crdt)
repository and its release notes.

## [Unreleased]

### Added
- `Update::decode_v2_with_block_limit`, which decodes a v2 update from untrusted input while
  refusing ones that would decode into more than a given number of blocks.
- `Any::MAX_DECODE_DEPTH`: decoding an `Any` value nested deeper than this returns an error.
- `SECURITY.md` describing how to report vulnerabilities privately.

### Fixed
- Deleting and garbage-collecting deeply nested shared types no longer recurses, so it cannot
  overflow the stack.
- Decoding reserves memory in proportion to the remaining input rather than to lengths read
  from it.
- `ItemPtr`'s `Hash` now agrees with its `Eq` (both use the item ID).
- `IdSet` squashing no longer computes an out-of-range index for an empty block list.
- yffi: `yinput_null`/`yinput_undefined` and the matching `YOutput` constructors no longer
  return uninitialized memory in the value field.
- ywasm: `Awareness` no longer creates aliasing mutable references; re-entrant use from a JS
  callback is rejected at runtime.
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
