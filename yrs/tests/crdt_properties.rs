//! CRDT properties of yrs documents, checked with proptest over random edit histories.
//!
//! A history is a list of steps. Each step is either a transaction of edits by one peer (on a
//! text, an array and a map) or a sync from one peer to another. Every local transaction's
//! update is recorded. The properties:
//!
//! - **Convergence:** peers that have seen the same updates have the same content.
//! - **Commutativity:** applying all updates to a fresh document gives the same content in any
//!   order (updates that arrive before their dependencies wait as pending).
//! - **Merge ≡ sequential apply:** one merged update gives the same content as the updates
//!   applied one by one, in both encodings.
//! - **Diffs:** the diff of a full state against a peer's state vector brings that peer up to
//!   date, and contains nothing that peer already has.
//! - **Encode/decode identity:** re-encoding a decoded full-state update is stable, converting
//!   between the encodings and back gives the same bytes, and no form loses content.

use proptest::prelude::*;
use yrs::types::ToJson;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{
    merge_updates_v1, merge_updates_v2, Any, Array, Doc, GetString, Map, ReadTxn, StateVector,
    Text, Transact, Update,
};

const MAX_PEERS: usize = 4;

#[derive(Debug, Clone)]
enum Edit {
    TextInsert(u32, String),
    TextRemove(u32, u32),
    ArrayInsert(u32, i64),
    ArrayRemove(u32, u32),
    MapSet(u8, i64),
    MapRemove(u8),
}

#[derive(Debug, Clone)]
enum Step {
    /// One transaction of edits by a peer.
    Edit(usize, Vec<Edit>),
    /// The second peer catches up with the first.
    Sync(usize, usize),
}

fn edit() -> impl Strategy<Value = Edit> {
    // Positions are taken modulo the current length, so any number is valid. Text is ASCII, so
    // every byte offset is a character boundary.
    prop_oneof![
        (any::<u32>(), "[a-z]{1,4}").prop_map(|(i, s)| Edit::TextInsert(i, s)),
        (any::<u32>(), 1..4u32).prop_map(|(i, n)| Edit::TextRemove(i, n)),
        (any::<u32>(), any::<i64>()).prop_map(|(i, v)| Edit::ArrayInsert(i, v)),
        (any::<u32>(), 1..3u32).prop_map(|(i, n)| Edit::ArrayRemove(i, n)),
        (0..4u8, any::<i64>()).prop_map(|(k, v)| Edit::MapSet(k, v)),
        (0..4u8).prop_map(Edit::MapRemove),
    ]
}

fn step(peers: usize) -> impl Strategy<Value = Step> {
    prop_oneof![
        3 => (0..peers, prop::collection::vec(edit(), 1..4)).prop_map(|(p, e)| Step::Edit(p, e)),
        1 => (0..peers, 0..peers).prop_map(|(from, to)| Step::Sync(from, to)),
    ]
}

fn history() -> impl Strategy<Value = (usize, Vec<Step>)> {
    (2..=MAX_PEERS).prop_flat_map(|peers| (Just(peers), prop::collection::vec(step(peers), 1..24)))
}

fn new_doc(client_id: u64) -> Doc {
    let doc = Doc::with_client_id(client_id);
    // Define every root, so content compares equal whether or not a root was ever edited.
    doc.get_or_insert_text("text");
    doc.get_or_insert_array("array");
    doc.get_or_insert_map("map");
    doc
}

/// The roots of a document, looked up before a transaction opens (`get_or_insert_*` opens its own
/// and would wait on an open one forever).
struct Roots {
    text: yrs::TextRef,
    array: yrs::ArrayRef,
    map: yrs::MapRef,
}

impl Roots {
    fn of(doc: &Doc) -> Self {
        Roots {
            text: doc.get_or_insert_text("text"),
            array: doc.get_or_insert_array("array"),
            map: doc.get_or_insert_map("map"),
        }
    }
}

fn apply(roots: &Roots, edit: &Edit, txn: &mut yrs::TransactionMut) {
    let Roots { text, array, map } = roots;
    match edit {
        Edit::TextInsert(i, s) => {
            let at = i % (text.len(txn) + 1);
            text.insert(txn, at, s);
        }
        Edit::TextRemove(i, n) => {
            let len = text.len(txn);
            if len > 0 {
                let at = i % len;
                text.remove_range(txn, at, (*n).min(len - at));
            }
        }
        Edit::ArrayInsert(i, v) => {
            let at = i % (array.len(txn) + 1);
            array.insert(txn, at, *v);
        }
        Edit::ArrayRemove(i, n) => {
            let len = array.len(txn);
            if len > 0 {
                let at = i % len;
                array.remove_range(txn, at, (*n).min(len - at));
            }
        }
        Edit::MapSet(k, v) => {
            map.insert(txn, format!("k{}", k), *v);
        }
        Edit::MapRemove(k) => {
            map.remove(txn, &format!("k{}", k));
        }
    }
}

