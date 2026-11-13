# Comparison with Yjs

Plan item 6.9. `benchmarks/compare/compare.mjs` runs crdt-benchmarks workloads against Yjs and
ywasm in one Node process, on the same inputs, and checks that both end with the same text.

## Running it

The harness doesn't install Yjs; point it at a directory where Yjs is installed:

```sh
wasm-pack build --release --target nodejs ./ywasm
mkdir -p /tmp/yjs && (cd /tmp/yjs && npm init -y >/dev/null && npm install yjs)
node benchmarks/compare/compare.mjs /tmp/yjs 5
```

Each workload runs once to warm up, then `runs` times (5 by default), and the median is reported.
Every edit is its own transaction, as an editor makes them. The workloads:

- B1.1 appends 6,000 characters one at a time.
- B1.4 inserts 6,000 characters at seeded random positions.
- B4.1 applies the real-world editing trace in `assets/bench-input/b4-editing-trace.bin`
  (259,778 edits).
- B4.2 loads that document from one update.

## Results

Apple M5 Max, Node v24.16.0, Yjs 13.6.33, ywasm built from `next` (2026-11-13):

| workload | Yjs | ywasm | ywasm / Yjs |
|---|---|---|---|
| B1.1 append N chars | 9.42 ms | 2.24 ms | 0.24 |
| B1.4 insert N chars at random positions | 77.21 ms | 22.75 ms | 0.29 |
| B4.1 apply the editing trace | 2909.71 ms | 229.75 ms | 0.08 |
| B4.2 load the trace as one update | 15.00 ms | 4.16 ms | 0.28 |

For reference, native yrs on the same machine (criterion, `cargo bench -p yrs`): B1.1 1.3 ms,
B1.4 18.8 ms, B4.1 161 ms, B4.2 2.1 ms. The B4.1 figures for both yrs builds are after the
search markers (#24); before them, native yrs took 2.25 s.

Timings only compare on the same machine and Node version, so rerun the harness rather than
comparing these numbers with others.

## Not covered

- **Loro, Automerge and y-octo.** They need their own adapters, and their data models differ
  (Automerge's text has no Yjs-style transactions), so each workload needs a definition that is
  fair to both sides. That is the remaining part of 6.9.
- **Memory.** This harness measures time only. `ReadTxn::memory_usage` and
  `yrs/tests/allocations.rs` cover yrs's own memory.
