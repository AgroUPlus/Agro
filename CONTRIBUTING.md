# Contributing to Agro

## Licence

Agro is licensed **AGPL-3.0** (see `LICENSE`). That applies to the whole work, including any
contribution merged into it.

The AGPL's section 13 is the one worth reading before you build anything on top: if you run a
modified Agro where other people can reach it over a network, those people must be offered its
source. This is deliberate, and it is why the project can be given away without also giving away
the ability to run it as a service.

## Contributor License Agreement

Every contribution requires agreement to [`CLA.md`](CLA.md). Add this line to your commit
messages, or state it once in the pull request description:

```
Contribution-License: I agree to the CLA at CLA.md
```

You keep the copyright in your work. The CLA grants the right to **sublicense**, which is what
keeps the project's licence changeable in future — that possibility ends permanently at the first
contribution merged without one, which is why this is asked for up front rather than later.

## Before you open a pull request

- `cargo build` — the React dashboard must be built first (`dashboard/dist/` is compiled into
  the binary by `rust-embed`), so a fresh clone needs `npm --prefix dashboard ci && npm --prefix
  dashboard run build`.
- `cargo test` — including the boundary suites (`guest_boundary_tests.rs`,
  `social_boundary_tests.rs`). They assert what one account can and cannot see of another. A
  change that makes them pass by loosening an assertion is the one change to explain in detail.

## `src/norm.rs` is a port, and must stay one

It is ported from Wanda's `TrackDeduplicator`, deliberately. Both projects are AGPL-3.0, so the
port is licence-clean. Keep them in step: if the two ends normalise a title even slightly
differently, the shared index holds two conventions and the library diff quietly produces
nonsense. Change it in both places or in neither.