/// Runs a history on fresh peers and returns them with every local update, in the order made.
fn run(peers: usize, steps: &[Step]) -> (Vec<Doc>, Vec<Vec<u8>>) {
    let docs: Vec<Doc> = (0..peers).map(|i| new_doc(i as u64 + 1)).collect();
    let mut updates = Vec::new();
    for step in steps {
        match step {
            Step::Edit(peer, edits) => {
                let doc = &docs[*peer];
                let roots = Roots::of(doc);
                let mut txn = doc.transact_mut();
                for e in edits {
                    apply(&roots, e, &mut txn);
                }
                let update = txn.encode_update_v1();
                drop(txn);
                updates.push(update);
            }
            Step::Sync(from, to) if from != to => sync(&docs[*from], &docs[*to]),
            Step::Sync(..) => {}
        }
    }
    (docs, updates)
}

fn sync(from: &Doc, to: &Doc) {
    let sv = to.transact().state_vector();
    let diff = from.transact().encode_state_as_update_v1(&sv);
    to.transact_mut()
        .apply_update(Update::decode_v1(&diff).unwrap())
        .unwrap();
}

fn content(doc: &Doc) -> (String, Any, Any) {
    let roots = Roots::of(doc);
    let txn = doc.transact();
    (
        roots.text.get_string(&txn),
        roots.array.to_json(&txn),
        roots.map.to_json(&txn),
    )
}

fn state_vector(doc: &Doc) -> StateVector {
    doc.transact().state_vector()
}

/// A fresh document with the given updates applied, one transaction each.
fn replay<'a>(updates: impl IntoIterator<Item = &'a Vec<u8>>) -> Doc {
    let doc = new_doc(1000);
    for u in updates {
        doc.transact_mut()
            .apply_update(Update::decode_v1(u).unwrap())
            .unwrap();
    }
    doc
}

fn fully_synced(docs: &[Doc]) {
    for a in docs {
        for b in docs {
            if !std::ptr::eq(a, b) {
                sync(a, b);
            }
        }
    }
}

proptest! {
    // 256 cases by default; set PROPTEST_CASES for a longer run.
    #![proptest_config(ProptestConfig::with_cases(
        std::env::var("PROPTEST_CASES").ok().and_then(|n| n.parse().ok()).unwrap_or(256)
    ))]

    #[test]
    fn peers_that_have_seen_the_same_updates_converge((peers, steps) in history()) {
        let (docs, updates) = run(peers, &steps);
        fully_synced(&docs);
        let expected = content(&docs[0]);
        for doc in &docs[1..] {
            prop_assert_eq!(content(doc), expected.clone());
            prop_assert_eq!(state_vector(doc), state_vector(&docs[0]));
        }
        // A fresh peer replaying the updates in the order they were made agrees too.
        prop_assert_eq!(content(&replay(&updates)), expected);
    }

    #[test]
    fn applying_updates_in_any_order_gives_the_same_content(
        (peers, steps, order) in history().prop_flat_map(|(peers, steps)| {
            let n = steps.iter().filter(|s| matches!(s, Step::Edit(..))).count();
            (Just(peers), Just(steps), Just((0..n).collect::<Vec<_>>()).prop_shuffle())
        })
    ) {
        let (_, updates) = run(peers, &steps);
        let in_order = replay(&updates);
        let shuffled = replay(order.iter().map(|&i| &updates[i]));
        prop_assert_eq!(content(&shuffled), content(&in_order));
        prop_assert_eq!(state_vector(&shuffled), state_vector(&in_order));
        // Nothing is left waiting once every update has arrived.
        let txn = shuffled.transact();
        prop_assert!(txn.store().pending_update().is_none());
        prop_assert!(txn.store().pending_ds().is_none());
    }

    #[test]
    fn a_merged_update_equals_applying_updates_one_by_one((peers, steps) in history()) {
        let (_, updates) = run(peers, &steps);
        let sequential = replay(&updates);

        let merged_v1 = merge_updates_v1(&updates).unwrap();
        let from_v1 = replay([&merged_v1]);
        prop_assert_eq!(content(&from_v1), content(&sequential));
        prop_assert_eq!(state_vector(&from_v1), state_vector(&sequential));

        let updates_v2: Vec<Vec<u8>> = updates
            .iter()
            .map(|u| Update::decode_v1(u).unwrap().encode_v2())
            .collect();
        let merged_v2 = merge_updates_v2(&updates_v2).unwrap();
        let from_v2 = new_doc(1000);
        from_v2
            .transact_mut()
            .apply_update(Update::decode_v2(&merged_v2).unwrap())
            .unwrap();
        prop_assert_eq!(content(&from_v2), content(&sequential));
        prop_assert_eq!(state_vector(&from_v2), state_vector(&sequential));
    }

    #[test]
    fn a_diff_brings_a_peer_up_to_date_with_only_what_it_lacks(
        (peers, steps, seen) in history().prop_flat_map(|(peers, steps)| {
            let n = steps.iter().filter(|s| matches!(s, Step::Edit(..))).count();
            (Just(peers), Just(steps), 0..=n)
        })
    ) {
        let (_, updates) = run(peers, &steps);
        let full = replay(&updates);
        let behind = replay(&updates[..seen]);
        let sv = state_vector(&behind);

        let diff = full.transact().encode_state_as_update_v1(&sv);
        let diff = Update::decode_v1(&diff).unwrap();
        // Every block in the diff starts at or after what the peer already has.
        for (client, clock) in diff.state_vector_lower().iter() {
            prop_assert!(*clock >= sv.get(client), "client {} starts below the peer's clock", client);
        }
        behind.transact_mut().apply_update(diff).unwrap();
        prop_assert_eq!(content(&behind), content(&full));
        prop_assert_eq!(state_vector(&behind), state_vector(&full));
    }

    #[test]
    fn re_encoding_a_full_state_update_is_stable_and_loses_nothing((peers, steps) in history()) {
        let (docs, _) = run(peers, &steps);
        fully_synced(&docs);
        let txn = docs[0].transact();
        let v1 = txn.encode_state_as_update_v1(&StateVector::default());
        let v2 = txn.encode_state_as_update_v2(&StateVector::default());
        drop(txn);

        // A document writes the parent-sub flag on map items that have an origin, but a decoded
        // update no longer knows the key, so the first re-encoding clears that bit. Yjs does the
        // same (`mergeUpdates` turns 0xa8 into 0x88), so bytes are compared from the first
        // re-encoding on.
        let v1_once = Update::decode_v1(&v1).unwrap().encode_v1();
        let v2_once = Update::decode_v2(&v2).unwrap().encode_v2();
        prop_assert_eq!(&Update::decode_v1(&v1_once).unwrap().encode_v1(), &v1_once);
        prop_assert_eq!(&Update::decode_v2(&v2_once).unwrap().encode_v2(), &v2_once);
        // Converting to the other encoding and back gives the same bytes.
        let v1_as_v2 = Update::decode_v1(&v1).unwrap().encode_v2();
        prop_assert_eq!(&Update::decode_v2(&v1_as_v2).unwrap().encode_v1(), &v1_once);
        prop_assert_eq!(&v1_as_v2, &v2_once);

        // Every form carries the same content.
        for (format, update) in [(1, &v1), (1, &v1_once), (2, &v2), (2, &v2_once)] {
            let doc = new_doc(1000);
            let update = if format == 1 {
                Update::decode_v1(update).unwrap()
            } else {
                Update::decode_v2(update).unwrap()
            };
            doc.transact_mut().apply_update(update).unwrap();
            prop_assert_eq!(content(&doc), content(&docs[0]));
        }
    }
}

