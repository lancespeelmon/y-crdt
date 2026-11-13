//! Logic that the yrs bindings share, so yffi and ywasm behave the same (plan item 7.7).
//!
//! Only binding-agnostic rules live here: what an update utility returns, how undo options are
//! built from a binding's settings, and how deep preliminary values may nest. Reading C structs
//! or JS values, and shaping the output for them, stays in each binding.
#![deny(missing_docs)]

use std::collections::HashSet;
use std::sync::Arc;
use yrs::encoding::read::Error as DecodeError;
use yrs::time::Clock;
use yrs::undo::Options as UndoOptions;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, Options, Origin, ReadTxn, StateVector, Transact, Update};

/// A lib0 encoding of updates and state vectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// lib0 v1, Yjs' default.
    V1,
    /// lib0 v2.
    V2,
}

impl Encoding {
    /// Decodes an update.
    pub fn decode_update(self, bytes: &[u8]) -> Result<Update, DecodeError> {
        match self {
            Encoding::V1 => Update::decode_v1(bytes),
            Encoding::V2 => Update::decode_v2(bytes),
        }
    }

    /// Encodes an update.
    pub fn encode_update(self, update: &Update) -> Vec<u8> {
        match self {
            Encoding::V1 => update.encode_v1(),
            Encoding::V2 => update.encode_v2(),
        }
    }
}

/// Why an update utility failed.
#[derive(Debug)]
pub enum UpdateUtilError {
    /// An input couldn't be decoded.
    Decode(DecodeError),
    /// A decoded update couldn't be applied.
    Apply(yrs::error::UpdateError),
}

impl std::fmt::Display for UpdateUtilError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateUtilError::Decode(e) => e.fmt(f),
            UpdateUtilError::Apply(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for UpdateUtilError {}

impl From<DecodeError> for UpdateUtilError {
    fn from(e: DecodeError) -> Self {
        UpdateUtilError::Decode(e)
    }
}

/// Merges updates into one, like Yjs' `mergeUpdates`. No updates give an encoded empty update
/// (as in Yjs), not an empty buffer.
pub fn merge_updates<I, B>(encoding: Encoding, updates: I) -> Result<Vec<u8>, UpdateUtilError>
where
    I: IntoIterator<Item = B>,
    B: AsRef<[u8]>,
{
    Ok(match encoding {
        Encoding::V1 => yrs::merge_updates_v1(updates)?,
        Encoding::V2 => yrs::merge_updates_v2(updates)?,
    })
}

/// The part of `update` that a peer at `state_vector` is missing, like Yjs' `diffUpdate`. Both
/// inputs use `encoding`.
pub fn diff_update(
    encoding: Encoding,
    update: &[u8],
    state_vector: &[u8],
) -> Result<Vec<u8>, UpdateUtilError> {
    Ok(match encoding {
        Encoding::V1 => yrs::diff_updates_v1(update, state_vector)?,
        Encoding::V2 => yrs::diff_updates_v2(update, state_vector)?,
    })
}

/// The state vector `update` brings a document to, like Yjs' `encodeStateVectorFromUpdate`.
pub fn state_vector_from_update(
    encoding: Encoding,
    update: &[u8],
) -> Result<Vec<u8>, UpdateUtilError> {
    Ok(match encoding {
        Encoding::V1 => yrs::encode_state_vector_from_update_v1(update)?,
        Encoding::V2 => yrs::encode_state_vector_from_update_v2(update)?,
    })
}

/// Applies `updates` to a fresh document and returns its state, collecting deleted content if
/// `gc` is set: a snapshot of what the updates add up to. Every input is decoded before any is
/// applied, so a bad one fails the whole call.
pub fn apply_updates<I, B>(
    encoding: Encoding,
    gc: bool,
    updates: I,
) -> Result<Vec<u8>, UpdateUtilError>
where
    I: IntoIterator<Item = B>,
    B: AsRef<[u8]>,
{
    let decoded = updates
        .into_iter()
        .map(|u| encoding.decode_update(u.as_ref()))
        .collect::<Result<Vec<_>, _>>()?;
    let doc = Doc::with_options(Options {
        skip_gc: !gc,
        ..Options::default()
    });
    {
        let mut txn = doc.transact_mut();
        for update in decoded {
            txn.apply_update(update).map_err(UpdateUtilError::Apply)?;
        }
    }
    let txn = doc.transact();
    let sv = StateVector::default();
    Ok(match encoding {
        Encoding::V1 => txn.encode_state_as_update_v1(&sv),
        Encoding::V2 => txn.encode_state_as_update_v2(&sv),
    })
}

/// A binding's undo manager settings, before they become [UndoOptions].
#[derive(Default)]
pub struct UndoSettings {
    /// How long changes are merged into one stack item. `None` keeps yrs' default (500 ms).
    pub capture_timeout_millis: Option<u64>,
    /// Yjs' `ignoreRemoteMapChanges`.
    pub ignore_remote_map_changes: bool,
    /// Yjs' `trackedOrigins`, when given: the origins, and whether it listed `null` (changes
    /// made without an origin). As in Yjs, a list without `null` doesn't track those.
    pub tracked_origins: Option<(Vec<Origin>, bool)>,
}

impl UndoSettings {
    /// The undo manager options these settings describe, timed by `clock`, with yrs' defaults
    /// for the rest.
    pub fn into_options<M>(self, clock: Arc<dyn Clock>) -> UndoOptions<M> {
        let mut o = UndoOptions::with_clock(clock);
        if let Some(millis) = self.capture_timeout_millis {
            o.capture_timeout_millis = millis;
        }
        o.ignore_remote_map_changes = self.ignore_remote_map_changes;
        if let Some((origins, track_null)) = self.tracked_origins {
            o.tracked_origins = origins.into_iter().collect::<HashSet<_>>();
            o.track_null_origin = Some(track_null);
        }
        o
    }
}

/// How deep preliminary values (collections inside collections being inserted) may nest. Input
/// that refers to itself would otherwise be followed until the stack overflows, which no binding
/// can catch.
pub const MAX_PRELIM_DEPTH: usize = 512;

/// A preliminary value nested deeper than [MAX_PRELIM_DEPTH].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooDeep;

impl std::fmt::Display for TooDeep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "preliminary value nested more than {MAX_PRELIM_DEPTH} levels deep (does it contain itself?)"
        )
    }
}

