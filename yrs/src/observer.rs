use crate::Origin;
use arc_swap::{ArcSwapOption, AsRaw};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Data structure used to handle publish/subscribe callbacks of a specific type.
///
/// Subscribing and unsubscribing take `&self`, so a callback may do either while it runs. A
/// callback subscribed while [Observer::trigger] runs is first called by the next trigger, as
/// with Yjs, which calls the listeners registered when an event started.
pub struct Observer<F> {
    callbacks: HashMap<Origin, ArcSwapOption<F>>,
    /// Subscriptions not yet moved into `callbacks`, which can't change while a trigger walks it.
    pending: Mutex<Vec<(Origin, F)>>,
    /// Whether `pending` may be non-empty, so [Observer::has_subscribers] (called on every
    /// commit) doesn't lock it when there is nothing to find.
    has_pending: AtomicBool,
}

impl<F> Observer<F> {
    /// Creates a new [Observer] with no active callbacks.
    pub fn new() -> Self {
        Observer {
            callbacks: HashMap::new(),
            pending: Mutex::new(Vec::new()),
            has_pending: AtomicBool::new(false),
        }
    }

    /// Returns `true` if this observer has any active subscribers.
    pub fn has_subscribers(&self) -> bool {
        !self.callbacks.is_empty()
            || (self.has_pending.load(Ordering::Acquire) && !self.pending().is_empty())
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, Vec<(Origin, F)>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Subscribes a callback under a given key. If a callback with the same key already
    /// exists, it will be replaced. The callback can be removed via [Observer::unsubscribe].
    pub fn subscribe(&self, id: impl Into<Origin>, callback: F) {
        let id = id.into();
        let mut pending = self.pending();
        pending.retain(|(other, _)| *other != id);
        pending.push((id, callback));
        self.has_pending.store(true, Ordering::Release);
    }

    /// Removes a callback by its key. Returns `true` if the callback was found and removed.
    pub fn unsubscribe(&self, id: &Origin) -> bool {
        let mut found = false;
        if let Some(e) = self.callbacks.get(id) {
            found = e.load().is_some();
            e.store(None);
        }
        let mut pending = self.pending();
        let before = pending.len();
        pending.retain(|(other, _)| other != id);
        found || pending.len() != before
    }

    /// Calls `each` for every registered callback, giving mutable access to the callback.
    pub fn trigger<E: FnMut(&mut F)>(&mut self, mut each: E) {
        self.drain_pending();
        for cell in self.callbacks.values_mut() {
            let guard = cell.load();
            if let Some(cb) = guard.as_ref() {
                // observer triggers requires &mut self and internal ArcSwap(Some) is assigned
                // only once, so it should never be at risk of multiple concurrent calls
                let callback = unsafe { cb.as_raw().as_mut().unwrap() };
                each(callback);
            }
        }
        self.drain_pending();
    }

    /// Drops unsubscribed callbacks and moves pending subscriptions in, replacing any callback
    /// under the same key.
    fn drain_pending(&mut self) {
        self.callbacks.retain(|_, v| v.load().is_some());
        if !*self.has_pending.get_mut() {
            return;
        }
        let pending = std::mem::take(&mut *self.pending());
        *self.has_pending.get_mut() = false;
        for (id, callback) in pending {
            self.callbacks
                .insert(id, ArcSwapOption::new(Some(Arc::new(callback))));
        }
    }
}

impl<F> Default for Observer<F> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod test {
    use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
    use std::sync::mpsc::channel;
    use std::sync::Arc;

    use crate::observer::Observer;
    use crate::Origin;

    struct Cb(Box<dyn FnMut(*mut Observer<Cb>) + Send + Sync + 'static>);

    fn trigger(o: &mut Observer<Cb>) {
        let ptr: *mut Observer<Cb> = o;
        o.trigger(|cb| (cb.0)(ptr));
    }

    #[test]
    fn subscribe_and_unsubscribe() {
        let mut o: Observer<Box<dyn FnMut(&u32) + Send + Sync + 'static>> = Observer::new();
        let s1_state = Arc::new(AtomicU32::new(0));
        let s2_state = Arc::new(AtomicU32::new(0));

        let a = s1_state.clone();
        let b = s2_state.clone();
        o.subscribe(
            1usize,
            Box::new(move |&value| {
                a.store(value, Ordering::Release);
            }),
        );
        o.subscribe(
            2usize,
            Box::new(move |&value| {
                b.store(value * 2, Ordering::Release);
            }),
        );

        o.trigger(|fun| fun(&1));
        assert_eq!(s1_state.load(Ordering::Acquire), 1);
        assert_eq!(s2_state.load(Ordering::Acquire), 2);

        o.trigger(|fun| fun(&2));
        assert_eq!(s1_state.load(Ordering::Acquire), 2);
        assert_eq!(s2_state.load(Ordering::Acquire), 4);

        assert!(o.unsubscribe(&Origin::from(1usize)));
        assert!(o.unsubscribe(&Origin::from(2usize)));

        // callbacks were unsubscribed, they are no longer called
        o.trigger(|fun| fun(&3));
        assert_eq!(s1_state.load(Ordering::Acquire), 2);
        assert_eq!(s2_state.load(Ordering::Acquire), 4);
    }

    #[test]
    fn subscribers_predicate() {
        let mut o: Observer<Box<dyn FnMut(&u32) + Send + Sync + 'static>> = Observer::new();
        assert!(!o.has_subscribers());

        o.subscribe(1usize, Box::new(move |_| {}));
        assert!(o.has_subscribers());

        o.unsubscribe(&Origin::from(1usize));
        o.drain_pending();
        assert!(!o.has_subscribers());
    }

    #[test]
    fn subscribe_replaces_previous() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut o: Observer<Box<dyn FnMut(u32) + Send + Sync + 'static>> = Observer::new();
        let ta = tx.clone();
        o.subscribe(1, Box::new(move |i| ta.send(10 + i).unwrap()));
        o.trigger(|fun| fun(1));
        assert_eq!(rx.try_recv().unwrap(), 11);

        // override the callback with the same key
        o.subscribe(1, Box::new(move |i| tx.send(100 + i).unwrap()));
        o.trigger(|fun| fun(2));
        assert_eq!(rx.try_recv().unwrap(), 102);
    }

