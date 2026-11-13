//! Estimating the heap memory a document holds, for servers that keep many documents loaded
//! and need to decide which to compact or evict (see [crate::Doc::compact]).

use crate::block::{Block, Item, ItemContent, ItemPtr};
use crate::branch::Branch;
use crate::store::Store;
use crate::Any;
use std::mem::size_of;
use std::sync::Arc;

/// An estimate of the heap memory held by a document's blocks, from
/// [crate::ReadTxn::memory_usage]. It counts the items, what their content holds (strings,
/// values, nested types' entries) and the per-client block lists, by allocated capacity where
/// the content keeps its own (strings, lists, maps), so spare room counts. It's still an
/// estimate: allocator overhead and the document's other bookkeeping (pending updates,
/// subscriptions) aren't included.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct MemoryUsage {
    /// Blocks in the store: items, plus collected and skipped ranges.
    pub blocks: usize,
    /// Bytes of the items themselves.
    pub items: usize,
    /// Bytes the items' content holds.
    pub content: usize,
    /// Bytes of the per-client block lists that index the items.
    pub index: usize,
}

impl MemoryUsage {
    /// The sum of the parts, in bytes.
    pub fn total(&self) -> usize {
        self.items + self.content + self.index
    }

    pub(crate) fn of(store: &Store) -> Self {
        let mut usage = MemoryUsage::default();
        for (_, list) in store.blocks.iter() {
            usage.index += list.capacity() * size_of::<Block>();
            for block in list.iter() {
                usage.blocks += 1;
                if let Block::Item(item) = block.as_ref() {
                    usage.items += size_of::<Item>();
                    usage.content += content_bytes(&item.content);
                }
            }
        }
        usage
    }
}

fn content_bytes(content: &ItemContent) -> usize {
    match content {
        ItemContent::Any(values) => values.capacity() * size_of::<Any>() + any_slice(values),
        ItemContent::Binary(bytes) => bytes.capacity(),
        ItemContent::Deleted(_) => 0,
        ItemContent::Doc(_, _) => 0, // a subdocument is a document of its own
        ItemContent::JSON(strings) => strings
            .iter()
            .map(|s| size_of::<String>() + s.capacity())
            .sum(),
        ItemContent::Embed(any) => any_bytes(any),
        ItemContent::Format(key, value) => key.len() + size_of::<Any>() + any_bytes(value),
        ItemContent::String(s) => s.heap_bytes(),
        ItemContent::Type(branch) => branch_bytes(branch),
        #[allow(unreachable_patterns)]
        _ => 0,
    }
}

fn branch_bytes(branch: &Branch) -> usize {
    let entries = branch.map.capacity() * size_of::<(Arc<str>, ItemPtr)>();
    let keys: usize = branch.map.keys().map(|k| k.len()).sum();
    size_of::<Branch>() + entries + keys
}

fn any_slice(values: &[Any]) -> usize {
    values.iter().map(any_bytes).sum()
}

fn any_bytes(any: &Any) -> usize {
    match any {
        Any::String(s) => s.len(),
        Any::Buffer(b) => b.len(),
        Any::Array(values) => values.len() * size_of::<Any>() + any_slice(values),
        Any::Map(map) => map
            .iter()
            .map(|(k, v)| size_of::<String>() + k.len() + size_of::<Any>() + any_bytes(v))
            .sum(),
        _ => 0,
    }
}
