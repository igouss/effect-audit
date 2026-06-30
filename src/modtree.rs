//! Resolve which source files are reachable *only* through a test-gated
//! module, so the scanner can skip them.
//!
//! Why this is needed: the cfg gate that makes a module test-only lives on the
//! `mod` declaration in the parent, not in the module's own file. A linter that
//! parses files independently never sees it. Concretely:
//!
//! ```ignore
//! // src/lib.rs
//! #[cfg(test)]
//! mod test_helpers;          // <- the gate is HERE, not in test_helpers.rs
//! ```
//!
//! Parsing `src/test_helpers.rs` on its own shows no gate, so its effects would
//! be flagged as production. The fix is to start at the crate root, follow
//! `mod` declarations, and exclude any subtree entered through a *test-only*
//! `mod` (one [`crate::cfg_pred`] deems unsatisfiable without `test`). A name
//! heuristic ("skip anything called `test_helpers`") would be both over- and
//! under-inclusive; following the actual module graph is correct.
//!
//! Note the deliberate boundary: a module gated `#[cfg(any(test, feature =
//! "x"))]` is *not* test-only — it compiles in a non-test build when feature
//! `x` is on — so it is audited, not skipped. Exempt such a module with an
//! explicit `fc-allow` or a baseline entry, never a silent name match.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use syn::{Attribute, Expr, ExprLit, Item, ItemMod, Lit, Meta};

use crate::cfg_pred;

/// Exclusion roots (files and directories) under `src_dir` that are reachable
/// only via a test-gated `mod`. A candidate file is skipped when any returned
/// root is a path-prefix of it.
pub fn test_gated_roots(src_dir: &Path) -> BTreeSet<PathBuf> {
    let mut excluded: BTreeSet<PathBuf> = BTreeSet::new();
    let Some(root_file) = crate_root(src_dir) else {
        return excluded;
    };
    // Worklist of (module file to parse, directory its submodules live in).
    let mut worklist: Vec<(PathBuf, PathBuf)> = vec![(root_file, src_dir.to_path_buf())];
    while let Some((file, mod_dir)) = worklist.pop() {
        let Ok(source) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Ok(parsed) = syn::parse_file(&source) else {
            continue;
        };
        collect(&parsed.items, &mod_dir, &mut worklist, &mut excluded);
    }
    excluded
}

/// The crate root file: `lib.rs` then `main.rs`, whichever exists.
fn crate_root(src_dir: &Path) -> Option<PathBuf> {
    ["lib.rs", "main.rs"]
        .into_iter()
        .map(|name: &str| src_dir.join(name))
        .find(|path: &PathBuf| path.is_file())
}

/// Walk the items of one module, queueing non-test submodules and recording
/// test-gated ones for exclusion.
fn collect(
    items: &[Item],
    mod_dir: &Path,
    worklist: &mut Vec<(PathBuf, PathBuf)>,
    excluded: &mut BTreeSet<PathBuf>,
) {
    for item in items {
        let Item::Mod(item_mod) = item else {
            continue;
        };
        match &item_mod.content {
            // Inline module: its submodules live in `<mod_dir>/<name>/`. A
            // test-gated inline module is already skipped by the AST scanner,
            // so only recurse into non-test ones.
            Some((_, inner)) => {
                if cfg_pred::is_test_only(&item_mod.attrs) {
                    continue;
                }
                let child_dir: PathBuf = mod_dir.join(item_mod.ident.to_string());
                collect(inner, &child_dir, worklist, excluded);
            }
            // External module (`mod foo;`): resolve its file and submodule dir.
            None => visit_external(item_mod, mod_dir, worklist, excluded),
        }
    }
}

