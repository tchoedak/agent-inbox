//! Contract tests for `agent-inbox emit`.
//!
//! These assert the promises the emit contract makes to producers, since those
//! are what agents will write into arbitrary projects and rely on for months.

use std::path::Path;
use std::process::Command;

use agent_inbox::emit::{ArtifactSpec, EmitRequest, emit};
use agent_inbox::store::Store;

fn request(topic: &str, artifacts: Vec<ArtifactSpec>) -> EmitRequest {
    EmitRequest {
        topic: topic.to_string(),
        artifacts,
        bucket: Some("2026-08-13".to_string()),
        timestamp: None,
        title: None,
        cadence: None,
        summary: None,
        tags: Vec::new(),
        run_id: None,
        source_project: None,
        stdin_name: None,
    }
}

fn spec(path: &Path) -> ArtifactSpec {
    path.display().to_string().parse().unwrap()
}

fn write(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    path
}

fn fixture() -> (tempfile::TempDir, tempfile::TempDir, Store) {
    let store_dir = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let store = Store::open(store_dir.path()).unwrap();
    (store_dir, work, store)
}

#[test]
fn delivers_an_edition_and_copies_the_artifact() {
    let (_s, work, store) = fixture();
    let report = write(work.path(), "report.md", "# today\n");

    let out = emit(&store, request("trading-perf", vec![spec(&report)])).unwrap();

    assert_eq!(out.topic, "trading-perf");
    assert_eq!(out.revision, 1);
    assert!(!out.superseded);

    // The artifact is a plain file in the documented layout, not a blob in the DB.
    let landed = store
        .artifacts_dir("trading-perf", "2026-08-13", 1)
        .join("report.md");
    assert_eq!(std::fs::read_to_string(&landed).unwrap(), "# today\n");

    // Deleting the producer's copy must not affect the archive.
    std::fs::remove_file(&report).unwrap();
    assert!(landed.exists());
}

#[test]
fn infers_roles_from_extension_and_honours_explicit_ones() {
    let (_s, work, store) = fixture();
    let md = write(work.path(), "report.md", "md");
    let html = write(work.path(), "report.html", "<p>html</p>");
    let csv = write(work.path(), "rows.csv", "a,b");
    let odd = write(work.path(), "notes.log", "log");

    let mut req = request(
        "mixed",
        vec![
            spec(&md),
            spec(&html),
            spec(&csv),
            format!("{}:data", odd.display()).parse().unwrap(),
        ],
    );
    req.bucket = Some("2026-08-13".into());
    emit(&store, req).unwrap();

    let mut roles: Vec<(String, String)> = store
        .conn
        .prepare("SELECT filename, role FROM artifacts ORDER BY filename")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    roles.sort();

    assert_eq!(
        roles,
        vec![
            ("notes.log".to_string(), "data".to_string()),
            ("report.html".to_string(), "primary".to_string()),
            ("report.md".to_string(), "terminal".to_string()),
            ("rows.csv".to_string(), "data".to_string()),
        ]
    );
}

#[test]
fn an_unknown_extension_without_a_role_is_rejected() {
    let (_s, work, store) = fixture();
    let odd = write(work.path(), "notes.log", "log");

    let err = emit(&store, request("odd", vec![spec(&odd)])).unwrap_err();
    assert!(
        format!("{err:#}").contains("cannot infer a role"),
        "unhelpful error: {err:#}"
    );
}

#[test]
fn separator_and_case_variants_are_one_topic() {
    let (_s, work, store) = fixture();
    let report = write(work.path(), "r.md", "x");

    for name in ["trading-perf", "trading_perf", "Trading Perf"] {
        let mut req = request(name, vec![spec(&report)]);
        req.bucket = Some("2026-08-13".into());
        emit(&store, req).unwrap();
    }

    assert_eq!(store.topic_slugs().unwrap(), vec!["trading-perf"]);
}

#[test]
fn a_rerun_supersedes_and_retains_the_previous_revision() {
    let (_s, work, store) = fixture();
    let first = write(work.path(), "r.md", "first");
    emit(&store, request("perf", vec![spec(&first)])).unwrap();

    let second = write(work.path(), "r.md", "second");
    let out = emit(&store, request("perf", vec![spec(&second)])).unwrap();

    assert_eq!(out.revision, 2);
    assert!(out.superseded);

    // Exactly one current edition, and it is the new one.
    let current: Vec<i64> = store
        .conn
        .prepare("SELECT revision FROM editions WHERE is_current = 1")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(current, vec![2]);

    // The superseded revision is retained and still readable.
    let old = store.artifacts_dir("perf", "2026-08-13", 1).join("r.md");
    assert_eq!(std::fs::read_to_string(old).unwrap(), "first");
    let new = store.artifacts_dir("perf", "2026-08-13", 2).join("r.md");
    assert_eq!(std::fs::read_to_string(new).unwrap(), "second");
}

#[test]
fn topic_title_and_cadence_are_last_write_wins() {
    let (_s, work, store) = fixture();
    let report = write(work.path(), "r.md", "x");

    let mut first = request("perf", vec![spec(&report)]);
    first.title = Some("Old title".into());
    first.cadence = Some("daily".into());
    emit(&store, first).unwrap();

    let mut second = request("perf", vec![spec(&report)]);
    second.title = Some("New title".into());
    emit(&store, second).unwrap();

    let (title, cadence): (String, String) = store
        .conn
        .query_row("SELECT title, cadence FROM topics", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(title, "New title");
    // An omitted flag leaves the existing value alone rather than clearing it.
    assert_eq!(cadence, "daily");
}

#[test]
fn a_near_miss_warns_and_records_but_still_delivers() {
    let (_s, work, store) = fixture();
    let report = write(work.path(), "r.md", "x");

    emit(&store, request("trading-perf", vec![spec(&report)])).unwrap();
    let out = emit(&store, request("trading-perf-daily", vec![spec(&report)])).unwrap();

    assert_eq!(out.warnings.len(), 1, "expected a near-miss warning");
    assert_eq!(out.revision, 1, "the report must still land");

    let recorded: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM warnings WHERE kind = 'slug-near-miss'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        recorded, 1,
        "the warning must survive cron discarding stderr"
    );
}

