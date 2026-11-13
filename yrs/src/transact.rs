use crate::{Doc, Origin, Store, Transaction, TransactionMut};
use async_lock::futures::{Read, Write};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use thiserror::Error;

/// Trait implemented by [Doc] and shared types, used for carrying over the responsibilities of
/// creating new transactions, used as a unit of work in Yrs.
pub trait Transact {
    /// Creates and returns a lightweight read-only transaction.
    ///
    /// # Errors
    ///
    /// While it's possible to have multiple read-only transactions active at the same time,
    /// this method will return a [TransactionAcqError::SharedAcqFailed] error whenever called
    /// while a read-write transaction (see: [Self::try_transact_mut]) is active at the same time.
    fn try_transact(&self) -> Result<Transaction<'_>, TransactionAcqError>;

    /// Creates and returns a read-write capable transaction. This transaction can be used to
    /// mutate the contents of underlying document store and upon dropping or committing it may
    /// subscription callbacks.
    ///
    /// # Errors
    ///
    /// Only one read-write transaction can be active at the same time. If any other transaction -
    /// be it a read-write or read-only one - is active at the same time, this method will return
    /// a [TransactionAcqError::ExclusiveAcqFailed] error.
    fn try_transact_mut(&self) -> Result<TransactionMut<'_>, TransactionAcqError>;

    /// Creates and returns a read-write capable transaction with an `origin` classifier attached.
    /// This transaction can be used to mutate the contents of underlying document store and upon
    /// dropping or committing it may subscription callbacks.
    ///
    /// An `origin` may be used to identify context of operations made (example updates performed
    /// locally vs. incoming from remote replicas) and it's used i.e. by [`UndoManager`][crate::undo::UndoManager].
    ///
    /// # Errors
    ///
    /// Only one read-write transaction can be active at the same time. If any other transaction -
    /// be it a read-write or read-only one - is active at the same time, this method will return
    /// a [TransactionAcqError::ExclusiveAcqFailed] error.
    fn try_transact_mut_with<T>(
        &self,
        origin: T,
    ) -> Result<TransactionMut<'_>, TransactionAcqError>
    where
        T: Into<Origin>;

    /// Creates and returns a read-write capable transaction with an `origin` classifier attached.
    /// This transaction can be used to mutate the contents of underlying document store and upon
    /// dropping or committing it may subscription callbacks.
    ///
    /// An `origin` may be used to identify context of operations made (example updates performed
    /// locally vs. incoming from remote replicas) and it's used i.e. by [`UndoManager`][crate::undo::UndoManager].
    ///
    /// # Errors
    ///
    /// Only one read-write transaction can be active at the same time. If any other transaction -
    /// be it a read-write or read-only one - is active at the same time, this method will
    /// block the thread until exclusive access can be acquired, unless building for wasm
    /// which panics. If blocking is undesirable, use `try_transact_mut_with(origin).unwrap()` instead`.
    fn transact_mut_with<T>(&self, origin: T) -> TransactionMut<'_>
    where
        T: Into<Origin>;

    /// Creates and returns a lightweight read-only transaction.
    ///
    /// # Errors
    ///
    /// While it's possible to have multiple read-only transactions active at the same time,
    /// this method will block the thread until exclusive access can be acquired, unless building for wasm
    /// which panics. If blocking is undesirable, use `try_transact(origin).unwrap()` instead`.
    fn transact(&self) -> Transaction<'_>;

    /// Creates and returns a read-write capable transaction. This transaction can be used to
    /// mutate the contents of underlying document store and upon dropping or committing it may
    /// subscription callbacks.
    ///
    /// # Errors
    ///
    /// Only one read-write transaction can be active at the same time. If any other transaction -
    /// be it a read-write or read-only one - is active at the same time, this method will block
    /// the thread until exclusive access can be acquired, unless building for wasm
    /// which panics. If blocking is undesirable, use `try_transact_mut(origin).unwrap()` instead`.
    fn transact_mut(&self) -> TransactionMut<'_>;
}

