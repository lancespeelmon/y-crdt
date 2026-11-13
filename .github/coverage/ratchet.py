#!/usr/bin/env python3
"""Coverage ratchet for yrs.

Reads the JSON summary from `cargo llvm-cov --json --summary-only` and compares line coverage of
each module group with the floor in `floor.json`. Fails if any group drops more than the tolerance
below its floor. When a group rises above its floor, prints the new numbers so the floor can be
raised in the same PR (the ratchet only moves up).

    python3 .github/coverage/ratchet.py coverage.json [--write]

`--write` rewrites `floor.json` with the measured numbers.
"""
import json
import pathlib
import sys

FLOOR = pathlib.Path(__file__).with_name("floor.json")

# Module groups, as path prefixes under yrs/src. "total" covers every yrs source file.
GROUPS = {
    "encoding": ["encoding/", "updates/", "state_vector.rs"],
    "update": ["update.rs"],
    "block": ["block.rs", "block_store.rs", "block_iter.rs", "slice.rs"],
    "store": ["store.rs", "id_set.rs", "gc.rs"],
    "transaction": ["transaction.rs"],
    "sync": ["sync/"],
    "total": [""],
}

# Coverage of the property tests moves a little from run to run (proptest picks new cases), and
# Linux and macOS builds differ slightly, so a drop smaller than this is not a regression.
TOLERANCE = 0.5


def measure(report):
    totals = {name: [0, 0] for name in GROUPS}
    for file in report["data"][0]["files"]:
        path = file["filename"].replace("\\", "/")
        marker = "/yrs/src/"
        if marker not in path:
            continue
        rel = path.split(marker, 1)[1]
        lines = file["summary"]["lines"]
        for name, prefixes in GROUPS.items():
            if any(rel.startswith(p) for p in prefixes):
                totals[name][0] += lines["covered"]
                totals[name][1] += lines["count"]
    result = {}
    for name, (covered, count) in totals.items():
        if count == 0:
            sys.exit(f"coverage group {name!r} matched no files; update GROUPS in {__file__}")
        result[name] = round(100.0 * covered / count, 2)
    return result


def main():
    args = sys.argv[1:]
    if not args:
        sys.exit(__doc__)
    measured = measure(json.loads(pathlib.Path(args[0]).read_text()))
    if "--write" in args:
        FLOOR.write_text(json.dumps(measured, indent=2) + "\n")
        print(f"wrote {FLOOR}")
        return

    floor = json.loads(FLOOR.read_text())
    failed = False
    raised = False
    print(f"{'group':<12} {'floor':>7} {'now':>7}")
    for name, now in measured.items():
        was = floor.get(name)
        if was is None:
            print(f"{name:<12} {'-':>7} {now:>7.2f}  (new group, add it to floor.json)")
            raised = True
            continue
        mark = ""
        if now < was - TOLERANCE:
            mark = "  FAIL: dropped below the floor"
            failed = True
        elif now > was + TOLERANCE:
            mark = "  up: raise the floor"
            raised = True
        print(f"{name:<12} {was:>7.2f} {now:>7.2f}{mark}")
    if raised:
        print("\nCoverage went up. Raise the floor with:\n"
              "  python3 .github/coverage/ratchet.py coverage.json --write")
    if failed:
        print("\nLine coverage dropped. Add tests for the new or changed code.")
        sys.exit(1)


if __name__ == "__main__":
    main()
