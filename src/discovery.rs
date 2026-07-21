//! Imperative shell: ask cargo which crates make up the functional core and
//! where their source lives.
//!
//! Two roles qualify, and they are not held to the same standard. A
//! `role = "domain"` crate may vouch for a pure-value dependency through
//! `pure-deps`; a `role = "kernel"` crate may not vouch for anything, because a
//! crate whose premise is that it has no dependency graph by construction has
//! nothing to vouch for. Source scanning is identical for both.
//!
//! This deliberately reuses the exact marker the hex-arch dependency gate keys
//! on — `[package.metadata.hex-arch] role` — so the two gates always agree on
//! *which* crates are the functional core.

use std::path::Path;

use anyhow::{Context, Result};
use cargo_metadata::camino::{Utf8Path, Utf8PathBuf};
use cargo_metadata::{Dependency, DependencyKind, MetadataCommand, Package};

/// Which layer of the core a crate belongs to. Decides how strictly its
/// manifest is judged, and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// `role = "kernel"`: tolerates no normal dependency at all.
    Kernel,
    /// `role = "domain"`: tolerates dependencies it vouches for via `pure-deps`.
    Domain,
}

impl Role {
    /// The manifest spelling, for reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Kernel => "kernel",
            Self::Domain => "domain",
        }
    }
}

/// A core crate to audit: its role, source directory, the raw production-build
/// dependency names, and any declared pure-value allowlist. The *policy* (which
/// deps are findings) lives in [`crate::effect`]; this struct carries only the
/// facts read from the manifest.
#[derive(Debug)]
pub struct CoreCrate {
    /// Which core layer this crate declares.
    pub role: Role,
    /// Crate name, for reports.
    pub name: String,
    /// Repo-relative path to the crate's `Cargo.toml`.
    pub manifest_rel: String,
    /// Absolute path to the crate's `src/` directory.
    pub src_dir: Utf8PathBuf,
    /// Candidate normal dependency names from the manifest. Optional
    /// (feature-gated) deps are included only under `--strict`.
    pub deps: Vec<String>,
    /// The crate's declared pure-value allowlist from
    /// `[package.metadata.hex-arch] pure-deps`, or `None` when it declares none
    /// (legacy denylist mode). `Some(empty)` means "nothing is pure".
    pub pure_deps: Option<Vec<String>>,
}

impl CoreCrate {
    /// Repo-relative path to the crate's directory, used to build clickable
    /// file locations. Empty string when the crate is the workspace root (the
    /// manifest is a bare `Cargo.toml` with no directory prefix).
    pub fn dir_rel(&self) -> &str {
        match self.manifest_rel.rsplit_once('/') {
            Some((dir, _)) => dir, // "crates/foo/Cargo.toml" -> "crates/foo"
            None => "",            // "Cargo.toml" -> root crate
        }
    }
}

/// Discover every core crate (`role = "kernel"` or `role = "domain"`) in the
/// workspace rooted at `root`.
///
/// `strict` controls manifest dependency reporting: when false, only normal
/// non-optional deps are considered (the default-feature production build);
/// when true, optional (feature-gated) deps are flagged too — an `optional`
/// `reqwest` is still `reqwest` sitting in a domain crate.
pub fn core_crates(root: &Path, strict: bool) -> Result<Vec<CoreCrate>> {
    let metadata: cargo_metadata::Metadata = MetadataCommand::new()
        .current_dir(root)
        .no_deps()
        .exec()
        .context("cargo metadata failed")?;

    let workspace_root: &Utf8Path = metadata.workspace_root.as_path();
    let mut crates: Vec<CoreCrate> = metadata
        .packages
        .iter()
        .filter_map(|pkg: &Package| {
            role_of(pkg).map(|role: Role| to_core_crate(pkg, role, workspace_root, strict))
        })
        .collect();
    crates.sort_by(|a: &CoreCrate, b: &CoreCrate| a.name.cmp(&b.name));
    Ok(crates)
}

