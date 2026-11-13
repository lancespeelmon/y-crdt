#!/usr/bin/env python3
"""Stores criterion results as a baseline in the repository, and compares runs against one.

    cargo bench -p yrs -- --warm-up-time 1 --measurement-time 3
    python3 benchmarks/baseline.py save benchmarks/baselines/<name>.json
    # ...change things, bench again, then:
    python3 benchmarks/baseline.py compare benchmarks/baselines/<name>.json [--threshold 10]

`save` reads every `target/criterion/**/new/estimates.json` and writes each benchmark's median
(in nanoseconds) and its confidence interval. `compare` prints the change per benchmark and exits
with status 1 if any is slower than the threshold (percent) beyond the noise of both runs.
Baselines are only comparable on the same machine: name them after it.
"""
import argparse
import json
import pathlib
import platform
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
CRITERION = ROOT / "target" / "criterion"


def collect():
    results = {}
    for est in sorted(CRITERION.glob("**/new/estimates.json")):
        bench_dir = est.parent.parent
        meta = bench_dir / "new" / "benchmark.json"
        name = json.loads(meta.read_text())["full_id"] if meta.exists() else str(
            bench_dir.relative_to(CRITERION))
        median = json.loads(est.read_text())["median"]
        results[name] = {
            "median_ns": round(median["point_estimate"], 1),
            "low_ns": round(median["confidence_interval"]["lower_bound"], 1),
            "high_ns": round(median["confidence_interval"]["upper_bound"], 1),
        }
    if not results:
        sys.exit(f"no criterion results under {CRITERION}; run `cargo bench -p yrs` first")
    return results


def save(path):
    commit = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT,
                            capture_output=True, text=True).stdout.strip()
    rustc = subprocess.run(["rustc", "--version"], capture_output=True, text=True).stdout.strip()
    doc = {
        "commit": commit,
        "rustc": rustc,
        "machine": f"{platform.system()} {platform.machine()}",
        "benchmarks": collect(),
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(doc, indent=2, sort_keys=True) + "\n")
    print(f"saved {len(doc['benchmarks'])} benchmarks to {path}")


def compare(path, threshold):
    base = json.loads(path.read_text())["benchmarks"]
    now = collect()
    regressions = 0
    print(f"{'benchmark':60} {'baseline':>12} {'now':>12} {'change':>8}")
    for name in sorted(set(base) & set(now)):
        b, n = base[name], now[name]
        change = (n["median_ns"] - b["median_ns"]) / b["median_ns"] * 100
        # Only a slowdown whose intervals don't overlap counts.
        slower = change > threshold and n["low_ns"] > b["high_ns"]
        regressions += slower
        flag = "  SLOWER" if slower else ""
        print(f"{name[:60]:60} {fmt(b['median_ns']):>12} {fmt(n['median_ns']):>12} "
              f"{change:+7.1f}%{flag}")
    for name in sorted(set(now) - set(base)):
        print(f"{name[:60]:60} {'(new)':>12} {fmt(now[name]['median_ns']):>12}")
    for name in sorted(set(base) - set(now)):
        print(f"{name[:60]:60} {fmt(base[name]['median_ns']):>12} {'(missing)':>12}")
    sys.exit(1 if regressions else 0)


def fmt(ns):
    for unit, scale in (("s", 1e9), ("ms", 1e6), ("µs", 1e3)):
        if ns >= scale:
            return f"{ns / scale:.2f} {unit}"
    return f"{ns:.0f} ns"


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("save")
    s.add_argument("path", type=pathlib.Path)
    c = sub.add_parser("compare")
    c.add_argument("path", type=pathlib.Path)
    c.add_argument("--threshold", type=float, default=10.0)
    args = p.parse_args()
    if args.cmd == "save":
        save(args.path)
    else:
        compare(args.path, args.threshold)


if __name__ == "__main__":
    main()
