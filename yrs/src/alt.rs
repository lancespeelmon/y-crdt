//! `alt` module contains a set of auxiliary functions that can be used for common operations
//! over document [Update]s directly on their binary representation.

use crate::block::{Block, ItemContent};
use crate::doc::Options;
use crate::encoding::read::{Cursor, Error};
use crate::types::TypeRef;
use crate::update::Update;
use crate::updates::decoder::{Decode, DecoderV2};
use crate::updates::encoder::{Encode, Encoder, EncoderV1, EncoderV2};
use crate::{Any, ClientID, Doc, Number, Snapshot, StateVector};
use std::collections::HashMap;
use std::sync::Arc;

/// Merges a sequence of updates (encoded using lib0 v1 encoding) together, producing another
/// update (also lib0 v1 encoded) in the result. Returned binary is a combination of all input
/// `updates`, compressed.
///
/// Returns an error whenever any of the input updates couldn't be decoded.
pub fn merge_updates_v1<I, B>(updates: I) -> Result<Vec<u8>, Error>
where
    I: IntoIterator<Item = B>,
    B: AsRef<[u8]>,
{
    let mut i = updates.into_iter();
    let (lo, hi) = i.size_hint();
    let mut merge = Vec::with_capacity(hi.unwrap_or(lo));
    while let Some(buf) = i.next() {
        let parsed = Update::decode_v1(buf.as_ref())?;
        merge.push(parsed);
    }
    Ok(Update::merge_updates(merge).encode_v1())
}

/// Merges updates as they arrive, keeping the result decoded, for a server that stores a
/// document as its update history instead of loading it into a [Doc].
///
/// Calling [merge_updates_v1] on the stored history plus each new update decodes the whole
/// history every time. An accumulator decodes each update once: it collects them and folds them
/// into its merged update every `batch` updates, and encodes only when asked.
///
/// ```
/// use yrs::{merge_updates_v1, Doc, ReadTxn, StateVector, Text, Transact, UpdateAccumulator};
///
/// let doc = Doc::new();
/// let text = doc.get_or_insert_text("text");
/// let mut acc = UpdateAccumulator::new(64);
/// let mut all = Vec::new();
/// for chunk in ["a", "b", "c"] {
///     let sv = doc.transact().state_vector();
///     text.push(&mut doc.transact_mut(), chunk);
///     let update = doc.transact().encode_state_as_update_v1(&sv);
///     acc.push_v1(&update).unwrap();
///     all.push(update);
/// }
/// assert_eq!(acc.encode_v1(), merge_updates_v1(&all).unwrap());
/// ```
#[derive(Debug, Default)]
pub struct UpdateAccumulator {
    merged: Option<Update>,
    pending: Vec<Update>,
    batch: usize,
}

impl UpdateAccumulator {
    /// An empty accumulator that folds updates in every `batch` updates (at least 1). Larger
    /// batches merge less often but hold more updates unmerged.
    pub fn new(batch: usize) -> Self {
        UpdateAccumulator {
            merged: None,
            pending: Vec::new(),
            batch: batch.max(1),
        }
    }

    /// Adds an update encoded with lib0 v1.
    ///
    /// # Errors
    ///
    /// If `update` doesn't decode. The accumulator is unchanged then.
    pub fn push_v1(&mut self, update: &[u8]) -> Result<(), Error> {
        self.push(Update::decode_v1(update)?);
        Ok(())
    }

    /// Adds an update encoded with lib0 v2.
    ///
    /// # Errors
    ///
    /// If `update` doesn't decode. The accumulator is unchanged then.
    pub fn push_v2(&mut self, update: &[u8]) -> Result<(), Error> {
        self.push(Update::decode_v2(update)?);
        Ok(())
    }

    /// Adds a decoded update.
    pub fn push(&mut self, update: Update) {
        self.pending.push(update);
        if self.pending.len() >= self.batch {
            self.fold();
        }
    }

