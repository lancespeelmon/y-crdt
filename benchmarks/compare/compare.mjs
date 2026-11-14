// Runs the same crdt-benchmarks workloads against Yjs, ywasm and, when installed, Loro and
// Automerge in one Node process and prints a table. Plan item 6.9; see benchmarks/COMPARISON.md
// for how to run it and the last results.
//
//   node benchmarks/compare/compare.mjs <dir with node_modules/yjs> [runs]
//
// ywasm is loaded from ywasm/pkg (build it with `wasm-pack build --release --target nodejs ./ywasm`).
// Loro (`loro-crdt`) and Automerge (`@automerge/automerge`) are optional: each is compared when
// it is installed in the same directory as Yjs.

import { createRequire } from 'node:module'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const here = path.dirname(fileURLToPath(import.meta.url))
const root = path.resolve(here, '../..')
const yjsDir = process.argv[2]
if (!yjsDir) {
  console.error('usage: node compare.mjs <dir with node_modules/yjs> [runs]')
  process.exit(2)
}
const runs = Number(process.argv[3] ?? 5)
const yjsRequire = createRequire(path.join(path.resolve(yjsDir), 'index.js'))
const Y = yjsRequire('yjs')
// Some packages don't export their package.json, so the version is read from the file.
const versionOf = (name) =>
  JSON.parse(readFileSync(path.resolve(yjsDir, 'node_modules', name, 'package.json'), 'utf8')).version
const optional = (name) => {
  let lib
  try {
    lib = yjsRequire(name)
  } catch (e) {
    if (e.code === 'MODULE_NOT_FOUND') return null
    throw e
  }
  return { lib, version: versionOf(name) }
}
const yjsVersion = versionOf('yjs')
const loro = optional('loro-crdt')
const automerge = optional('@automerge/automerge')
const W = createRequire(import.meta.url)(path.join(root, 'ywasm/pkg/ywasm.js'))

// The B4 editing trace, in the format yrs/benches/benches.rs reads: a var-uint count, then
// ops tagged 1 (insert: index, string) or 2 (delete: index, length).
function readTrace (file) {
  const buf = readFileSync(file)
  let pos = 0
  const varUint = () => {
    let num = 0; let mult = 1
    for (;;) {
      const b = buf[pos++]
      num += (b & 0x7f) * mult
      if (b < 0x80) return num
      mult *= 128
    }
  }
  const string = () => {
    const len = varUint()
    const s = buf.toString('utf8', pos, pos + len)
    pos += len
    return s
  }
  const n = varUint()
  const ops = new Array(n)
  for (let i = 0; i < n; i++) {
    const tag = varUint()
    ops[i] = tag === 1 ? [1, varUint(), string()] : [2, varUint(), varUint()]
  }
  return ops
}

const trace = readTrace(path.join(root, 'assets/bench-input/b4-editing-trace.bin'))
const b4Update = new Uint8Array(readFileSync(path.join(root, 'assets/bench-input/b4-update.bin')))

// A seeded generator, so both libraries get the same random positions.
function rng (seed) {
  let s = seed >>> 0
  return () => { s ^= s << 13; s >>>= 0; s ^= s >>> 17; s ^= s << 5; s >>>= 0; return s / 4294967296 }
}

// Each adapter makes every edit its own transaction (a Loro commit, an Automerge change).
// `load` reads the B4 document in the library's own format: the Yjs update for Yjs and ywasm,
// and for the others their own snapshot of the document B4.1 produces (see `snapshot`).
const libs = {
  yjs: {
    label: 'Yjs',
    doc: () => new Y.Doc(),
    text: d => d.getText('text'),
    insert: (d, t, i, s) => d.transact(() => t.insert(i, s)),
    remove: (d, t, i, n) => d.transact(() => t.delete(i, n)),
    load: () => { const d = new Y.Doc(); Y.applyUpdate(d, b4Update); return d.getText('text').toString() },
    str: t => t.toString()
  },
  ywasm: {
    label: 'ywasm',
    doc: () => new W.YDoc({}),
    text: d => d.getText('text'),
    insert: (d, t, i, s) => t.insert(i, s),
    remove: (d, t, i, n) => t.delete(i, n),
    load: () => { const d = new W.YDoc({}); W.applyUpdate(d, b4Update); return d.getText('text').toString() },
    str: t => t.toString()
  }
}

