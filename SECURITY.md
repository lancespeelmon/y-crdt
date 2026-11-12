# Security Policy

This repository is a fork of [y-crdt/y-crdt](https://github.com/y-crdt/y-crdt). Active development
happens on the `next` branch.

## Reporting a vulnerability

**Please don't report security issues in public issues, pull requests or discussions.**

Report them privately with GitHub's
[private vulnerability reporting](https://github.com/lancespeelmon/y-crdt/security/advisories/new)
for this repository. Please include:

- the affected crate (`yrs`, `yffi` or `ywasm`) and version or commit,
- what an attacker controls (for example a remote peer sending updates or awareness messages, or
  a caller of the C API) and the impact (crash, memory corruption, document divergence, resource
  exhaustion),
- a reproducer, ideally the smallest input bytes or program that triggers it.

We aim to acknowledge reports promptly. Fixes are developed privately, and the details
are published after a fix is available. If the issue also affects upstream y-crdt, we coordinate
disclosure with its maintainers before publishing anything.

## Scope

Of particular interest is anything reachable from data a remote peer controls: decoding and
applying v1/v2 updates, state vectors, delete sets, snapshots, sticky indexes, the sync protocol and
awareness messages. Panics count too, because a panic on such input can take down a server
process.
