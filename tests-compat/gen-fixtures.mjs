// Generates golden fixtures with real Yjs, for yrs/tests/yjs_compat.rs.
//
//   npm run gen        (writes ../yrs/tests/fixtures/yjs-13.json)
//
// Every fixture is produced by the pinned Yjs release in package-lock.json, with fixed client
// IDs, so the output is deterministic and CI can check that the committed file is up to date.
import * as Y from 'yjs'
import * as encoding from 'lib0/encoding'
import * as syncProtocol from 'y-protocols/sync'
import * as awarenessProtocol from 'y-protocols/awareness'
import fs from 'node:fs'
import { createRequire } from 'node:module'

const out = process.argv[2]
if (!out) throw new Error('usage: node gen-fixtures.mjs <output.json>')

const hex = (u) => Buffer.from(u).toString('hex')
const doc = (clientID, opts) => {
  const d = new Y.Doc(opts)
  d.clientID = clientID
  return d
}
const F = {}

// Binary content: Y.Array/Y.Map store Uint8Array values as ContentBinary.
{
  const d = doc(1)
  d.getArray('a').push([new Uint8Array([1, 2, 3])])
  d.getMap('m').set('bin', new Uint8Array([4, 5]))
  F.binary = {
    v1: hex(Y.encodeStateAsUpdate(d)),
    v2: hex(Y.encodeStateAsUpdateV2(d)),
    array: [[1, 2, 3]],
    map_bin: [4, 5],
  }
}

// ContentJSON: what Yjs <= 12 (and early 13) wrote for array/map values. Yjs 13 still reads and
// writes it; it is built here through Yjs' own Item/ContentJSON classes.
{
  const d = doc(2)
  const arr = d.getArray('a')
  d.transact((tr) => {
    const item = new Y.Item(
      Y.createID(2, 0), null, null, null, null, arr, null,
      new Y.ContentJSON([{ x: 1 }, 'str', undefined, 2.5])
    )
    item.integrate(tr, 0)
  })
  F.content_json = {
    v1: hex(Y.encodeStateAsUpdate(d)),
    v2: hex(Y.encodeStateAsUpdateV2(d)),
    json: JSON.stringify(arr.toJSON()),
    len: arr.length,
  }
}

// XmlHook carries a hook name after its type ref.
{
  const d = doc(3)
  const frag = d.getXmlFragment('f')
  const hook = new Y.XmlHook('custom-hook')
  const p = new Y.XmlElement('p')
  p.setAttribute('a', '1')
  p.setAttribute('b', '2')
  frag.insert(0, [hook, p])
  F.xml_hook = {
    v1: hex(Y.encodeStateAsUpdate(d)),
    v2: hex(Y.encodeStateAsUpdateV2(d)),
    hook_name: hook.hookName,
    element: p.toString(),
    fragment: frag.toString(),
  }
}

// A surrogate pair split across two items. V2 concatenates strings into one column before
// UTF-8 encoding, so the two lone halves become one 4-byte character there.
{
  const d = doc(4)
  const t = d.getText('t')
  t.insert(0, 'a\uD83D')
  t.insert(0, '\uDE00b')
  F.surrogate_split = {
    v1: hex(Y.encodeStateAsUpdate(d)),
    v2: hex(Y.encodeStateAsUpdateV2(d)),
    utf16_len: t.length,
    // Rust strings can't hold lone surrogates; each half reads as U+FFFD.
    text: t.toString().replace(/[\uD800-\uDFFF]/g, '�'),
  }
}

// State vectors and snapshots in both formats.
{
  const d = doc(5, { gc: false })
  const t = d.getText('t')
  t.insert(0, 'hello world')
  t.delete(2, 3)
  t.delete(7, 1)
  const u2 = Y.encodeStateAsUpdateV2(d)
  const s = Y.snapshot(d)
  F.state_vector = {
    update_v1: hex(Y.encodeStateAsUpdate(d)),
    update_v2: hex(u2),
    sv: hex(Y.encodeStateVector(d)),
    sv_from_update_v2: hex(Y.encodeStateVectorFromUpdateV2(u2)),
    diff_v2_empty_sv: hex(Y.diffUpdateV2(u2, new Uint8Array([0]))),
    snapshot_v1: hex(Y.encodeSnapshot(s)),
    snapshot_v2: hex(Y.encodeSnapshotV2(s)),
    text: t.toString(),
  }
}

