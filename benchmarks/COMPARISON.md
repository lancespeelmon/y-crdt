# Comparison with other CRDT libraries

Plan item 6.9. Two harnesses run crdt-benchmarks workloads on the same inputs and check that
every library ends with the same text:

- `benchmarks/compare/compare.mjs`: Yjs, ywasm, Loro and Automerge in one Node process.
- `benchmarks/compare-native`: native yrs and y-octo, which has no Node package, in one Rust
  binary. It is a standalone crate, so y-octo stays out of the main workspace's dependencies.

## Running it

The Node harness doesn't install the JS libraries; point it at a directory where they are
installed. Yjs is required; Loro (`loro-crdt`) and Automerge (`@automerge/automerge`) are
compared when present:

```sh
wasm-pack build --release --target nodejs ./ywasm
mkdir -p /tmp/crdts && (cd /tmp/crdts && npm init -y >/dev/null &&
  npm install --ignore-scripts yjs loro-crdt @automerge/automerge)
node benchmarks/compare/compare.mjs /tmp/crdts 5

cargo run --release --manifest-path benchmarks/compare-native/Cargo.toml -- 5
```

Each workload runs once to warm up, then `runs` times (5 by default), and the median is reported.
Every edit is its own transaction, as an editor makes them: a Yjs transaction, a Loro commit, an
Automerge change, a yrs transaction, and one y-octo call. All five libraries count text
positions in UTF-16 code units. The workloads:

- B1.1 appends 6,000 characters one at a time.
- B1.4 inserts 6,000 characters at seeded random positions (the same positions in both
  harnesses).
- B4.1 applies the real-world editing trace in `assets/bench-input/b4-editing-trace.bin`
  (259,778 edits).
- B4.2 loads the B4 document. Yjs, ywasm, yrs and y-octo read the same Yjs update
  (`b4-update.bin`). Loro and Automerge can't read Yjs updates, so they load their own snapshot
  of the document their B4.1 run produced. That measures each format as its library stores it,
  not the same bytes.

## Results

Apple M5 Max, Node v24.16.0, Yjs 13.6.33, Loro 1.17.1, Automerge 3.5.0, y-octo 0.1.1 (with
`large_refs`), ywasm and yrs built from `next` (2026-11-13). The machine was under other load
(load average about 18) during this run, so the absolute times are higher than on an idle
machine; the libraries in each table ran back to back in one process, so compare the ratios.

In Node, times relative to Yjs:

| workload | Yjs | ywasm | Loro | Automerge |
|---|---|---|---|---|
| B1.1 append N chars | 23.56 ms | 5.43 ms (0.23×) | 14.42 ms (0.61×) | 238.57 ms (10.13×) |
| B1.4 insert N chars at random positions | 196.63 ms | 63.89 ms (0.32×) | 24.07 ms (0.12×) | 313.28 ms (1.59×) |
| B4.1 apply the editing trace | 3873.64 ms | 311.78 ms (0.08×) | 403.24 ms (0.10×) | 9953.94 ms (2.57×) |
| B4.2 load the document | 18.46 ms | 4.63 ms (0.25×) | 0.78 ms (0.04×) | 382.20 ms (20.71×) |

Native, times relative to yrs:

| workload | yrs | y-octo |
|---|---|---|
| B1.1 append N chars | 1.04 ms | 292.69 ms (280.71×) |
| B1.4 insert N chars at random positions | 46.54 ms | 4.11 ms (0.09×) |
| B4.1 apply the editing trace | 288.64 ms | 388.56 ms (1.35×) |
| B4.2 load the trace as one update | 3.11 ms | 3.63 ms (1.17×) |

What stands out:

- **ywasm against Loro.** ywasm is faster on appends and on the editing trace, Loro on random
  inserts and on loading. Loro's snapshot keeps the document state, so loading it does less
  work than integrating a Yjs update.
- **Random inserts are yrs's weakest workload.** Loro and y-octo both take about a tenth of
  its time on B1.4. A search marker only helps near the previous edit, and each random insert
  walks from the nearest marker.
- **y-octo appends are slow.** About 50 µs per character, against 0.2 µs for yrs: each append
  seems to walk the text from its start.
- **y-octo needs `large_refs`** for the editing trace. Without it, the trace panics once an
  item has more than 65,535 references ("Too many refs on Somr").
- **Automerge** is 1.6–20× Yjs here. Each edit is a separate change with its own hash and
  metadata, which is what the "every edit its own transaction" rule asks for but isn't how
  Automerge is usually driven.

For reference, earlier criterion runs of native yrs on an idle machine (`cargo bench -p yrs`):
B1.1 1.3 ms, B1.4 18.8 ms, B4.1 161 ms, B4.2 2.1 ms.

Timings only compare on the same machine and versions, so rerun the harnesses rather than
comparing these numbers with others.

## Not covered

- **Memory.** These harnesses measure time only. `ReadTxn::memory_usage` and
  `yrs/tests/allocations.rs` cover yrs's own memory.
- **Sync between peers.** The workloads are single-document. Two-peer workloads (B2, B3) need
  each library's own sync protocol.
