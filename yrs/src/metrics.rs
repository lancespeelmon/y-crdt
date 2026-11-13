//! Hooks for observing what a document does, for servers that export metrics. yrs depends on no
//! metrics crate: implement [MetricsHook] over whichever one you use and install it with
//! [Options::metrics](crate::Options::metrics). Every method has an empty default, so implement
//! only the events you count.
//!
//! With the `tracing` feature, the same operations also emit `tracing` spans (`yrs::decode`,
//! `yrs::apply_update`, `yrs::commit`, `yrs::encode`) with the counts as fields.

use std::sync::Arc;

/// Receives counts from a document's operations. Calls happen inside the operation, while its
/// transaction is held, so keep them cheap (increment a counter) and don't open transactions.
pub trait MetricsHook: Send + Sync {
    /// An update was applied: how many blocks and delete ranges it carried, and how many blocks
    /// and delete ranges are left waiting for missing dependencies afterwards.
    fn update_applied(&self, _applied: UpdateCounts, _pending: UpdateCounts) {}

    /// A read-write transaction committed, having inserted and deleted these many ranges, as the
    /// transaction recorded them (before adjacent ranges are merged).
    fn transaction_committed(&self, _inserted_ranges: usize, _deleted_ranges: usize) {}

    /// The document state, or a diff of it, was encoded into this many bytes.
    fn state_encoded(&self, _bytes: usize) {}
}

/// Sizes of an update, or of the pending state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UpdateCounts {
    /// Blocks (inserted ranges, skips and GC ranges).
    pub blocks: usize,
    /// Deleted ranges.
    pub delete_ranges: usize,
}

/// The hook of [Options::metrics](crate::Options::metrics). Two are equal only if they are the
/// same `Arc`.
#[derive(Clone)]
pub struct Metrics(pub Arc<dyn MetricsHook>);

impl Metrics {
    /// Wraps `hook`.
    pub fn new<H: MetricsHook + 'static>(hook: H) -> Self {
        Metrics(Arc::new(hook))
    }
}

impl std::fmt::Debug for Metrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Metrics")
    }
}

impl PartialEq for Metrics {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// Enters a `tracing` span with the `tracing` feature, and does nothing without it.
macro_rules! span {
    ($name:literal $(, $field:ident)*) => {{
        #[cfg(feature = "tracing")]
        let span = tracing::debug_span!($name $(, $field = tracing::field::Empty)*).entered();
        #[cfg(not(feature = "tracing"))]
        let span = $crate::metrics::NoSpan;
        span
    }};
}
pub(crate) use span;

/// Records a field of a span from [span!], with the `tracing` feature.
macro_rules! record {
    ($span:expr, $field:ident, $value:expr) => {{
        #[cfg(feature = "tracing")]
        $span.record(stringify!($field), $value);
        #[cfg(not(feature = "tracing"))]
        let _ = (&$span, &$value);
    }};
}
pub(crate) use record;

/// What [span!] gives without the `tracing` feature.
#[cfg(not(feature = "tracing"))]
pub(crate) struct NoSpan;

#[cfg(test)]
mod test {
    use super::{Metrics, MetricsHook, UpdateCounts};
    use crate::updates::decoder::Decode;
    use crate::{ClientID, Doc, Options, ReadTxn, StateVector, Text, Transact, Update};
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<String>>);

    impl MetricsHook for Recorder {
        fn update_applied(&self, applied: UpdateCounts, pending: UpdateCounts) {
            let line = format!("applied {} pending {}", applied.blocks, pending.blocks);
            self.0.lock().unwrap().push(line);
        }
        fn transaction_committed(&self, inserted: usize, deleted: usize) {
            let line = format!("commit +{} -{}", inserted, deleted);
            self.0.lock().unwrap().push(line);
        }
        fn state_encoded(&self, bytes: usize) {
            self.0
                .lock()
                .unwrap()
                .push(format!("encoded {}", bytes > 0));
        }
    }

    #[test]
    fn the_hook_sees_applies_commits_pending_blocks_and_encodes() {
        let recorder = std::sync::Arc::new(Recorder::default());
        let doc = Doc::with_options(Options {
            metrics: Some(Metrics(recorder.clone())),
            ..Options::with_client_id(ClientID::new(1))
        });
        let source = Doc::with_client_id(2);
        let text = source.get_or_insert_text("t");
        text.insert(&mut source.transact_mut(), 0, "a");
        let first = source.transact().state_vector();
        text.insert(&mut source.transact_mut(), 1, "b");
        // Only the second insert: it waits for the first.
        let second = source.transact().encode_state_as_update_v1(&first);
        doc.transact_mut()
            .apply_update(Update::decode_v1(&second).unwrap())
            .unwrap();
        doc.get_or_insert_text("t");
        doc.transact()
            .encode_state_as_update_v1(&StateVector::default());

        let lines = recorder.0.lock().unwrap().clone();
        assert_eq!(
            lines,
            [
                "applied 1 pending 1",
                "commit +0 -0",
                "commit +0 -0",
                "encoded true"
            ]
        );
    }
}
