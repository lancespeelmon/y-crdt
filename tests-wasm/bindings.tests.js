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
