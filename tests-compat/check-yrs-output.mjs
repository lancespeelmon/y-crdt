// Decodes what yrs encoded (written by `YRS_COMPAT_OUT=... cargo test --test yjs_compat`) with
// real Yjs, and checks the result.
//
//   node check-yrs-output.mjs <yrs-output.json>
//
// Each entry: { format: 1 | 2, update: hex, check, expect } with check one of
//   "roots":   expect = { <root>: { type, value } }, type "text" | "array" | "map" | "xml", value the
//              expected toJSON()/toString();
//   "subdocs": expect = { <map root>: { <key>: { guid, autoLoad, shouldLoad, meta } } }.
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
    const compare = (what, actual, value) => {
      if (!isDeepStrictEqual(actual, value)) {
        throw new Error(`${what}: expected ${JSON.stringify(value)}, got ${JSON.stringify(actual)}`)
      }
    }
    if (check === 'roots') {
      for (const [rootName, { type, value }] of Object.entries(expect)) {
        compare(`root ${rootName}`, root(doc, rootName, type), value)
      }
    } else if (check === 'subdocs') {
      for (const [rootName, keys] of Object.entries(expect)) {
        for (const [key, value] of Object.entries(keys)) {
          const sub = doc.getMap(rootName).get(key)
          if (!(sub instanceof Y.Doc)) throw new Error(`${rootName}.${key} is not a subdocument`)
          const { guid, autoLoad, shouldLoad, meta } = sub
          compare(`${rootName}.${key}`, { guid, autoLoad, shouldLoad, meta }, value)
        }
      }
    } else {
      throw new Error(`unknown check ${check}`)
    }
    console.log(`ok   ${name}`)
  } catch (e) {
    failures++
    console.log(`FAIL ${name}: ${e.message}`)
  }
}
console.log(`${names.length - failures}/${names.length} yrs outputs decoded correctly by yjs ${JSON.parse(fs.readFileSync(new URL('./node_modules/yjs/package.json', import.meta.url))).version}`)
if (names.length === 0 || failures > 0) process.exit(1)
