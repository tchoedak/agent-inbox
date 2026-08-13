use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use agent_inbox::agentdocs::{self, Dirs, Target};
use agent_inbox::emit::{ArtifactSpec, EmitRequest, emit};
use agent_inbox::query;
use agent_inbox::store::Store;

#[derive(Parser)]
#[command(
    name = "agent-inbox",
    version,
    about = "A local inbox for scheduled reports"
)]
struct Cli {
    /// Store location. Defaults to $AGENT_INBOX_HOME, then ~/.local/share/agent-inbox.
    #[arg(long, global = true)]
    home: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Deliver a finished report into the inbox.
    Emit(Box<EmitArgs>),

    /// List topics and how much history each has.
    Topics,

    /// List the editions of one topic, newest first.
    Editions {
        #[arg(long)]
        topic: String,
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },

    /// Print the integration guide for coding agents.
    ///
    /// This is the authoritative instructions for the installed version. Every
    /// harness adapter points here rather than copying the contract.
    AgentGuide,

    /// Install harness adapters that point agents at `agent-inbox agent-guide`.
    InstallAgentDocs {
        /// claude, codex, agents-md, or all. Defaults to whatever is detected.
        #[arg(long)]
        target: Vec<String>,

        /// Directory to write AGENTS.md into. Defaults to the current directory.
        #[arg(long)]
        project: Option<std::path::PathBuf>,
    },
}

#[derive(clap::Args)]
struct EmitArgs {
    /// Topic name. Normalized to a slug, and created on first use.
    #[arg(long)]
    topic: String,

    /// An artifact: `path`, or `path:role` where role is terminal, primary, or data.
    /// Repeat for each file. Use `-` to read one artifact from stdin.
    // allow_hyphen_values so the documented stdin form `-:terminal` reaches
    // the parser instead of being taken for a flag.
    #[arg(long = "artifact", required = true, allow_hyphen_values = true)]
    artifacts: Vec<ArtifactSpec>,

    /// Grouping key, defaulting to today. Set it explicitly to backfill.
    #[arg(long)]
    bucket: Option<String>,

    /// When the report was produced. Defaults to now.
    #[arg(long)]
    timestamp: Option<String>,

    /// Human-readable topic title. Applied on every emit; last write wins.
    #[arg(long)]
    title: Option<String>,

    /// Expected cadence: daily, weekly, hourly, none.
    #[arg(long)]
    cadence: Option<String>,

    /// One-line summary of this edition, written by the producer.
    #[arg(long)]
    summary: Option<String>,

    /// Tag as key=value. Repeatable.
    #[arg(long = "tag")]
    tags: Vec<String>,

    /// Identifier for the run that produced this edition.
    #[arg(long)]
    run_id: Option<String>,

    /// Project this report came from.
    #[arg(long)]
    source_project: Option<String>,

    /// Filename to give the artifact read from stdin.
    #[arg(long)]
    stdin_name: Option<String>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            // Loud, and never silent: this runs under cron, where a swallowed
            // failure means a report that quietly stops arriving.
            eprintln!("agent-inbox: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();

    // Bare `agent-inbox` opens the reader.
    let Some(command) = cli.command else {
        let root = match cli.home {
            Some(path) => path,
            None => Store::default_root()?,
        };
        return agent_inbox::tui::run(&Store::open(&root)?);
    };

    // Printing the guide must work anywhere, including where no store exists.
    if let Command::AgentGuide = command {
        print!("{}", agentdocs::GUIDE);
        return Ok(());
    }
    if let Command::InstallAgentDocs { target, project } = command {
        return install_agent_docs(target, project);
    }

    let root = match cli.home {
        Some(path) => path,
        None => Store::default_root()?,
    };
    let store = Store::open(&root)?;

    match command {
        Command::AgentGuide | Command::InstallAgentDocs { .. } => unreachable!("handled above"),

        Command::Topics => {
            let topics = query::topics(&store)?;
            if topics.is_empty() {
                println!("no topics yet");
            }
            for t in topics {
                println!(
                    "{}\t{}\t{} edition{}\tlatest {}\t{}",
                    t.slug,
                    t.cadence.as_deref().unwrap_or("-"),
                    t.editions,
                    if t.editions == 1 { "" } else { "s" },
                    t.latest_bucket.as_deref().unwrap_or("-"),
                    t.title.as_deref().unwrap_or(""),
                );
            }
        }

        Command::Editions { topic, limit } => {
            let editions = query::editions(&store, &topic, limit)?;
            if editions.is_empty() {
                println!("no editions for topic `{topic}`");
            }
            for e in editions {
                println!(
                    "{}\trev {}\t{}\t{}",
                    e.bucket, e.revision, e.timestamp, e.artifacts
                );
            }
        }

        Command::Emit(args) => {
            let args = *args;
            let tags = args
                .tags
                .iter()
                .map(|raw| match raw.split_once('=') {
                    Some((k, v)) => Ok((k.to_string(), v.to_string())),
                    None => anyhow::bail!("tag `{raw}` is not in key=value form"),
                })
                .collect::<Result<Vec<_>>>()?;

            let outcome = emit(
                &store,
                EmitRequest {
                    topic: args.topic,
                    artifacts: args.artifacts,
                    bucket: args.bucket,
                    timestamp: args.timestamp,
                    title: args.title,
                    cadence: args.cadence,
                    summary: args.summary,
                    tags,
                    run_id: args.run_id,
                    source_project: args.source_project,
                    stdin_name: args.stdin_name,
                },
            )?;

            for warning in &outcome.warnings {
                eprintln!("agent-inbox: warning: {warning}");
            }

            let verb = if outcome.superseded {
                "superseded"
            } else {
                "delivered"
            };
            println!(
                "{verb} {}/{} rev {} ({} artifact{})",
                outcome.topic,
                outcome.bucket,
                outcome.revision,
                outcome.artifact_count,
                if outcome.artifact_count == 1 { "" } else { "s" },
            );
        }
    }

    Ok(())
}

fn install_agent_docs(targets: Vec<String>, project: Option<std::path::PathBuf>) -> Result<()> {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .context("HOME is not set, so harness config directories cannot be located")?;
    let project = match project {
        Some(path) => path,
        None => std::env::current_dir()?,
    };

    let dirs = Dirs::from_env(&home);

    let chosen: Vec<Target> = if targets.is_empty() {
        // Nothing named: install where a harness actually appears to live.
        Target::all()
            .into_iter()
            .filter(|t| t.detected(&dirs))
            .collect()
    } else if targets.iter().any(|t| t == "all") {
        Target::all().to_vec()
    } else {
        targets
            .iter()
            .map(|t| t.parse::<Target>())
            .collect::<Result<Vec<_>>>()?
    };

    for target in chosen {
        let done = agentdocs::install(target, &dirs, &project)?;
        println!(
            "{} {} -> {}",
            if done.updated { "wrote" } else { "unchanged" },
            done.target.label(),
            done.path.display()
        );
    }
    Ok(())
}