impl Transact for Doc {
    fn try_transact(&self) -> Result<Transaction<'_>, TransactionAcqError> {
        self.store.check_reentrant(false)?;
        match self.store.try_read() {
            Some(store) => Ok(Transaction::new(store).held(self.store.hold(false))),
            None => Err(TransactionAcqError::SharedAcqFailed),
        }
    }

    fn try_transact_mut(&self) -> Result<TransactionMut<'_>, TransactionAcqError> {
        self.try_write(None)
    }

    fn try_transact_mut_with<T>(&self, origin: T) -> Result<TransactionMut<'_>, TransactionAcqError>
    where
        T: Into<Origin>,
    {
        self.try_write(Some(origin.into()))
    }

    fn transact_mut_with<T>(&self, origin: T) -> TransactionMut<'_>
    where
        T: Into<Origin>,
    {
        self.write_blocking(Some(origin.into()))
    }

    fn transact(&self) -> Transaction<'_> {
        refuse_deadlock(self.store.check_reentrant(false));
        let lock = self.store.read_blocking();
        Transaction::new(lock).held(self.store.hold(false))
    }

    fn transact_mut(&self) -> TransactionMut<'_> {
        self.write_blocking(None)
    }
}

impl Doc {
    fn try_write(&self, origin: Option<Origin>) -> Result<TransactionMut<'_>, TransactionAcqError> {
        self.store.check_reentrant(true)?;
        match self.store.try_write() {
            Some(store) => {
                let mut txn = TransactionMut::new(self.clone(), store, origin);
                txn.held = Some(self.store.hold(true));
                Ok(txn)
            }
            None => Err(TransactionAcqError::ExclusiveAcqFailed),
        }
    }

    fn write_blocking(&self, origin: Option<Origin>) -> TransactionMut<'_> {
        refuse_deadlock(self.store.check_reentrant(true));
        let lock = self.store.write_blocking();
        let mut txn = TransactionMut::new(self.clone(), lock, origin);
        txn.held = Some(self.store.hold(true));
        txn
    }
}

/// The blocking `transact*` methods can't return an error, and waiting would never end.
fn refuse_deadlock(check: Result<(), TransactionAcqError>) {
    if let Err(e) = check {
        panic!(
            "{} Opening a transaction inside an observer callback? Use the transaction the \
             callback is given, or the try_transact* methods.",
            e
        );
    }
}

/// Trait implemented by [Doc] and shared types, used for carrying over the responsibilities of
/// creating new transactions, used as a unit of work in Yrs.
pub trait AsyncTransact<'doc> {
    /// The future that resolves to a read-only [Transaction].
    type Read: Future<Output = Transaction<'doc>>;
    /// The future that resolves to a read-write [TransactionMut].
    type Write: Future<Output = TransactionMut<'doc>>;

    /// Returns a future that resolves to a read-only transaction once it can be acquired.
    fn transact(&'doc self) -> Self::Read;
    /// Returns a future that resolves to a read-write transaction once it can be acquired.
    fn transact_mut(&'doc self) -> Self::Write;

    /// Creates and returns a read-write capable transaction with an `origin` classifier attached.
    /// This transaction can be used to mutate the contents of underlying document store and upon
    /// dropping or committing it may subscription callbacks.
    ///
    /// An `origin` may be used to identify context of operations made (example updates performed
    /// locally vs. incoming from remote replicas) and it's used i.e. by [`UndoManager`][crate::undo::UndoManager].
    fn transact_mut_with<T>(&'doc self, origin: T) -> Self::Write
    where
        T: Into<Origin>;
}

impl<'doc> AsyncTransact<'doc> for Doc {
    type Read = AcquireTransaction<'doc>;
    type Write = AcquireTransactionMut<'doc>;

    fn transact(&'doc self) -> Self::Read {
        let fut = self.store.read_async();
        AcquireTransaction { fut }
    }

    fn transact_mut(&'doc self) -> Self::Write {
        let fut = self.store.write_async();
        AcquireTransactionMut {
            doc: self.clone(),
            origin: None,
            fut,
        }
    }

    fn transact_mut_with<T>(&'doc self, origin: T) -> Self::Write
    where
        T: Into<Origin>,
    {
        let fut = self.store.write_async();
        AcquireTransactionMut {
            doc: self.clone(),
            origin: Some(origin.into()),
            fut,
        }
    }
}

/// A future that resolves to a [Transaction] when the document's lock is available. Created by [AsyncTransact::transact].
pub struct AcquireTransaction<'doc> {
    fut: Read<'doc, Store>,
}

impl<'doc> Unpin for AcquireTransaction<'doc> {}

impl<'doc> Future for AcquireTransaction<'doc> {
    type Output = Transaction<'doc>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let pinned = unsafe { Pin::new_unchecked(&mut self.fut) };
        pinned.poll(cx).map(Transaction::new)
    }
}

/// A future that resolves to a [TransactionMut] when the document's lock is available. Created by [AsyncTransact::transact_mut] and [AsyncTransact::transact_mut_with].
pub struct AcquireTransactionMut<'doc> {
    doc: Doc,
    origin: Option<Origin>,
    fut: Write<'doc, Store>,
}

