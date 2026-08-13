//! Turning an artifact into lines a terminal can show.
//!
//! Two paths, per the rendering decision: a `terminal` artifact is markdown and
//! is styled directly, while HTML falls back to `w3m -dump`, which is the only
//! converter tried that has a real table layout engine.

use std::path::Path;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Render an artifact for display, given its role.
pub fn artifact_lines(path: &Path, role: &str) -> Vec<Line<'static>> {
    match std::fs::read_to_string(path) {
        Ok(_) if role == "primary" && is_html(path) => match html_to_text(path) {
            Some(text) => plain_lines(&text),
            None => vec![
                warn("This edition only has an HTML artifact, and w3m is not installed."),
                Line::from(""),
                warn("Install w3m to read HTML here, or press o to open it in a browser."),
                warn("Better: have the producer also emit a markdown `terminal` artifact."),
            ],
        },
        Ok(body) if is_html(path) => match html_to_text(path) {
            Some(text) => plain_lines(&text),
            None => plain_lines(&body),
        },
        Ok(body) => markdown_lines(&body),
        Err(err) => vec![warn(&format!("cannot read {}: {err}", path.display()))],
    }
}

fn is_html(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("html") | Some("htm")
    )
}

fn html_to_text(path: &Path) -> Option<String> {
    let out = std::process::Command::new("w3m")
        .args(["-dump", "-cols", "100"])
        .arg(path)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn warn(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::default().fg(Color::Yellow),
    ))
}

fn plain_lines(body: &str) -> Vec<Line<'static>> {
    body.lines().map(|l| Line::from(l.to_string())).collect()
}

/// A deliberately small markdown renderer. Reports are headings, lists, tables,
/// and emphasis - handling those well beats handling everything badly.
fn markdown_lines(body: &str) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut in_code = false;

    for raw in body.lines() {
        let line = raw.trim_end();

        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            out.push(Line::from(Span::styled(
                format!("  {line}"),
                Style::default().fg(Color::Cyan),
            )));
            continue;
        }

        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("### ") {
            out.push(Line::from(Span::styled(
                rest.to_string(),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            )));
        } else if let Some(rest) = trimmed.strip_prefix("## ") {
            out.push(Line::from(""));
            out.push(Line::from(Span::styled(
                rest.to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
        } else if let Some(rest) = trimmed.strip_prefix("# ") {
            out.push(Line::from(Span::styled(
                rest.to_string(),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )));
        } else if trimmed.starts_with("---") && trimmed.chars().all(|c| c == '-') {
            out.push(Line::from(Span::styled(
                "─".repeat(60),
                Style::default().fg(Color::DarkGray),
            )));
        } else if let Some(rest) = trimmed.strip_prefix("- ").or(trimmed.strip_prefix("* ")) {
            let mut spans = vec![Span::styled("  • ", Style::default().fg(Color::DarkGray))];
            spans.extend(inline(rest));
            out.push(Line::from(spans));
        } else if trimmed.starts_with('|') {
            // Tables are already aligned by the producer. Monospace does the work.
            out.push(Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(Color::Gray),
            )));
        } else {
            out.push(Line::from(inline(line)));
        }
    }
    out
}

/// Bold and code spans. Anything else is left as written rather than guessed at.
fn inline(text: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut rest = text;

    while let Some(start) = rest.find("**") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("**") else { break };
        if start > 0 {
            spans.push(Span::raw(rest[..start].to_string()));
        }
        spans.push(Span::styled(
            after[..end].to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ));
        rest = &after[end + 2..];
    }
    if !rest.is_empty() {
        spans.push(Span::raw(rest.to_string()));
    }
    if spans.is_empty() {
        spans.push(Span::raw(text.to_string()));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn headings_lists_and_bold_survive() {
        let out = markdown_lines("# Title\n\n- **Accounts:** 7\n- Total: 5\n");
        let text = text_of(&out);
        assert!(text.contains("Title"));
        assert!(text.contains("• Accounts: 7"), "got: {text}");
        assert!(text.contains("• Total: 5"));
    }

    #[test]
    fn table_rows_are_left_intact() {
        let out = markdown_lines("| Symbol | Qty |\n| --- | --- |\n| AMZN | 2 |\n");
        let text = text_of(&out);
        assert!(text.contains("| Symbol | Qty |"));
        assert!(text.contains("| AMZN | 2 |"));
    }

    #[test]
    fn unmatched_emphasis_does_not_eat_the_line() {
        let out = markdown_lines("a **dangling emphasis marker\n");
        assert_eq!(text_of(&out), "a **dangling emphasis marker");
    }
}