    /// Number of updates added since the last fold.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    fn fold(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let merged = self.merged.take();
        let pending = std::mem::take(&mut self.pending);
        self.merged = Some(Update::merge_updates(merged.into_iter().chain(pending)));
    }

    /// Everything added so far as one update, encoded with lib0 v1.
    pub fn encode_v1(&mut self) -> Vec<u8> {
        self.fold();
        match &self.merged {
            Some(update) => update.encode_v1(),
            None => Update::new().encode_v1(),
        }
    }

    /// Everything added so far as one update, encoded with lib0 v2.
    pub fn encode_v2(&mut self) -> Vec<u8> {
        self.fold();
        match &self.merged {
            Some(update) => update.encode_v2(),
            None => Update::new().encode_v2(),
        }
    }

    /// Everything added so far as one decoded update, leaving the accumulator empty.
    pub fn into_update(mut self) -> Update {
        self.fold();
        self.merged.unwrap_or_default()
    }
}

/// Merges a sequence of updates (encoded using lib0 v2 encoding) together, producing another
/// update (also lib0 v2 encoded) in the result. Returned binary is a combination of all input
/// `updates`, compressed.
///
/// Returns an error whenever any of the input updates couldn't be decoded.
pub fn merge_updates_v2<I, B>(updates: I) -> Result<Vec<u8>, Error>
where
    I: IntoIterator<Item = B>,
    B: AsRef<[u8]>,
{
    let mut i = updates.into_iter();
    let (lo, hi) = i.size_hint();
    let mut merge = Vec::with_capacity(hi.unwrap_or(lo));
    while let Some(buf) = i.next() {
        let update = Update::decode_v2(buf.as_ref())?;
        merge.push(update);
    }
    Ok(Update::merge_updates(merge).encode_v2())
}

/// Decodes a input `update` (encoded using lib0 v1 encoding) and returns an encoded [StateVector]
/// of that update.
///
/// Returns an error whenever any of the input update couldn't be decoded.
pub fn encode_state_vector_from_update_v1(update: &[u8]) -> Result<Vec<u8>, Error> {
    let update = Update::decode_v1(update)?;
    Ok(update.state_vector().encode_v1())
}

/// Decodes a input `update` (encoded using lib0 v2 encoding) and returns an encoded [StateVector]
/// of that update.
///
/// Returns an error whenever any of the input update couldn't be decoded.
pub fn encode_state_vector_from_update_v2(update: &[u8]) -> Result<Vec<u8>, Error> {
    let update = Update::decode_v2(update)?;
    Ok(update.state_vector().encode_v2())
}

/// Givens an input `update` (encoded using lib0 v1 encoding) of document **A** and an encoded
/// `state_vector` of document **B**, returns a lib0 v1 encoded update, that contains all changes
/// from **A** which have not been observed by **B** (based on its state vector).
///
/// Returns an error whenever any of the input arguments couldn't be decoded.
pub fn diff_updates_v1(update: &[u8], state_vector: &[u8]) -> Result<Vec<u8>, Error> {
    let sv = StateVector::decode_v1(state_vector)?;
    let update = Update::decode_v1(update)?;
    let mut encoder = EncoderV1::new();
    update.encode_diff(&sv, &mut encoder);
    // for delete set, don't decode/encode it - just copy the remaining part from the decoder
    let result = encoder.to_vec();
    Ok(result)
}

/// Givens an input `update` (encoded using lib0 v2 encoding) of document **A** and an encoded
/// `state_vector` of document **B**, returns a lib0 v2 encoded update, that contains all changes
/// from **A** which have not been observed by **B** (based on its state vector).
///
/// Returns an error whenever any of the input arguments couldn't be decoded.
pub fn diff_updates_v2(update: &[u8], state_vector: &[u8]) -> Result<Vec<u8>, Error> {
    let sv = StateVector::decode_v2(state_vector)?;
    let cursor = Cursor::new(update);
    let mut decoder = DecoderV2::new(cursor)?;
    let update = Update::decode(&mut decoder)?;
    let mut encoder = EncoderV2::new();
    update.encode_diff(&sv, &mut encoder);
    // for delete set, don't decode/encode it - just copy the remaining part from the decoder
    Ok(encoder.to_vec())
}

