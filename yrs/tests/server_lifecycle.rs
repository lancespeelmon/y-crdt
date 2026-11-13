//! Plan item 6.8: streaming encode, memory accounting and compaction.

use yrs::updates::decoder::Decode;
use yrs::{Doc, GetString, ReadTxn, StateVector, Text, Transact, Update};

/// A document typed one character per transaction with some deletions, as an editor makes.
fn typed(chars: u32) -> Doc {
    let doc = Doc::with_client_id(1);
    let text = doc.get_or_insert_text("text");
    for i in 0..chars {
        let mut txn = doc.transact_mut();
        let len = text.len(&txn);
        text.insert(&mut txn, (i * 7) % (len + 1), "a");
        if i % 5 == 4 {
            text.remove_range(&mut txn, len / 2, 1);
        }
    }
    doc
}

#[test]
fn a_streamed_state_is_the_encoded_state() {
    for chars in [0, 10, 40_000] {
        let doc = typed(chars);
        let txn = doc.transact();
        let mut out = Vec::new();
        txn.write_state_as_update_v1(&StateVector::default(), &mut out)
            .unwrap();
        assert_eq!(
            out,
            txn.encode_state_as_update_v1(&StateVector::default()),
            "{chars} chars"
        );
        // the largest is several chunks long
        assert!(
            out.len() > 64 * 1024 || chars < 40_000,
            "{} bytes",
            out.len()
        );
    }
}

#[test]
fn a_streamed_state_includes_pending_updates() {
    let source = typed(20);
    let update = source
        .transact()
        .encode_state_as_update_v1(&StateVector::default());
    // A peer that gets only a later update holds it as pending.
    let later = {
        let sv = source.transact().state_vector();
        let text = source.get_or_insert_text("text");
        text.push(&mut source.transact_mut(), "z");
        source.transact().encode_state_as_update_v1(&sv)
    };
    let peer = Doc::with_client_id(2);
    peer.transact_mut()
        .apply_update(Update::decode_v1(&later).unwrap())
        .unwrap();
    let txn = peer.transact();
    let mut out = Vec::new();
    txn.write_state_as_update_v1(&StateVector::default(), &mut out)
        .unwrap();
    assert_eq!(out, txn.encode_state_as_update_v1(&StateVector::default()));
    drop(txn);
    // and what was streamed brings a third peer to the same state, once the rest arrives
    let third = Doc::with_client_id(3);
    third
        .transact_mut()
        .apply_update(Update::decode_v1(&out).unwrap())
        .unwrap();
    third
        .transact_mut()
        .apply_update(Update::decode_v1(&update).unwrap())
        .unwrap();
    let text = third.get_or_insert_text("text");
    assert_eq!(
        text.get_string(&third.transact()),
        source
            .get_or_insert_text("text")
            .get_string(&source.transact())
    );
}

#[test]
fn a_failing_writer_is_reported() {
    struct Broken;
    impl std::io::Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("disk full"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let doc = typed(40_000);
    let err = doc
        .transact()
        .write_state_as_update_v1(&StateVector::default(), &mut Broken)
        .unwrap_err();
    assert_eq!(err.to_string(), "disk full");
}

/// A document typed at its end, one character per transaction: each squash appends to the
/// item's string, which grows its capacity in steps.
fn appended(chars: u32) -> Doc {
    let doc = Doc::with_client_id(1);
    let text = doc.get_or_insert_text("text");
    for i in 0..chars {
        let mut txn = doc.transact_mut();
        let len = text.len(&txn);
        text.insert(&mut txn, len, if i % 50 == 0 { "\n" } else { "a" });
    }
    doc
}

#[test]
fn a_compacted_document_has_the_same_content_in_less_memory() {
    for doc in [typed(20_000), appended(20_000)] {
        compacts(doc);
    }
}

fn compacts(doc: Doc) {
    let compacted = doc.compact();
    assert_eq!(compacted.guid(), doc.guid());
    assert_eq!(compacted.client_id(), doc.client_id());
    let (old_text, new_text) = (
        doc.get_or_insert_text("text"),
        compacted.get_or_insert_text("text"),
    );
    let (before, after) = (doc.transact(), compacted.transact());
    assert_eq!(before.state_vector(), after.state_vector());
    assert_eq!(old_text.get_string(&before), new_text.get_string(&after));
    let (b, a) = (before.memory_usage(), after.memory_usage());
    assert!(b.total() > 0 && a.total() <= b.total(), "{b:?} -> {a:?}");
}

#[test]
fn memory_usage_grows_with_the_content() {
    let small = typed(100).transact().memory_usage();
    let large = typed(5_000).transact().memory_usage();
    assert!(large.blocks > small.blocks);
    assert!(large.total() > small.total(), "{small:?} vs {large:?}");
}
