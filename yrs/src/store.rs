// Internals: re-exported only through the hidden `yrs::internals` module, exempt from the docs requirement.
#![allow(missing_docs)]

use crate::block::{Block, ClientID, ItemContent, ItemPtr};
use crate::block_store::BlockStore;
use crate::branch::{Branch, BranchPtr};
use crate::doc::{DocAddr, Options};
use crate::error::Error;
use crate::event::SubdocsEvent;
use crate::id_set::DeleteSet;
use crate::slice::ItemSlice;
use crate::transact::TransactionAcqError;
use crate::types::{Path, PathSegment, TypeRef};
use crate::update::PendingUpdate;
use crate::updates::encoder::{Encode, Encoder};
use crate::{
    Doc, Observer, OffsetKind, Snapshot, TransactionCleanupEvent, TransactionMut, UpdateEvent,
    Uuid, ID,
};
use crate::{IdSet, StateVector};
use arc_swap::{ArcSwap, DefaultStrategy, Guard};
use async_lock::futures::{Read, Write};
use async_lock::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::borrow::Borrow;
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Deref;
use std::sync::Arc;

/// Store is a core element of a document. It contains all of the information, like block store
/// map of root types, pending updates waiting to be applied once a missing update information
/// arrives and all subscribed callbacks.
pub struct Store {
    pub(crate) client_id: ClientID,
    pub(crate) offset_kind: OffsetKind,
    pub(crate) skip_gc: bool,
    pub(crate) gc_filter: Option<crate::doc::GcFilter>,
    #[cfg_attr(not(feature = "weak"), allow(dead_code))]
    pub(crate) weak_links: bool,
    /// What undo redid each redone item as (Yjs' `item.redone`), kept here rather than in every
    /// item. Keyed by the item's ID when it was redone, with its length then: a later split
    /// half finds its target at the same offset (redone items are never squashed).
    pub(crate) redone: BTreeMap<(ClientID, u32), (u32, ID)>,
    pub(crate) metrics: Option<crate::metrics::Metrics>,
    pub(crate) cleanup_formatting: bool,

    /// Root types (a.k.a. top-level types). These types are defined by users at the document level,
    /// they have their own unique names and represent core shared types that expose operations
    /// which can be called concurrently by remote peers in a conflict-free manner.
    pub(crate) types: HashMap<Arc<str>, Box<Branch>>,

    /// A block store of a current document. It represent all blocks (inserted or tombstoned
    /// operations) integrated - and therefore visible - into a current document.
    pub(crate) blocks: BlockStore,

    /// A pending update. It contains blocks, which are not yet integrated into `blocks`, usually
    /// because due to issues in update exchange, there were some missing blocks that need to be
    /// integrated first before the data from `pending` can be applied safely.
    pub(crate) pending: Option<PendingUpdate>,

    /// A pending delete set. Just like `pending`, it contains deleted ranges of blocks that have
    /// not been yet applied due to missing blocks that prevent `pending` update to be integrated
    /// into `blocks`.
    pub(crate) pending_ds: Option<IdSet>,

    pub(crate) subdocs: HashMap<DocAddr, Doc>,

    pub(crate) events: Option<Box<StoreEvents>>,

    /// Pointer to a parent block - present only if a current document is a sub-document of another
    /// document.
    pub(crate) parent: Option<ItemPtr>,

    /// Dependencies between items and weak links pointing to these items.
    pub(crate) linked_by: HashMap<ItemPtr, HashSet<BranchPtr>>,

    /// Collections a write transaction needs, kept empty between transactions so each one
    /// doesn't allocate them again. See [TxnScratch].
    pub(crate) scratch: TxnScratch,
}

/// A write transaction's per-transaction collections, recycled through [Store::scratch].
#[derive(Default)]
pub(crate) struct TxnScratch {
    pub(crate) changed: HashMap<crate::types::TypePtr, HashSet<Option<Arc<str>>>>,
    pub(crate) changed_parent_types: Vec<BranchPtr>,
    pub(crate) merge_blocks: Vec<crate::ID>,
    /// Emptied key sets from `changed`, so a transaction that changes a type reuses one.
    pub(crate) key_sets: Vec<HashSet<Option<Arc<str>>>>,
}