// Subdocument options.
{
  const d = doc(6)
  const sub = new Y.Doc({ guid: 'sub-guid', meta: { title: 'x' }, autoLoad: false })
  d.getMap('m').set('sub', sub)
  const sub2 = new Y.Doc({ guid: 'sub-guid-2', autoLoad: true })
  d.getMap('m').set('sub2', sub2)
  F.subdoc = { v1: hex(Y.encodeStateAsUpdate(d)), v2: hex(Y.encodeStateAsUpdateV2(d)) }
}

// Relative positions.
{
  const d = doc(7)
  const t = d.getText('t')
  t.insert(0, 'abc')
  const legacy = encoding.createEncoder()
  encoding.writeVarUint(legacy, 0) // item form, written before assoc existed
  encoding.writeVarUint(legacy, 7)
  encoding.writeVarUint(legacy, 1)
  F.relative_position = {
    doc_v1: hex(Y.encodeStateAsUpdate(d)),
    end_after: hex(Y.encodeRelativePosition(Y.createRelativePositionFromTypeIndex(t, 3, 0))),
    mid_before: hex(Y.encodeRelativePosition(Y.createRelativePositionFromTypeIndex(t, 1, -1))),
    legacy_no_assoc: hex(encoding.toUint8Array(legacy)),
  }
}

// Any values of every lib0 type that yrs maps without loss.
{
  const d = doc(8)
  const m = d.getMap('m')
  m.set('i31', 2 ** 31 - 1)
  m.set('i31n', -(2 ** 31))
  m.set('i32', 2 ** 32)
  m.set('safe', 2 ** 53 - 1)
  m.set('f32', 1.5)
  m.set('f64', 0.1)
  m.set('null', null)
  m.set('str', 'héllo 😀')
  m.set('bool', true)
  m.set('arr', [1, 'x', null, true, { k: [] }])
  F.any = { v1: hex(Y.encodeStateAsUpdate(d)), v2: hex(Y.encodeStateAsUpdateV2(d)), json: JSON.stringify(m.toJSON()) }
}

// Three clients and a delete spanning all of them.
{
  const docs = [11, 12, 13].map((c) => doc(c))
  docs.forEach((d, i) => d.getText('t').insert(0, 'abc'[i].repeat(4)))
  for (const a of docs) for (const b of docs) if (a !== b) Y.applyUpdate(b, Y.encodeStateAsUpdate(a))
  docs[0].getText('t').delete(1, 10)
  F.multi_client = {
    v1: hex(Y.encodeStateAsUpdate(docs[0])),
    v2: hex(Y.encodeStateAsUpdateV2(docs[0])),
    text: docs[0].getText('t').toString(),
  }
}

// Incremental updates and their merge.
{
  const d = doc(14)
  const ups = []
  d.on('update', (u) => ups.push(u))
  const t = d.getText('t')
  t.insert(0, 'abc')
  t.insert(1, 'XY')
  t.delete(0, 2)
  t.format(0, 2, { bold: true })
  d.getMap('m').set('k', 'v')
  d.getMap('m').set('k', 'w')
  const ups2 = ups.map((u) => Y.convertUpdateFormatV1ToV2(u))
  F.merge = {
    inputs_v1: ups.map(hex),
    merged_v1: hex(Y.mergeUpdates(ups)),
    inputs_v2: ups2.map(hex),
    merged_v2: hex(Y.mergeUpdatesV2(ups2)),
    delta: JSON.stringify(t.toDelta()),
  }
}

// y-protocols framing.
{
  const d = doc(15)
  d.getText('t').insert(0, 'hi')
  const msg = (f) => {
    const e = encoding.createEncoder()
    encoding.writeVarUint(e, 0)
    f(e)
    return hex(encoding.toUint8Array(e))
  }
  const aw = new awarenessProtocol.Awareness(d)
  aw.setLocalState({ user: { name: 'a', n: 1.5 } })
  const awareness = hex(awarenessProtocol.encodeAwarenessUpdate(aw, [15]))
  aw.setLocalState(null)
  const awarenessNull = hex(awarenessProtocol.encodeAwarenessUpdate(aw, [15]))
  aw.destroy()
  F.protocol = {
    sync_step1: msg((e) => syncProtocol.writeSyncStep1(e, d)),
    sync_step2: msg((e) => syncProtocol.writeSyncStep2(e, d, new Uint8Array([0]))),
    sync_update: msg((e) => syncProtocol.writeUpdate(e, Y.encodeStateAsUpdate(d))),
    awareness,
    awareness_null: awarenessNull,
  }
}

const require = createRequire(import.meta.url)
const result = { yjs: require('yjs/package.json').version, fixtures: F }
fs.writeFileSync(out, JSON.stringify(result, null, 2) + '\n')
console.log(`wrote ${Object.keys(F).length} fixtures from yjs ${result.yjs} to ${out}`)
