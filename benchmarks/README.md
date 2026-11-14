# Benchmarks

Plan item 6.9. There are three kinds of measurement, each with a stored reference.

## Run time: criterion with stored baselines

The criterion benchmarks are in `yrs/benches/`. `benches.rs` holds the crdt-benchmarks B1–B4
suites; there are also id-set, UTF-16 and serialization benchmarks.

```sh
cargo bench -p yrs -- --warm-up-time 1 --measurement-time 3
python3 benchmarks/baseline.py save benchmarks/baselines/<machine>.json
```

After a change, bench again and compare:

```sh
python3 benchmarks/baseline.py compare benchmarks/baselines/<machine>.json --threshold 10
```

`compare` lists every benchmark's change. It exits with 1 if one is more than `--threshold`
percent slower and its confidence interval doesn't overlap the baseline's. Timings only compare
on the same machine, so a baseline is named after the machine it was taken on, and the JSON
records the commit and compiler.

CI runs each benchmark once (`cargo bench -p yrs -- --test`) so the harness keeps building. It
doesn't compare timings, since shared runners are too noisy for that.

## Allocations: budgets in a test

`yrs/tests/allocations.rs` counts heap allocations (with dhat) for the hot paths: typing a
character, a middle insert, array and map writes, reading text, and applying a remote update.
Each has a budget. A change that allocates more fails the test, and one that allocates less
should lower the budget. It runs with the normal test suite: allocation counts don't depend on
the machine.

## Size: the wasm module

ywasm is built with `wasm-opt -Oz`, and CI fails if `ywasm_bg.wasm` grows past the budget in
`.github/workflows/main.yml`. Raise the budget deliberately, in the same change that needs it.

## Against other CRDT libraries

`benchmarks/compare/compare.mjs` runs B1 and B4 workloads against Yjs, ywasm, Loro and
Automerge in Node, and `benchmarks/compare-native` runs them against native yrs and y-octo. See
[COMPARISON.md](COMPARISON.md) for how to run them and the last results. Peak memory per
document size isn't covered yet.