impl TxnScratch {
    /// Beyond this many entries a collection isn't kept: one big transaction shouldn't hold its
    /// memory for the life of the document.
    const KEEP: usize = 64;
    /// How many emptied key sets to keep, and how large one may have grown to be kept.
    const KEEP_KEY_SETS: usize = 16;

    /// Empties the collections for the next transaction, dropping any that grew large.
    pub(crate) fn recycle(mut self) -> Self {
        fn keep<T: Default>(c: T, cap: usize) -> T {
            if cap > TxnScratch::KEEP {
                T::default()
            } else {
                c
            }
        }
        for (_, mut keys) in self.changed.drain() {
            if self.key_sets.len() < Self::KEEP_KEY_SETS && keys.capacity() <= Self::KEEP_KEY_SETS {
                keys.clear();
                self.key_sets.push(keys);
            }
        }
        self.changed_parent_types.clear();
        self.merge_blocks.clear();
        let changed_cap = self.changed.capacity();
        let parents_cap = self.changed_parent_types.capacity();
        let merge_cap = self.merge_blocks.capacity();
        TxnScratch {
            changed: keep(self.changed, changed_cap),
            changed_parent_types: keep(self.changed_parent_types, parents_cap),
            merge_blocks: keep(self.merge_blocks, merge_cap),
            key_sets: self.key_sets,
        }
    }
}

impl Store {
    /// Create a new empty store in context of a given `client_id`.
    pub(crate) fn new(options: &Options) -> Self {
        Store {
            redone: BTreeMap::new(),
            client_id: options.client_id,
            offset_kind: options.offset_kind,
            skip_gc: options.skip_gc,
            gc_filter: options.gc_filter.clone(),
            weak_links: options.weak_links,
            metrics: options.metrics.clone(),
            cleanup_formatting: options.cleanup_formatting,
            types: HashMap::default(),
            blocks: BlockStore::default(),
            subdocs: HashMap::default(),
            linked_by: HashMap::default(),
            scratch: TxnScratch::default(),
            events: None,
            pending: None,
            pending_ds: None,
            parent: None,
        }
    }

    /// If there are any missing updates, this method will return a pending update which contains
    /// updates waiting for their predecessors to arrive in order to be integrated.
    pub fn pending_update(&self) -> Option<&PendingUpdate> {
        self.pending.as_ref()
    }

    /// Returns a mutable reference to the pending update if it exists.
    pub fn pending_update_mut(&mut self) -> Option<&mut PendingUpdate> {
        self.pending.as_mut()
    }

    /// If there are some delete updates waiting for missing updates to arrive in order to be
    /// applied, this method will return them.
    pub fn pending_ds(&self) -> Option<&IdSet> {
        self.pending_ds.as_ref()
    }

    /// Returns a mutable reference to the pending delete set if it exists.
    pub fn pending_ds_mut(&mut self) -> Option<&mut IdSet> {
        self.pending_ds.as_mut()
    }

    pub fn is_subdoc(&self) -> bool {
        self.parent.is_some()
    }

    /// Get the latest clock sequence number observed and integrated into a current store client.
    /// This is exclusive value meaning it describes a clock value of the beginning of the next
    /// block that's about to be inserted. You cannot use that clock value to find any existing
    /// block content.
    pub fn get_local_state(&self) -> u32 {
        self.blocks.get_clock(&self.client_id)
    }

    /// Returns a branch reference to a complex type identified by its pointer. Returns `None` if
    /// no such type could be found or was ever defined.
    pub(crate) fn get_type<K: Borrow<str>>(&self, key: K) -> Option<BranchPtr> {
        let ptr = BranchPtr::from(self.types.get(key.borrow())?);
        Some(ptr)
    }

