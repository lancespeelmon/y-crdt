//! Errors thrown by JS observers while a transaction commits.
//!
//! yrs calls observers synchronously inside the commit, where a JS exception can't unwind
//! through Rust. As in Yjs, every observer still runs and the transaction finishes; the first
//! error is then thrown from the ywasm call that committed. Calls that commit run inside
//! [committing], which collects the errors raised during it. Scopes nest, so a commit made from
//! inside an observer reports only its own observers' errors. An error with no scope to throw
//! from (a transaction committed by `free()` or the garbage collector) is reported through
//! `console.error`, as a browser reports an exception thrown by an event listener.

use std::cell::RefCell;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::JsValue;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = error)]
    fn console_error(value: &JsValue);
}

#[derive(Default)]
struct Scope {
    /// How many [committing] calls are running.
    depth: usize,
    /// The first error raised in the innermost of them.
    first: Option<JsValue>,
}

thread_local! {
    static SCOPE: RefCell<Scope> = RefCell::default();
}

/// Records the result of calling a JS observer: the first error of the current commit is kept
/// and thrown once it finishes.
pub(crate) fn observed(result: Result<JsValue, JsValue>) {
    if let Err(e) = result {
        let uncaught = SCOPE.with(|s| {
            let mut s = s.borrow_mut();
            if s.depth == 0 {
                Some(e)
            } else {
                s.first.get_or_insert(e);
                None
            }
        });
        if let Some(e) = uncaught {
            console_error(&e);
        }
    }
}

/// Runs `f`, which may commit a transaction, and throws the first error an observer raised
/// during it. An error `f` returns itself takes precedence, and the observer's error then goes
/// to `console.error`.
pub(crate) fn committing<T>(f: impl FnOnce() -> crate::Result<T>) -> crate::Result<T> {
    let outer = SCOPE.with(|s| {
        let mut s = s.borrow_mut();
        s.depth += 1;
        s.first.take()
    });
    // Restores the outer scope even if `f` unwinds.
    struct Restore(Option<JsValue>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let outer = self.0.take();
            SCOPE.with(|s| {
                let mut s = s.borrow_mut();
                s.depth -= 1;
                s.first = outer;
            });
        }
    }
    let restore = Restore(outer);
    let result = f();
    let mine = SCOPE.with(|s| s.borrow_mut().first.take());
    drop(restore);
    match (result, mine) {
        (Ok(_), Some(e)) => Err(e),
        // The call's own error is thrown; the observer's still gets reported.
        (Err(e), Some(observer)) => {
            console_error(&observer);
            Err(e)
        }
        (result, None) => result,
    }
}
