// Runs the same crdt-benchmarks workloads against Yjs and ywasm in one Node process and prints
// a table. Plan item 6.9; see benchmarks/COMPARISON.md for how to run it and the last results.
//
//   node benchmarks/compare/compare.mjs <dir with node_modules/yjs> [runs]
//
// ywasm is loaded from ywasm/pkg (build it with `wasm-pack build --release --target nodejs ./ywasm`).

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
const yjsVersion = yjsRequire('yjs/package.json').version
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

const libs = {
  yjs: {
    doc: () => new Y.Doc(),
    text: d => d.getText('text'),
    insert: (d, t, i, s) => d.transact(() => t.insert(i, s)),
    remove: (d, t, i, n) => d.transact(() => t.delete(i, n)),
    apply: (d, u) => Y.applyUpdate(d, u),
    str: t => t.toString()
  },
  ywasm: {
    doc: () => new W.YDoc({}),
    text: d => d.getText('text'),
    insert: (d, t, i, s) => t.insert(i, s),
    remove: (d, t, i, n) => t.delete(i, n),
    apply: (d, u) => W.applyUpdate(d, u),
    str: t => t.toString()
  }
}

const N = 6000
const workloads = {
  'B1.1 append N chars': (L) => {
    const d = L.doc(); const t = L.text(d)
    for (let i = 0; i < N; i++) L.insert(d, t, i, 'a')
    return L.str(t).length
  },
  'B1.4 insert N chars at random positions': (L) => {
    const d = L.doc(); const t = L.text(d); const r = rng(1)
    for (let i = 0; i < N; i++) L.insert(d, t, Math.floor(r() * (i + 1)), 'a')
    return L.str(t).length
  },
  'B4.1 apply the editing trace': (L) => {
    const d = L.doc(); const t = L.text(d)
    for (const op of trace) {
      if (op[0] === 1) L.insert(d, t, op[1], op[2])
      else L.remove(d, t, op[1], op[2])
    }
    return L.str(t).length
  },
  'B4.2 load the trace as one update': (L) => {
    const d = L.doc(); const t = L.text(d)
    L.apply(d, b4Update)
    return L.str(t).length
  }
}

function median (xs) {
  const s = [...xs].sort((a, b) => a - b)
  return s[Math.floor(s.length / 2)]
}

const rows = []
for (const [name, run] of Object.entries(workloads)) {
  const row = { name }
  const lens = {}
  for (const [lib, L] of Object.entries(libs)) {
    run(L) // warm up
    const times = []
    for (let i = 0; i < runs; i++) {
      const start = performance.now()
      lens[lib] = run(L)
      times.push(performance.now() - start)
    }
    row[lib] = median(times)
  }
  if (lens.yjs !== lens.ywasm) throw new Error(`${name}: texts differ (${lens.yjs} vs ${lens.ywasm})`)
  rows.push(row)
}

console.log(`Node ${process.version}, Yjs ${yjsVersion}, median of ${runs} runs\n`)
console.log('| workload | Yjs | ywasm | ywasm / Yjs |')
console.log('|---|---|---|---|')
for (const r of rows) {
  console.log(`| ${r.name} | ${r.yjs.toFixed(2)} ms | ${r.ywasm.toFixed(2)} ms | ${(r.ywasm / r.yjs).toFixed(2)} |`)
}
