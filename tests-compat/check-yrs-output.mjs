// Decodes what yrs encoded (written by `YRS_COMPAT_OUT=... cargo test --test yjs_compat`) with
// real Yjs, and checks the result.
//
//   node check-yrs-output.mjs <yrs-output.json>
//
// Each entry: { format: 1 | 2, update: hex, check: "roots", expect: { <root>: { type, value } } }
// where type is "text" | "array" | "map" | "xml" and value is the expected toJSON()/toString().
import * as Y from 'yjs'
import fs from 'node:fs'
import { isDeepStrictEqual } from 'node:util'

const path = process.argv[2]
if (!path) throw new Error('usage: node check-yrs-output.mjs <yrs-output.json>')
const entries = JSON.parse(fs.readFileSync(path, 'utf8'))

// Uint8Array values become plain number arrays so they compare with the JSON expectations.
const plain = (v) => JSON.parse(JSON.stringify(v, (_, x) => (x instanceof Uint8Array ? Array.from(x) : x)))

const root = (doc, name, type) => {
  switch (type) {
    case 'text': return doc.getText(name).toString()
    case 'array': return plain(doc.getArray(name).toJSON())
    case 'map': return plain(doc.getMap(name).toJSON())
    case 'xml': return doc.getXmlFragment(name).toString()
    default: throw new Error(`unknown root type ${type}`)
  }
}

let failures = 0
const names = Object.keys(entries)
for (const name of names) {
  const { format, update, check, expect } = entries[name]
  try {
    const doc = new Y.Doc()
    const bytes = Buffer.from(update, 'hex')
    if (format === 1) Y.applyUpdate(doc, bytes)
    else if (format === 2) Y.applyUpdateV2(doc, bytes)
    else throw new Error(`unknown format ${format}`)
    if (check !== 'roots') throw new Error(`unknown check ${check}`)
    for (const [rootName, { type, value }] of Object.entries(expect)) {
      const actual = root(doc, rootName, type)
      if (!isDeepStrictEqual(actual, value)) {
        throw new Error(`root ${rootName}: expected ${JSON.stringify(value)}, got ${JSON.stringify(actual)}`)
      }
    }
    console.log(`ok   ${name}`)
  } catch (e) {
    failures++
    console.log(`FAIL ${name}: ${e.message}`)
  }
}
console.log(`${names.length - failures}/${names.length} yrs outputs decoded correctly by yjs ${JSON.parse(fs.readFileSync(new URL('./node_modules/yjs/package.json', import.meta.url))).version}`)
if (names.length === 0 || failures > 0) process.exit(1)