    /// Returns a branch reference to a complex type identified by its pointer. Returns `None` if
    /// no such type could be found or was ever defined.
    pub(crate) fn get_or_create_type<K: Into<Arc<str>>>(
        &mut self,
        key: K,
        type_ref: TypeRef,
    ) -> BranchPtr {
        let key = key.into();
        match self.types.entry(key.clone()) {
            Entry::Occupied(e) => {
                let mut branch = BranchPtr::from(e.get());
                branch.repair_type_ref(type_ref);
                branch
            }
            Entry::Vacant(e) => {
                let mut branch = Branch::new(type_ref);
                let mut branch_ref = BranchPtr::from(&mut branch);
                branch_ref.name = Some(key);
                e.insert(branch);
                branch_ref
            }
        }
    }

    /// Encodes all changes from current transaction block store up to a given `snapshot`.
    /// This enables to encode state of a document at some specific point in the past.
    pub fn encode_state_from_snapshot<E: Encoder>(
        &self,
        snapshot: &Snapshot,
        encoder: &mut E,
    ) -> Result<(), Error> {
        if !self.skip_gc {
            return Err(Error::Gc);
        }
        self.write_blocks_to(&snapshot.state_map, encoder);
        snapshot.delete_set.encode(encoder);

        Ok(())
    }

    pub(crate) fn write_blocks_to<E: Encoder>(&self, sv: &StateVector, encoder: &mut E) {
        let local_sv = self.blocks.get_state_vector();
        let mut diff = Vec::with_capacity(sv.len());
        for (&client_id, &clock) in sv.iter() {
            // A client at clock 0 has nothing in the snapshot, and `clock - 1` below would
            // underflow. Yjs skips it too (`createDocFromSnapshot`).
            if clock > 0 && local_sv.contains_client(&client_id) {
                diff.push((client_id, clock.min(local_sv.get(&client_id))));
            }
        }
        // Write items with higher client ids first
        // This heavily improves the conflict algorithm.
        diff.sort_by(|a, b| b.0.cmp(&a.0));

        encoder.write_var(diff.len());
        for (client, clock) in diff {
            let blocks = self.blocks.get_client(&client).unwrap();
            // `clock` is already at most the client's clock (see `diff` above)
            let last_idx = blocks.find_index(clock - 1).unwrap();
            // write # encoded structs
            encoder.write_var(last_idx + 1);
            encoder.write_client(client);
            encoder.write_var(0);
            for i in 0..last_idx {
                let block = blocks[i].as_slice();
                block.encode(encoder);
            }
            let last_block = &blocks[last_idx];
            // write first struct with an offset
            let mut slice = last_block.as_slice();
            slice.trim_end(slice.clock_end() - (clock - 1));
            slice.encode(encoder);
        }
    }

    /// Compute a diff to sync with another client.
    ///
    /// This is the most efficient method to sync with another client by only
    /// syncing the differences.
    ///
    /// The sync protocol in Yrs/js is:
    /// * Send StateVector to the other client.
    /// * The other client comutes a minimal diff to sync by using the StateVector.
    pub fn encode_diff<E: Encoder>(&self, sv: &StateVector, encoder: &mut E) {
        //TODO: this could be actually 2 steps:
        // 1. create Diff of block store and remote state vector (it can have lifetime of bock store)
        // 2. make Diff implement Encode trait and encode it
        // this way we can add some extra utility method on top of Diff (like introspection) without need of decoding it.
        self.write_blocks_from(sv, encoder);
        let delete_set = IdSet::from_store(&self.blocks);
        delete_set.encode(encoder);
    }

