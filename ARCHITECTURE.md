# Architecture

This is a map of `yrs`, the Rust port of [Yjs](https://github.com/yjs/yjs), for people changing it.
It says where things live and how they fit together. For the API, see the crate docs.

The workspace has three crates:

| Crate | What it is |
|---|---|
| `yrs` | The CRDT: documents, shared types, the update format, the sync protocol. |
| `yffi` | A C ABI over `yrs`, with a generated header at `tests-ffi/include/libyrs.h`. |
| `ywasm` | JavaScript bindings over `yrs`, built with wasm-bindgen. |

The bindings hold raw pointers into `yrs`. They reach its internals through `yrs::internals`, a
hidden module exempt from semver.

## The item model

A document is a set of **items**: `Item` in `yrs/src/block.rs`. Every insert creates one. An item
has:

- an `ID`: the inserting peer's `ClientID` and a per-client **clock**. A client's clocks are
  contiguous: an item of length *n* uses *n* clocks;
- its `content`, which is `ItemContent`: a string, JSON or `Any` values, binary, an embed, a
  format marker, a nested shared type (a `Branch`), a subdocument, or `Deleted(len)`;
- its `parent`, the shared type it belongs to, and, for map entries, `parent_sub`, the key;
- `origin` and `right_origin`: the IDs of its neighbours *when it was inserted*. These never
  change, and they are what makes concurrent inserts converge;
- `left` and `right`: its *current* neighbours, as raw `ItemPtr`s into the block store;
- flags (deleted, countable, keep, marker) and `redone`, for undo.

A **shared type** (`TextRef`, `ArrayRef`, `MapRef`, the XML types, `WeakRef`) is a handle to a
`Branch` (`yrs/src/branch.rs`). The branch holds the head of its doubly linked list of children
(`start`), a map from key to the latest item for that key (`map`), cached lengths, observers, and
the item that contains it (`item`), or a name if it is a root type.

Items are never removed while their ID may still be referenced: deleting sets a flag, and the
content is dropped later by garbage collection.

## Integration: YATA

Inserting an item means finding its place among concurrent inserts with the same `origin`. This
is the YATA rule in `TransactionMut::integrate_item` and `Item::resolve_conflict`
(`yrs/src/block.rs`), the same algorithm as Yjs' `Item.integrate`. Items between the origin and
the right origin are scanned, and conflicts are ordered by origin, then by client ID. Every peer
that integrates the same set of items, in any order, gets the same list.

Integration needs both origins to be present. An update whose dependencies haven't arrived goes
into `Store::pending` and is retried when later updates fill the gap, as in Yjs.

## The block store

`BlockStore` (`yrs/src/block_store.rs`) maps each client to a `ClientBlockList`: that client's
blocks in clock order. A block is an `Item`, a `GC` range (collected items whose IDs are still
known) or a `Skip` (a gap in an update). Lookup by ID is a binary search, with a guess based on
clock density first. Splitting an item (to insert inside it) replaces it with two items; adjacent
blocks are merged back when a transaction commits.

Items are heap allocations owned by their block, so `ItemPtr`s stay valid while blocks move
inside the `Vec`. Branches are owned the same way by the item that contains them, or by
`Store::types` for root types.

`Store` (`yrs/src/store.rs`) holds the block store, the root types, the pending update and
delete set, subdocuments, and the document's options.

## Transactions

All access goes through transactions (`yrs/src/transaction.rs`): `Transaction` for reads,
`TransactionMut` for writes. `Doc` holds its store behind a read-write lock (`DocStore` in
`yrs/src/store.rs`). A thread that would wait on a lock it already holds is refused with
`TransactionAcqError::Reentrant` instead of deadlocking.

A write transaction records what it changed: the state vector before it (`before_state`), the
items it deleted (`delete_set`), and the types it touched (`changed`). `TransactionMut::commit`,
which runs on drop, then:

1. calls type observers, then deep observers, with events built from that record;
2. cleans up formatting markers made redundant, if enabled;
3. calls the after-transaction observers;
4. garbage collects the deleted items, unless GC is off (`Options::skip_gc`);
5. merges adjacent blocks that the transaction split or deleted;
6. emits the update events (`observe_update_v1` and `_v2`), which carry the transaction's
   changes encoded as an update;
7. adds and removes subdocuments.

Observers run while the transaction is still held. They can read through it but can't open
another one on the same document.

## Garbage collection

`yrs/src/gc.rs`. A deleted item can't be removed from the list, since later inserts may name it as
an origin. GC replaces its content with `ItemContent::Deleted`, and replaces deleted items whose
parent is itself deleted with `GC` ranges, which keep only the ID range. Nested types are
collected recursively. `Options::gc_filter` (Yjs' `gcFilter`) keeps items it returns `false` for,
and items flagged *keep* (held by the UndoManager or a snapshot) are never collected.

## Encoding

Updates (`yrs/src/update.rs`, `yrs/src/updates/`) are the unit of sync. An update is the items a
peer is missing, grouped by client in clock order, plus a **delete set** (`IdSet`): ranges of IDs
deleted. A **state vector** (`StateVector`) is each client's next clock, so the difference
between two documents is "everything after the other side's state vector".

There are two wire formats, both byte-compatible with Yjs: v1 (`EncoderV1`/`DecoderV1`, varints
throughout) and v2 (`EncoderV2`/`DecoderV2`, which splits fields into run-length-encoded columns
and is usually smaller). Values inside items use lib0's `Any` encoding (`yrs/src/any.rs`).

Decoding is bounded: every length read from the input is checked against the bytes left before
anything is allocated, and nesting is limited (`yrs/tests/decode_limits.rs`).

`yrs/src/alt.rs` has update utilities that work without a document: merging, diffing, reading
metadata, converting between v1 and v2, and obfuscating, as in Yjs' `utils/updates.js`.

## The sync protocol

`yrs/src/protocol/` (feature `protocol`, on by default) implements y-protocols: the sync
messages (state vector, then the missing update) in `protocol.rs`, and awareness (presence
state, with Yjs' timeouts) in `awareness.rs`. `yrs/tests/relay.rs` is a reference server: one
actor per document, with limits, eviction and a persistence hook.

## Undo

`UndoManager` (`yrs/src/undo.rs`) keeps stacks of (insertions, deletions) as ID sets. Undoing
deletes the inserted items and redoes the deleted ones (`Item::redo`), which inserts copies and
links the originals to them through `redone`. Items the manager may still redo are flagged
*keep*, so GC leaves them alone.

## Where to look

| Task | Start in |
|---|---|
| A shared type's API | `yrs/src/types/` |
| Converting values in and out | `yrs/src/input.rs` (`In`), `yrs/src/out.rs` (`Out`), `yrs/src/any.rs` |
| Positions that survive edits | `yrs/src/sticky_index.rs` |
| Events and observers | `yrs/src/types/mod.rs` (`Event`), `yrs/src/observer.rs` |
| Tracing and metrics | `yrs/src/metrics.rs` |
| Compatibility with Yjs | `yrs/tests/yjs_compat.rs`, `tests-compat/` |
| Convergence properties | `yrs/tests/crdt_properties.rs` |

See [docs/THREADING.md](docs/THREADING.md) for the threading model and the rules unsafe code follows.
