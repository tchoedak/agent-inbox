//! Tests for agent discoverability.
//!
//! The design bet is that the binary is the single source of truth for its own
//! instructions, and every harness adapter is a pointer at it. These tests
//! guard that bet: adapters must stay pointers, and installing must never
//! damage a file a human already wrote.

use std::process::Command;

use agent_inbox::agentdocs::{self, Target};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_agent-inbox"))
}

#[test]
fn the_guide_prints_without_a_store() {
    let empty = tempfile::tempdir().unwrap();

    // No AGENT_INBOX_HOME, no HOME: an agent must be able to read the contract
    // anywhere, including somewhere the inbox has never run.
    let output = bin()
        .arg("agent-guide")
        .env_remove("AGENT_INBOX_HOME")
        .env("HOME", empty.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("agent-inbox emit --topic"));
    assert!(text.contains("Never swallow the failure") || text.contains("never swallow"));
    // It must not have created a store as a side effect of printing docs.
    assert!(!empty.path().join(".local/share/agent-inbox").exists());
}

#[test]
fn the_guide_documents_every_flag_the_cli_accepts() {
    let help = String::from_utf8(bin().args(["emit", "--help"]).output().unwrap().stdout).unwrap();
    let guide = agentdocs::GUIDE;

    for flag in [
        "--topic",
        "--artifact",
        "--bucket",
        "--timestamp",
        "--title",
        "--cadence",
        "--summary",
        "--tag",
        "--run-id",
        "--source-project",
        "--stdin-name",
    ] {
        assert!(help.contains(flag), "emit --help lost {flag}");
        assert!(guide.contains(flag), "the guide never mentions {flag}");
    }
}

#[test]
fn installing_creates_a_pointer_not_a_copy() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".claude")).unwrap();

    let done = agentdocs::install(Target::Claude, home.path(), project.path()).unwrap();
    assert!(done.updated);

    let skill = std::fs::read_to_string(&done.path).unwrap();
    assert!(skill.starts_with("---\n"), "skills need frontmatter");
    assert!(skill.contains("name: agent-inbox"));
    assert!(skill.contains("agent-inbox agent-guide"));

    // The point of the design: adapters do not restate the contract, so they
    // cannot drift out of date when it changes.
    assert!(
        skill.len() < agentdocs::GUIDE.len() / 4,
        "the adapter is duplicating the guide instead of pointing at it"
    );
}

#[test]
fn installing_twice_changes_nothing_the_second_time() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();

    for target in Target::all() {
        let first = agentdocs::install(target, home.path(), project.path()).unwrap();
        assert!(first.updated, "{} should install", target.label());
        let after_first = std::fs::read_to_string(&first.path).unwrap();

        let second = agentdocs::install(target, home.path(), project.path()).unwrap();
        assert!(!second.updated, "{} rewrote itself", target.label());
        assert_eq!(after_first, std::fs::read_to_string(&second.path).unwrap());
    }
}

#[test]
fn installing_never_damages_a_human_written_agents_file() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let agents = project.path().join("AGENTS.md");

    let original = "# House rules\n\nAlways run the linter.\n";
    std::fs::write(&agents, original).unwrap();

    agentdocs::install(Target::AgentsMd, home.path(), project.path()).unwrap();
    let after = std::fs::read_to_string(&agents).unwrap();

    assert!(
        after.starts_with(original),
        "existing content was disturbed"
    );
    assert!(after.contains("## agent-inbox"));
    assert_eq!(after.matches("## agent-inbox").count(), 1);
}

#[test]
fn auto_detection_skips_harnesses_that_are_not_present() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".claude")).unwrap();

    assert!(Target::Claude.detected(home.path()));
    assert!(!Target::Codex.detected(home.path()));
    // AGENTS.md is a convention rather than an installation, so it always applies.
    assert!(Target::AgentsMd.detected(home.path()));
}

#[test]
fn install_reports_what_it_touched() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();

    let output = bin()
        .args(["install-agent-docs", "--target", "agents-md"])
        .args(["--project", project.path().to_str().unwrap()])
        .env("HOME", home.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("wrote agents-md"), "stdout: {stdout}");
    assert!(project.path().join("AGENTS.md").exists());
}

#[test]
fn an_unknown_target_fails_loudly() {
    let output = bin()
        .args(["install-agent-docs", "--target", "emacs"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown target"), "stderr: {stderr}");
}
