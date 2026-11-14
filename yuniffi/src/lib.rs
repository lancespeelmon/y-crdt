//! Swift and Kotlin bindings for yrs, generated with [UniFFI](https://mozilla.github.io/uniffi-rs/)
//! (plan item 7.7).
//!
//! The API is small and complete enough to sync a document: a [YDoc] with text, map and array
//! roots, the lib0 v1 update exchange (state vectors, diffs, updates, an update observer), and the
//! update utilities shared with yffi and ywasm through `ybindings-core`. Each call runs in its own
//! transaction. Values are the JSON-like primitives of [YValue].
//!
//! Generate the bindings with the generator built from the same UniFFI version:
//!
//! ```sh
//! cargo build -p yuniffi --release
//! cargo run -p yuniffi --features cli --bin uniffi-bindgen -- generate \
//!     --library target/release/libyuniffi.dylib --language swift --out-dir out/swift
//! ```
//!
//! `yuniffi/build-apple.sh` and `yuniffi/build-android.sh` build the mobile artifacts.

use std::sync::Arc;
use ybindings_core::Encoding;
use yrs::updates::decoder::Decode;
use yrs::{
    Any, Array, ArrayRef, ClientID, Doc, GetString, Map, MapRef, Number, Out, ReadTxn, StateVector,
    Text, TextRef, Transact, Update,
};

uniffi::setup_scaffolding!();

/// Why a call failed.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum YError {
    /// The bytes aren't a valid lib0 v1 update or state vector.
    #[error("invalid encoding: {message}")]
    InvalidEncoding { message: String },
    /// The update couldn't be applied.
    #[error("update failed: {message}")]
    UpdateFailed { message: String },
    /// An index or range runs past the end of the collection.
    #[error("index out of range: {message}")]
    OutOfRange { message: String },
    /// A client ID must fit in 53 bits, as in Yjs.
    #[error("client ID {client_id} doesn't fit in 53 bits")]
    InvalidClientId { client_id: u64 },
    /// The value can't be stored: a number that isn't finite.
    #[error("unsupported value: {message}")]
    UnsupportedValue { message: String },
}

/// A value stored in a map or an array. Shared types nested in a document read as
/// [YValue::Shared], since these bindings only create primitives.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum YValue {
    Null,
    Bool {
        value: bool,
    },
    /// An integer. Yjs peers read it as a JS number (or a `BigInt` beyond ±2^53).
    Int {
        value: i64,
    },
    /// A float.
    Number {
        value: f64,
    },
    /// A JavaScript `BigInt`.
    BigInt {
        value: i64,
    },
    String {
        value: String,
    },
    Bytes {
        value: Vec<u8>,
    },
    /// A nested shared type (text, map, array, XML) created by another binding or peer.
    Shared,
}

impl TryFrom<YValue> for Any {
    type Error = YError;

    fn try_from(value: YValue) -> Result<Self, YError> {
        Ok(match value {
            YValue::Null => Any::Null,
            YValue::Bool { value } => Any::Bool(value),
            YValue::Int { value } => Any::Number(Number::Int(value)),
            YValue::Number { value } if value.is_finite() => Any::Number(Number::Float(value)),
            YValue::Number { value } => {
                return Err(YError::UnsupportedValue {
                    message: format!("{value} is not a finite number"),
                })
            }
            YValue::BigInt { value } => Any::Number(Number::BigInt(value)),
            YValue::String { value } => Any::from(value),
            YValue::Bytes { value } => Any::from(value),
            YValue::Shared => {
                return Err(YError::UnsupportedValue {
                    message: "these bindings don't create nested shared types".into(),
                })
            }
        })
    }
}

impl From<Out> for YValue {
    fn from(out: Out) -> Self {
        match out {
            Out::Any(any) => any.into(),
            _ => YValue::Shared,
        }
    }
}

impl From<Any> for YValue {
    fn from(any: Any) -> Self {
        match any {
            Any::Null | Any::Undefined => YValue::Null,
            Any::Bool(value) => YValue::Bool { value },
            Any::Number(Number::Int(value)) => YValue::Int { value },
            Any::Number(Number::Float(value)) => YValue::Number { value },
            Any::Number(Number::BigInt(value)) => YValue::BigInt { value },
            Any::String(value) => YValue::String {
                value: value.to_string(),
            },
            Any::Buffer(value) => YValue::Bytes {
                value: value.to_vec(),
            },
            // JSON arrays and objects come back as their JSON text.
            other => YValue::String {
                value: other.to_string(),
            },
        }
    }
}

