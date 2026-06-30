//! The value the scanner produces: one located, classified effect.

use crate::effect::EffectKind;

/// A single side effect found in a domain crate, located precisely enough to
/// click on and described well enough to fix without re-reading the rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// What kind of effect leaked into the core.
    pub kind: EffectKind,
    /// Repo-relative path of the offending file (or its `Cargo.toml`).
    pub file: String,
    /// 1-based line number; `0` for manifest-level findings with no line.
    pub line: usize,
    /// The offending construct, reconstructed (e.g. `SystemTime::now`).
    pub snippet: String,
}

impl Finding {
    /// A stable one-line identity for the baseline ratchet and de-duplication.
    /// Excludes the line number so it survives unrelated edits to the same file
    /// — a baseline keyed on line numbers would churn on every commit.
    pub fn signature(&self) -> String {
        format!("{}\t{}\t{}", self.file, self.kind.label(), self.snippet)
    }

    /// The finding as a JSON object, for `--format json` / CI annotations.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "kind": self.kind.label(),
            "file": self.file,
            "line": self.line,
            "snippet": self.snippet,
        })
    }
}
