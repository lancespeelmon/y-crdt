//! A change to a type nobody observes builds no event, but deep observers above it still get one.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use yrs::{DeepObservable, Doc, Map, MapPrelim, Observable, Text, TextPrelim, Transact};

#[test]
fn a_deep_observer_above_an_unobserved_type_still_gets_its_event() {
    let doc = Doc::new();
    let root = doc.get_or_insert_map("root");
    let (inner, text) = {
        let mut txn = doc.transact_mut();
        let inner = root.insert(&mut txn, "inner", MapPrelim::default());
        let text = inner.insert(&mut txn, "text", TextPrelim::new(""));
        (inner, text)
    };

    let deep = Arc::new(AtomicUsize::new(0));
    let paths = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (d, p) = (deep.clone(), paths.clone());
    root.observe_deep("deep", move |_, events: &yrs::types::Events| {
        d.fetch_add(events.iter().count(), Ordering::SeqCst);
        for e in events.iter() {
            p.lock().unwrap().push(e.path());
        }
    });

    // Neither the text nor the inner map has observers of its own.
    text.push(&mut doc.transact_mut(), "abc");
    assert_eq!(deep.load(Ordering::SeqCst), 1);
    assert_eq!(paths.lock().unwrap().last().unwrap().len(), 2);

    // A type's own observer still fires when nothing above observes deeply.
    let own = Arc::new(AtomicUsize::new(0));
    let o = own.clone();
    inner.observe("own", move |_, _| {
        o.fetch_add(1, Ordering::SeqCst);
    });
    root.unobserve_deep("deep");
    inner.insert(&mut doc.transact_mut(), "k", 1);
    assert_eq!(own.load(Ordering::SeqCst), 1);
    assert_eq!(
        deep.load(Ordering::SeqCst),
        1,
        "the dropped deep observer stays silent"
    );

    // And with no observers left at all, edits still commit.
    inner.unobserve("own");
    text.push(&mut doc.transact_mut(), "d");
    assert_eq!(own.load(Ordering::SeqCst), 1);
}
