//! With the `tracing` feature, document operations emit spans. In its own test binary: tracing
//! caches per callsite whether anything listens, so a test in the same process that hits a
//! callsite first, without a subscriber, would hide it.
#![cfg(feature = "tracing")]

use std::sync::Mutex;
use yrs::updates::decoder::Decode;
use yrs::{Doc, ReadTxn, StateVector, Text, Transact, Update};

#[test]
fn operations_emit_tracing_spans() {
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    #[derive(Default)]
    struct Names(Mutex<Vec<&'static str>>);
    impl Subscriber for Names {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, span: &Attributes<'_>) -> Id {
            let mut names = self.0.lock().unwrap();
            names.push(span.metadata().name());
            Id::from_u64(names.len() as u64)
        }
        fn record(&self, _: &Id, _: &Record<'_>) {}
        fn record_follows_from(&self, _: &Id, _: &Id) {}
        fn event(&self, _: &Event<'_>) {}
        fn enter(&self, _: &Id) {}
        fn exit(&self, _: &Id) {}
    }

    let names = std::sync::Arc::new(Names::default());
    tracing::subscriber::with_default(names.clone(), || {
        let source = Doc::with_client_id(1);
        let text = source.get_or_insert_text("t");
        text.insert(&mut source.transact_mut(), 0, "a");
        let bytes = source
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        let doc = Doc::new();
        doc.transact_mut()
            .apply_update(Update::decode_v1(&bytes).unwrap())
            .unwrap();
    });
    let names = names.0.lock().unwrap();
    for name in [
        "yrs::commit",
        "yrs::encode",
        "yrs::decode",
        "yrs::apply_update",
    ] {
        assert!(names.contains(&name), "{} missing from {:?}", name, names);
    }
}
