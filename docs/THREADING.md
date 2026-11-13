# Threading and unsafe code

## Using a document from several threads

With the `sync` feature (on by default), `Doc`, the shared types, `Awareness` and the
UndoManager are `Send + Sync`, and every observer callback must be `Send + Sync` too. Without
it, callbacks may hold `Rc`s and other thread-bound values, and a document and everything made
from it must stay on the thread that created it.

A document's store is behind a read-write lock: any number of read transactions, or one
read-write transaction, at a time.

- `transact` and `transact_mut` block until the lock is free.
- `try_transact` and `try_transact_mut` return `TransactionAcqError` instead of blocking.
- `transact_async` and `transact_mut_async` wait asynchronously.

A thread that would wait for a lock it already holds is refused instead of deadlocking:
`try_transact*` returns `TransactionAcqError::Reentrant`, and `transact*` panics with an
explanation. This usually means opening a transaction in an observer callback. Use the
transaction the callback is given instead.

Keep transactions short. Every other thread waits for a read-write transaction, and observers
run before the transaction is released.

Shared types are handles, not values: a `TextRef` read in one transaction can be used in a later
one, but only for the document it came from. Their contents are only meaningful inside a
transaction.

### Servers

A server that holds many documents usually gives each one an owner, an actor that applies
updates and answers sync messages in order, rather than sharing a `Doc` between request
handlers. `yrs/tests/relay.rs` is a worked example: one actor thread per document, limits on
message and update size, eviction of idle documents, and a persistence hook.

## Unsafe code

`yrs` uses raw pointers for the item list: `ItemPtr` and `BranchPtr` (in `yrs::internals`). An item
has many aliases (its neighbours' `left` and `right`, its parent's `start` or `map`), and a
doubly linked list can't be expressed with borrows. Unsafe code must follow these rules:

1. **A pointer is only used under a transaction of its document.** Reads need a `Transaction`,
   writes a `TransactionMut`. The store lock is what keeps two threads from touching the same
   items.
2. **Writes go through pointers that came from the owner.** Items and branches are heap
   allocations owned by the block store or by their parent item. Pointers that will be written
   through are taken from that owner, not rebuilt from a `&Item` or `&Branch`.
3. **No reference outlives a call that may change the list.** Integrating or deleting an item
   writes through its neighbours' pointers, so code doesn't hold a `&mut Item` across such a call.
4. **Every `unsafe` block says why it is sound**, in a `// SAFETY:` comment.

The bindings add a rule of their own: a raw pointer handed to C or JavaScript is valid only while
its document lives, and only for use under a transaction. yffi's header documents this for each
function.