/// The clock ranges an update covers, as Yjs' `parseUpdateMeta` returns them: for each client,
/// `from` is the clock of its first block and `to` the clock right after its last one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateMeta {
    /// The lowest clock of each client in the update.
    pub from: StateVector,
    /// One past the highest clock of each client in the update.
    pub to: StateVector,
}

impl UpdateMeta {
    fn of(update: &Update) -> Self {
        let mut meta = UpdateMeta::default();
        for (&client, blocks) in update.blocks.clients.iter() {
            if let (Some(first), Some(last)) = (blocks.front(), blocks.back()) {
                // `set_max` on an empty vector stores the value as is, including clock 0.
                meta.from.set_max(client, first.clock_start());
                meta.to.set_max(client, last.clock_start() + last.len());
            }
        }
        meta
    }
}

/// [UpdateMeta] of a lib0 v1 encoded update, without applying it to a document. Like Yjs, it
/// counts skipped ranges (gaps in a merged update) as covered.
pub fn parse_update_meta_v1(update: &[u8]) -> Result<UpdateMeta, Error> {
    Ok(UpdateMeta::of(&Update::decode_v1(update)?))
}

/// [UpdateMeta] of a lib0 v2 encoded update. See [parse_update_meta_v1].
pub fn parse_update_meta_v2(update: &[u8]) -> Result<UpdateMeta, Error> {
    Ok(UpdateMeta::of(&Update::decode_v2(update)?))
}

/// Re-encodes a lib0 v1 update in the v2 format, like Yjs' `convertUpdateFormatV1ToV2`.
pub fn convert_update_v1_to_v2(update: &[u8]) -> Result<Vec<u8>, Error> {
    Ok(Update::decode_v1(update)?.encode_v2())
}

/// Re-encodes a lib0 v2 update in the v1 format, like Yjs' `convertUpdateFormatV2ToV1`.
pub fn convert_update_v2_to_v1(update: &[u8]) -> Result<Vec<u8>, Error> {
    Ok(Update::decode_v2(update)?.encode_v1())
}

/// Whether `snapshot` already contains everything `update` adds, like Yjs'
/// `snapshotContainsUpdate`: every block of the update is within the snapshot's state vector,
/// and every deletion is in its delete set. Skipped ranges count, as in Yjs.
pub fn snapshot_contains_update_v1(snapshot: &Snapshot, update: &[u8]) -> Result<bool, Error> {
    Ok(snapshot_contains(snapshot, &Update::decode_v1(update)?))
}

/// [snapshot_contains_update_v1] for a lib0 v2 encoded update.
pub fn snapshot_contains_update_v2(snapshot: &Snapshot, update: &[u8]) -> Result<bool, Error> {
    Ok(snapshot_contains(snapshot, &Update::decode_v2(update)?))
}

fn snapshot_contains(snapshot: &Snapshot, update: &Update) -> bool {
    let blocks_covered = update.blocks.clients.iter().all(|(client, blocks)| {
        let known = snapshot.state_map.get(client);
        blocks
            .back()
            .is_none_or(|last| last.clock_start() + last.len() <= known)
    });
    blocks_covered && update.delete_set.diff(&snapshot.delete_set).is_empty()
}

/// What [obfuscate_update_v1] replaces besides text and values, which it always replaces.
/// Everything is on by default, as in Yjs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObfuscateOptions {
    /// Replace formatting attribute names and values.
    pub formatting: bool,
    /// Replace subdocument GUIDs and options.
    pub subdocs: bool,
    /// Replace XML element tags and hook names.
    pub yxml: bool,
}

impl Default for ObfuscateOptions {
    fn default() -> Self {
        ObfuscateOptions {
            formatting: true,
            subdocs: true,
            yxml: true,
        }
    }
}