fn invalid(e: impl std::fmt::Display) -> YError {
    YError::InvalidEncoding {
        message: e.to_string(),
    }
}

/// Receives the update each committed change produces, ready to send to other peers.
#[uniffi::export(with_foreign)]
pub trait UpdateObserver: Send + Sync {
    fn on_update(&self, update: Vec<u8>);
}

/// A Yjs document.
#[derive(uniffi::Object)]
pub struct YDoc {
    doc: Doc,
}

#[uniffi::export]
impl YDoc {
    /// A document with a random client ID.
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(YDoc { doc: Doc::new() })
    }

    /// A document with the given client ID, which must fit in 53 bits.
    #[uniffi::constructor]
    pub fn with_client_id(client_id: u64) -> Result<Arc<Self>, YError> {
        if client_id >> 53 != 0 {
            return Err(YError::InvalidClientId { client_id });
        }
        Ok(Arc::new(YDoc {
            doc: Doc::with_client_id(client_id),
        }))
    }

    pub fn client_id(&self) -> u64 {
        self.doc.client_id().get()
    }

    pub fn guid(&self) -> String {
        self.doc.guid().to_string()
    }

    /// The root text called `name`, created if missing.
    pub fn text(&self, name: String) -> Arc<YText> {
        Arc::new(YText {
            doc: self.doc.clone(),
            text: self.doc.get_or_insert_text(name),
        })
    }

    /// The root map called `name`, created if missing.
    pub fn map(&self, name: String) -> Arc<YMap> {
        Arc::new(YMap {
            doc: self.doc.clone(),
            map: self.doc.get_or_insert_map(name),
        })
    }

    /// The root array called `name`, created if missing.
    pub fn array(&self, name: String) -> Arc<YArray> {
        Arc::new(YArray {
            doc: self.doc.clone(),
            array: self.doc.get_or_insert_array(name),
        })
    }

    /// This document's state vector (lib0 v1).
    pub fn state_vector(&self) -> Vec<u8> {
        use yrs::updates::encoder::Encode;
        self.doc.transact().state_vector().encode_v1()
    }

    /// What a peer with `state_vector` lacks, as a lib0 v1 update; everything if it's `None`.
    pub fn encode_state_as_update(&self, state_vector: Option<Vec<u8>>) -> Result<Vec<u8>, YError> {
        let sv = match state_vector {
            Some(bytes) => StateVector::decode_v1(&bytes).map_err(invalid)?,
            None => StateVector::default(),
        };
        Ok(self.doc.transact().encode_state_as_update_v1(&sv))
    }

    /// Applies a lib0 v1 update from another peer.
    pub fn apply_update(&self, update: Vec<u8>) -> Result<(), YError> {
        let update = Encoding::V1.decode_update(&update).map_err(invalid)?;
        self.doc
            .transact_mut()
            .apply_update(update)
            .map_err(|e| YError::UpdateFailed {
                message: e.to_string(),
            })
    }

    /// Calls `observer` with the update of every committed change, under `key`. Observing again
    /// under the same key replaces the observer.
    pub fn observe_updates(&self, key: String, observer: Arc<dyn UpdateObserver>) {
        self.doc
            .observe_update_v1(key, move |_, e| observer.on_update(e.update.clone()))
            .expect("no transaction is open during a binding call");
    }

    /// Stops the observer registered under `key`. Returns whether there was one.
    pub fn unobserve_updates(&self, key: String) -> bool {
        self.doc
            .unobserve_update_v1(key)
            .expect("no transaction is open during a binding call")
    }
}

/// A shared text.
#[derive(uniffi::Object)]
pub struct YText {
    doc: Doc,
    text: TextRef,
}

#[uniffi::export]
impl YText {
    pub fn insert(&self, index: u32, chunk: String) -> Result<(), YError> {
        let mut txn = self.doc.transact_mut();
        check_index(self.text.len(&txn), index)?;
        self.text.insert(&mut txn, index, &chunk);
        Ok(())
    }