    pub(crate) fn write_blocks_from<E: Encoder>(&self, sv: &StateVector, encoder: &mut E) {
        let local_sv = self.blocks.get_state_vector();
        let mut diff = Self::diff_state_vectors(&local_sv, sv);

        // Write items with higher client ids first
        // This heavily improves the conflict algorithm.
        diff.sort_by(|a, b| b.0.cmp(&a.0));

        encoder.write_var(diff.len());
        for (client, clock) in diff {
            let blocks = self.blocks.get_client(&client).unwrap();
            let clock = clock.max(
                blocks
                    .get(0)
                    .map(|i| i.as_ref().clock_start())
                    .unwrap_or_default(),
            ); // make sure the first id exists
            let start = blocks.find_index(clock).unwrap();
            // write # encoded structs
            encoder.write_var(blocks.len() - start);
            encoder.write_client(client);
            encoder.write_var(clock);
            let first_block = blocks.get(start).unwrap().as_ref();
            // write first struct with an offset
            let offset = clock - first_block.clock_start();
            let mut slice = first_block.as_slice();
            slice.trim_start(offset);
            slice.encode(encoder);
            for i in (start + 1)..blocks.len() {
                let block = &blocks[i];
                block.as_slice().encode(encoder);
            }
        }
    }

    fn diff_state_vectors(local_sv: &StateVector, remote_sv: &StateVector) -> Vec<(ClientID, u32)> {
        let mut diff = Vec::new();
        for (client, &remote_clock) in remote_sv.iter() {
            let local_clock = local_sv.get(client);
            if local_clock > remote_clock {
                diff.push((*client, remote_clock));
            }
        }
        for (client, _) in local_sv.iter() {
            if !remote_sv.contains_client(client) {
                diff.push((*client, 0));
            }
        }
        diff
    }

    pub fn get_type_from_path(&self, path: &Path) -> Option<BranchPtr> {
        let mut i = path.iter();
        if let Some(PathSegment::Key(root_name)) = i.next() {
            let mut current = self.get_type(root_name.clone())?;
            while let Some(segment) = i.next() {
                match segment {
                    PathSegment::Key(key) => {
                        let child = current.map.get(key)?;
                        if let ItemContent::Type(child_branch) = &child.content {
                            current = BranchPtr::from(child_branch.as_ref());
                        } else {
                            return None;
                        }
                    }
                    PathSegment::Index(index) => {
                        if let Some((ItemContent::Type(child_branch), _)) = current.get_at(*index) {
                            current = child_branch.into();
                        } else {
                            return None;
                        }
                    }
                }
            }
            Some(current)
        } else {
            None
        }
    }

    /// Consumes current block slice view, materializing it into actual block representation equivalent,
    /// splitting underlying block along [ItemSlice::start]/[ItemSlice::end] offsets.
    ///
    /// Returns a block created this way, that represents the boundaries that current [ItemSlice]
    /// was representing.
    pub(crate) fn materialize(&mut self, mut slice: ItemSlice) -> ItemPtr {
        let id = slice.id().clone();
        let blocks = self.blocks.get_client_mut(&id.client).unwrap();
        let mut links = None;
        let item = slice.ptr.deref();
        if item.info.is_linked() {
            links = self.linked_by.get(&slice.ptr).cloned();
        }

        let mut index = None;
        let mut ptr = if slice.adjacent_left() {
            slice.ptr
        } else {
            let mut i = blocks.find_index(id.clock).unwrap();
            if let Some(new) = slice.ptr.splice(slice.start, OffsetKind::Utf16) {
                if let Some(source) = links.clone() {
                    let dest = self
                        .linked_by
                        .entry(ItemPtr::from(new.as_ref()))
                        .or_default();
                    dest.extend(source);
                }
                blocks.insert(i + 1, Block::Item(new));
                i += 1;
                //todo: txn merge blocks insert?
                index = Some(i);
            }
            let ptr = blocks[i].as_item().unwrap();
            slice = ItemSlice::new(ptr, 0, slice.end - slice.start);
            ptr
        };

        if !slice.adjacent_right() {
            // split block on the right side
            let i = if let Some(i) = index {
                i
            } else {
                let last_id = slice.last_id();
                blocks.find_index(last_id.clock).unwrap()
            };
            let new = ptr.splice(slice.len(), OffsetKind::Utf16).unwrap();
            if let Some(source) = links {
                let dest = self
                    .linked_by
                    .entry(ItemPtr::from(new.as_ref()))
                    .or_default();
                dest.extend(source);
            }
            blocks.insert(i + 1, Block::Item(new));
            //todo: txn merge blocks insert?
        }

        ptr
    }

