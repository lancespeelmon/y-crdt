//! A transaction that would wait forever on one the same thread already holds is refused instead
//! of deadlocking, and dropping an undo manager never panics.

use std::sync::{Arc, Mutex};
use yrs::{Doc, GetString, Observable, Text, Transact, TransactionAcqError, UndoManager};

#[test]
fn a_second_transaction_on_the_same_thread_is_refused() {
    let doc = Doc::new();
    let txn = doc.transact_mut();
    assert!(matches!(
        doc.try_transact(),
        Err(TransactionAcqError::Reentrant)
    ));
    assert!(matches!(
        doc.try_transact_mut(),
        Err(TransactionAcqError::Reentrant)
    ));
    drop(txn);

    let read = doc.transact();
    assert!(matches!(
        doc.try_transact_mut(),
        Err(TransactionAcqError::Reentrant)
    ));
    // More readers on the same thread are fine.
    let again = doc.transact();
    drop((read, again));

    // Once released, transactions open normally again.
    drop(doc.transact_mut());
    drop(doc.try_transact_mut().unwrap());
}

#[test]
fn opening_a_transaction_inside_an_observer_reports_reentrancy() {
    let doc = Doc::new();
    let text = doc.get_or_insert_text("t");
    let seen = Arc::new(Mutex::new(None));
    let doc2 = doc.clone();
    let seen2 = seen.clone();
    let text2 = text.clone();
    text.observe("test", move |txn, _| {
        // The callback's own transaction is the one to use.
        assert_eq!(text2.get_string(txn), "hi");
        *seen2.lock().unwrap() = Some(doc2.try_transact_mut().map(|_| ()));
    });
    text.insert(&mut doc.transact_mut(), 0, "hi");
    assert!(matches!(
        *seen.lock().unwrap(),
        Some(Err(TransactionAcqError::Reentrant))
    ));
}

#[test]
#[should_panic(expected = "would deadlock")]
fn a_blocking_transaction_that_would_deadlock_panics_instead() {
    let doc = Doc::new();
    let _txn = doc.transact_mut();
    let _again = doc.transact_mut();
}

#[test]
fn transactions_on_other_threads_still_wait_for_each_other() {
    let doc = Doc::new();
    let txn = doc.transact_mut();
    let other = doc.clone();
    let handle = std::thread::spawn(move || {
        // Another thread isn't re-entering: it waits, then succeeds.
        let text = other.get_or_insert_text("t");
        text.insert(&mut other.transact_mut(), 0, "x");
    });
    std::thread::sleep(std::time::Duration::from_millis(50));
    drop(txn);
    handle.join().unwrap();
}

#[test]
fn dropping_an_undo_manager_during_a_transaction_does_not_panic() {
    let doc = Doc::new();
    let text = doc.get_or_insert_text("t");
    let mut undo: UndoManager = UndoManager::new();
    undo.expand_scope(&doc, &text);
    {
        let mut txn = doc.transact_mut();
        text.insert(&mut txn, 0, "a");
        // Its callbacks can't be removed now; they turn inert instead.
        drop(undo);
        text.insert(&mut txn, 1, "b");
    }
    // Later transactions run the inert callbacks, which remove themselves.
    text.insert(&mut doc.transact_mut(), 2, "c");
    text.insert(&mut doc.transact_mut(), 3, "d");
    assert_eq!(text.get_string(&doc.transact()), "abcd");
}