/// Replaces the user content of a lib0 v1 update (text, values, map keys and, per `options`,
/// formatting, subdocument GUIDs and XML names) with placeholders of the same size, keeping its
/// structure. Like Yjs' `obfuscateUpdate`, it is for sharing an update that triggers a bug
/// without sharing the document. Root type names and IDs are kept.
pub fn obfuscate_update_v1(update: &[u8], options: ObfuscateOptions) -> Result<Vec<u8>, Error> {
    let mut update = Update::decode_v1(update)?;
    obfuscate(&mut update, options);
    Ok(update.encode_v1())
}

/// [obfuscate_update_v1] for a lib0 v2 encoded update.
pub fn obfuscate_update_v2(update: &[u8], options: ObfuscateOptions) -> Result<Vec<u8>, Error> {
    let mut update = Update::decode_v2(update)?;
    obfuscate(&mut update, options);
    Ok(update.encode_v2())
}

/// Yjs' `createObfuscator`: each item gets a counter `i`, which becomes its placeholder content.
/// Map keys, formatting and XML names are cached, so equal names stay equal.
fn obfuscate(update: &mut Update, options: ObfuscateOptions) {
    let mut i: u32 = 0;
    let mut map_keys: HashMap<Arc<str>, Arc<str>> = HashMap::new();
    let mut node_names: HashMap<Arc<str>, Arc<str>> = HashMap::new();
    let mut format_keys: HashMap<Arc<str>, Arc<str>> = HashMap::new();
    let mut format_values: HashMap<String, Any> = HashMap::new();
    // Visit clients in the order they are encoded (highest first), so the output is stable.
    let mut clients: Vec<ClientID> = update.blocks.clients.keys().copied().collect();
    clients.sort_unstable_by(|a, b| b.cmp(a));
    for client in clients {
        let blocks = update
            .blocks
            .clients
            .get_mut(&client)
            .expect("listed client");
        for block in blocks.iter_mut() {
            let item = match block {
                Block::Item(item) => item,
                Block::GC(_) | Block::Skip(_) => continue,
            };
            match &mut item.content {
                ItemContent::Deleted(_) => {}
                ItemContent::Type(branch) => {
                    if options.yxml {
                        match &branch.type_ref {
                            TypeRef::XmlElement(name) => {
                                let name = cached(&mut node_names, name, || format!("node-{i}"));
                                branch.type_ref = TypeRef::XmlElement(name);
                            }
                            TypeRef::XmlHook(name) => {
                                let name = cached(&mut node_names, name, || format!("hook-{i}"));
                                branch.type_ref = TypeRef::XmlHook(name);
                            }
                            _ => {}
                        }
                    }
                }
                ItemContent::Any(values) => {
                    for v in values.iter_mut() {
                        *v = Any::Number(Number::Int(i64::from(i)));
                    }
                }
                ItemContent::Binary(bytes) => *bytes = vec![i as u8],
                ItemContent::Doc(_, doc) => {
                    if options.subdocs {
                        *doc = Doc::with_options(Options {
                            guid: i.to_string().into(),
                            ..Options::default()
                        });
                    }
                }
                ItemContent::Embed(value) => *value = Any::Map(Arc::default()),
                ItemContent::Format(key, value) => {
                    if options.formatting {
                        *key = cached(&mut format_keys, key, || i.to_string());
                        // The end of a formatting range (null) stays one.
                        if **value != Any::Null {
                            let replacement = format_values
                                // Debug tells \"1\" from 1, unlike Display.
                                .entry(format!("{:?}", value))
                                .or_insert_with(|| {
                                    let i = Any::Number(Number::Int(i64::from(i)));
                                    std::iter::once(("i", i)).collect()
                                })
                                .clone();
                            **value = replacement;
                        }
                    }
                }
                ItemContent::JSON(values) => {
                    for v in values.iter_mut() {
                        *v = i.to_string();
                    }
                }
                ItemContent::String(text) => {
                    // The same length in UTF-16 units, which item clocks count.
                    let digit = char::from(b'0' + (i % 10) as u8);
                    let placeholder: String =
                        std::iter::repeat(digit).take(item.len as usize).collect();
                    *text = placeholder.as_str().into();
                }
            }
            if let Some(key) = &item.parent_sub {
                let key = cached(&mut map_keys, key, || i.to_string());
                item.parent_sub = Some(key);
            }
            i += 1;
        }
    }
}

