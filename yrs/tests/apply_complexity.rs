//! Applying an update must stay linear in its size, whatever it holds.

use std::time::{Duration, Instant};
use yrs::updates::decoder::Decode;
use yrs::{merge_updates_v1, Doc, Map, MapPrelim, ReadTxn, StateVector, Transact, Update};

/// A document with `n` nested maps under the root map, and one update that sets a key in each of
/// them and then deletes them all. A single encode would garbage-collect the inserts, so it's two
/// updates merged.
fn nested_maps_set_then_deleted(n: usize) -> (Vec<u8>, Vec<u8>) {
    let doc = Doc::with_client_id(1);
    let root = doc.get_or_insert_map("root");
    let nested: Vec<_> = {
        let mut txn = doc.transact_mut();
        (0..n)
            .map(|i| root.insert(&mut txn, format!("m{i}"), MapPrelim::default()))
            .collect()
    };
    let base = doc
        .transact()
        .encode_state_as_update_v1(&StateVector::default());

    let sv = doc.transact().state_vector();
    {
        let mut txn = doc.transact_mut();
        for m in &nested {
            m.insert(&mut txn, "k", 1);
        }
    }
    let sets = doc.transact().encode_state_as_update_v1(&sv);
    let sv = doc.transact().state_vector();
    {
        let mut txn = doc.transact_mut();
        for i in 0..n {
            root.remove(&mut txn, &format!("m{i}"));
        }
    }
    let deletes = doc.transact().encode_state_as_update_v1(&sv);
    (base, merge_updates_v1([sets, deletes]).unwrap())
}

fn time_apply(n: usize) -> Duration {
    let (base, update) = nested_maps_set_then_deleted(n);
    (0..3)
        .map(|_| {
            let doc = Doc::with_client_id(2);
            doc.transact_mut()
                .apply_update(Update::decode_v1(&base).unwrap())
                .unwrap();
            let update = Update::decode_v1(&update).unwrap();
            let start = Instant::now();
            doc.transact_mut().apply_update(update).unwrap();
            start.elapsed()
        })
        .min()
        .unwrap()
}

/// Removing each deleted type from the transaction's changed set used to shift the rest of it,
/// so this update took time quadratic in the number of maps: four times the maps took sixteen
/// times as long. The ratio is compared rather than the time, so a slow machine doesn't fail it.
#[test]
fn deleting_many_changed_types_in_one_update_is_linear() {
    let small = time_apply(5_000);
    let large = time_apply(20_000);
    let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-6);
    assert!(
        ratio < 9.0,
        "4x the maps took {ratio:.1}x as long ({small:?} -> {large:?}); linear is about 4x"
    );
}
