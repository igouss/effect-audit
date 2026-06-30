//! The baseline ratchet: a checked-in file of *accepted* finding signatures.
//!
//! Inline `fc-allow` is right for a handful of deliberate exceptions; it does
//! not scale to adopting this gate on a codebase that already has hundreds of
//! leaks. The baseline does: the audit fails only on findings *not* in the
//! file, so a team can freeze today's debt and block new debt from day one.
//!
//! It can only shrink. A baseline entry that matches no current finding is
//! *stale* — the leak was fixed (good) but the file now lies, so a stale entry
//! is itself a failure that forces a `--update-baseline`. Same discipline as
//! `hex-lint-exceptions.toml`.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result};

use crate::finding::Finding;

/// The result of measuring a finding set against a baseline.
pub struct Ratchet {
    /// Findings absent from the baseline — new debt; these fail the build.
    pub fresh: Vec<Finding>,
    /// Baseline signatures that matched nothing this run — stale entries that
    /// must be removed (the ratchet only shrinks).
    pub stale: Vec<String>,
}

/// Load accepted signatures from a baseline file. Blank lines and `#` comments
/// are ignored. A missing file is an empty baseline (nothing accepted).
pub fn load(path: &Path) -> Result<BTreeSet<String>> {
    if !path.exists() {
        return Ok(BTreeSet::new());
    }
    let text: String = std::fs::read_to_string(path)
        .with_context(|| format!("read baseline {}", path.display()))?;
    Ok(parse(&text))
}

/// Parse baseline text into a set of signatures.
fn parse(text: &str) -> BTreeSet<String> {
    text.lines()
        .map(str::trim)
        .filter(|line: &&str| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

/// Partition findings against the baseline into fresh (unaccepted) and stale
/// (accepted-but-gone) sets.
pub fn apply(findings: Vec<Finding>, baseline: &BTreeSet<String>) -> Ratchet {
    let mut matched: BTreeSet<String> = BTreeSet::new();
    let mut fresh: Vec<Finding> = Vec::new();
    for finding in findings {
        let signature: String = finding.signature();
        if baseline.contains(&signature) {
            matched.insert(signature);
        } else {
            fresh.push(finding);
        }
    }
    let stale: Vec<String> = baseline.difference(&matched).cloned().collect();
    Ratchet { fresh, stale }
}

/// Render a baseline file from the current findings, sorted and de-duplicated.
pub fn render(findings: &[Finding]) -> String {
    let signatures: BTreeSet<String> = findings.iter().map(Finding::signature).collect();
    let mut out: String = String::new();
    out.push_str("# effect-audit baseline — accepted effects in the functional core.\n");
    out.push_str("# Format: <file>\\t<kind>\\t<snippet>. Ratchet: this file may only shrink.\n");
    out.push_str("# Regenerate with: effect-audit --baseline <this-file> --update-baseline\n");
    for signature in &signatures {
        out.push_str(signature);
        out.push('\n');
    }
    out
}

/// Write the baseline file for `--update-baseline`.
pub fn write(path: &Path, findings: &[Finding]) -> Result<()> {
    std::fs::write(path, render(findings))
        .with_context(|| format!("write baseline {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::EffectKind;

    fn finding(file: &str, snippet: &str) -> Finding {
        Finding {
            kind: EffectKind::Clock,
            file: file.to_owned(),
            line: 1,
            snippet: snippet.to_owned(),
        }
    }

    #[test]
    fn a_baselined_finding_is_not_fresh() {
        let f: Finding = finding("a.rs", "SystemTime::now");
        let base: BTreeSet<String> = parse(&f.signature());
        let ratchet: Ratchet = apply(vec![f], &base);
        assert!(ratchet.fresh.is_empty());
        assert!(ratchet.stale.is_empty());
    }

    #[test]
    fn an_unbaselined_finding_is_fresh() {
        let ratchet: Ratchet = apply(vec![finding("a.rs", "Utc::now")], &BTreeSet::new());
        assert_eq!(ratchet.fresh.len(), 1);
    }

    #[test]
    fn a_baseline_entry_with_no_matching_finding_is_stale() {
        let base: BTreeSet<String> = parse("old.rs\tclock\tInstant::now");
        let ratchet: Ratchet = apply(Vec::new(), &base);
        assert_eq!(
            ratchet.stale,
            vec!["old.rs\tclock\tInstant::now".to_owned()]
        );
    }

    #[test]
    fn parse_ignores_comments_and_blanks() {
        let parsed: BTreeSet<String> = parse("# header\n\na.rs\tclock\tx\n");
        assert_eq!(parsed.len(), 1);
    }
}