#[test]
fn a_missing_artifact_writes_nothing_at_all() {
    let (_s, work, store) = fixture();
    let good = write(work.path(), "good.md", "x");
    let missing = work.path().join("nope.md");

    let err = emit(&store, request("perf", vec![spec(&good), spec(&missing)])).unwrap_err();
    assert!(format!("{err:#}").contains("cannot be read"), "{err:#}");

    // No edition, no topic, and critically no half-written artifact directory.
    let editions: i64 = store
        .conn
        .query_row("SELECT COUNT(*) FROM editions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(editions, 0);
    assert!(!store.root.join("artifacts/perf").exists());
    assert_eq!(
        staging_entries(&store),
        0,
        "staging must not be left behind"
    );
}

#[test]
fn backfilling_an_older_bucket_lands_in_that_bucket() {
    let (_s, work, store) = fixture();
    let report = write(work.path(), "r.md", "old news");

    let mut req = request("perf", vec![spec(&report)]);
    req.bucket = Some("2026-07-21".into());
    emit(&store, req).unwrap();

    let bucket: String = store
        .conn
        .query_row("SELECT bucket FROM editions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(bucket, "2026-07-21");
    assert!(
        store
            .artifacts_dir("perf", "2026-07-21", 1)
            .join("r.md")
            .exists()
    );
}

#[test]
fn duplicate_filenames_in_one_edition_are_rejected() {
    let (_s, work, store) = fixture();
    let a = write(work.path(), "r.md", "a");
    let nested = work.path().join("sub");
    std::fs::create_dir(&nested).unwrap();
    let b = write(&nested, "r.md", "b");

    let err = emit(&store, request("perf", vec![spec(&a), spec(&b)])).unwrap_err();
    assert!(format!("{err:#}").contains("share the filename"), "{err:#}");
}

#[test]
fn concurrent_emits_to_one_bucket_both_land() {
    let store_dir = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    Store::open(store_dir.path()).unwrap();

    let handles: Vec<_> = (0..4)
        .map(|i| {
            let root = store_dir.path().to_path_buf();
            let file = write(work.path(), &format!("r{i}.md"), "x");
            std::thread::spawn(move || {
                let store = Store::open(&root).unwrap();
                emit(&store, request("perf", vec![spec(&file)])).unwrap()
            })
        })
        .collect();

    let mut revisions: Vec<i64> = handles
        .into_iter()
        .map(|h| h.join().unwrap().revision)
        .collect();
    revisions.sort();

    // Every emit got its own revision: no two clobbered each other.
    assert_eq!(revisions, vec![1, 2, 3, 4]);

    let store = Store::open(store_dir.path()).unwrap();
    let current: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM editions WHERE is_current = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(current, 1, "exactly one revision may be current");
}

#[test]
fn the_binary_exits_non_zero_and_says_why() {
    let store_dir = tempfile::tempdir().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_agent-inbox"))
        .args(["--home", store_dir.path().to_str().unwrap()])
        .args(["emit", "--topic", "perf", "--artifact", "/nope/missing.md"])
        .output()
        .unwrap();

    assert!(!output.status.success(), "cron must see a real failure");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("agent-inbox:"), "stderr was: {stderr}");
    assert!(stderr.contains("missing.md"), "stderr was: {stderr}");
}

#[test]
fn the_binary_delivers_a_minimal_call() {
    let store_dir = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let report = write(work.path(), "report.md", "# hello\n");

    let output = Command::new(env!("CARGO_BIN_EXE_agent-inbox"))
        .args(["--home", store_dir.path().to_str().unwrap()])
        .args([
            "emit",
            "--topic",
            "trading-perf",
            "--artifact",
            report.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("delivered trading-perf"),
        "stdout: {stdout}"
    );
}

fn staging_entries(store: &Store) -> usize {
    std::fs::read_dir(store.root.join(".staging"))
        .map(|d| d.count())
        .unwrap_or(0)
}

#[test]
fn an_artifact_can_arrive_on_stdin() {
    use std::io::Write;
    use std::process::Stdio;

    let store_dir = tempfile::tempdir().unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_agent-inbox"))
        .args(["--home", store_dir.path().to_str().unwrap()])
        .args([
            "emit",
            "--topic",
            "piped",
            "--artifact",
            "-:terminal",
            "--stdin-name",
            "digest.md",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"# piped report\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());

    let store = Store::open(store_dir.path()).unwrap();
    let bucket: String = store
        .conn
        .query_row("SELECT bucket FROM editions", [], |r| r.get(0))
        .unwrap();
    let landed = store.artifacts_dir("piped", &bucket, 1).join("digest.md");
    assert_eq!(std::fs::read_to_string(landed).unwrap(), "# piped report\n");
}

#[test]
fn stdin_without_a_name_is_rejected() {
    let store_dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_agent-inbox"))
        .args(["--home", store_dir.path().to_str().unwrap()])
        .args(["emit", "--topic", "piped", "--artifact", "-:terminal"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--stdin-name"), "stderr was: {stderr}");
}
