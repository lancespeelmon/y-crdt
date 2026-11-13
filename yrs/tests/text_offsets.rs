//! Typed text offsets mean the same position whatever unit the document counts in.

use yrs::types::Attrs;
use yrs::{
    Any, CharOffset, Doc, GetString, OffsetError, OffsetKind, Options, Text, Transact, Utf16Offset,
    Utf8Offset,
};

fn doc(kind: OffsetKind) -> Doc {
    Doc::with_options(Options {
        offset_kind: kind,
        ..Options::default()
    })
}

/// "a😀é" then " b" as a second item, so positions cross item boundaries.
/// chars:  a=1 😀=1 é=1 ' '=1 b=1
/// utf16:  a=1 😀=2 é=1 ' '=1 b=1
/// utf8:   a=1 😀=4 é=2 ' '=1 b=1
const FIRST: &str = "a😀é";
const SECOND: &str = " b";

#[test]
fn defaults_to_utf16_like_yjs() {
    assert_eq!(Options::default().offset_kind, OffsetKind::Utf16);
}

#[test]
fn typed_offsets_resolve_to_the_documents_units() {
    for kind in [OffsetKind::Utf16, OffsetKind::Bytes] {
        let doc = doc(kind);
        let text = doc.get_or_insert_text("t");
        let mut txn = doc.transact_mut();
        text.push(&mut txn, FIRST);
        text.push(&mut txn, SECOND);

        // Each case: the same position in chars, UTF-16 and UTF-8, and in the doc's units.
        let positions = [
            (0, 0, 0),
            (1, 1, 1),
            (2, 3, 5),
            (3, 4, 7),
            (4, 5, 8),
            (5, 6, 9),
        ];
        for (chars, utf16, utf8) in positions {
            let native = match kind {
                OffsetKind::Utf16 => utf16,
                OffsetKind::Bytes => utf8,
            };
            assert_eq!(
                text.offset(&txn, CharOffset(chars)),
                Ok(native),
                "{kind:?} char {chars}"
            );
            assert_eq!(
                text.offset(&txn, Utf16Offset(utf16)),
                Ok(native),
                "{kind:?} utf16 {utf16}"
            );
            assert_eq!(
                text.offset(&txn, Utf8Offset(utf8)),
                Ok(native),
                "{kind:?} utf8 {utf8}"
            );
        }
    }
}

#[test]
fn offsets_inside_a_character_or_past_the_end_are_refused() {
    let doc = doc(OffsetKind::Utf16);
    let text = doc.get_or_insert_text("t");
    let mut txn = doc.transact_mut();
    text.push(&mut txn, FIRST);

    // Between the surrogates of the emoji, and inside its UTF-8 sequence.
    assert_eq!(
        text.offset(&txn, Utf16Offset(2)),
        Err(OffsetError::InsideCharacter(2))
    );
    assert_eq!(
        text.offset(&txn, Utf8Offset(3)),
        Err(OffsetError::InsideCharacter(3))
    );
    assert!(matches!(
        text.offset(&txn, CharOffset(4)),
        Err(OffsetError::OutOfRange(e)) if e.available == 3
    ));
}

#[test]
fn deleted_text_and_formatting_do_not_count() {
    let doc = doc(OffsetKind::Bytes);
    let text = doc.get_or_insert_text("t");
    let mut txn = doc.transact_mut();
    text.push(&mut txn, "héllo wörld");
    text.remove_range(&mut txn, 0, 7); // "héllo " is 7 bytes
    let bold = Attrs::from([("bold".into(), Any::Bool(true))]);
    text.format(&mut txn, 0, 3, bold);
    assert_eq!(text.get_string(&txn), "wörld");
    assert_eq!(text.offset(&txn, CharOffset(2)), Ok(3));
    assert_eq!(text.offset(&txn, CharOffset(5)), Ok(6));
}

#[test]
fn edits_at_typed_offsets() {
    let doc = doc(OffsetKind::Bytes);
    let text = doc.get_or_insert_text("t");
    let mut txn = doc.transact_mut();
    text.push(&mut txn, FIRST);

    text.try_insert_at(&mut txn, CharOffset(2), "!").unwrap();
    assert_eq!(text.get_string(&txn), "a😀!é");
    text.try_remove_range_at(&mut txn, Utf16Offset(1), Utf16Offset(3))
        .unwrap();
    assert_eq!(text.get_string(&txn), "a!é");

    let bold = Attrs::from([("bold".into(), Any::Bool(true))]);
    text.try_format_at(&mut txn, CharOffset(1), CharOffset(3), bold)
        .unwrap();
    assert_eq!(text.get_string(&txn), "a!é");

    // Reversed and bad ranges change nothing.
    assert_eq!(
        text.try_remove_range_at(&mut txn, CharOffset(2), CharOffset(1)),
        Err(OffsetError::Reversed(2, 1))
    );
    assert!(text.try_insert_at(&mut txn, CharOffset(9), "x").is_err());
    assert_eq!(text.get_string(&txn), "a!é");
}