    pub fn push(&self, chunk: String) {
        self.text.push(&mut self.doc.transact_mut(), &chunk);
    }

    pub fn delete(&self, index: u32, len: u32) -> Result<(), YError> {
        let mut txn = self.doc.transact_mut();
        check_range(self.text.len(&txn), index, len)?;
        self.text.remove_range(&mut txn, index, len);
        Ok(())
    }

    /// The length in the document's offset units (UTF-16 code units by default).
    pub fn len(&self) -> u32 {
        self.text.len(&self.doc.transact())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get_string(&self) -> String {
        self.text.get_string(&self.doc.transact())
    }
}

/// A shared map.
#[derive(uniffi::Object)]
pub struct YMap {
    doc: Doc,
    map: MapRef,
}

#[uniffi::export]
impl YMap {
    pub fn set(&self, key: String, value: YValue) -> Result<(), YError> {
        let value = Any::try_from(value)?;
        self.map.insert(&mut self.doc.transact_mut(), key, value);
        Ok(())
    }

    pub fn get(&self, key: String) -> Option<YValue> {
        self.map.get(&self.doc.transact(), &key).map(YValue::from)
    }

    /// Removes `key`, returning its value if it had one.
    pub fn remove(&self, key: String) -> Option<YValue> {
        self.map
            .remove(&mut self.doc.transact_mut(), &key)
            .map(YValue::from)
    }

    pub fn contains_key(&self, key: String) -> bool {
        self.map.contains_key(&self.doc.transact(), &key)
    }

    pub fn len(&self) -> u32 {
        self.map.len(&self.doc.transact())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn keys(&self) -> Vec<String> {
        let txn = self.doc.transact();
        let mut keys: Vec<String> = self.map.keys(&txn).map(str::to_string).collect();
        keys.sort();
        keys
    }
}

/// A shared array.
#[derive(uniffi::Object)]
pub struct YArray {
    doc: Doc,
    array: ArrayRef,
}

#[uniffi::export]
impl YArray {
    pub fn insert(&self, index: u32, value: YValue) -> Result<(), YError> {
        let value = Any::try_from(value)?;
        let mut txn = self.doc.transact_mut();
        check_index(self.array.len(&txn), index)?;
        self.array.insert(&mut txn, index, value);
        Ok(())
    }

    pub fn push(&self, value: YValue) -> Result<(), YError> {
        let value = Any::try_from(value)?;
        self.array.push_back(&mut self.doc.transact_mut(), value);
        Ok(())
    }

    pub fn delete(&self, index: u32, len: u32) -> Result<(), YError> {
        let mut txn = self.doc.transact_mut();
        check_range(self.array.len(&txn), index, len)?;
        self.array.remove_range(&mut txn, index, len);
        Ok(())
    }

    pub fn get(&self, index: u32) -> Option<YValue> {
        self.array
            .get(&self.doc.transact(), index)
            .map(YValue::from)
    }

    pub fn len(&self) -> u32 {
        self.array.len(&self.doc.transact())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn to_list(&self) -> Vec<YValue> {
        let txn = self.doc.transact();
        self.array.iter(&txn).map(YValue::from).collect()
    }
}

fn check_index(len: u32, index: u32) -> Result<(), YError> {
    yrs::error::OutOfRange::check(len, index, 0).map_err(|e| YError::OutOfRange {
        message: e.to_string(),
    })
}

fn check_range(len: u32, index: u32, count: u32) -> Result<(), YError> {
    yrs::error::OutOfRange::check(len, index, count).map_err(|e| YError::OutOfRange {
        message: e.to_string(),
    })
}

/// Merges lib0 v1 updates into one, without a document.
#[uniffi::export]
pub fn merge_updates(updates: Vec<Vec<u8>>) -> Result<Vec<u8>, YError> {
    ybindings_core::merge_updates(Encoding::V1, updates).map_err(invalid)
}

/// The part of a lib0 v1 `update` that a peer with `state_vector` lacks.
#[uniffi::export]
pub fn diff_update(update: Vec<u8>, state_vector: Vec<u8>) -> Result<Vec<u8>, YError> {
    ybindings_core::diff_update(Encoding::V1, &update, &state_vector).map_err(invalid)
}

/// The state vector a lib0 v1 update would bring a document to.
#[uniffi::export]
pub fn state_vector_from_update(update: Vec<u8>) -> Result<Vec<u8>, YError> {
    ybindings_core::state_vector_from_update(Encoding::V1, &update).map_err(invalid)
}

/// Decodes `update` to check it, without applying it anywhere.
#[uniffi::export]
pub fn validate_update(update: Vec<u8>) -> Result<(), YError> {
    Update::decode_v1(&update).map(|_| ()).map_err(invalid)
}

/// A document's client ID as a [ClientID], for tests.
#[cfg(test)]
fn client(doc: &YDoc) -> ClientID {
    doc.doc.client_id()
}

#[cfg(test)]
mod test {
    use super::*;
    use std::sync::Mutex;