    /// Returns a collection of sub documents linked within the structures of this document store.
    pub fn subdocs(&self) -> SubdocsIter {
        SubdocsIter(self.subdocs.values())
    }

    /// Returns a collection of globally unique identifiers of sub documents linked within
    /// the structures of this document store.
    pub fn subdoc_guids(&self) -> SubdocGuids {
        SubdocGuids(self.subdocs.values())
    }

    /// The ID that `item` was redone as, if undo redid it.
    pub(crate) fn redone_of(&self, item: &crate::block::Item) -> Option<ID> {
        if !item.info.is_redone() {
            return None;
        }
        let (client, clock) = (item.id.client, item.id.clock);
        let (&(c, start), &(len, target)) = self.redone.range(..=(client, clock)).next_back()?;
        if c != client || clock >= start + len {
            return None;
        }
        Some(ID::new(target.client, target.clock + (clock - start)))
    }

    /// Records that `item` (already flagged) was redone as `target`.
    pub(crate) fn set_redone(&mut self, item: &crate::block::Item, target: ID) {
        self.redone
            .insert((item.id.client, item.id.clock), (item.len, target));
    }

    pub(crate) fn follow_redone(&self, id: &ID) -> Option<ItemSlice> {
        let mut next_id = Some(*id);
        let mut slice = None;
        while let Some(next) = next_id.as_mut() {
            slice = self.blocks.get_item_clean_start(next);
            if let Some(slice) = &slice {
                next_id = self
                    .redone_of(&slice.ptr)
                    .map(|redone| ID::new(redone.client, redone.clock + slice.start));
            } else {
                break;
            }
        }
        slice
    }
}

impl Encode for Store {
    /// Encodes the document state to a binary format.
    ///
    /// Document updates are idempotent and commutative. Caveats:
    /// * It doesn't matter in which order document updates are applied.
    /// * As long as all clients receive the same document updates, all clients
    ///   end up with the same content.
    /// * Even if an update contains known information, the unknown information
    ///   is extracted and integrated into the document structure.
    fn encode<E: Encoder>(&self, encoder: &mut E) {
        self.encode_diff(&StateVector::default(), encoder)
    }
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::fmt::Display for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct(&self.client_id.to_string());
        if !self.types.is_empty() {
            s.field("root types", &self.types);
        }
        if !self.blocks.is_empty() {
            s.field("blocks", &self.blocks);
        }
        if let Some(pending) = self.pending.as_ref() {
            s.field("pending", pending);
        }
        if let Some(pending_ds) = self.pending_ds.as_ref() {
            s.field("pending delete set", pending_ds);
        }

        if let Some(parent) = self.parent.as_ref() {
            s.field("parent block", parent.id());
        }
        if !self.linked_by.is_empty() {
            s.field("links", &self.linked_by);
        }
        s.finish()
    }
}

#[repr(transparent)]
#[derive(Debug, Clone)]
pub(crate) struct DocStore(pub(crate) Arc<StoreInner>);

impl DocStore {
    pub fn new(options: Options, parent: Option<ItemPtr>) -> Self {
        let mut store = Store::new(&options);
        let options = ArcSwap::new(options.into());
        store.parent = parent;
        DocStore(Arc::new(StoreInner {
            options,
            store: RwLock::new(store),
            holders: Default::default(),
        }))
    }

    /// Fails if the current thread already holds a transaction that a new one (read-write if
    /// `write`) would wait on forever: any transaction, for a read-write one, or a read-write
    /// one, for a read-only one.
    pub(crate) fn check_reentrant(&self, write: bool) -> Result<(), TransactionAcqError> {
        let me = std::thread::current().id();
        let h = self.0.holders.lock().unwrap_or_else(|e| e.into_inner());
        if h.writer == Some(me) || (write && h.readers.contains(&me)) {
            Err(TransactionAcqError::Reentrant)
        } else {
            Ok(())
        }
    }

