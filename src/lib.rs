//! agent-inbox: a local report inbox for scheduled jobs.
//!
//! Producers call `agent-inbox emit` when they finish a report. The inbox
//! copies the artifacts into its own store, so history survives the producer
//! being tidied up or deleted outright.

pub mod agentdocs;
pub mod emit;
pub mod query;
pub mod slug;
pub mod store;

use chrono::SecondsFormat;

pub fn now_rfc3339() -> String {
    chrono::Local::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub fn today_bucket() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}
