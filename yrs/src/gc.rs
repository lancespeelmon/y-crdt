use crate::block::{Block, ClientID, Item};
use crate::doc::GcFilter;
use crate::{IdSet, Store, TransactionMut, ID};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(crate) struct GCCollector {
    marked: HashMap<ClientID, Vec<u32>>,
    /// The IDs in `marked`, kept only when there is a [GcFilter] to ask.
    marked_ids: Option<HashSet<ID>>,
}

impl GCCollector {
    /// Whether a deleted item may be collected: it isn't kept, and the [Options::gc_filter] (if
    /// any) allows it. Like Yjs, the filter is not asked about the contents of a type already
    /// being collected: they go with it.
    ///
    /// [Options::gc_filter]: crate::Options::gc_filter
    fn collectable(&mut self, item: &Item, filter: Option<&GcFilter>) -> bool {
        if item.info.is_keep() {
            return false;
        }
        match filter {
            None => true,
            Some(filter) => {
                let marked = self
                    .marked_ids
                    .as_ref()
                    .is_some_and(|m| m.contains(&item.id));
                !marked && filter.allows(item)
            }
        }
    }

    /// Garbage collect all blocks deleted within current transaction scope.
    pub fn collect(txn: &mut TransactionMut) {
        let mut gc = Self::default();
        gc.mark_in_scope(&mut txn.store, None, &txn.delete_set);
        gc.collect_marked(txn);
    }

    /// Garbage collect all deleted blocks from current transaction's document store.
    pub fn collect_all(txn: &mut TransactionMut, delete_set: Option<&IdSet>) {
        let mut gc = Self::default();
        match delete_set {
            None => gc.mark_all(txn),
            Some(ds) => gc.mark_in_scope(&mut txn.store, Some(&mut txn.merge_blocks), ds),
        }
        gc.collect_marked(txn);
    }

    /// Mark deleted items based on a provided delete set.
    fn mark_in_scope(
        &mut self,
        store: &mut Store,
        mut merge_blocks: Option<&mut Vec<ID>>,
        delete_set: &IdSet,
    ) {
        let filter = store.gc_filter.clone();
        self.track_marked(filter.is_some());
        for (client, range) in delete_set.iter() {
            if let Some(blocks) = store.blocks.get_client_mut(client) {
                for delete_item in range.iter().rev() {
                    let mut start = delete_item.start;
                    if let Some(mut i) = blocks.find_index(start) {
                        while i < blocks.len() {
                            let mut block = unsafe { blocks.get(i).unwrap_unchecked() };
                            let block = block.as_mut();
                            let len = block.len();
                            start += len;
                            if start > delete_item.end {
                                break;
                            } else {
                                if let Block::Item(item) = block {
                                    if self.collectable(item, filter.as_ref()) {
                                        item.gc(self);
                                    }
                                    if let Some(merge_blocks) = merge_blocks.as_deref_mut() {
                                        merge_blocks.push(item.id);
                                    }
                                }
                                i += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    fn mark_all(&mut self, txn: &mut TransactionMut) {
        let filter = txn.store.gc_filter.clone();
        self.track_marked(filter.is_some());
        for (_, client_blocks) in txn.store.blocks.iter_mut() {
            for mut block in client_blocks.iter() {
                if let Block::Item(item) = block.as_mut() {
                    if item.is_deleted() {
                        if self.collectable(item, filter.as_ref()) {
                            item.gc(self);
                        }
                        txn.merge_blocks.push(item.id);
                    }
                }
            }
        }
    }

    fn track_marked(&mut self, track: bool) {
        if track && self.marked_ids.is_none() {
            self.marked_ids = Some(HashSet::new());
        }
    }

    /// Marks item with a given [ID] as a candidate for being GCed.
    pub(crate) fn mark(&mut self, id: &ID) {
        let client = self.marked.entry(id.client).or_default();
        client.push(id.clock);
        if let Some(ids) = self.marked_ids.as_mut() {
            ids.insert(*id);
        }
    }

    /// Garbage collects all items marked for GC.
    fn collect_marked(self, txn: &mut TransactionMut) {
        for (client_id, clocks) in self.marked.into_iter() {
            let client = txn.store.blocks.get_client_blocks_mut(client_id);
            for clock in clocks {
                if let Some(index) = client.find_index(clock) {
                    let block = unsafe { client.get(index).unwrap_unchecked() }.as_mut();
                    if let Block::Item(item) = block {
                        if item.is_deleted() {
                            let gc = Block::GC(item.block_range());
                            *block = gc;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use crate::block::ItemContent;
    use crate::branch::BranchID;
    use crate::{Array, ClientID, Doc, GcFilter, Options, ReadTxn, Text, Transact, ID};

    #[test]
    fn the_gc_filter_decides_which_deleted_items_are_collected() {
        let options = Options {
            gc_filter: Some(GcFilter::new(|item| {
                item.parent_id() != Some(BranchID::Root("kept".into()))
            })),
            ..Options::with_client_id(ClientID::new(1))
        };
        let doc = Doc::with_options(options);
        let kept = doc.get_or_insert_text("kept");
        let dropped = doc.get_or_insert_text("dropped");
        let nested = doc.get_or_insert_array("nested");
        {
            let mut txn = doc.transact_mut();
            kept.insert(&mut txn, 0, "abc"); // clocks 0..3
            dropped.insert(&mut txn, 0, "def"); // clocks 3..6
            nested.insert(&mut txn, 0, crate::TextPrelim::new("ghi")); // 6, then 7..10
        }
        {
            let mut txn = doc.transact_mut();
            kept.remove_range(&mut txn, 0, 3);
            dropped.remove_range(&mut txn, 0, 3);
            nested.remove(&mut txn, 0);
        }
        let txn = doc.transact();
        let content = |clock| {
            let id = ID::new(ClientID::new(1), clock);
            txn.store()
                .blocks
                .get_item(&id)
                .map(|item| item.content().clone())
        };
        assert!(
            matches!(content(0), Some(ItemContent::String(_))),
            "a deleted item the filter refuses keeps its content"
        );
        assert!(matches!(content(3), Some(ItemContent::Deleted(3))));
        assert!(matches!(content(6), Some(ItemContent::Deleted(1))));
        assert!(
            content(7).is_none(),
            "the contents of a collected type are collected without asking the filter"
        );
    }
}