    /// Records that the current thread now holds a transaction on this document.
    pub(crate) fn hold(&self, write: bool) -> Held<'_> {
        let thread = std::thread::current().id();
        let mut h = self.0.holders.lock().unwrap_or_else(|e| e.into_inner());
        if write {
            h.writer = Some(thread);
        } else {
            h.readers.push(thread);
        }
        Held {
            holders: &self.0.holders,
            thread,
            write,
        }
    }

    pub(crate) fn try_read(&self) -> Option<RwLockReadGuard<Store>> {
        self.0.store.try_read()
    }

    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn read_blocking(&self) -> RwLockReadGuard<Store> {
        // Uncontended is the common case: skip setting up a waiter for it.
        match self.0.store.try_read() {
            Some(guard) => guard,
            None => self.0.store.read_blocking(),
        }
    }

    #[cfg(target_family = "wasm")]
    pub(crate) fn read_blocking(&self) -> RwLockReadGuard<Store> {
        self.0.store.try_read().unwrap()
    }

    pub(crate) fn read_async(&self) -> Read<Store> {
        self.0.store.read()
    }

    pub(crate) fn try_write(&self) -> Option<RwLockWriteGuard<Store>> {
        self.0.store.try_write()
    }

    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn write_blocking(&self) -> RwLockWriteGuard<Store> {
        // Uncontended is the common case: skip setting up a waiter for it.
        match self.0.store.try_write() {
            Some(guard) => guard,
            None => self.0.store.write_blocking(),
        }
    }

    #[cfg(target_family = "wasm")]
    pub(crate) fn write_blocking(&self) -> RwLockWriteGuard<Store> {
        self.0.store.try_write().unwrap()
    }

    pub(crate) fn write_async(&self) -> Write<Store> {
        self.0.store.write()
    }

    pub(crate) fn options(&self) -> Guard<Arc<Options>, DefaultStrategy> {
        self.0.options.load()
    }

    /// Sets [Doc::should_load] flag, returning previous value.
    pub(crate) fn set_should_load(&self, should_load: bool) -> bool {
        self.0
            .options
            .rcu(|options| {
                let mut options = options.deref().clone();
                options.should_load = should_load;
                options
            })
            .should_load
    }

    pub(crate) fn set_subdoc_data(&self, client_id: ClientID, collection_id: Option<Arc<str>>) {
        self.0.options.rcu(|options| {
            let mut options = options.deref().clone();
            options.client_id = client_id;
            if options.collection_id.is_none() {
                options.collection_id = collection_id.clone();
            }
            options
        });
    }
}

#[derive(Debug)]
pub(crate) struct StoreInner {
    options: ArcSwap<Options>,
    store: RwLock<Store>,
    /// Threads that hold a blocking transaction on this document, to refuse one that would
    /// deadlock (see [DocStore::hold]).
    holders: std::sync::Mutex<Holders>,
}

#[derive(Default, Debug)]
struct Holders {
    writer: Option<std::thread::ThreadId>,
    readers: Vec<std::thread::ThreadId>,
}

/// Records that the current thread holds a transaction, until dropped.
#[derive(Debug)]
pub(crate) struct Held<'a> {
    holders: &'a std::sync::Mutex<Holders>,
    thread: std::thread::ThreadId,
    write: bool,
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        let mut h = self.holders.lock().unwrap_or_else(|e| e.into_inner());
        if self.write {
            if h.writer == Some(self.thread) {
                h.writer = None;
            }
        } else if let Some(i) = h.readers.iter().position(|t| *t == self.thread) {
            h.readers.swap_remove(i);
        }
    }
}

#[repr(transparent)]
pub struct SubdocsIter<'doc>(std::collections::hash_map::Values<'doc, DocAddr, Doc>);

