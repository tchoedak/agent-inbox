//! The index. SQLite holds metadata and state; reports themselves never go in
//! here, so a corrupt index still leaves every artifact readable with `ls`.
//!
//! Layout under `$AGENT_INBOX_HOME` (default `~/.local/share/agent-inbox`):
//!
//! ```text
//! index.db
//! artifacts/<topic>/<bucket>/<revision>/...
//! .staging/<uuid>/
//! ```
//!
//! Both the layout and the schema are public and documented: read them from
//! anything. Writes go through this binary, because atomicity depends on one
//! owner.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::Connection;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS topics (
    slug           TEXT PRIMARY KEY,
    title          TEXT,
    cadence        TEXT,
    source_project TEXT,
    created_at     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS editions (
    id         INTEGER PRIMARY KEY,
    topic_slug TEXT    NOT NULL REFERENCES topics(slug),
    bucket     TEXT    NOT NULL,
    revision   INTEGER NOT NULL,
    is_current INTEGER NOT NULL DEFAULT 0,
    timestamp  TEXT    NOT NULL,
    summary    TEXT,
    run_id     TEXT,
    created_at TEXT    NOT NULL,
    read_at    TEXT,
    UNIQUE (topic_slug, bucket, revision)
);

-- At most one current edition per bucket. The database is the authority on
-- which revision is current, so nothing ever infers it from the filesystem.
CREATE UNIQUE INDEX IF NOT EXISTS editions_one_current
    ON editions (topic_slug, bucket) WHERE is_current = 1;

CREATE TABLE IF NOT EXISTS artifacts (
    id          INTEGER PRIMARY KEY,
    edition_id  INTEGER NOT NULL REFERENCES editions(id) ON DELETE CASCADE,
    role        TEXT    NOT NULL,
    filename    TEXT    NOT NULL,
    origin_path TEXT,
    bytes       INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS tags (
    edition_id INTEGER NOT NULL REFERENCES editions(id) ON DELETE CASCADE,
    key        TEXT    NOT NULL,
    value      TEXT    NOT NULL
);

-- Warnings exist because cron discards stderr. The TUI is the only place
-- they will actually be read.
CREATE TABLE IF NOT EXISTS warnings (
    id           INTEGER PRIMARY KEY,
    created_at   TEXT NOT NULL,
    topic_slug   TEXT,
    kind         TEXT NOT NULL,
    message      TEXT NOT NULL,
    dismissed_at TEXT
);
"#;

pub struct Store {
    pub root: PathBuf,
    pub conn: Connection,
}

impl Store {
    /// Resolve the store root: `$AGENT_INBOX_HOME`, else the XDG data dir.
    pub fn default_root() -> Result<PathBuf> {
        if let Some(explicit) = std::env::var_os("AGENT_INBOX_HOME") {
            return Ok(PathBuf::from(explicit));
        }
        let home = std::env::var_os("HOME").context(
            "neither AGENT_INBOX_HOME nor HOME is set, so the store location cannot be resolved",
        )?;
        Ok(PathBuf::from(home).join(".local/share/agent-inbox"))
    }

    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(root.join("artifacts"))
            .with_context(|| format!("creating store at {}", root.display()))?;
        std::fs::create_dir_all(root.join(".staging"))?;

        let conn = Connection::open(root.join("index.db"))
            .with_context(|| format!("opening index at {}", root.join("index.db").display()))?;
        // WAL so several cron jobs emitting in the same minute do not collide.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // Wait rather than fail outright if another emit holds the write lock.
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.execute_batch(SCHEMA).context("applying schema")?;

        Ok(Self { root, conn })
    }

    pub fn artifacts_dir(&self, topic: &str, bucket: &str, revision: i64) -> PathBuf {
        self.root
            .join("artifacts")
            .join(topic)
            .join(bucket)
            .join(revision.to_string())
    }

    pub fn staging_dir(&self) -> PathBuf {
        self.root
            .join(".staging")
            .join(uuid::Uuid::new_v4().to_string())
    }

    pub fn topic_slugs(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT slug FROM topics")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn record_warning(&self, topic: &str, kind: &str, message: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO warnings (created_at, topic_slug, kind, message)
             VALUES (?1, ?2, ?3, ?4)",
            (crate::now_rfc3339(), topic, kind, message),
        )?;
        Ok(())
    }
}
