<!-- agent-inbox:begin -->
## agent-inbox

Use when building or modifying anything that produces a report, digest, summary, or export on a recurring schedule (cron, launchd, systemd timer, CI schedule, or a scheduled agent routine), so its output is delivered to the local agent-inbox instead of left as a stray file. Also use when asked to wire a project into agent-inbox, or to check what scheduled reports exist.

Run `agent-inbox agent-guide` for the authoritative integration guide: when to wire a producer in, the exact `emit` call, artifact roles, topic naming, and how to verify delivery. Print it rather than relying on remembered details.

Quick reference: `agent-inbox emit --topic <slug> --artifact <path>[:<role>]`, where role is `terminal`, `primary`, or `data`. Never swallow a non-zero exit.
<!-- agent-inbox:end -->