impl<'doc> Iterator for SubdocsIter<'doc> {
    type Item = &'doc Doc;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

#[repr(transparent)]
pub struct SubdocGuids<'doc>(std::collections::hash_map::Values<'doc, DocAddr, Doc>);

impl<'doc> Iterator for SubdocGuids<'doc> {
    type Item = Uuid;

    fn next(&mut self) -> Option<Self::Item> {
        let d = self.0.next()?;
        Some(d.guid())
    }
}

macro_rules! define_event_type {
    ($name:ident ($($args:tt)*)) => {
        #[cfg(feature = "sync")]
        pub type $name = Box<dyn FnMut($($args)*) + Send + Sync + 'static>;
        #[cfg(not(feature = "sync"))]
        pub type $name = Box<dyn FnMut($($args)*) + 'static>;
    };
}

define_event_type!(TransactionCleanupFn(
    &TransactionMut,
    &TransactionCleanupEvent
));
define_event_type!(AfterTransactionFn(&mut TransactionMut));
define_event_type!(UpdateFn(&TransactionMut, &UpdateEvent));
define_event_type!(SubdocsFn(&TransactionMut, &SubdocsEvent));
define_event_type!(DestroyFn(&TransactionMut, &Doc));
define_event_type!(BeforeObserverCallsFn(&TransactionMut));

#[derive(Default)]
pub struct StoreEvents {
    /// Handles subscriptions for the transaction cleanup event. Events are called with the
    /// newest updates once they are committed and compacted.
    pub transaction_cleanup_events: Observer<TransactionCleanupFn>,

    /// Handles subscriptions for the `afterTransactionCleanup` event. Events are called with the
    /// newest updates once they are committed and compacted.
    pub after_transaction_events: Observer<AfterTransactionFn>,

    /// A subscription handler. It contains all callbacks with registered by user functions that
    /// are supposed to be called, once a new update arrives.
    pub update_v1_events: Observer<UpdateFn>,

    /// A subscription handler. It contains all callbacks with registered by user functions that
    /// are supposed to be called, once a new update arrives.
    pub update_v2_events: Observer<UpdateFn>,

    /// Handles subscriptions for subdocs events.
    pub subdocs_events: Observer<SubdocsFn>,

    pub destroy_events: Observer<DestroyFn>,

    /// Handles subscriptions for the `beforeObserverCalls` event. Callbacks are called after
    /// the transaction body completes but before type-level observers are triggered.
    pub before_observer_calls_events: Observer<BeforeObserverCallsFn>,
}

impl StoreEvents {
    pub fn emit_update_v1(&mut self, txn: &TransactionMut) {
        if self.update_v1_events.has_subscribers() {
            if !txn.delete_set.is_empty() || txn.after_state() != txn.before_state() {
                let update = UpdateEvent::new_v1(txn);
                self.update_v1_events
                    .trigger(|callback| callback(txn, &update));
            }
        }
    }

    pub fn emit_update_v2(&mut self, txn: &TransactionMut) {
        if self.update_v2_events.has_subscribers() {
            if !txn.delete_set.is_empty() || txn.after_state() != txn.before_state() {
                let update = UpdateEvent::new_v2(txn);
                self.update_v2_events.trigger(|fun| fun(txn, &update));
            }
        }
    }

    pub fn emit_after_transaction(&mut self, txn: &mut TransactionMut) {
        self.after_transaction_events.trigger(|fun| fun(txn));
    }

    pub fn emit_transaction_cleanup(&mut self, txn: &TransactionMut) {
        if self.transaction_cleanup_events.has_subscribers() {
            let event = TransactionCleanupEvent::new(txn);
            self.transaction_cleanup_events
                .trigger(|fun| fun(txn, &event));
        }
    }

    pub fn emit_before_observer_calls(&mut self, txn: &TransactionMut) {
        self.before_observer_calls_events.trigger(|fun| fun(txn));
    }
}
