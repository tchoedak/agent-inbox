# Security policy

## Reporting a vulnerability

Report security issues privately through GitHub's
[private vulnerability reporting](https://github.com/nure-ai/agent-inbox/security/advisories/new).
Please do not open a public issue for a vulnerability.

Expect an acknowledgement within a week.

## What agent-inbox handles

This is a local, single-user tool with no network surface. It does not phone home, fetch anything,
or open a port. The realistic risks are about **data at rest**, because the whole purpose of the
tool is to collect real output from real systems.

- **The store holds real reports.** `~/.local/share/agent-inbox/` accumulates whatever your
  producers emit - which may include financial figures, account identifiers, credentials printed
  into a report by accident, or personal data. It is protected only by filesystem permissions.
- **Artifacts are copied, not referenced.** Deleting the producer's copy does not delete the
  inbox's. Removing a report means removing it from the store.
- **`--source-project` and artifact origin paths are recorded** in the index, which can reveal
  directory structure.

### If you contribute

**Never commit a real report as a test fixture.** Test data must be fabricated. This repository's
`.gitignore` blocks report-shaped filenames for that reason, and CI fails the build if planning or
report artifacts would end up in a published crate. Both exist because real account data was
committed here once, before the project was public.