/// The history both regressions below come from: peer 1 inserts next to peer 0's items, then
/// peer 0 inserts next to peer 1's.
fn interleaved_history(map_edit: bool) -> Vec<Step> {
    use Edit::*;
    // Positions are taken modulo the length (see `apply`); these are the ones proptest shrank to.
    let mut steps = vec![
        Step::Edit(
            0,
            vec![
                ArrayInsert(3044134820, -1),
                TextRemove(668047514, 3),
                ArrayInsert(3680969930, -2),
            ],
        ),
        Step::Sync(0, 1),
        Step::Edit(
            1,
            vec![
                ArrayInsert(1429539236, -3),
                TextInsert(4244475177, "p".into()),
            ],
        ),
        Step::Edit(1, vec![ArrayInsert(985096308, -4)]),
        Step::Sync(1, 0),
        Step::Edit(
            0,
            vec![
                ArrayInsert(850968186, -5),
                ArrayInsert(3561700470, -6),
                MapRemove(3),
            ],
        ),
    ];
    if map_edit {
        steps.insert(2, Step::Edit(0, vec![MapSet(2, -9)]));
    }
    steps
}

fn assert_out_of_order_delivery_converges(steps: &[Step], order: &[usize]) {
    let (_, updates) = run(2, steps);
    let in_order = replay(&updates);
    let shuffled = replay(order.iter().map(|&i| &updates[i]));
    assert_eq!(content(&shuffled), content(&in_order));
    let txn = shuffled.transact();
    assert!(
        txn.store().pending_update().is_none(),
        "updates left pending"
    );
}

#[test]
fn a_pending_update_whose_dependencies_form_a_chain_is_retried() {
    // Peer 1's second edit and peer 0's last edit arrive first, then peer 1's first edit: each
    // waits on the next, and all of them on peer 0's first edit. The pending update used to
    // record only that it waited for peer 1, so it was never retried.
    assert_out_of_order_delivery_converges(&interleaved_history(false), &[2, 3, 1, 0]);
}

#[test]
fn a_pending_update_waiting_on_a_hole_is_retried_when_the_hole_is_filled() {
    // Peer 0's map edit arrives early and is integrated behind a Skip, which moves peer 0's
    // clock past the items others wait for. Waiting for "peer 0's clock to pass N" then never
    // fires; the pending update has to wait for the IDs themselves.
    assert_out_of_order_delivery_converges(&interleaved_history(true), &[3, 1, 2, 4, 0]);
}
