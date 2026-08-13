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
}

pub struct EditionSummary {
    pub bucket: String,
    pub revision: i64,
    pub timestamp: String,
    pub artifacts: String,
}

pub fn topics(store: &Store) -> Result<Vec<TopicSummary>> {
    let mut stmt = store.conn.prepare(
        "SELECT t.slug, t.title, t.cadence,
                COUNT(DISTINCT e.bucket),
                MAX(e.bucket)
         FROM topics t
         LEFT JOIN editions e ON e.topic_slug = t.slug AND e.is_current = 1
         GROUP BY t.slug
         ORDER BY t.slug",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(TopicSummary {
            slug: r.get(0)?,
            title: r.get(1)?,
            cadence: r.get(2)?,
            editions: r.get(3)?,
            latest_bucket: r.get(4)?,
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
