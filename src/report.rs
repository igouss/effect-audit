//! Render findings into a human-readable report. Pure: findings in, string
//! out — `main` owns the actual printing.

use std::collections::BTreeSet;

use crate::finding::Finding;

/// How many distinct files the findings span (a manifest counts as a file).
pub fn distinct_files(findings: &[Finding]) -> usize {
    findings
        .iter()
        .map(|finding: &Finding| finding.file.as_str())
        .collect::<BTreeSet<&str>>()
        .len()
}

/// Render findings (and any stale baseline entries) as a JSON object, for
/// `--format json` and CI annotations:
/// `{"findings": [...], "stale_baseline": [...], "skipped_unparseable": [...]}`.
/// `skipped_unparseable` lists domain files `--skip-unparseable` tolerated; it
/// keeps a machine consumer from reading an empty `findings` array as "clean"
/// when some files were never actually audited.
pub fn render_json(findings: &[Finding], stale_baseline: &[String], skipped: &[String]) -> String {
    let value: serde_json::Value = serde_json::json!({
        "findings": findings.iter().map(Finding::to_json).collect::<Vec<_>>(),
        "stale_baseline": stale_baseline,
        "skipped_unparseable": skipped,
    });
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_owned())
}

/// Format the full report for a non-empty finding set, grouped by file and
/// ordered by line. Each line is `file:line  [kind]  snippet`, clickable in a
/// terminal, followed by the fix hint for each effect kind that appeared.
pub fn render(findings: &[Finding]) -> String {
    let mut sorted: Vec<&Finding> = findings.iter().collect();
    sorted.sort_by(|a: &&Finding, b: &&Finding| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));

    let mut out: String = String::new();
    out.push_str("\nFUNCTIONAL-CORE VIOLATION: effects leaked into the core.\n\n");
    out.push_str("  Bernhardt's rule: the functional core is pure (values in,\n");
    out.push_str("  values out); all effects live in the imperative shell.\n\n");

    let mut current_file: Option<&str> = None;
    for finding in &sorted {
        if current_file != Some(finding.file.as_str()) {
            out.push_str(&format!("  {}\n", finding.file));
            current_file = Some(finding.file.as_str());
        }
        let location: String = if finding.line == 0 {
            format!("    {}", finding.file)
        } else {
            format!("    {}:{}", finding.file, finding.line)
        };
        out.push_str(&format!(
            "{location}  [{}]  {}\n",
            finding.kind.label(),
            finding.snippet
        ));
    }

    out.push_str("\n  How to fix:\n");
    for kind in distinct_kinds(&sorted) {
        out.push_str(&format!("    [{}] {}\n", kind.label(), kind.hint()));
    }
    out.push_str("\n  To silence one deliberate effect: add `// fc-allow: <why>`\n");
    out.push_str("  on the offending line or the line above it.\n");
    out
}

/// The distinct effect kinds present, in first-seen order, so the hint block
/// lists each rule once.
fn distinct_kinds(findings: &[&Finding]) -> Vec<crate::effect::EffectKind> {
    let mut seen: Vec<crate::effect::EffectKind> = Vec::new();
    for finding in findings {
        if !seen.contains(&finding.kind) {
            seen.push(finding.kind);
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::EffectKind;

    #[test]
    fn render_lists_location_kind_and_a_single_hint_per_kind() {
        let findings: Vec<Finding> = vec![
            Finding {
                kind: EffectKind::Clock,
                file: "crates/a/src/lib.rs".to_owned(),
                line: 12,
                snippet: "SystemTime::now".to_owned(),
            },
            Finding {
                kind: EffectKind::Clock,
                file: "crates/a/src/lib.rs".to_owned(),
                line: 20,
                snippet: "Instant::now".to_owned(),
            },
        ];
        let text: String = render(&findings);
        assert!(text.contains("crates/a/src/lib.rs:12"));
        assert!(text.contains("[clock]"));
        // Two findings, one kind -> exactly one hint line for that kind.
        assert_eq!(text.matches(EffectKind::Clock.hint()).count(), 1);
    }
}
