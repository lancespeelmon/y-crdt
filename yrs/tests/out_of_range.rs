//! The `try_*` methods refuse positions and ranges past the end of a sequence before changing
//! anything, and the panicking methods panic for the same inputs.

use yrs::error::OutOfRange;
use yrs::types::text::YChange;
use yrs::types::Attrs;
use yrs::{
    Any, Array, Doc, GetString, Text, Transact, WriteTxn, XmlElementPrelim, XmlFragment,
    XmlTextPrelim,
};

fn bold() -> Attrs {
    Attrs::from([("bold".into(), Any::Bool(true))])
}

#[test]
fn text_try_insert_past_the_end_changes_nothing() {
    let doc = Doc::new();
    let text = doc.get_or_insert_text("text");
    let mut txn = doc.transact_mut();
    text.insert(&mut txn, 0, "abc");

    assert_eq!(text.try_insert(&mut txn, 3, "d"), Ok(()));
    let err = text.try_insert(&mut txn, 5, "x").unwrap_err();
    assert_eq!((err.index, err.len, err.available), (5, 0, 4));
    // An empty chunk is checked too, so a bad index never passes silently.
    assert!(text.try_insert(&mut txn, 5, "").is_err());
    assert_eq!(text.get_string(&txn), "abcd");
}

#[test]
fn text_try_remove_range_past_the_end_removes_nothing() {
    let doc = Doc::new();
    let text = doc.get_or_insert_text("text");
    let mut txn = doc.transact_mut();
    text.insert(&mut txn, 0, "abcdef");

    let err = text.try_remove_range(&mut txn, 4, 3).unwrap_err();
    assert_eq!(
        err,
        OutOfRange::check(6, 4, 3).unwrap_err(),
        "the error describes the whole range"
    );
    assert_eq!(text.get_string(&txn), "abcdef");
    // A length that overflows u32 is refused, not wrapped.
    assert!(text.try_remove_range(&mut txn, 1, u32::MAX).is_err());

    assert_eq!(text.try_remove_range(&mut txn, 4, 2), Ok(()));
    assert_eq!(text.get_string(&txn), "abcd");
}

#[test]
fn text_try_format_past_the_end_formats_nothing() {
    let doc = Doc::new();
    let text = doc.get_or_insert_text("text");
    let mut txn = doc.transact_mut();
    text.insert(&mut txn, 0, "abc");

    assert!(text.try_format(&mut txn, 1, 5, bold()).is_err());
    let diff = text.diff(&txn, YChange::identity);
    assert_eq!(diff.len(), 1);
    assert!(diff[0].attributes.is_none());

    assert_eq!(text.try_format(&mut txn, 1, 2, bold()), Ok(()));
    assert_eq!(text.diff(&txn, YChange::identity).len(), 2);
}

#[test]
#[should_panic(expected = "out of bounds")]
fn text_insert_past_the_end_panics() {
    let doc = Doc::new();
    let text = doc.get_or_insert_text("text");
    let mut txn = doc.transact_mut();
    text.insert(&mut txn, 0, "abc");
    text.insert(&mut txn, 4, "x");
}

#[test]
#[should_panic(expected = "out of bounds")]
fn text_insert_with_attributes_past_the_end_panics() {
    let doc = Doc::new();
    let text = doc.get_or_insert_text("text");
    let mut txn = doc.transact_mut();
    text.insert_with_attributes(&mut txn, 1, "x", bold());
}

#[test]
fn array_try_insert_and_remove_range_check_bounds_first() {
    let doc = Doc::new();
    let array = doc.get_or_insert_array("array");
    let mut txn = doc.transact_mut();
    array.insert_range(&mut txn, 0, [1, 2, 3]);

    assert!(array.try_insert(&mut txn, 4, 9).is_err());
    assert!(array.try_remove_range(&mut txn, 2, 2).is_err());
    assert_eq!(array.len(&txn), 3);

    assert!(array.try_insert(&mut txn, 3, 4).is_ok());
    assert_eq!(array.try_remove_range(&mut txn, 0, 4), Ok(()));
    assert_eq!(array.len(&txn), 0);
}

#[test]
#[should_panic(expected = "out of bounds")]
fn array_insert_range_of_nothing_past_the_end_panics() {
    let doc = Doc::new();
    let array = doc.get_or_insert_array("array");
    let mut txn = doc.transact_mut();
    array.insert_range(&mut txn, 1, Vec::<i64>::new());
}

#[test]
fn xml_try_insert_and_remove_range_check_bounds_first() {
    let doc = Doc::new();
    let mut txn = doc.transact_mut();
    let xml = txn.get_or_insert_xml_fragment("xml");
    xml.insert(&mut txn, 0, XmlElementPrelim::empty("p"));

    assert!(xml
        .try_insert(&mut txn, 2, XmlTextPrelim::new("x"))
        .is_err());
    assert!(xml.try_remove_range(&mut txn, 0, 2).is_err());
    assert_eq!(xml.len(&txn), 1);

    assert!(xml.try_insert(&mut txn, 1, XmlTextPrelim::new("x")).is_ok());
    assert_eq!(xml.try_remove_range(&mut txn, 0, 2), Ok(()));
    assert_eq!(xml.len(&txn), 0);
}

#[test]
fn out_of_range_message_names_the_range() {
    let err = OutOfRange::check(3, 2, 4).unwrap_err();
    assert_eq!(err.to_string(), "range 2..6 is out of bounds for length 3");
}
