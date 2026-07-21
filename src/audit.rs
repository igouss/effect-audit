//! Orchestration shell: walk a domain crate's manifest and source, producing
//! findings. The I/O lives here; the per-construct judgement lives in the pure
//! [`crate::scan`] / [`crate::effect`] core.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use ignore::WalkBuilder;

use crate::discovery::{CoreCrate, Role};
use crate::effect::{self, EffectKind};
use crate::finding::Finding;
use crate::modtree;
use crate::scan::{self, ScanConfig};

/// Policy for one audit run. Wraps the pure [`ScanConfig`] with the one shell
/// decision the scanner cannot make: what to do with a file `syn` cannot parse.
#[derive(Clone, Copy, Default)]
pub struct AuditConfig {
    /// Knobs for the pure per-file scan (e.g. `--strict` async flagging).
    pub scan: ScanConfig,
    /// Tolerate a domain file that fails to parse instead of failing the run.
    /// Off by default: an unparseable `role = "domain"` file is a tool error,
    /// because the alternative is claiming "functional core holds" over code we
    /// never read. On (via `--skip-unparseable`) the file is recorded as skipped
    /// and excluded from any clean claim — useful when `syn` lags a nightly.
    pub skip_unparseable: bool,
}

/// The result of an audit: the findings, plus the domain files that were
/// tolerated-but-skipped because they would not parse (only ever populated when
/// [`AuditConfig::skip_unparseable`] is set — otherwise a parse failure aborts).
/// The skipped list exists so the shell can refuse to print a clean verdict over
/// code it never read.
#[derive(Default)]
pub struct AuditOutcome {
    pub findings: Vec<Finding>,
    pub skipped: Vec<String>,
}

/// Audit every domain crate: manifest deps plus source files.
pub fn audit(crates: &[CoreCrate], config: AuditConfig) -> Result<AuditOutcome> {
    let kernels: BTreeSet<&str> = kernel_names(crates);
    let mut outcome: AuditOutcome = AuditOutcome::default();
    for domain in crates {
        outcome.findings.extend(manifest_findings(domain, &kernels));
        source_findings(domain, config, &mut outcome)?;
    }
    Ok(outcome)
}

/// Turn a core crate's dependencies into manifest-level findings.
///
/// A domain crate is judged by its allowlist (or the legacy denylist). A kernel
/// crate is judged by a stricter rule that takes no allowlist at all: every
/// normal dependency is a finding, named by its effect where the taxonomy knows
/// one and as an unvetted dependency otherwise. A `pure-deps` list on a kernel
/// crate is therefore inert — [`crate::discovery::CoreCrate::vouches_in_vain`]
/// is what says so out loud.
pub fn manifest_findings(domain: &CoreCrate, kernels: &BTreeSet<&str>) -> Vec<Finding> {
    let pure_deps: Option<&[String]> = domain.pure_deps.as_deref();
    domain
        .deps
        .iter()
        .filter_map(|name: &String| {
            classify(domain.role, name, pure_deps, kernels).map(|kind: EffectKind| Finding {
                kind,
                file: domain.manifest_rel.clone(),
                line: 0,
                snippet: format!("dependency: {name}"),
            })
        })
        .collect()
}

/// The dependency policy for one role: the kernel tolerates nothing, the domain
/// tolerates what it vouches for.
fn classify(
    role: Role,
    name: &str,
    pure_deps: Option<&[String]>,
    kernels: &BTreeSet<&str>,
) -> Option<EffectKind> {
    match role {
        // The kernel layer is closed under itself: another kernel crate in the
        // same workspace is the one dependency that carries no new surface.
        // This is the same line hex-lint's role matrix draws, and the two gates
        // disagreeing about the floor of the system would be its own defect.
        Role::Kernel if kernels.contains(name) => None,
        Role::Kernel => Some(effect::unvetted_or_known(name)),
        Role::Domain => effect::classify_dependency(name, pure_deps),
    }
}

/// The names of every `role = "kernel"` crate in the workspace.
fn kernel_names(crates: &[CoreCrate]) -> BTreeSet<&str> {
    crates
        .iter()
        .filter(|c: &&CoreCrate| c.role == Role::Kernel)
        .map(|c: &CoreCrate| c.name.as_str())
        .collect()
}

/// Scan every production `.rs` file under a domain crate's `src/` directory,
/// skipping files reachable only through a test-gated module. Findings and any
/// tolerated-skip notices are accumulated into `outcome`.
pub fn source_findings(
    domain: &CoreCrate,
    config: AuditConfig,
    outcome: &mut AuditOutcome,
) -> Result<()> {
    let src_dir: &Path = domain.src_dir.as_std_path();
    if !src_dir.is_dir() {
        return Ok(());
    }
    let test_gated: BTreeSet<PathBuf> = modtree::test_gated_roots(src_dir);
    for entry in WalkBuilder::new(src_dir).build() {
        let entry: ignore::DirEntry = entry.context("walk src dir")?;
        let path: &Path = entry.path();
        if path.extension().and_then(OsStr::to_str) != Some("rs") {
            continue;
        }
        if modtree::is_excluded(&test_gated, path) {
            continue;
        }
        scan_one_file(domain, src_dir, path, config, outcome)?;
    }
    Ok(())
}

/// Parse and scan a single file, building a repo-relative path for findings.
///
/// A file `syn` cannot parse is a **tool error** by default (exit 2 at the
/// shell): an unread `role = "domain"` file cannot be vouched for, and silently
/// skipping it while still printing "functional core holds" is the catastrophic
/// false-green this whole tool exists to prevent. Only `--skip-unparseable`
/// downgrades it to a recorded skip — and even then the file is added to
/// `outcome.skipped` so the clean verdict is withheld.
fn scan_one_file(
    domain: &CoreCrate,
    src_dir: &Path,
    path: &Path,
    config: AuditConfig,
    outcome: &mut AuditOutcome,
) -> Result<()> {
    let tail: &Path = path.strip_prefix(src_dir).unwrap_or(path);
    let rel: String = source_rel_path(domain.dir_rel(), &tail.to_string_lossy());
    let source: String =
        std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    match syn::parse_file(&source) {
        Ok(parsed) => {
            outcome
                .findings
                .extend(scan::scan_file(&parsed, &rel, &source, config.scan));
            Ok(())
        }
        Err(err) if config.skip_unparseable => {
            eprintln!("effect-audit: skipping unparseable domain file {rel}: {err}");
            outcome.skipped.push(rel);
            Ok(())
        }
        Err(err) => bail!(
            "cannot parse domain file {rel}: {err}\n  \
             (this file is unaudited; pass --skip-unparseable to tolerate it, \
             e.g. for nightly syntax `syn` cannot yet read)"
        ),
    }
}

/// Build the repo-relative `src/...` path, without a leading slash when the
/// crate is the workspace root (its `dir_rel` is empty).
fn source_rel_path(dir_rel: &str, tail: &str) -> String {
    if dir_rel.is_empty() {
        format!("src/{tail}")
    } else {
        format!("{dir_rel}/src/{tail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_crate_path_has_no_leading_slash_or_cargo_toml() {
        // Regression: a workspace-root domain crate must render `src/lib.rs`,
        // not `Cargo.toml/src/lib.rs` or `/src/lib.rs`.
        assert_eq!(source_rel_path("", "lib.rs"), "src/lib.rs");
    }

    #[test]
    fn nested_crate_path_keeps_its_directory() {
        assert_eq!(
            source_rel_path("crates/foo", "bar/baz.rs"),
            "crates/foo/src/bar/baz.rs"
        );
    }
}