/// The core role a package declares, or `None` when it declares neither of the
/// two roles this tool audits.
fn role_of(pkg: &Package) -> Option<Role> {
    match pkg
        .metadata
        .get("hex-arch")
        .and_then(|hex: &serde_json::Value| hex.get("role"))
        .and_then(|role: &serde_json::Value| role.as_str())
    {
        Some("kernel") => Some(Role::Kernel),
        Some("domain") => Some(Role::Domain),
        _ => None,
    }
}

/// Build a [`CoreCrate`] from a metadata package.
fn to_core_crate(pkg: &Package, role: Role, workspace_root: &Utf8Path, strict: bool) -> CoreCrate {
    let crate_dir: &Utf8Path = pkg
        .manifest_path
        .parent()
        .unwrap_or(pkg.manifest_path.as_path());
    let manifest_rel: String = pkg
        .manifest_path
        .strip_prefix(workspace_root)
        .map(Utf8Path::to_string)
        .unwrap_or_else(|_| pkg.manifest_path.to_string());
    // Dev-dependencies are always excluded (they are for tests). Optional deps
    // are off in the default build, so excluded unless `strict`. The audit
    // otherwise judges the production functional core, not test scaffolding.
    let deps: Vec<String> = pkg
        .dependencies
        .iter()
        .filter(|dep: &&Dependency| dep.kind == DependencyKind::Normal && (strict || !dep.optional))
        .map(|dep: &Dependency| dep.name.clone())
        .collect();
    CoreCrate {
        role,
        name: pkg.name.clone(),
        manifest_rel,
        src_dir: crate_dir.join("src"),
        deps,
        pure_deps: read_pure_deps(&pkg.metadata),
    }
}

/// Read `[package.metadata.hex-arch] pure-deps` as a list of crate names.
///
/// Returns `None` when the key is absent *or malformed* (not an array) — in
/// either case the crate falls back to the legacy denylist. `Some(empty)` for an
/// explicit empty array, which means "nothing is pure". Pure: JSON in, list out.
fn read_pure_deps(metadata: &serde_json::Value) -> Option<Vec<String>> {
    let array: &Vec<serde_json::Value> = metadata
        .get("hex-arch")
        .and_then(|hex: &serde_json::Value| hex.get("pure-deps"))?
        .as_array()?;
    Some(
        array
            .iter()
            .filter_map(|item: &serde_json::Value| item.as_str().map(str::to_owned))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crate_with_manifest(manifest_rel: &str) -> CoreCrate {
        CoreCrate {
            role: Role::Domain,
            name: "x".to_owned(),
            manifest_rel: manifest_rel.to_owned(),
            src_dir: Utf8PathBuf::from("/x/src"),
            deps: Vec::new(),
            pure_deps: None,
        }
    }

    #[test]
    fn dir_rel_of_a_nested_crate_is_its_directory() {
        assert_eq!(
            crate_with_manifest("crates/foo/Cargo.toml").dir_rel(),
            "crates/foo"
        );
    }

    #[test]
    fn dir_rel_of_a_root_crate_is_empty() {
        // Regression: a workspace-root crate must not render "Cargo.toml".
        assert_eq!(crate_with_manifest("Cargo.toml").dir_rel(), "");
    }

    #[test]
    fn pure_deps_absent_is_none() {
        let meta: serde_json::Value = serde_json::json!({ "hex-arch": { "role": "domain" } });
        assert_eq!(read_pure_deps(&meta), None);
    }

    #[test]
    fn pure_deps_present_parses_the_array() {
        let meta: serde_json::Value =
            serde_json::json!({ "hex-arch": { "pure-deps": ["serde", "uuid"] } });
        assert_eq!(
            read_pure_deps(&meta),
            Some(vec!["serde".to_owned(), "uuid".to_owned()])
        );
    }

    #[test]
    fn an_empty_pure_deps_array_is_some_empty() {
        let meta: serde_json::Value = serde_json::json!({ "hex-arch": { "pure-deps": [] } });
        assert_eq!(read_pure_deps(&meta), Some(Vec::new()));
    }

    #[test]
    fn a_malformed_non_array_pure_deps_falls_back_to_none() {
        // A string, not an array -> treat as absent (legacy denylist), not a crash.
        let meta: serde_json::Value = serde_json::json!({ "hex-arch": { "pure-deps": "serde" } });
        assert_eq!(read_pure_deps(&meta), None);
    }
}
