//! The ingest half of the contract.
//!
//! One emit is one edition. Every artifact arrives in a single call or the
//! call fails and writes nothing, which is what makes partial editions
//! impossible rather than merely handled.

use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use rusqlite::OptionalExtension;

use crate::slug;
use crate::store::Store;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Rendered in the TUI directly. Markdown or plain text.
    Terminal,
    /// The canonical report. Opened in a browser, or converted for the terminal
    /// when no `terminal` artifact exists.
    Primary,
    /// Supporting data. Never the default view.
    Data,
}

impl Role {
    /// Roles are inferred from the extension so the common call stays short.
    /// The explicit form exists because a producer with two markdown files
    /// means something by the difference.
    fn infer(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "md" | "markdown" | "txt" | "text" => Role::Terminal,
            "html" | "htm" => Role::Primary,
            "csv" | "json" | "tsv" | "yaml" | "yml" => Role::Data,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Terminal => "terminal",
            Role::Primary => "primary",
            Role::Data => "data",
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Role {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "terminal" => Role::Terminal,
            "primary" => Role::Primary,
            "data" => Role::Data,
            other => bail!("unknown role `{other}` (expected terminal, primary, or data)"),
        })
    }
}

/// A `--artifact` value: `path`, or `path:role`.
#[derive(Debug, Clone)]
pub struct ArtifactSpec {
    pub path: PathBuf,
    pub role: Option<Role>,
}

impl FromStr for ArtifactSpec {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        // Split from the right, and only treat the tail as a role if it is one.
        // Otherwise a path containing a colon would be silently mangled.
        if let Some((path, tail)) = s.rsplit_once(':')
            && let Ok(role) = tail.parse::<Role>()
        {
            return Ok(Self {
                path: PathBuf::from(path),
                role: Some(role),
            });
        }
        Ok(Self {
            path: PathBuf::from(s),
            role: None,
        })
    }
}

pub struct EmitRequest {
    pub topic: String,
    pub artifacts: Vec<ArtifactSpec>,
    pub bucket: Option<String>,
    pub timestamp: Option<String>,
    pub title: Option<String>,
    pub cadence: Option<String>,
    pub summary: Option<String>,
    pub tags: Vec<(String, String)>,
    pub run_id: Option<String>,
    pub source_project: Option<String>,
    pub stdin_name: Option<String>,
}

#[derive(Debug)]
pub struct EmitOutcome {
    pub topic: String,
    pub bucket: String,
    pub revision: i64,
    pub superseded: bool,
    pub artifact_count: usize,
    pub warnings: Vec<String>,
}

/// An artifact copied into staging and ready to be indexed:
/// filename, role, where it came from, and its size.
type StagedArtifact = (String, Role, Option<String>, u64);

/// Resolved artifact: where the bytes are now, and what they will be called.
struct Resolved {
    source: Source,
    filename: String,
    role: Role,
    origin: Option<String>,
}

enum Source {
    File(PathBuf),
    Stdin,
}

pub fn emit(store: &Store, mut req: EmitRequest) -> Result<EmitOutcome> {
    let topic = slug::normalize(&req.topic);
    if topic.is_empty() {
        bail!("topic `{}` normalizes to an empty slug", req.topic);
    }
    if req.artifacts.is_empty() {
        bail!("at least one --artifact is required");
    }

    let bucket = req.bucket.take().unwrap_or_else(crate::today_bucket);
    let timestamp = req.timestamp.take().unwrap_or_else(crate::now_rfc3339);

    let resolved = resolve_artifacts(&req.artifacts, req.stdin_name.as_deref())?;

    // Warn on near-misses before doing any work, so the message is emitted even
    // if a later step fails. Never fails the emit: losing a day's report to a
    // naming heuristic is worse than two topics you reconcile by hand.
    let existing = store.topic_slugs()?;
    let warnings: Vec<String> = slug::near_misses(&topic, existing.iter().map(String::as_str))
        .into_iter()
        .map(|other| {
            format!("topic `{topic}` closely resembles existing topic `{other}` - if these are the same report, they are now split")
        })
        .collect();

    let staging = store.staging_dir();
    std::fs::create_dir_all(&staging)?;
    let staged = match stage(&resolved, &staging) {
        Ok(staged) => staged,
        Err(err) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(err);
        }
    };

    let outcome = commit(store, &topic, &bucket, &timestamp, &req, &staged, &staging);

    // Whatever happened, never leave staging behind.
    let _ = std::fs::remove_dir_all(&staging);

    let mut outcome = outcome?;

    for message in &warnings {
        store.record_warning(&topic, "slug-near-miss", message)?;
    }
    outcome.warnings = warnings;
    Ok(outcome)
}