    #[test]
    fn subscribe_during_trigger_replaces_callback() {
        let (tx, rx) = channel();
        let mut o: Observer<Cb> = Observer::new();
        let tb = tx.clone();
        o.subscribe(
            1usize,
            Cb(Box::new(move |o| {
                tx.send("a").unwrap();
                let tb = tb.clone();
                let replacement = Cb(Box::new(move |_| tb.send("b").unwrap()));
                unsafe { (*o).subscribe(1usize, replacement) };
            })),
        );

        trigger(&mut o);
        assert_eq!(rx.try_recv().unwrap(), "a");
        assert!(
            rx.try_recv().is_err(),
            "replacement called within the same trigger"
        );

        trigger(&mut o);
        assert_eq!(rx.try_recv().unwrap(), "b");
        assert!(rx.try_recv().is_err(), "replaced callback was still called");
    }

    #[test]
    fn subscribing_during_a_trigger_takes_effect_from_the_next_one() {
        let calls = Arc::new(AtomicU32::new(0));
        let mut o: Observer<Cb> = Observer::new();
        let outer = calls.clone();
        o.subscribe(
            1usize,
            Cb(Box::new(move |o| {
                outer.fetch_add(1, Ordering::SeqCst);
                let inner = outer.clone();
                // Subscribing from inside a callback is fine; the new one waits for the next event.
                unsafe { &*o }.subscribe(
                    2usize,
                    Cb(Box::new(move |_| {
                        inner.fetch_add(100, Ordering::SeqCst);
                    })),
                );
            })),
        );
        trigger(&mut o);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        trigger(&mut o);
        assert_eq!(calls.load(Ordering::SeqCst), 1 + 1 + 100);
    }

    #[test]
    fn unsubscribe_during_trigger() {
        let calls = Arc::new(AtomicU32::new(0));
        let mut o: Observer<Cb> = Observer::new();
        // each callback unsubscribes the other one
        for (me, other) in [(1usize, 2usize), (2, 1)] {
            let calls = calls.clone();
            o.subscribe(
                me,
                Cb(Box::new(move |o| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    unsafe { (&*o).unsubscribe(&Origin::from(other)) };
                })),
            );
        }

        trigger(&mut o);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "unsubscribed callback was called"
        );
        assert!(o.has_subscribers());
        trigger(&mut o);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    struct DropCounter(Arc<AtomicI32>);

    impl DropCounter {
        fn new(counter: Arc<AtomicI32>) -> Self {
            counter.fetch_add(1, Ordering::SeqCst);
            DropCounter(counter)
        }
    }

    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn unsubscribe() {
        let counter = Arc::new(AtomicI32::new(0));
        let mut o: Observer<DropCounter> = Observer::new();
        for i in 0..100 {
            assert_eq!(counter.load(Ordering::SeqCst), 0);

            o.subscribe(i, DropCounter::new(counter.clone()));

            assert_eq!(counter.load(Ordering::SeqCst), 1);

            let unsubscribed = o.unsubscribe(&Origin::from(i));
            assert!(unsubscribed, "unsubscribe failed for {}", i);
        }
    }

    #[test]
    fn unsubscribe2() {
        let counter = Arc::new(AtomicI32::new(0));
        let mut o: Observer<DropCounter> = Observer::new();
        for i in 0..100 {
            o.subscribe(i, DropCounter::new(counter.clone()));
        }

        assert_eq!(counter.load(Ordering::SeqCst), 100);

        for i in 0..100 {
            let unsubscribed = o.unsubscribe(&Origin::from(i));
            assert!(unsubscribed, "unsubscribe failed for {}", i);
        }
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }
}
