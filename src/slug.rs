//! Topic identity: normalization, and detection of near-misses against
//! existing topics. Per the map, a near-miss warns and records; it never
//! fails an emit and never silently merges topics.

use strsim::levenshtein;

/// Normalize a topic name to its canonical slug.
///
/// `trading_perf`, `Trading Perf`, and `--trading--perf--` are one topic.
pub fn normalize(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut pending_sep = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_sep && !out.is_empty() {
                out.push('-');
            }
            pending_sep = false;
            out.extend(ch.to_lowercase());
        } else {
            pending_sep = true;
        }
    }
    out
}

/// Find existing slugs close enough to `slug` to be worth warning about.
///
/// Deliberately conservative: a false positive is noise in the TUI, but an
/// over-eager match would train the human to ignore the warning entirely.
pub fn near_misses<'a>(slug: &str, existing: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    existing
        .into_iter()
        .filter(|other| *other != slug)
        .filter(|other| is_near(slug, other))
        .map(str::to_owned)
        .collect()
}

fn is_near(a: &str, b: &str) -> bool {
    // One name being a prefix-extension of the other is the case that actually
    // happens: `trading-perf` and `trading-perf-daily`.
    if a.starts_with(b) || b.starts_with(a) {
        return true;
    }
    // Otherwise require real closeness, scaled to length so short slugs do not
    // collide with everything.
    let shortest = a.len().min(b.len());
    if shortest < 5 {
        return false;
    }
    levenshtein(a, b) <= 1 + shortest / 12
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_separators_and_case() {
        assert_eq!(normalize("trading_perf"), "trading-perf");
        assert_eq!(normalize("Trading Perf"), "trading-perf");
        assert_eq!(normalize("--trading--perf--"), "trading-perf");
        assert_eq!(normalize("TradingPerf"), "tradingperf");
    }

    #[test]
    fn flags_prefix_extensions() {
        let found = near_misses("trading-perf-daily", ["trading-perf"]);
        assert_eq!(found, vec!["trading-perf"]);
    }

    #[test]
    fn flags_single_character_typos() {
        let found = near_misses("tradingperf", ["tradingperx"]);
        assert_eq!(found, vec!["tradingperx"]);
    }

    #[test]
    fn leaves_genuinely_different_topics_alone() {
        let found = near_misses("job-scrape", ["trading-perf", "frontier-radar"]);
        assert!(found.is_empty(), "unexpected near miss: {found:?}");
    }

    #[test]
    fn does_not_flag_short_unrelated_slugs() {
        assert!(near_misses("cpu", ["gpu"]).is_empty());
    }
}