impl<'doc> Unpin for AcquireTransactionMut<'doc> {}

impl<'doc> Future for AcquireTransactionMut<'doc> {
    type Output = TransactionMut<'doc>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let pinned = unsafe { Pin::new_unchecked(&mut self.fut) };
        match pinned.poll(cx) {
            Poll::Ready(store) => {
                let doc = self.doc.clone();
                let origin = self.origin.take();
                Poll::Ready(TransactionMut::new(doc, store, origin))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// An error returned when a transaction couldn't be acquired.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum TransactionAcqError {
    /// A read-only transaction couldn't be acquired, because a read-write transaction is active.
    #[error("Failed to acquire read-only transaction. Drop read-write transaction and retry.")]
    SharedAcqFailed,
    /// A read-write transaction couldn't be acquired, because other transactions are active.
    #[error("Failed to acquire read-write transaction. Drop other transactions and retry.")]
    ExclusiveAcqFailed,
    /// The parent document that contains this collection has been dropped.
    #[error("All references to a parent document containing this structure has been dropped.")]
    DocumentDropped,
    /// This thread already holds a transaction on the document that the new one would wait on
    /// forever: any transaction for a read-write one, or a read-write one for a read-only one.
    /// Typically a transaction opened inside an observer callback. Use the transaction the
    /// callback is given instead.
    #[error("This thread already holds a transaction on this document; a new one would deadlock.")]
    Reentrant,
}

#[cfg(test)]
mod test {
    use crate::{Doc, GetString, Text, Transact};
    use std::sync::{Arc, Barrier};
    use std::time::{Duration, Instant};

    #[test]
    fn multi_thread_transact_mut() {
        let doc = Doc::new();
        let txt = doc.get_or_insert_text("text");

        const N: usize = 3;
        let barrier = Arc::new(Barrier::new(N + 1));

        let start = Instant::now();
        for _ in 0..N {
            let d = doc.clone();
            let t = txt.clone();
            let b = barrier.clone();
            std::thread::spawn(move || {
                // let mut txn = d.try_transact_mut().unwrap(); // this will hang forever
                let mut txn = d.transact_mut();
                let n = fastrand::u64(0..5);
                std::thread::sleep(Duration::from_millis(n * 100));
                t.insert(&mut txn, 0, "a");
                drop(txn);
                b.wait();
            });
        }

        barrier.wait();
        println!("{} threads executed in {:?}", N, Instant::now() - start);

        let expected: String = (0..N).map(|_| 'a').collect();
        let txn = doc.transact();
        let str = txt.get_string(&txn);
        assert_eq!(str, expected);
    }
}
