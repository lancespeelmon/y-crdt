use std::time::Instant;
use yrs::updates::decoder::Decode;
use yrs::{Doc, GetString, ReadTxn, StateVector, Text, Transact, Update};

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .map(|s| s.parse().unwrap())
        .unwrap_or(2000);
    let src = Doc::with_client_id(1);
    let txt = src.get_or_insert_text("t");
    let mut updates = Vec::with_capacity(n);
    for i in 0..n {
        let sv = src.transact().state_vector();
        txt.insert(&mut src.transact_mut(), i as u32, "x");
        updates.push(src.transact().encode_diff_v1(&sv));
    }
    let _ = StateVector::default();
    let dst = Doc::with_client_id(2);
    let t2 = dst.get_or_insert_text("t");
    let start = Instant::now();
    for u in updates.iter().rev() {
        dst.transact_mut()
            .apply_update(Update::decode_v1(u).unwrap())
            .unwrap();
    }
    let el = start.elapsed();
    assert_eq!(t2.get_string(&dst.transact()).len(), n);
    println!("n={n} reverse apply: {el:?}");
}
