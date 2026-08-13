# Contributing

Contributions are welcome. This is a small, opinionated tool, so a short conversation before a
large change will save everyone time - open an issue first for anything structural.

## Getting set up

```sh
git clone https://github.com/nure-ai/agent-inbox
cd agent-inbox
cargo test
```

There is nothing else to install. SQLite is bundled into the binary.

## Before opening a pull request

```sh
cargo fmt --all
cargo clippy --all-targets
cargo test
```

CI runs these on Linux and macOS, checks the minimum supported Rust version, and verifies that a
published crate would contain no local or report artifacts. `RUSTFLAGS: -D warnings` is set, so a
warning fails the build.

## The rules that are not negotiable

**Never commit a real report, in any form.** Not as a test fixture, not as an example in the docs,
not as an attachment on an issue. This tool exists to collect real output from real systems, so a
convenient real example is a data leak waiting to happen. Fabricate test data.

**`emit` must never fail silently.** It runs under cron where nobody is watching, and a swallowed
error means a report that quietly stops arriving and is not missed for a month. Non-zero exit, a
message on stderr, no partial state.

**An edition is atomic.** All artifacts land or none do. If you change the write path, the staging
plus `rename(2)` plus single-transaction structure is load-bearing, not incidental.

**The emit contract is additive-only.** Flags may be added. None is ever removed or repurposed,
because agents write these calls into arbitrary projects where they run unattended for months.

## Documentation

`docs/AGENT_GUIDE.md` is compiled into the binary and printed by `agent-inbox agent-guide`. It is
the authoritative integration guide, and harness adapters point at it rather than restating it. If
you add a flag to `emit`, document it there - a test asserts that every flag in `emit --help` also
appears in the guide, so forgetting fails CI.

Keep adapters as pointers. A test asserts the Claude adapter stays well under the guide's length,
so that adapters cannot start accumulating contract detail that later goes stale.

## Style

Prefer boring and inspectable over clever. Comments should explain why something is the way it is,
particularly where a simpler-looking alternative was rejected for a reason.