fn cached(
    cache: &mut HashMap<Arc<str>, Arc<str>>,
    key: &Arc<str>,
    make: impl FnOnce() -> String,
) -> Arc<str> {
    cache
        .entry(key.clone())
        .or_insert_with(|| make().into())
        .clone()
}

#[cfg(test)]
mod test {
    use crate::{diff_updates_v1, encode_state_vector_from_update_v1, merge_updates_v1};

    mod utilities {
        use crate::types::text::YChange;
        use crate::types::Attrs;
        use crate::updates::decoder::Decode;
        use crate::updates::encoder::Encode;
        use crate::{
            convert_update_v1_to_v2, convert_update_v2_to_v1, obfuscate_update_v1,
            obfuscate_update_v2, parse_update_meta_v1, parse_update_meta_v2,
            snapshot_contains_update_v1, snapshot_contains_update_v2, Any, ClientID, Doc,
            GetString, Map, ObfuscateOptions, ReadTxn, StateVector, Text, Transact, Update,
            XmlFragment, XmlTextPrelim,
        };
        use std::collections::HashMap;
        use std::sync::Arc;

        fn sv(entries: &[(u64, u32)]) -> StateVector {
            let mut sv = StateVector::default();
            for &(client, clock) in entries {
                sv.set_max(ClientID::new(client), clock);
            }
            sv
        }

        #[test]
        fn update_meta_gives_each_clients_first_and_next_clock() {
            let a = Doc::with_client_id(1);
            let b = Doc::with_client_id(2);
            let text = a.get_or_insert_text("t");
            text.insert(&mut a.transact_mut(), 0, "hello");
            let early = a.transact().state_vector();
            text.insert(&mut a.transact_mut(), 5, " world");
            b.transact_mut()
                .apply_update(
                    Update::decode_v1(
                        &a.transact()
                            .encode_state_as_update_v1(&StateVector::default()),
                    )
                    .unwrap(),
                )
                .unwrap();
            b.get_or_insert_text("t")
                .insert(&mut b.transact_mut(), 0, "»");

            let txn = b.transact();
            let full = txn.encode_state_as_update_v1(&StateVector::default());
            let meta = parse_update_meta_v1(&full).unwrap();
            assert_eq!(meta.from, sv(&[(1, 0), (2, 0)]));
            assert_eq!(meta.to, sv(&[(1, 11), (2, 1)]));

            let diff = txn.encode_state_as_update_v2(&early);
            let meta = parse_update_meta_v2(&diff).unwrap();
            assert_eq!(meta.from, sv(&[(1, 5), (2, 0)]));
            assert_eq!(meta.to, sv(&[(1, 11), (2, 1)]));

            let empty = parse_update_meta_v1(Update::EMPTY_V1).unwrap();
            assert_eq!(empty, Default::default());
        }

        #[test]
        fn converted_updates_build_the_same_document() {
            let doc = Doc::with_client_id(1);
            let map = doc.get_or_insert_map("m");
            let text = doc.get_or_insert_text("t");
            {
                let mut txn = doc.transact_mut();
                map.insert(&mut txn, "k", "v");
                text.insert(&mut txn, 0, "abc");
                text.remove_range(&mut txn, 1, 1);
            }
            let v1 = doc
                .transact()
                .encode_state_as_update_v1(&StateVector::default());
            let v2 = convert_update_v1_to_v2(&v1).unwrap();
            assert_eq!(convert_update_v2_to_v1(&v2).unwrap(), v1);

            let copy = Doc::new();
            copy.transact_mut()
                .apply_update(Update::decode_v2(&v2).unwrap())
                .unwrap();
            assert_eq!(
                copy.get_or_insert_text("t").get_string(&copy.transact()),
                "ac"
            );
            assert!(convert_update_v1_to_v2(&[0xff]).is_err());
        }