fn resolve_artifacts(specs: &[ArtifactSpec], stdin_name: Option<&str>) -> Result<Vec<Resolved>> {
    let mut out = Vec::with_capacity(specs.len());
    let mut seen_stdin = false;

    for spec in specs {
        if spec.path.as_os_str() == "-" {
            if seen_stdin {
                bail!("stdin can only be used for one artifact");
            }
            seen_stdin = true;
            let filename = stdin_name.context(
                "reading an artifact from stdin requires --stdin-name, since a pipe has no filename",
            )?;
            let role = spec
                .role
                .or_else(|| Role::infer(Path::new(filename)))
                .context(
                    "reading an artifact from stdin requires an explicit role, e.g. `-:terminal`",
                )?;
            out.push(Resolved {
                source: Source::Stdin,
                filename: filename.to_string(),
                role,
                origin: None,
            });
            continue;
        }

        let meta = std::fs::metadata(&spec.path)
            .with_context(|| format!("artifact `{}` cannot be read", spec.path.display()))?;
        if !meta.is_file() {
            bail!("artifact `{}` is not a file", spec.path.display());
        }
        let filename = spec
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .with_context(|| format!("artifact `{}` has no usable filename", spec.path.display()))?
            .to_string();
        let role = spec
            .role
            .or_else(|| Role::infer(&spec.path))
            .with_context(|| {
                format!(
                    "cannot infer a role for `{}` - name it explicitly, e.g. `{}:data`",
                    spec.path.display(),
                    spec.path.display()
                )
            })?;
        let origin = std::fs::canonicalize(&spec.path)
            .ok()
            .map(|p| p.display().to_string());

        out.push(Resolved {
            source: Source::File(spec.path.clone()),
            filename,
            role,
            origin,
        });
    }

    let mut names: Vec<&str> = out.iter().map(|r| r.filename.as_str()).collect();
    names.sort_unstable();
    if let Some(dup) = names.windows(2).find(|w| w[0] == w[1]) {
        bail!("two artifacts share the filename `{}`", dup[0]);
    }

    Ok(out)
}

/// Copy every artifact into a staging directory. Nothing touches the store
/// proper until the whole edition is present on disk.
fn stage(resolved: &[Resolved], staging: &Path) -> Result<Vec<StagedArtifact>> {
    let mut staged = Vec::with_capacity(resolved.len());
    for item in resolved {
        let dest = staging.join(&item.filename);
        let bytes = match &item.source {
            Source::File(path) => std::fs::copy(path, &dest)
                .with_context(|| format!("copying `{}` into the store", path.display()))?,
            Source::Stdin => {
                let mut buf = Vec::new();
                std::io::stdin()
                    .read_to_end(&mut buf)
                    .context("reading artifact from stdin")?;
                std::fs::write(&dest, &buf)?;
                buf.len() as u64
            }
        };
        staged.push((item.filename.clone(), item.role, item.origin.clone(), bytes));
    }
    Ok(staged)
}

#[allow(clippy::too_many_arguments)]
fn commit(
    store: &Store,
    topic: &str,
    bucket: &str,
    timestamp: &str,
    req: &EmitRequest,
    staged: &[StagedArtifact],
    staging: &Path,
) -> Result<EmitOutcome> {
    let conn = &store.conn;
    // IMMEDIATE takes the write lock up front, so concurrent emits serialize
    // here rather than discovering a conflict halfway through.
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch("BEGIN IMMEDIATE").ok();

    let now = crate::now_rfc3339();

    // Topics auto-create. Title and cadence are last-write-wins on every emit,
    // because there is no registration moment to attach them to.
    tx.execute(
        "INSERT INTO topics (slug, title, cadence, source_project, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(slug) DO UPDATE SET
             title          = COALESCE(excluded.title, topics.title),
             cadence        = COALESCE(excluded.cadence, topics.cadence),
             source_project = COALESCE(excluded.source_project, topics.source_project)",
        (topic, &req.title, &req.cadence, &req.source_project, &now),
    )?;

    let previous: Option<i64> = tx
        .query_row(
            "SELECT MAX(revision) FROM editions WHERE topic_slug = ?1 AND bucket = ?2",
            (topic, bucket),
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let revision = previous.unwrap_or(0) + 1;
    let superseded = previous.is_some();

    // Retire the outgoing edition first: the partial unique index permits only
    // one current revision per bucket.
    tx.execute(
        "UPDATE editions SET is_current = 0 WHERE topic_slug = ?1 AND bucket = ?2",
        (topic, bucket),
    )?;

    tx.execute(
        "INSERT INTO editions
             (topic_slug, bucket, revision, is_current, timestamp, summary, run_id, created_at)
         VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6, ?7)",
        (
            topic,
            bucket,
            revision,
            timestamp,
            &req.summary,
            &req.run_id,
            &now,
        ),
    )?;
    let edition_id = tx.last_insert_rowid();

    for (filename, role, origin, bytes) in staged {
        tx.execute(
            "INSERT INTO artifacts (edition_id, role, filename, origin_path, bytes)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            (edition_id, role.as_str(), filename, origin, *bytes as i64),
        )?;
    }
    for (key, value) in &req.tags {
        tx.execute(
            "INSERT INTO tags (edition_id, key, value) VALUES (?1, ?2, ?3)",
            (edition_id, key, value),
        )?;
    }

    // Move the artifacts into place inside the transaction, so a failure here
    // rolls the index back with it. rename(2) is atomic within a filesystem:
    // a half-written edition never appears under artifacts/.
    let target = store.artifacts_dir(topic, bucket, revision);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(staging, &target)
        .with_context(|| format!("installing edition at {}", target.display()))?;

    if let Err(err) = tx.commit() {
        // The index did not take, so the artifacts must not remain visible.
        let _ = std::fs::remove_dir_all(&target);
        return Err(err.into());
    }

    Ok(EmitOutcome {
        topic: topic.to_string(),
        bucket: bucket.to_string(),
        revision,
        superseded,
        artifact_count: staged.len(),
        warnings: Vec::new(),
    })
}
