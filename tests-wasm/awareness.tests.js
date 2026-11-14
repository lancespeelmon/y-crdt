import * as Y from 'ywasm'
import * as t from 'lib0/testing'

/**
 * @param {t.TestCase} tc
 */
export const testAwareness = tc => {
    const doc1 = new Y.YDoc({clientID: 0})
    const doc2 = new Y.YDoc({clientID: 1})
    const aw1 = new Y.Awareness(doc1)
    const aw2 = new Y.Awareness(doc2)
    aw1.on('update', /** @param {any} p */({added, updated, removed}) => {
        const enc = Y.encodeAwarenessUpdate(aw1, added.concat(updated).concat(removed))
        Y.applyAwarenessUpdate(aw2, enc, 'custom')
    })
    let lastChangeLocal = /** @type {any} */ (null)
    aw1.on('change', change => {
        lastChangeLocal = change
    })
    let lastChange = /** @type {any} */ (null)
    aw2.on('change', change => {
        lastChange = change
    })
    aw1.setLocalState({x: 3})
    t.assert(/** @type {any} */ (aw2.meta.get(0)).clock === 1)
    t.compare(lastChange.added, [0])
    // When creating an Awareness instance, the local client is already marked as available, so it is not updated.
    //t.compare(lastChangeLocal, {added: [], updated: [0], removed: []})

    // update state
    lastChange = null
    lastChangeLocal = null
    aw1.setLocalState({x: 4})
    t.compare(aw2.getStates().get(0), {x: 4})
    t.compare(lastChangeLocal, {added: [], updated: [0], removed: []})
    t.compare(lastChangeLocal, lastChange)

    lastChange = null
    lastChangeLocal = null
    aw1.setLocalState({x: 4})
    t.assert(lastChange === null)
    t.assert(/** @type {any} */ (aw2.meta.get(0)).clock === 3)
    t.compare(lastChangeLocal, lastChange)
    aw1.setLocalState(null)
    t.assert(lastChange.removed.length === 1)
    t.compare(aw1.getStates().get(0), undefined)
    t.compare(lastChangeLocal, lastChange)
}
/**
 * Handlers run after the change is complete, so they can read and change the awareness again.
 *
 * @param {t.TestCase} tc
 */
export const testAwarenessReentrantHandlers = tc => {
    const doc = new Y.YDoc({clientID: 7})
    const aw = new Y.Awareness(doc)
    const seen = /** @type {any[]} */ ([])
    aw.on('change', () => {
        seen.push(aw.getStates().get(7))
        const enc = Y.encodeAwarenessUpdate(aw, [7])
        t.assert(enc.length > 0)
        if (aw.getLocalState().x === 1) {
            aw.setLocalState({x: 2})
        }
    })
    aw.setLocalState({x: 1})
    t.compare(seen, [{x: 1}, {x: 2}])
    t.compare(aw.getLocalState(), {x: 2})

    const other = new Y.Awareness(new Y.YDoc({clientID: 8}))
    other.on('update', ({added, updated}) => {
        if (added.length + updated.length === 0) return
        t.compare(other.getStates().get(7), {x: 2})
        Y.removeAwarenessStates(other, added.concat(updated).map(BigInt))
    })
    Y.applyAwarenessUpdate(other, Y.encodeAwarenessUpdate(aw, [7]), 'test')
    t.assert(other.getStates().get(7) === undefined)
}

/**
 * A throwing handler doesn't stop the others, and the change it observed stays applied.
 *
 * @param {t.TestCase} tc
 */
export const testAwarenessThrowingHandler = tc => {
    const aw = new Y.Awareness(new Y.YDoc({clientID: 9}))
    let called = 0
    aw.on('change', () => { throw new Error('boom') })
    aw.on('update', () => { called++ })
    t.fails(() => aw.setLocalState({x: 1}))
    t.compare(aw.getLocalState(), {x: 1})
    t.assert(called === 1)
}