/// Handle a `mod foo;` declaration: exclude the whole subtree if it is
/// test-gated, otherwise queue its file for further walking.
fn visit_external(
    item_mod: &ItemMod,
    mod_dir: &Path,
    worklist: &mut Vec<(PathBuf, PathBuf)>,
    excluded: &mut BTreeSet<PathBuf>,
) {
    let name: String = item_mod.ident.to_string();
    // `#[path = "..."]` overrides the conventional file location. Resolve the
    // file, then derive the module's *own* submodule directory from it — never
    // the file's parent, which for a `#[path]` file would be the whole src dir
    // and over-exclude every sibling.
    let child_file: Option<PathBuf> = match path_attr(&item_mod.attrs) {
        Some(rel) => {
            let file: PathBuf = mod_dir.join(rel);
            file.is_file().then_some(file)
        }
        None => resolve_external_file(mod_dir, &name),
    };
    let child_dir: PathBuf = child_file
        .as_deref()
        .map_or_else(|| mod_dir.join(&name), submodule_dir);

    if cfg_pred::is_test_only(&item_mod.attrs) {
        // Exclude both the single-file form and the directory-module subtree.
        if let Some(file) = child_file {
            excluded.insert(file);
        }
        excluded.insert(child_dir);
        return;
    }
    if let Some(file) = child_file {
        worklist.push((file, child_dir));
    }
}

/// The directory a module's own submodules live in, given its file:
/// `foo/mod.rs` → `foo/`; `foo.rs` (or a `#[path]` file) → `foo/`. Narrow by
/// construction, so excluding it never reaches a sibling module's file.
fn submodule_dir(file: &Path) -> PathBuf {
    if file.file_name() == Some(std::ffi::OsStr::new("mod.rs")) {
        return file.parent().map_or_else(PathBuf::new, Path::to_path_buf);
    }
    let parent: &Path = file.parent().unwrap_or(Path::new(""));
    match file.file_stem() {
        Some(stem) => parent.join(stem),
        None => parent.to_path_buf(),
    }
}

/// Resolve `mod name;` to its file: `<dir>/name.rs`, else `<dir>/name/mod.rs`.
fn resolve_external_file(mod_dir: &Path, name: &str) -> Option<PathBuf> {
    let flat: PathBuf = mod_dir.join(format!("{name}.rs"));
    if flat.is_file() {
        return Some(flat);
    }
    let nested: PathBuf = mod_dir.join(name).join("mod.rs");
    nested.is_file().then_some(nested)
}

/// The `#[path = "..."]` override on a `mod` declaration, if any.
fn path_attr(attrs: &[Attribute]) -> Option<String> {
    attrs.iter().find_map(|attr: &Attribute| {
        if !attr.path().is_ident("path") {
            return None;
        }
        match &attr.meta {
            Meta::NameValue(name_value) => match &name_value.value {
                Expr::Lit(ExprLit {
                    lit: Lit::Str(string),
                    ..
                }) => Some(string.value()),
                _ => None,
            },
            _ => None,
        }
    })
}

/// Whether `path` falls under any exclusion root.
pub fn is_excluded(roots: &BTreeSet<PathBuf>, path: &Path) -> bool {
    roots.iter().any(|root: &PathBuf| path.starts_with(root))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `gated` fixture: lib.rs gates `skipme` under `#[cfg(test)]` and
    /// `prod` under `#[cfg(not(test))]`.
    fn gated_src() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gated/src")
    }

    #[test]
    fn excludes_cfg_test_module_but_keeps_not_test_module() {
        let roots: BTreeSet<PathBuf> = test_gated_roots(&gated_src());
        assert!(
            is_excluded(&roots, &gated_src().join("skipme.rs")),
            "#[cfg(test)] mod must be excluded"
        );
        assert!(
            !is_excluded(&roots, &gated_src().join("prod.rs")),
            "#[cfg(not(test))] mod must be audited"
        );
    }

    #[test]
    fn excludes_a_path_attribute_module() {
        // `#[cfg(test)] #[path = "custom_path.rs"] mod relocated;`
        let roots: BTreeSet<PathBuf> = test_gated_roots(&gated_src());
        assert!(
            is_excluded(&roots, &gated_src().join("custom_path.rs")),
            "a #[path] test module must be excluded by its real file"
        );
    }

    #[test]
    fn a_missing_crate_root_yields_no_exclusions() {
        let roots: BTreeSet<PathBuf> = test_gated_roots(Path::new("/no/such/dir/src"));
        assert!(roots.is_empty());
    }

    #[test]
    fn a_lib_with_no_gated_modules_excludes_nothing() {
        let roots: BTreeSet<PathBuf> = test_gated_roots(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/clean/src"),
        );
        assert!(roots.is_empty());
    }
}