if (loro) {
  const { LoroDoc } = loro.lib
  libs.loro = {
    label: 'Loro',
    doc: () => new LoroDoc(),
    text: d => d.getText('text'),
    // Loro's JS text indices are UTF-16 code units, as in Yjs.
    insert: (d, t, i, s) => { t.insert(i, s); d.commit() },
    remove: (d, t, i, n) => { t.delete(i, n); d.commit() },
    snapshot: d => d.export({ mode: 'snapshot' }),
    load: (bytes) => { const d = new LoroDoc(); d.import(bytes); return d.getText('text').toString() },
    str: t => t.toString()
  }
}

if (automerge) {
  const A = automerge.lib
  // Automerge documents are immutable values, so the adapter keeps the current one in a box.
  libs.automerge = {
    label: 'Automerge',
    doc: () => ({ doc: A.from({ text: '' }) }),
    text: d => d,
    insert: (d, _t, i, s) => { d.doc = A.change(d.doc, x => A.splice(x, ['text'], i, 0, s)) },
    remove: (d, _t, i, n) => { d.doc = A.change(d.doc, x => A.splice(x, ['text'], i, n)) },
    snapshot: d => A.save(d.doc),
    load: (bytes) => A.load(bytes).text,
    str: d => d.doc.text
  }
}

// The workloads below return the final text, so every library can be checked against Yjs.
function applyTrace (L) {
  const d = L.doc(); const t = L.text(d)
  for (const op of trace) {
    if (op[0] === 1) L.insert(d, t, op[1], op[2])
    else L.remove(d, t, op[1], op[2])
  }
  return { d, t }
}

// The libraries that load their own snapshot get it from one run of the editing trace.
const snapshots = {}
for (const [name, L] of Object.entries(libs)) {
  if (L.snapshot) snapshots[name] = L.snapshot(applyTrace(L).d)
}

const N = 6000
const workloads = {
  'B1.1 append N chars': (L) => {
    const d = L.doc(); const t = L.text(d)
    for (let i = 0; i < N; i++) L.insert(d, t, i, 'a')
    return L.str(t)
  },
  'B1.4 insert N chars at random positions': (L) => {
    const d = L.doc(); const t = L.text(d); const r = rng(1)
    // A different character each time, so a wrong position shows up in the final text.
    for (let i = 0; i < N; i++) L.insert(d, t, Math.floor(r() * (i + 1)), String.fromCharCode(97 + (i % 26)))
    return L.str(t)
  },
  'B4.1 apply the editing trace': (L) => L.str(applyTrace(L).t),
  'B4.2 load the document': (L, name) => L.load(snapshots[name])
}

function median (xs) {
  const s = [...xs].sort((a, b) => a - b)
  return s[Math.floor(s.length / 2)]
}

const rows = []
for (const [name, run] of Object.entries(workloads)) {
  const row = { name }
  const texts = {}
  for (const [lib, L] of Object.entries(libs)) {
    run(L, lib) // warm up
    const times = []
    for (let i = 0; i < runs; i++) {
      const start = performance.now()
      texts[lib] = run(L, lib)
      times.push(performance.now() - start)
    }
    row[lib] = median(times)
    if (texts[lib] !== texts.yjs) throw new Error(`${name}: ${lib}'s text differs from Yjs's`)
  }
  rows.push(row)
}

const names = Object.keys(libs)
const versions = [`Yjs ${yjsVersion}`]
if (loro) versions.push(`Loro ${loro.version}`)
if (automerge) versions.push(`Automerge ${automerge.version}`)
console.log(`Node ${process.version}, ${versions.join(', ')}, median of ${runs} runs\n`)
console.log(`| workload | ${names.map(n => libs[n].label).join(' | ')} |`)
console.log(`|---|${names.map(() => '---').join('|')}|`)
for (const r of rows) {
  const cells = names.map(n => n === 'yjs'
    ? `${r[n].toFixed(2)} ms`
    : `${r[n].toFixed(2)} ms (${(r[n] / r.yjs).toFixed(2)}×)`)
  console.log(`| ${r.name} | ${cells.join(' | ')} |`)
}