        #[test]
        fn a_snapshot_contains_the_updates_made_before_it_only() {
            let doc = Doc::with_client_id(1);
            let text = doc.get_or_insert_text("t");
            text.insert(&mut doc.transact_mut(), 0, "hello");
            let before = doc
                .transact()
                .encode_state_as_update_v1(&StateVector::default());
            let snapshot = doc.transact().snapshot();
            assert!(snapshot_contains_update_v1(&snapshot, &before).unwrap());

            let sv = doc.transact().state_vector();
            text.insert(&mut doc.transact_mut(), 5, "!");
            let insert = doc.transact().encode_state_as_update_v2(&sv);
            assert!(!snapshot_contains_update_v2(&snapshot, &insert).unwrap());

            // A deletion of text the snapshot has is still news to it.
            let sv = doc.transact().state_vector();
            text.remove_range(&mut doc.transact_mut(), 0, 1);
            let delete = doc.transact().encode_state_as_update_v1(&sv);
            let later = doc.transact().snapshot();
            assert!(!snapshot_contains_update_v1(&snapshot, &delete).unwrap());
            assert!(snapshot_contains_update_v1(&later, &delete).unwrap());
            assert!(snapshot_contains_update_v1(&later, &before).unwrap());
        }

        #[test]
        fn obfuscation_keeps_the_structure_and_drops_the_content() {
            let doc = Doc::with_client_id(1);
            let text = doc.get_or_insert_text("text");
            let map = doc.get_or_insert_map("map");
            let xml = doc.get_or_insert_xml_fragment("xml");
            {
                let mut txn = doc.transact_mut();
                text.insert(&mut txn, 0, "secret 🙂 text");
                let bold = Attrs::from([(Arc::from("bold"), Any::Bool(true))]);
                text.format(&mut txn, 0, 6, bold);
                map.insert(&mut txn, "password", "hunter2");
                map.insert(&mut txn, "password", "hunter3");
                map.insert(&mut txn, "token", vec![1u8, 2, 3]);
                let el = xml.insert(&mut txn, 0, crate::XmlElementPrelim::empty("confidential"));
                el.insert(&mut txn, 0, XmlTextPrelim::new("inner secret"));
            }
            let original = doc
                .transact()
                .encode_state_as_update_v1(&StateVector::default());

            for (bytes, v2) in [
                (
                    obfuscate_update_v1(&original, ObfuscateOptions::default()).unwrap(),
                    false,
                ),
                (
                    obfuscate_update_v2(
                        &convert_update_v1_to_v2(&original).unwrap(),
                        ObfuscateOptions::default(),
                    )
                    .unwrap(),
                    true,
                ),
            ] {
                let v1 = if v2 {
                    convert_update_v2_to_v1(&bytes).unwrap()
                } else {
                    bytes
                };
                let haystack = String::from_utf8_lossy(&v1);
                for secret in [
                    "secret",
                    "hunter",
                    "password",
                    "token",
                    "confidential",
                    "bold",
                ] {
                    assert!(
                        !haystack.contains(secret),
                        "{} survived obfuscation",
                        secret
                    );
                }
                assert_eq!(
                    parse_update_meta_v1(&v1).unwrap(),
                    parse_update_meta_v1(&original).unwrap(),
                    "the same clock ranges"
                );

                let copy = Doc::new();
                copy.transact_mut()
                    .apply_update(Update::decode_v1(&v1).unwrap())
                    .unwrap();
                let txn = copy.transact();
                let t = txn.get_text("text").unwrap();
                // Placeholders are ASCII, so their byte length is the original's UTF-16 length.
                let utf16 = "secret 🙂 text".encode_utf16().count() as u32;
                assert_eq!(t.len(&txn), utf16, "same UTF-16 length");
                let diff = t.diff(&txn, YChange::identity);
                assert_eq!(diff.len(), 2, "formatting ranges are kept: {:?}", diff);
                let m = txn.get_map("map").unwrap();
                assert_eq!(m.len(&txn), 2);
                let keys: HashMap<_, _> = m.iter(&txn).map(|(k, v)| (k.to_string(), v)).collect();
                assert!(keys.keys().all(|k| k.parse::<u32>().is_ok()), "{:?}", keys);
                let frag = txn.get_xml_fragment("xml").unwrap();
                assert!(frag.get_string(&txn).starts_with("<node-"));
            }
        }

