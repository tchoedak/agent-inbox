//! Read-only views over the index.
//!
//! These exist so an agent can check what topics already exist before inventing
//! a name, and confirm a wire-up actually delivered. A new topic name forks the
//! history, and history is the whole point.

use anyhow::Result;

use crate::store::Store;

pub struct TopicSummary {
    pub slug: String,
    pub title: Option<String>,
    pub cadence: Option<String>,
    pub editions: i64,
    pub latest_bucket: Option<String>,
    /// Derived from whether the topic's current edition has been read, never
    /// stored, so it cannot drift out of sync with the editions themselves.
    pub unread: bool,
}

pub struct EditionSummary {
    pub bucket: String,
    pub revision: i64,
    pub timestamp: String,
    pub artifacts: String,
}

/// One edition with enough detail for the reader: which files it holds, and
/// which of them should be shown.
pub struct EditionDetail {
    pub id: i64,
    pub bucket: String,
    pub revision: i64,
    pub timestamp: String,
    pub summary: Option<String>,
    pub read: bool,
    pub artifacts: Vec<ArtifactRef>,
}

#[derive(Clone)]
pub struct ArtifactRef {
    pub role: String,
    pub filename: String,
    pub path: std::path::PathBuf,
}

impl EditionDetail {
    /// The artifact the reader opens: the terminal rendition if the producer
    /// supplied one, else the canonical report, else whatever is there.
    pub fn display_artifact(&self) -> Option<&ArtifactRef> {
        self.artifacts
            .iter()
            .find(|a| a.role == "terminal")
            .or_else(|| self.artifacts.iter().find(|a| a.role == "primary"))
            .or_else(|| self.artifacts.first())
    }

    pub fn primary_artifact(&self) -> Option<&ArtifactRef> {
        self.artifacts
            .iter()
            .find(|a| a.role == "primary")
            .or_else(|| self.artifacts.first())
    }
}

/// Every current edition of a topic, newest first, with its artifacts resolved
/// to real paths.
pub fn edition_details(store: &Store, topic: &str) -> Result<Vec<EditionDetail>> {
    let mut stmt = store.conn.prepare(
        "SELECT id, bucket, revision, timestamp, summary, read_at IS NOT NULL
         FROM editions
         WHERE topic_slug = ?1 AND is_current = 1
         ORDER BY bucket DESC",
    )?;
    let rows: Vec<EditionDetail> = stmt
        .query_map((topic,), |r| {
            Ok(EditionDetail {
                id: r.get(0)?,
                bucket: r.get(1)?,
                revision: r.get(2)?,
                timestamp: r.get(3)?,
                summary: r.get(4)?,
                read: r.get(5)?,
                artifacts: Vec::new(),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut out = Vec::with_capacity(rows.len());
    for mut e in rows {
        let mut stmt = store
            .conn
            .prepare("SELECT role, filename FROM artifacts WHERE edition_id = ?1")?;
        let dir = store.artifacts_dir(topic, &e.bucket, e.revision);
        e.artifacts = stmt
            .query_map((e.id,), |r| {
                let role: String = r.get(0)?;
                let filename: String = r.get(1)?;
                Ok((role, filename))
            })?
            .map(|row| {
                row.map(|(role, filename)| ArtifactRef {
                    path: dir.join(&filename),
                    role,
                    filename,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        out.push(e);
    }
    Ok(out)
}

/// Opening an edition marks it read. Paging backward through history does not
/// call this - skimming backward is not the same as reading.
pub fn mark_read(store: &Store, edition_id: i64) -> Result<()> {
    store.conn.execute(
        "UPDATE editions SET read_at = ?1 WHERE id = ?2 AND read_at IS NULL",
        (crate::now_rfc3339(), edition_id),
    )?;
    Ok(())
}

/// Cheap change detector for the poll loop: if this changes, something arrived.
pub fn revision_token(store: &Store) -> Result<(i64, i64)> {
    Ok(store.conn.query_row(
        "SELECT COUNT(*), COALESCE(MAX(id), 0) FROM editions",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}

pub fn topics(store: &Store) -> Result<Vec<TopicSummary>> {
    // Ordered by most recent edition first. Deliberately not by unread: a list
    // that reshuffles based on what you have read defeats muscle memory, and the
    // unread marker already carries that signal.
    let mut stmt = store.conn.prepare(
        "SELECT t.slug, t.title, t.cadence,
                COUNT(DISTINCT e.bucket),
                MAX(e.bucket),
                (SELECT e2.read_at IS NULL FROM editions e2
                  WHERE e2.topic_slug = t.slug AND e2.is_current = 1
                  ORDER BY e2.bucket DESC LIMIT 1)
         FROM topics t
         LEFT JOIN editions e ON e.topic_slug = t.slug AND e.is_current = 1
         GROUP BY t.slug
         ORDER BY MAX(e.bucket) DESC, t.slug",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(TopicSummary {
            slug: r.get(0)?,
            title: r.get(1)?,
            cadence: r.get(2)?,
            editions: r.get(3)?,
            latest_bucket: r.get(4)?,
            unread: r.get::<_, Option<bool>>(5)?.unwrap_or(false),
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn editions(store: &Store, topic: &str, limit: i64) -> Result<Vec<EditionSummary>> {
    let slug = crate::slug::normalize(topic);
    let mut stmt = store.conn.prepare(
        "SELECT e.bucket, e.revision, e.timestamp,
                COALESCE(GROUP_CONCAT(a.filename || ' (' || a.role || ')', ', '), '')
         FROM editions e
         LEFT JOIN artifacts a ON a.edition_id = e.id
         WHERE e.topic_slug = ?1 AND e.is_current = 1
         GROUP BY e.id
         ORDER BY e.bucket DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map((&slug, limit), |r| {
        Ok(EditionSummary {
            bucket: r.get(0)?,
            revision: r.get(1)?,
            timestamp: r.get(2)?,
            artifacts: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}
