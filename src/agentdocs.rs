//! Making agent-inbox discoverable to coding agents, whatever harness they run in.
//!
//! The binary is the source of truth for its own instructions: `agent-inbox
//! agent-guide` prints the full integration doc. Every harness adapter is a
//! stub that points at that command rather than a copy of its content.
//!
//! That choice is what makes this portable. There is one document to maintain
//! instead of one per harness, adapters cannot drift out of date when the
//! contract changes, and a harness nobody has written an adapter for still
//! works as long as its agent can run a shell command.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// The canonical guide, compiled into the binary so it always matches the
/// version that is actually installed.
pub const GUIDE: &str = include_str!("../docs/AGENT_GUIDE.md");

/// One-line trigger description, shared by every adapter so that what makes an
/// agent reach for the inbox is defined once.
const TRIGGER: &str = "Use when building or modifying anything that produces a report, digest, \
summary, or export on a recurring schedule (cron, launchd, systemd timer, CI schedule, or a \
scheduled agent routine), so its output is delivered to the local agent-inbox instead of left as \
a stray file. Also use when asked to wire a project into agent-inbox, or to check what scheduled \
reports exist.";

const BEGIN: &str = "<!-- agent-inbox:begin -->";
const END: &str = "<!-- agent-inbox:end -->";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A Claude Code skill: frontmatter carries the trigger, body points at the guide.
    Claude,
    /// `~/.codex/AGENTS.md`, read by Codex across projects.
    Codex,
    /// An `AGENTS.md` in the current project, the cross-harness convention.
    AgentsMd,
}

impl Target {
    pub fn all() -> [Target; 3] {
        [Target::Claude, Target::Codex, Target::AgentsMd]
    }

    pub fn label(self) -> &'static str {
        match self {
            Target::Claude => "claude",
            Target::Codex => "codex",
            Target::AgentsMd => "agents-md",
        }
    }

    /// The harness's config directory.
    ///
    /// Both of these are relocatable by environment variable, and people do
    /// relocate them - a config dir at the default path is an assumption, not a
    /// fact. Writing an adapter to the wrong directory fails silently: the file
    /// exists, and the agent never reads it.
    pub fn config_dir(self, home: &Path) -> Option<PathBuf> {
        let from_env = |var: &str| std::env::var_os(var).map(PathBuf::from);
        match self {
            Target::Claude => Some(from_env("CLAUDE_CONFIG_DIR").unwrap_or(home.join(".claude"))),
            Target::Codex => Some(from_env("CODEX_HOME").unwrap_or(home.join(".codex"))),
            Target::AgentsMd => None,
        }
    }

    /// Where this adapter lives. `project` is the directory an AGENTS.md would
    /// be written into.
    pub fn path(self, home: &Path, project: &Path) -> PathBuf {
        match self {
            Target::Claude => self
                .config_dir(home)
                .expect("claude has a config dir")
                .join("skills/agent-inbox/SKILL.md"),
            Target::Codex => self
                .config_dir(home)
                .expect("codex has a config dir")
                .join("AGENTS.md"),
            Target::AgentsMd => project.join("AGENTS.md"),
        }
    }

    /// Whether this harness looks present, so auto-detection can skip the rest.
    /// AGENTS.md is always eligible: it is a convention, not an installation.
    pub fn detected(self, home: &Path) -> bool {
        match self.config_dir(home) {
            Some(dir) => dir.is_dir(),
            None => true,
        }
    }
}

impl std::str::FromStr for Target {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "claude" => Target::Claude,
            "codex" => Target::Codex,
            "agents-md" | "agents" => Target::AgentsMd,
            other => bail!("unknown target `{other}` (expected claude, codex, or agents-md)"),
        })
    }
}

/// A Claude Code skill. The frontmatter description is the whole trigger
/// mechanism, so it carries the conditions; the body stays a pointer.
fn claude_skill() -> String {
    format!(
        "---\nname: agent-inbox\ndescription: {TRIGGER}\n---\n\n\
         # agent-inbox\n\n\
         Run `agent-inbox agent-guide` and follow what it prints.\n\n\
         It is the authoritative integration guide for the installed version, covering when to \
         wire a producer in, the exact `emit` call, artifact roles, topic naming, and how to \
         verify delivery. Do not rely on remembered details of the contract - print the guide.\n"
    )
}