    fn sync(from: &YDoc, to: &YDoc) {
        let update = from
            .encode_state_as_update(Some(to.state_vector()))
            .unwrap();
        to.apply_update(update).unwrap();
    }

    #[test]
    fn two_documents_converge_through_updates() {
        let a = YDoc::with_client_id(1).unwrap();
        let b = YDoc::with_client_id(2).unwrap();
        a.text("t".into()).push("hello".into());
        b.text("t".into()).push("world".into());
        a.map("m".into())
            .set("k".into(), YValue::Number { value: 1.5 })
            .unwrap();
        b.array("a".into())
            .push(YValue::String { value: "x".into() })
            .unwrap();
        sync(&a, &b);
        sync(&b, &a);
        assert_eq!(
            a.text("t".into()).get_string(),
            b.text("t".into()).get_string()
        );
        assert_eq!(
            b.map("m".into()).get("k".into()),
            Some(YValue::Number { value: 1.5 })
        );
        assert_eq!(a.array("a".into()).to_list(), b.array("a".into()).to_list());
        assert_eq!(client(&a).get(), 1);
    }

    #[test]
    fn edits_past_the_end_are_errors_not_panics() {
        let doc = YDoc::new();
        let text = doc.text("t".into());
        assert!(matches!(
            text.insert(5, "x".into()),
            Err(YError::OutOfRange { .. })
        ));
        assert!(matches!(text.delete(0, 1), Err(YError::OutOfRange { .. })));
        let array = doc.array("a".into());
        assert!(matches!(
            array.insert(1, YValue::Null),
            Err(YError::OutOfRange { .. })
        ));
        assert!(matches!(
            YDoc::with_client_id(1 << 53),
            Err(YError::InvalidClientId { .. })
        ));
        assert!(matches!(
            array.push(YValue::Number { value: f64::NAN }),
            Err(YError::UnsupportedValue { .. })
        ));
        assert!(matches!(
            doc.apply_update(vec![0xff, 0xff]),
            Err(YError::InvalidEncoding { .. })
        ));
    }

    struct Collect(Mutex<Vec<Vec<u8>>>);

    impl UpdateObserver for Collect {
        fn on_update(&self, update: Vec<u8>) {
            self.0.lock().unwrap().push(update);
        }
    }

    #[test]
    fn the_update_observer_sees_each_commit_until_removed() {
        let doc = YDoc::new();
        let seen = Arc::new(Collect(Mutex::new(Vec::new())));
        doc.observe_updates("k".into(), seen.clone());
        doc.text("t".into()).push("a".into());
        doc.text("t".into()).push("b".into());
        assert!(doc.unobserve_updates("k".into()));
        doc.text("t".into()).push("c".into());
        let updates = seen.0.lock().unwrap().clone();
        assert_eq!(updates.len(), 2);
        let merged = merge_updates(updates).unwrap();
        let other = YDoc::new();
        other.apply_update(merged).unwrap();
        assert_eq!(other.text("t".into()).get_string(), "ab");
    }

    #[test]
    fn the_update_utilities_match_a_document() {
        let doc = YDoc::new();
        doc.text("t".into()).push("abc".into());
        let full = doc.encode_state_as_update(None).unwrap();
        validate_update(full.clone()).unwrap();
        assert_eq!(
            state_vector_from_update(full.clone()).unwrap(),
            doc.state_vector()
        );
        let empty = diff_update(full, doc.state_vector()).unwrap();
        let other = YDoc::new();
        other.apply_update(empty).unwrap();
        assert_eq!(other.text("t".into()).get_string(), "");
    }
}
