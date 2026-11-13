//! Allocation budgets for hot paths (plan items 6.5 and 6.9). A budget is the number of heap
//! allocations an operation makes today: going over it fails the test, and an optimization that
//! goes under it should lower the budget.
//!
//! dhat allows one profiler per process, so every measurement lives in the one test below.

use yrs::updates::decoder::Decode;
use yrs::{Array, Doc, GetString, Map, ReadTxn, StateVector, Text, Transact, Update};

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

/// Allocations made by `f`.
fn allocations(f: impl FnOnce()) -> u64 {
    let before = dhat::HeapStats::get().total_blocks;
    f();
    dhat::HeapStats::get().total_blocks - before
}

fn check_bytes(name: &str, budget: u64, used: u64) -> Option<String> {
    println!("{name}: {used} bytes for {TYPED_CHARS} chars (budget {budget})");
    (used > budget).then(|| format!("{name}: {used} bytes, over the budget of {budget}"))
}

fn check(name: &str, budget: u64, used: u64) -> Option<String> {
    println!("{name}: {used} allocations (budget {budget})");
    (used > budget).then(|| format!("{name}: {used} allocations, over the budget of {budget}"))
}

#[test]
fn hot_paths_stay_within_their_allocation_budgets() {
    let _profiler = dhat::Profiler::builder().testing().build();
    let mut over = Vec::new();

    let doc = Doc::with_client_id(1);
    let text = doc.get_or_insert_text("text");
    let array = doc.get_or_insert_array("array");
    let map = doc.get_or_insert_map("map");
    // Warm up: the first edit of each type creates its root and per-client lists.
    {
        let mut txn = doc.transact_mut();
        text.push(&mut txn, "abc");
        array.push_back(&mut txn, 1);
        map.insert(&mut txn, "k", 1);
    }
    // and the first read sets up what later reads reuse.
    let _ = text.get_string(&doc.transact());

    // One character typed at the end: the most common edit there is.
    let used = allocations(|| {
        let mut txn = doc.transact_mut();
        text.push(&mut txn, "d");
    });
    over.extend(check("text push of one char", BUDGET_TEXT_PUSH, used));

    // One character typed in the middle splits an item.
    let used = allocations(|| {
        let mut txn = doc.transact_mut();
        text.insert(&mut txn, 1, "x");
    });
    over.extend(check(
        "text insert in the middle",
        BUDGET_TEXT_INSERT_MID,
        used,
    ));

    let used = allocations(|| {
        let mut txn = doc.transact_mut();
        array.push_back(&mut txn, 2);
    });
    over.extend(check("array push", BUDGET_ARRAY_PUSH, used));

    let used = allocations(|| {
        let mut txn = doc.transact_mut();
        map.insert(&mut txn, "k", 2);
    });
    over.extend(check("map overwrite", BUDGET_MAP_SET, used));

    let used = allocations(|| {
        let txn = doc.transact();
        let _ = text.get_string(&txn);
    });
    over.extend(check("text read", BUDGET_TEXT_READ, used));

    // A remote one-char update applied to a peer.
    let peer = Doc::with_client_id(2);
    let update = doc
        .transact()
        .encode_state_as_update_v1(&StateVector::default());
    peer.transact_mut()
        .apply_update(Update::decode_v1(&update).unwrap())
        .unwrap();
    let sv = doc.transact().state_vector();
    {
        let mut txn = doc.transact_mut();
        text.push(&mut txn, "e");
    }
    let delta = doc.transact().encode_state_as_update_v1(&sv);
    let used = allocations(|| {
        let update = Update::decode_v1(&delta).unwrap();
        peer.transact_mut().apply_update(update).unwrap();
    });
    over.extend(check(
        "apply a one-char remote update",
        BUDGET_APPLY_REMOTE,
        used,
    ));

    // Memory held by a document typed one character per transaction, as an editor does, and
    // by the same text loaded from one update.
    let before = dhat::HeapStats::get().curr_bytes;
    let typed = Doc::with_client_id(3);
    let typed_text = typed.get_or_insert_text("text");
    for i in 0..TYPED_CHARS {
        let mut txn = typed.transact_mut();
        typed_text.push(&mut txn, if i % 7 == 0 { " " } else { "a" });
    }
    let typed_bytes = (dhat::HeapStats::get().curr_bytes - before) as u64;
    over.extend(check_bytes(
        "typed document",
        BUDGET_TYPED_BYTES,
        typed_bytes,
    ));

    let encoded = typed
        .transact()
        .encode_state_as_update_v1(&StateVector::default());
    let before = dhat::HeapStats::get().curr_bytes;
    let loaded = Doc::with_client_id(4);
    loaded
        .transact_mut()
        .apply_update(Update::decode_v1(&encoded).unwrap())
        .unwrap();
    let loaded_bytes = (dhat::HeapStats::get().curr_bytes - before) as u64;
    over.extend(check_bytes(
        "loaded document",
        BUDGET_LOADED_BYTES,
        loaded_bytes,
    ));
    drop((typed, loaded));

    assert!(over.is_empty(), "{}", over.join("\n"));
}

// Budgets as measured. Plan item 6.5's target, at most 2 allocations for a single-character
// transaction, is met: the item and the insert set's entry.
const BUDGET_TEXT_PUSH: u64 = 2;
const BUDGET_TEXT_INSERT_MID: u64 = 6;
const BUDGET_ARRAY_PUSH: u64 = 3;
const BUDGET_MAP_SET: u64 = 5;
const BUDGET_TEXT_READ: u64 = 1;
const BUDGET_APPLY_REMOTE: u64 = 9;

const TYPED_CHARS: usize = 10_000;
// Bytes are budgeted with 25% headroom over 18,392 and 12,008: they move between runs and
// platforms (hash map growth), unlike the allocation counts above.
const BUDGET_TYPED_BYTES: u64 = 23_000;
const BUDGET_LOADED_BYTES: u64 = 15_000;