impl std::error::Error for TooDeep {}

/// Checks the nesting depth of a value being converted, where the top level is 0.
pub fn check_depth(depth: usize) -> Result<(), TooDeep> {
    if depth > MAX_PRELIM_DEPTH {
        Err(TooDeep)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use yrs::{GetString, Text};

    fn text_update(encoding: Encoding, s: &str) -> Vec<u8> {
        let doc = Doc::new();
        let txt = doc.get_or_insert_text("t");
        txt.insert(&mut doc.transact_mut(), 0, s);
        let txn = doc.transact();
        match encoding {
            Encoding::V1 => txn.encode_state_as_update_v1(&StateVector::default()),
            Encoding::V2 => txn.encode_state_as_update_v2(&StateVector::default()),
        }
    }

    #[test]
    fn merging_no_updates_gives_an_empty_update_that_decodes() {
        for e in [Encoding::V1, Encoding::V2] {
            let merged = merge_updates(e, Vec::<Vec<u8>>::new()).unwrap();
            assert!(!merged.is_empty());
            assert!(e.decode_update(&merged).is_ok());
        }
    }

    #[test]
    fn apply_updates_gives_their_combined_state() {
        for e in [Encoding::V1, Encoding::V2] {
            let (a, b) = (text_update(e, "a"), text_update(e, "b"));
            let state = apply_updates(e, true, [&a, &b]).unwrap();
            let doc = Doc::new();
            let txt = doc.get_or_insert_text("t");
            doc.transact_mut()
                .apply_update(e.decode_update(&state).unwrap())
                .unwrap();
            assert_eq!(txt.get_string(&doc.transact()).len(), 2);
        }
    }

    #[test]
    fn a_bad_update_fails_apply_and_merge() {
        for e in [Encoding::V1, Encoding::V2] {
            let good = text_update(e, "a");
            assert!(apply_updates(e, true, [&good[..], &[0xff, 0xff][..]]).is_err());
            assert!(merge_updates(e, [&good[..], &[0xff, 0xff][..]]).is_err());
        }
    }

    #[test]
    fn undo_settings_keep_defaults_unless_set() {
        let clock = || Arc::new(yrs::time::SystemClock) as Arc<dyn Clock>;
        let o: UndoOptions<()> = UndoSettings::default().into_options(clock());
        assert_eq!(o.capture_timeout_millis, 500);
        assert_eq!(o.track_null_origin, None);
        let o: UndoOptions<()> = UndoSettings {
            capture_timeout_millis: Some(0),
            ignore_remote_map_changes: true,
            tracked_origins: Some((vec![Origin::from("me")], false)),
        }
        .into_options(clock());
        assert_eq!(o.capture_timeout_millis, 0);
        assert!(o.ignore_remote_map_changes);
        assert!(o.tracked_origins.contains(&Origin::from("me")));
        assert_eq!(o.track_null_origin, Some(false));
    }

    #[test]
    fn depth_limit() {
        assert!(check_depth(MAX_PRELIM_DEPTH).is_ok());
        assert_eq!(check_depth(MAX_PRELIM_DEPTH + 1), Err(TooDeep));
    }
}
