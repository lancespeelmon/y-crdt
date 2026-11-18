import * as Y from 'ywasm'
import * as t from 'lib0/testing'

/**
 * Non-string origins round-trip as the same value, however often they are read, and short
 * string origins stay strings.
 *
 * @param {t.TestCase} tc
 */
export const testOriginsRoundTrip = tc => {
    const doc = new Y.YDoc({clientID: 1})
    // map events convert the origin on every read (text events cache it)
    const map = doc.getMap('map')
    const seen = []
    map.observe((e) => {
        // read the origin twice: it used to be freed after the first read
        seen.push(e.origin)
        seen.push(e.origin)
    })
    const objectOrigin = {name: 'object origin'}
    // 'sync' is 4 bytes long, which used to be read back as a heap slot index
    const origins = [objectOrigin, 42, 'sync', 'longer string origin', null]
    let i = 0
    for (const origin of origins) {
        try {
            doc.transact(txn => map.set(`k${i++}`, 1, txn), origin)
        } catch (e) {
            // lib0's runner takes a thrown `null` (which the old code threw) for success
            throw new Error(`transaction with origin ${i - 1} threw ${e}`)
        }
    }
    t.compare(seen.length, origins.length * 2)
    for (let i = 0; i < origins.length; i++) {
        t.assert(seen[2 * i] === origins[i], `origin ${i} read once`)
        t.assert(seen[2 * i + 1] === origins[i], `origin ${i} read twice`)
    }
}

/**
 * @param {t.TestCase} tc
 */
export const testBigIntValues = tc => {
    const doc = new Y.YDoc({clientID: 1})
    const map = doc.getMap('map')
    map.set('big', 123n)
    t.compare(Number(map.get('big')), 123)
    let failed = false
    try {
        map.set('huge', 2n ** 80n)
    } catch (e) {
        failed = true
    }
    t.assert(failed, 'a BigInt that does not fit 64 bits is rejected')
}

/**
 * @param {t.TestCase} tc
 */
export const testUint8ArrayIsBinary = tc => {
    const doc = new Y.YDoc({clientID: 1})
    const map = doc.getMap('map')
    map.set('bin', new Uint8Array([1, 2, 3]))
    map.set('nested', [new Uint8Array([4, 5])])
    const bin = map.get('bin')
    t.assert(bin instanceof Uint8Array, 'stored as binary')
    t.compare(Array.from(bin), [1, 2, 3])
    t.compare(Array.from(map.get('nested')[0]), [4, 5])
}

/**
 * @param {t.TestCase} tc
 */
export const testWrongOrFreedObjectsAreRejected = tc => {
    const doc = new Y.YDoc({clientID: 1})
    const text = doc.getText('text')
    const other = doc.getMap('map')
    let failed = false
    try {
        // a ywasm object that isn't a transaction
        text.insert(0, 'x', undefined, other)
    } catch (e) {
        failed = true
    }
    t.assert(failed, 'a non-transaction object is rejected')

    const txn = doc.beginTransaction()
    txn.commit()
    txn.free()
    failed = false
    try {
        text.insert(0, 'x', undefined, txn)
    } catch (e) {
        failed = true
    }
    t.assert(failed, 'a freed transaction is rejected')
    t.compare(text.toString(), '')
}

/**
 * An UndoManager tracks object origins by identity, as Yjs does.
 *
 * @param {t.TestCase} tc
 */
export const testUndoManagerTracksObjectOrigins = tc => {
    const doc = new Y.YDoc({clientID: 1})
    const text = doc.getText('text')
    const tracked = {editor: 'mine'}
    const undoManager = new Y.YUndoManager({trackedOrigins: [tracked]})
    undoManager.addToScope([text])
    doc.transact(txn => text.insert(0, 'tracked', undefined, txn), tracked)
    doc.transact(txn => text.insert(0, 'other ', undefined, txn), {editor: 'someone else'})
    undoManager.undo()
    t.compareStrings(text.toString(), 'other ')
}