        #[test]
        fn obfuscation_options_can_keep_formatting_and_xml_names() {
            let doc = Doc::with_client_id(1);
            let xml = doc.get_or_insert_xml_fragment("xml");
            xml.insert(
                &mut doc.transact_mut(),
                0,
                crate::XmlElementPrelim::empty("visible"),
            );
            let update = doc
                .transact()
                .encode_state_as_update_v1(&StateVector::default());
            let keep = ObfuscateOptions {
                yxml: false,
                ..Default::default()
            };
            let out = obfuscate_update_v1(&update, keep).unwrap();
            assert!(String::from_utf8_lossy(&out).contains("visible"));
        }
    }

    #[test]
    fn an_accumulator_merges_like_merge_updates_whatever_its_batch() {
        use crate::updates::decoder::Decode;
        use crate::{
            convert_update_v1_to_v2, Array, Doc, Map, ReadTxn, Text, Transact, Update,
            UpdateAccumulator,
        };
        let peers: Vec<Doc> = (1..=3).map(Doc::with_client_id).collect();
        let mut updates = Vec::new();
        let mut rng = fastrand::Rng::with_seed(7);
        for i in 0..200u32 {
            let doc = &peers[rng.usize(..peers.len())];
            let sv = doc.transact().state_vector();
            {
                let text = doc.get_or_insert_text("t");
                let array = doc.get_or_insert_array("a");
                let map = doc.get_or_insert_map("m");
                let mut txn = doc.transact_mut();
                match i % 4 {
                    0 | 1 => {
                        let len = text.len(&txn);
                        text.insert(&mut txn, rng.u32(0..=len), "xy");
                    }
                    2 => {
                        array.push_back(&mut txn, i64::from(i));
                    }
                    _ => {
                        map.insert(&mut txn, format!("k{}", i % 5), i64::from(i));
                    }
                }
            }
            updates.push(doc.transact().encode_state_as_update_v1(&sv));
            if rng.bool() {
                // sync two peers, so later updates depend on others' items
                let (a, b) = (&peers[rng.usize(..3)], &peers[rng.usize(..3)]);
                let u = a
                    .transact()
                    .encode_state_as_update_v1(&b.transact().state_vector());
                b.transact_mut()
                    .apply_update(Update::decode_v1(&u).unwrap())
                    .unwrap();
            }
        }
        let expected = merge_updates_v1(&updates).unwrap();
        for batch in [1, 7, 64, 1000] {
            let mut acc = UpdateAccumulator::new(batch);
            for u in &updates {
                acc.push_v1(u).unwrap();
            }
            assert_eq!(acc.encode_v1(), expected, "batch {batch}");
        }
        let mut acc = UpdateAccumulator::new(5);
        for u in &updates {
            acc.push_v2(&convert_update_v1_to_v2(u).unwrap()).unwrap();
        }
        assert_eq!(acc.encode_v1(), expected);
        assert!(UpdateAccumulator::new(3).push_v1(&[0xff]).is_err());
    }

