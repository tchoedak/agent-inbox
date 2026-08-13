# agent-inbox

[![CI](https://github.com/nure-ai/agent-inbox/actions/workflows/ci.yml/badge.svg)](https://github.com/nure-ai/agent-inbox/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

**A local inbox for reports produced on a schedule.**

You keep building apps that generate a report every day - a trading summary, a scrape, a scan, a digest.
Each one drops a file somewhere different, and reading them means remembering where they all are.

agent-inbox gives them one destination.
Apps call `agent-inbox emit` when they finish, and the inbox copies the artifacts into its own store, keyed by **topic** and **date**.
One place to look each morning, with history per topic, and that history survives the producing app being tidied up or deleted outright.

Coding agents get this for free: `agent-inbox agent-guide` prints the full integration contract, so an agent building your next scheduled job can wire it in without being told how.

```sh
# in your daily job, once it has produced the report
agent-inbox emit --topic trading-perf --cadence daily \
    --artifact report.md:terminal --artifact report.html:primary
```

```console
$ agent-inbox topics
trading-perf   daily   24 editions   latest 2026-08-13   Daily trading performance
```

Reports are never stored inside the database.
Artifacts stay ordinary files in a documented layout, so a corrupt index still leaves every report readable with `ls` and `cat`.

## Status

**Early. The ingest half is done and in daily use; the reading half is not built yet.**

| | |
| --- | --- |
| `emit`, the store, topics and history | working, tested |
| Agent integration (`agent-guide`, adapters) | working, tested |
| `topics` / `editions` listing | working |
| **Terminal UI for browsing and reading** | **not built yet** |
| Read/unread state, retention | not built yet |
| Noticing that a report stopped arriving | not built yet |

Until the TUI lands, reading means `agent-inbox editions --topic <slug>` and opening the artifact
yourself. The store layout is documented and stable, so that is a one-liner.

## Install

```sh
cargo install --path .      # from a checkout
```

Make sure the install directory is on your `PATH`, including for whatever runs your scheduled jobs.
Cron does not read your shell profile, so use an absolute path or set `PATH` in the crontab.

SQLite is bundled into the binary. There is nothing else to install.

## Emitting

```sh
agent-inbox emit --topic trading-perf \
    --artifact report.md:terminal \
    --artifact report.html:primary
```

The minimal call is a topic and one artifact.
Everything else defaults.

| Flag | Meaning |
| --- | --- |
| `--topic` | Topic name. Normalized to a slug, created on first use. |
| `--artifact` | `path`, or `path:role`. Repeatable. `-` reads one artifact from stdin. |
| `--bucket` | Grouping key, default today. Set it to backfill an older date. |
| `--timestamp` | When the report was produced, default now. |
| `--title` | Topic display title. Applied every emit, last write wins. |
| `--cadence` | `daily`, `weekly`, `hourly`, `none`. |
| `--summary` | One-line summary of this edition, written by the producer. |
| `--tag` | `key=value`, repeatable. |
| `--run-id` | Identifier for the run that produced this edition. |
| `--source-project` | Project the report came from. |
| `--stdin-name` | Filename to give the artifact read from stdin. |
| `--home` | Store location. Overrides `$AGENT_INBOX_HOME`. |

### Roles

Every artifact has a role, inferred from its extension and overridable as `path:role`.

- `terminal` - rendered directly in the TUI. Inferred for `.md`, `.markdown`, `.txt`, `.text`.
- `primary` - the canonical report. Opened in a browser, or converted for the terminal when no `terminal` artifact exists. Inferred for `.html`, `.htm`.
- `data` - supporting data, never the default view. Inferred for `.csv`, `.json`, `.tsv`, `.yaml`, `.yml`.

Anything else must name its role explicitly.

### Guarantees

- **One emit is one edition.** Every artifact lands or none does; there is no partial edition.
- **A rerun supersedes.** Emitting into a bucket that already has an edition creates a new revision and retires the old one, which is retained and still readable.
- **Failure is loud.** Non-zero exit, a message on stderr, and nothing half-written. Designed for cron, where a swallowed error means a report that quietly stops arriving.
- **Near-misses warn, never fail.** Emitting to `trading-perf-daily` when `trading-perf` exists still delivers the report, and records a warning. Losing a day's report to a naming heuristic would be worse than two topics you reconcile by hand.
- **Concurrent emits are safe.** Several cron jobs firing in the same minute each get their own revision.

## Agent integration

Any coding agent - Claude Code, Codex, Cursor, Aider, or one that does not exist yet - can learn
the contract by running:

```sh
agent-inbox agent-guide
```

That prints the full integration guide: when to wire a producer in, the exact call, artifact roles,
topic naming, and how to verify delivery. It is compiled into the binary, so it always describes
the version actually installed.

Harness adapters are installed with:

```sh
agent-inbox install-agent-docs            # detects what is present
agent-inbox install-agent-docs --target all
agent-inbox install-agent-docs --target agents-md --project .
```

| Target | Written to |
| --- | --- |
| `claude` | `~/.claude/skills/agent-inbox/SKILL.md` |
| `codex` | `~/.codex/AGENTS.md` |
| `agents-md` | `AGENTS.md` in the given project |

**Every adapter is a pointer, not a copy.** Each one carries the trigger conditions and then says
"run `agent-inbox agent-guide`". There is one document to maintain rather than one per harness,
adapters cannot drift when the contract changes, and adding support for a new harness means writing
a stub rather than porting the guide.

Adapters that share a file with human-written content use a delimited block, so installing and
reinstalling never disturbs what is around it.

## Reading what is there

```sh
agent-inbox topics                      # topics, cadence, edition counts, latest bucket
agent-inbox editions --topic <slug>     # editions of one topic, newest first
```

## Store layout

Both the layout and the schema are public and stable.
Read them from anything - a statusline unread count, a search script, a backup job, an agent reading a report directly.
Writes go through the binary, because atomicity depends on one owner.

```text
$AGENT_INBOX_HOME            default ~/.local/share/agent-inbox
  index.db                   SQLite: topics, editions, artifacts, tags, warnings
  artifacts/<topic>/<bucket>/<revision>/
                             the report files themselves
  .staging/<uuid>/           editions under construction, never left behind
```

### Schema

- `topics` - `slug` (primary key), `title`, `cadence`, `source_project`, `created_at`.
- `editions` - `topic_slug`, `bucket`, `revision`, `is_current`, `timestamp`, `summary`, `run_id`, `created_at`, `read_at`. A partial unique index permits one current revision per bucket.
- `artifacts` - `edition_id`, `role`, `filename`, `origin_path`, `bytes`.
- `tags` - `edition_id`, `key`, `value`.
- `warnings` - `created_at`, `topic_slug`, `kind`, `message`, `dismissed_at`.

`/usr/bin/sqlite3` ships with macOS, so the index is inspectable without installing anything:

```sh
sqlite3 -header -column ~/.local/share/agent-inbox/index.db \
  "SELECT topic_slug, bucket, revision FROM editions WHERE is_current = 1 ORDER BY bucket DESC LIMIT 10;"
```

## Atomicity

Two mechanisms, each doing what it is good at.

Artifacts are copied into `.staging/<uuid>/` and moved into place with `rename(2)`, which is atomic within a filesystem, so a half-written edition never appears under `artifacts/`.
The index is written in a single transaction per emit.

A crash between them leaves an orphaned artifact directory that no index row references.
That is garbage rather than corruption, and it is the right failure asymmetry: there is never a visible edition with missing files.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

```sh
cargo test
cargo clippy --all-targets
cargo fmt --all
```

One rule worth repeating here: **never commit a real report**, as a fixture, an example, or an issue
attachment. This tool collects real output from real systems, so a convenient real example is a data
leak waiting to happen. Fabricate test data. CI enforces the packaging half of this.

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution you intentionally submit for inclusion in
this work shall be dual-licensed as above, without any additional terms or conditions.