/**
 * An observer that throws no longer traps the module: every observer still runs, the change is
 * applied, and the call that committed throws the first error, as in Yjs.
 *
 * @param {t.TestCase} tc
 */
export const testThrowingObserversDontTrap = tc => {
    const doc = new Y.YDoc({clientID: 1})
    const text = doc.getText('text')
    const map = doc.getMap('map')
    let observed = 0
    let afterTransaction = 0
    text.observe(() => { throw new Error('observe') })
    text.observe(() => { observed++ })
    map.observeDeep(() => { throw new Error('observeDeep') })
    doc.on('afterTransaction', () => { afterTransaction++ })

    const before = afterTransaction
    t.fails(() => text.insert(0, 'abc'))
    t.assert(observed === 1, 'the other observer ran')
    t.assert(afterTransaction === before + 1, 'the transaction finished')
    t.compareStrings(text.toString(), 'abc')

    // the document stays usable, and the first error of a transaction is the one thrown
    let message = null
    try {
        doc.transact(txn => {
            text.insert(3, 'd', undefined, txn)
            map.set('k', 1, txn)
        })
    } catch (e) {
        message = e.message
    }
    t.compareStrings(message, 'observe')
    t.compareStrings(text.toString(), 'abcd')
    t.compare(map.get('k'), 1)

    // applying an update commits too
    const other = new Y.YDoc({clientID: 2})
    other.on('update', () => { throw new Error('update') })
    t.fails(() => Y.applyUpdate(other, Y.encodeStateAsUpdate(doc)))
    t.compareStrings(other.getText('text').toString(), 'abcd')
}

/**
 * A commit made from inside an observer reports its own observers' errors to that observer,
 * not to the outer call.
 *
 * @param {t.TestCase} tc
 */
export const testNestedCommitsReportTheirOwnObserverErrors = tc => {
    const a = new Y.YDoc({clientID: 1})
    const b = new Y.YDoc({clientID: 2})
    const textB = b.getText('text')
    textB.observe(() => { throw new Error('b') })
    let caught = null
    a.getText('text').observe(() => {
        try {
            textB.insert(0, 'x')
        } catch (e) {
            caught = e.message
        }
    })
    a.getText('text').insert(0, 'y')
    t.compareStrings(caught, 'b')
    t.compareStrings(textB.toString(), 'x')
}

/**
 * UndoManager observers that throw don't trap: undo applies and throws the error.
 *
 * @param {t.TestCase} tc
 */
export const testThrowingUndoObserver = tc => {
    const doc = new Y.YDoc({clientID: 1})
    const text = doc.getText('text')
    const undoManager = new Y.YUndoManager()
    undoManager.addToScope([text])
    text.insert(0, 'abc')
    undoManager.on('stack-item-popped', () => { throw new Error('popped') })
    t.fails(() => undoManager.undo())
    t.compareStrings(text.toString(), '')
    text.insert(0, 'z')
    t.compareStrings(text.toString(), 'z')
}

/**
 * Getting a root type, subscribing and adding to an undo scope run no observers, as in Yjs, so a
 * throwing `afterTransaction` handler doesn't make them throw.
 *
 * @param {t.TestCase} tc
 */
export const testBookkeepingCallsRunNoObservers = tc => {
    const doc = new Y.YDoc({clientID: 1})
    let after = 0
    doc.on('afterTransaction', () => {
        after++
        throw new Error('afterTransaction')
    })
    const text = doc.getText('new text')
    doc.getMap('new map')
    const onUpdate = () => {}
    doc.on('update', onUpdate)
    doc.off('update', onUpdate)
    const undoManager = new Y.YUndoManager()
    undoManager.addToScope([text])
    t.assert(after === 0, 'no observer ran')
    t.fails(() => text.insert(0, 'a'))
    t.assert(after === 1)
}