/// A managed block for an AGENTS.md-style file. Delimited so it can be
/// rewritten in place without disturbing anything the human wrote around it.
fn agents_block() -> String {
    format!(
        "{BEGIN}\n\
         ## agent-inbox\n\n\
         {TRIGGER}\n\n\
         Run `agent-inbox agent-guide` for the authoritative integration guide: when to wire a \
         producer in, the exact `emit` call, artifact roles, topic naming, and how to verify \
         delivery. Print it rather than relying on remembered details.\n\n\
         Quick reference: `agent-inbox emit --topic <slug> --artifact <path>[:<role>]`, where role \
         is `terminal`, `primary`, or `data`. Never swallow a non-zero exit.\n\
         {END}\n"
    )
}

pub struct Installed {
    pub target: Target,
    pub path: PathBuf,
    pub updated: bool,
}

pub fn install(target: Target, home: &Path, project: &Path) -> Result<Installed> {
    let path = target.path(home, project);
    let parent = path
        .parent()
        .context("adapter path has no parent directory")?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;

    let (contents, updated) = match target {
        Target::Claude => {
            let new = claude_skill();
            let updated = std::fs::read_to_string(&path)
                .map(|old| old != new)
                .unwrap_or(true);
            (new, updated)
        }
        Target::Codex | Target::AgentsMd => {
            let existing = std::fs::read_to_string(&path).unwrap_or_default();
            let merged = splice_block(&existing, &agents_block());
            let updated = merged != existing;
            (merged, updated)
        }
    };

    if updated {
        std::fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))?;
    }

    Ok(Installed {
        target,
        path,
        updated,
    })
}

/// Replace an existing managed block, or append one, leaving surrounding
/// human-written content untouched.
fn splice_block(existing: &str, block: &str) -> String {
    if let (Some(start), Some(end)) = (existing.find(BEGIN), existing.find(END))
        && start < end
    {
        let mut out = String::with_capacity(existing.len() + block.len());
        out.push_str(&existing[..start]);
        out.push_str(block.trim_end());
        out.push_str(&existing[end + END.len()..]);
        return out;
    }
    if existing.trim().is_empty() {
        return block.to_string();
    }
    let mut out = existing.to_string();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');
    out.push_str(block);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_config_dir_honours_the_environment() {
        // Regression: the installer hardcoded ~/.claude, so on a machine with
        // CLAUDE_CONFIG_DIR set the skill was written where nothing reads it.
        // The install reported success and the agent never saw it.
        let home = Path::new("/home/someone");
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", "/home/someone/.claude-personal") };
        assert_eq!(
            Target::Claude.path(home, Path::new("/tmp")),
            Path::new("/home/someone/.claude-personal/skills/agent-inbox/SKILL.md")
        );
        unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") };
        assert_eq!(
            Target::Claude.path(home, Path::new("/tmp")),
            Path::new("/home/someone/.claude/skills/agent-inbox/SKILL.md")
        );
    }

    #[test]
    fn the_guide_is_embedded_and_non_trivial() {
        assert!(GUIDE.contains("agent-inbox emit --topic"));
        assert!(GUIDE.len() > 2000, "guide looks truncated");
    }

    #[test]
    fn the_claude_skill_carries_triggers_in_frontmatter() {
        let skill = claude_skill();
        assert!(skill.starts_with("---\nname: agent-inbox\n"));
        assert!(skill.contains("recurring schedule"));
        // The body must stay a pointer, not a copy of the contract.
        assert!(skill.contains("agent-inbox agent-guide"));
        assert!(!skill.contains("--stdin-name"));
    }

    #[test]
    fn appending_preserves_existing_content() {
        let out = splice_block("# My project\n\nSome notes.\n", &agents_block());
        assert!(out.starts_with("# My project\n\nSome notes.\n"));
        assert!(out.contains("## agent-inbox"));
    }

    #[test]
    fn reinstalling_replaces_the_block_rather_than_duplicating_it() {
        let first = splice_block("# Mine\n", &agents_block());
        let second = splice_block(&first, &agents_block());
        assert_eq!(first, second);
        assert_eq!(second.matches("## agent-inbox").count(), 1);
    }

    #[test]
    fn updating_a_block_leaves_surrounding_text_alone() {
        let existing = format!("# Top\n\n{BEGIN}\nold content\n{END}\n\n# Bottom\n");
        let out = splice_block(&existing, &agents_block());
        assert!(out.starts_with("# Top\n"));
        assert!(out.trim_end().ends_with("# Bottom"));
        assert!(!out.contains("old content"));
    }
}