    #[test]
    fn merging_duplicated_and_overlapping_updates_is_the_same_as_merging_each_once() {
        // Ties in the merge order (two inputs at the same block) only happen when inputs
        // overlap; whichever input wins a tie, the output must not change.
        use crate::updates::decoder::Decode;
        use crate::{GetString, ReadTxn, Text, Transact};
        let doc = crate::Doc::with_client_id(1);
        let peer = crate::Doc::with_client_id(2);
        let mut steps = Vec::new();
        for (d, s) in [(&doc, "ab"), (&peer, "cd"), (&doc, "ef"), (&peer, "gh")] {
            let sv = d.transact().state_vector();
            let t = d.get_or_insert_text("t");
            let mut txn = d.transact_mut();
            let len = t.len(&txn);
            t.insert(&mut txn, len, s);
            drop(txn);
            steps.push(d.transact().encode_state_as_update_v1(&sv));
        }
        let full = doc
            .transact()
            .encode_state_as_update_v1(&Default::default());
        let once = merge_updates_v1(&steps).unwrap();
        let mut doubled = steps.clone();
        doubled.extend(steps.iter().rev().cloned());
        doubled.push(full.clone());
        assert_eq!(merge_updates_v1(&doubled).unwrap(), once);
        assert_eq!(
            merge_updates_v1([&full, &full]).unwrap(),
            merge_updates_v1([&full]).unwrap()
        );

        // And it holds every edit: the two peers never synced, so all 8 characters are there.
        let check = crate::Doc::new();
        check
            .transact_mut()
            .apply_update(crate::Update::decode_v1(&once).unwrap())
            .unwrap();
        let merged = check.get_or_insert_text("t").get_string(&check.transact());
        assert_eq!(merged.len(), 8);
    }

    #[test]
    fn merge_updates_compatibility_v1() {
        let a = vec![
            1, 1, 220, 240, 237, 172, 15, 0, 4, 1, 4, 116, 101, 115, 116, 3, 97, 98, 99, 0,
        ];
        let b = vec![
            1, 1, 201, 139, 250, 201, 1, 0, 4, 1, 4, 116, 101, 115, 116, 2, 100, 101, 0,
        ];
        let expected = vec![
            2, 1, 220, 240, 237, 172, 15, 0, 4, 1, 4, 116, 101, 115, 116, 3, 97, 98, 99, 1, 201,
            139, 250, 201, 1, 0, 4, 1, 4, 116, 101, 115, 116, 2, 100, 101, 0,
        ];

        let actual = merge_updates_v1(&[a, b]).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn merge_updates_compatibility_v1_2() {
        let a = &[
            1, 1, 129, 231, 135, 164, 7, 0, 4, 1, 4, 49, 50, 51, 52, 1, 97, 0,
        ];
        let b = &[
            1, 1, 129, 231, 135, 164, 7, 1, 68, 129, 231, 135, 164, 7, 0, 1, 98, 0,
        ];
        let expected = &[
            1, 2, 129, 231, 135, 164, 7, 0, 4, 1, 4, 49, 50, 51, 52, 1, 97, 68, 129, 231, 135, 164,
            7, 0, 1, 98, 0,
        ];

        let actual = merge_updates_v1(&[a, b]).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn encode_state_vector_compatibility_v1() {
        let update = &[
            2, 1, 220, 240, 237, 172, 15, 0, 4, 1, 4, 116, 101, 115, 116, 3, 97, 98, 99, 1, 201,
            139, 250, 201, 1, 0, 4, 1, 4, 116, 101, 115, 116, 2, 100, 101, 0,
        ];
        let expected = &[2, 220, 240, 237, 172, 15, 3, 201, 139, 250, 201, 1, 2];
        let actual = encode_state_vector_from_update_v1(update).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn diff_updates_compatibility_v1() {
        let state_vector = &[1, 148, 189, 145, 162, 9, 3];
        let update = &[
            1, 2, 148, 189, 145, 162, 9, 0, 4, 1, 4, 116, 101, 115, 116, 3, 97, 98, 99, 68, 148,
            189, 145, 162, 9, 0, 2, 100, 101, 0,
        ];
        let expected = &[
            1, 1, 148, 189, 145, 162, 9, 3, 68, 148, 189, 145, 162, 9, 0, 2, 100, 101, 0,
        ];
        let actual = diff_updates_v1(update, state_vector).unwrap();
        assert_eq!(actual, expected);
    }
}
