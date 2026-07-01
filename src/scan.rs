//! The AST scanner: walk one parsed Rust file and collect every effect that
//! leaks into the functional core.
//!
//! Why an AST walk and not `grep`: `syn` parses real tokens, so a comment that
//! says `// reads the clock` and a string literal `"std::fs"` produce zero
//! findings for free. We hold that line *everywhere* — test-gating is decided
//! by [`crate::cfg_pred`] over the parsed predicate, static types are matched by
//! identifier token, never by `str::contains` on a stringified blob.
//!
//! The scanner is itself a tiny functional core: `scan_file` is pure (syntax
//! in, findings out, no I/O), and the `&mut self` on the visitor is local
//! mutation of owned state — the acceptable kind, the same distinction this
//! tool exists to police.

use std::collections::BTreeSet;

use proc_macro2::{TokenStream, TokenTree};
use quote::ToTokens;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{
    Attribute, Expr, ExprAsync, ExprMethodCall, ExprPath, ImplItemFn, Item, ItemFn, ItemStatic,
    ItemUse, Macro, StaticMutability, Token, TypePath, UseTree,
};

use crate::effect::{self, EffectKind};
use crate::finding::Finding;
use crate::{cfg_pred, suppress};

/// Knobs for one scan. `flag_async` (driven by `--strict`) flags `async fn` and
/// `async { }` blocks in the core: async is effect-shaped — it threads a runtime
/// and suspension points through code that should be pure values-in/values-out —
/// even when no `tokio` dependency is present. Low confidence, hence opt-in.
///
/// `flag_hash` (also driven by `--strict`) flags a `HashMap`/`HashSet` in the
/// core's surface — imports, constructor calls, and type positions. Iteration
/// order is nondeterministic, so any order that escapes the domain smuggles in
/// hash-seed randomness. The finding witnesses the type's *presence*, not a
/// proven order leak, so it is a conservative over-approximation — hence opt-in,
/// with `fc-allow`/baseline as the pressure valves.
#[derive(Clone, Copy, Default)]
pub struct ScanConfig {
    pub flag_async: bool,
    pub flag_hash: bool,
}

/// Scan a parsed file for effect leakage. `rel_path` is the repo-relative path
/// used in findings; `source` is the raw text, needed for `fc-allow` lookups.
pub fn scan_file(
    parsed: &syn::File,
    rel_path: &str,
    source: &str,
    config: ScanConfig,
) -> Vec<Finding> {
    let suppressed: BTreeSet<usize> = suppress::suppressed_lines(source);
    let mut clock_aliases: BTreeSet<String> = BTreeSet::new();
    collect_clock_aliases(&parsed.items, &mut clock_aliases);
    let mut hash_aliases: BTreeSet<String> = BTreeSet::new();
    collect_hash_aliases(&parsed.items, &mut hash_aliases);
    let mut scanner: Scanner<'_> = Scanner {
        rel_path,
        suppressed,
        clock_aliases,
        hash_aliases,
        config,
        findings: Vec::new(),
    };
    scanner.visit_file(parsed);
    scanner.findings
}

/// Accumulates findings while walking the syntax tree.
struct Scanner<'a> {
    rel_path: &'a str,
    /// 1-based lines carrying a justified `fc-allow:` marker in a comment.
    suppressed: BTreeSet<usize>,
    /// Local names bound to a clock type via `use Clock as Alias`.
    clock_aliases: BTreeSet<String>,
    /// Local names bound to a hash type via `use HashMap as Alias`.
    hash_aliases: BTreeSet<String>,
    config: ScanConfig,
    findings: Vec<Finding>,
}

impl Scanner<'_> {
    /// Push a finding unless its line carries an `fc-allow` marker.
    fn record(&mut self, kind: EffectKind, line: usize, snippet: String) {
        if suppress::is_suppressed(&self.suppressed, line) {
            return;
        }
        self.findings.push(Finding {
            kind,
            file: self.rel_path.to_owned(),
            line,
            snippet,
        });
    }

    /// Record an async construct as an effect, but only under `--strict`.
    fn flag_async(&mut self, line: usize, snippet: &str) {
        if self.config.flag_async {
            self.record(EffectKind::AsyncRuntime, line, snippet.to_owned());
        }
    }

    /// Record a hash-collection presence finding, but only under `--strict`.
    fn flag_hash(&mut self, line: usize, snippet: String) {
        if self.config.flag_hash {
            self.record(EffectKind::HashIteration, line, snippet);
        }
    }

    /// A `now`-family call on a type aliased to a clock (`use Instant as I; I::now()`).
    fn is_aliased_clock(&self, segments: &[String]) -> bool {
        let aliased: bool = segments
            .first()
            .is_some_and(|head: &String| self.clock_aliases.contains(head));
        aliased && segments.iter().any(|seg: &String| seg.starts_with("now"))
    }

    /// A hash-collection expr path: any segment names a hash type
    /// (`HashMap::new`), or the head is a locally-aliased hash type
    /// (`use HashMap as Map; Map::new()`).
    fn expr_path_names_hash(&self, segments: &[String]) -> bool {
        segments
            .iter()
            .any(|seg: &String| effect::is_hash_type(seg))
            || segments
                .first()
                .is_some_and(|head: &String| self.hash_aliases.contains(head))
    }

    /// A hash-collection type path: the leaf segment names a hash type
    /// (`&HashMap<..>`, `std::collections::HashSet`) or a locally-aliased hash
    /// type (`Map`). The leaf is the operative segment in type position.
    fn type_path_names_hash(&self, segments: &[String]) -> bool {
        segments.last().is_some_and(|last: &String| {
            effect::is_hash_type(last) || self.hash_aliases.contains(last)
        })
    }
}

impl<'ast> Visit<'ast> for Scanner<'_> {
    /// The single test-gate chokepoint. `syn`'s `visit_file` routes *every*
    /// item — top-level and nested, of every variant — through `visit_item`,
    /// and module recursion comes back through it too. Gating here once (instead
    /// of re-checking in `visit_item_fn`/`_use`/`_static`/`_macro`/the `impl`
    /// block, which is how four kinds leaked) means a new `Item` variant can
    /// never silently re-open the hole: trust the `#[cfg]` gate, not the name,
    /// so a `mod tests` with no `#[cfg(test)]` is still production code.
    ///
    /// Method-level gating (a `#[cfg(test)] fn` inside a non-test `impl`) is a
    /// different scope and stays in `visit_impl_item_fn`.
    fn visit_item(&mut self, node: &'ast Item) {
        if cfg_pred::is_test_only(item_attrs(node)) {
            return;
        }
        visit::visit_item(self, node);
    }

    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        if let Some(token) = node.sig.asyncness {
            self.flag_async(
                token.span().start().line,
                &format!("async fn {}", node.sig.ident),
            );
        }
        visit::visit_item_fn(self, node);
    }

    fn visit_impl_item_fn(&mut self, node: &'ast ImplItemFn) {
        if cfg_pred::is_test_only(&node.attrs) {
            return;
        }
        if let Some(token) = node.sig.asyncness {
            self.flag_async(
                token.span().start().line,
                &format!("async fn {}", node.sig.ident),
            );
        }
        visit::visit_impl_item_fn(self, node);
    }

    fn visit_expr_async(&mut self, node: &'ast ExprAsync) {
        self.flag_async(node.async_token.span().start().line, "async block");
        visit::visit_expr_async(self, node);
    }

    fn visit_item_use(&mut self, node: &'ast ItemUse) {
        let mut paths: Vec<Vec<String>> = Vec::new();
        flatten_use_tree(&node.tree, &mut Vec::new(), &mut paths);
        let line: usize = node.span().start().line;
        for segments in paths {
            if let Some(kind) = effect::classify_path(&segments) {
                self.record(kind, line, format!("use {}", segments.join("::")));
            } else if segments
                .last()
                .is_some_and(|leaf: &String| effect::is_hash_type(leaf))
            {
                self.flag_hash(line, format!("use {}", segments.join("::")));
            }
        }
    }

    fn visit_expr_path(&mut self, node: &'ast ExprPath) {
        let segments: Vec<String> = node
            .path
            .segments
            .iter()
            .map(|seg: &syn::PathSegment| seg.ident.to_string())
            .collect();
        let line: usize = node.path.span().start().line;
        if let Some(kind) = effect::classify_path(&segments) {
            self.record(kind, line, segments.join("::"));
        } else if self.is_aliased_clock(&segments) {
            self.record(EffectKind::Clock, line, segments.join("::"));
        } else if self.expr_path_names_hash(&segments) {
            self.flag_hash(line, segments.join("::"));
        }
        visit::visit_expr_path(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        // A clock read spelled as a method (`instant.elapsed()`) carries no
        // qualifying type in the path, so the path visitor never sees it. We
        // flag it on the method name alone — see [`effect::is_clock_method`].
        let method: String = node.method.to_string();
        if effect::is_clock_method(&method) {
            let line: usize = node.method.span().start().line;
            self.record(EffectKind::Clock, line, format!(".{method}()"));
        }
        visit::visit_expr_method_call(self, node);
    }

    /// A hash collection in *type* position (`fn f(m: &HashMap<..>)`, a field, a
    /// return type). We record on the leaf segment, then recurse so the inner
    /// type of a nested `Vec<HashMap<..>>` is still reached. Gated by `flag_hash`
    /// (`--strict`), so default behaviour is unchanged.
    fn visit_type_path(&mut self, node: &'ast TypePath) {
        let segments: Vec<String> = node
            .path
            .segments
            .iter()
            .map(|seg: &syn::PathSegment| seg.ident.to_string())
            .collect();
        if self.type_path_names_hash(&segments) {
            let line: usize = node.path.span().start().line;
            self.flag_hash(line, segments.join("::"));
        }
        visit::visit_type_path(self, node);
    }

    fn visit_macro(&mut self, node: &'ast Macro) {
        let Some(name) = node.path.segments.last().map(|seg| seg.ident.to_string()) else {
            visit::visit_macro(self, node);
            return;
        };
        let line: usize = node.path.span().start().line;
        if let Some(kind) = effect::classify_macro(&name) {
            self.record(kind, line, format!("{name}!"));
        }
        // Macro-defined global state the `static` visitor cannot see: the
        // declaration lives inside an opaque token tree. Both `thread_local!`
        // and `lazy_static!` are gated the same way — an immutable
        // `thread_local! { static D: u32 = 0; }` hands out `&u32`, which you
        // cannot mutate, so it is not shared *mutable* state.
        if matches!(name.as_str(), "thread_local" | "lazy_static")
            && tokens_name_interior_mutability(&node.tokens)
        {
            self.record(EffectKind::SharedMutableState, line, format!("{name}!"));
        }
        // CHECK B: an effect written inside an *evaluated* macro argument
        // (`format!("{}", SystemTime::now())`) is a real call at runtime, so it
        // must be seen. For the fixed allowlist of std macros that evaluate
        // their arguments call-by-value, re-parse the token stream as
        // comma-separated expressions and re-enter the scanner on each — nested
        // effects then land with their real spans and line numbers, and a
        // nested allowlisted macro recurses back through here naturally. On ANY
        // parse failure we stay completely silent (sound by omission), and
        // every non-allowlisted macro or proc-macro keeps its opaque tokens.
        if effect::evaluates_arguments(&name) {
            if let Some(args) = parse_macro_args(&node.tokens) {
                for arg in &args {
                    self.visit_expr(arg);
                }
            }
        }
        visit::visit_macro(self, node);
    }

    fn visit_item_static(&mut self, node: &'ast ItemStatic) {
        let line: usize = node.span().start().line;
        if matches!(node.mutability, StaticMutability::Mut(_)) {
            self.record(
                EffectKind::SharedMutableState,
                line,
                format!("static mut {}", node.ident),
            );
        } else if tokens_name_interior_mutability(&node.ty.to_token_stream()) {
            self.record(
                EffectKind::SharedMutableState,
                line,
                format!("static {}", node.ident),
            );
        }
        visit::visit_item_static(self, node);
    }
}

/// The attributes carried by an item, across every variant that has them.
/// `Item` is `#[non_exhaustive]`; an unrecognised or attr-less variant yields an
/// empty slice, so the default is "not test-only" → audit it. That is the sound
/// direction: the tool may over-report on an exotic future variant, but it will
/// never silently *skip* one — preserving the "never invents… by never hiding"
/// contract from the soundness side.
fn item_attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(i) => &i.attrs,
        Item::Enum(i) => &i.attrs,
        Item::ExternCrate(i) => &i.attrs,
        Item::Fn(i) => &i.attrs,
        Item::ForeignMod(i) => &i.attrs,
        Item::Impl(i) => &i.attrs,
        Item::Macro(i) => &i.attrs,
        Item::Mod(i) => &i.attrs,
        Item::Static(i) => &i.attrs,
        Item::Struct(i) => &i.attrs,
        Item::Trait(i) => &i.attrs,
        Item::TraitAlias(i) => &i.attrs,
        Item::Type(i) => &i.attrs,
        Item::Union(i) => &i.attrs,
        Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

/// Whether any identifier token in a stream names interior mutability. Token
/// idents are whole identifiers, so this never trips on a substring.
fn tokens_name_interior_mutability(tokens: &TokenStream) -> bool {
    let mut idents: Vec<String> = Vec::new();
    collect_idents(tokens.clone(), &mut idents);
    idents
        .iter()
        .any(|ident: &String| effect::is_interior_mutability_ident(ident))
}

/// Parse a macro's token stream as comma-separated expressions
/// (`Punctuated<Expr, ,>`), returning the arguments on success and `None` on
/// ANY parse error. Parse-all-or-nothing is the sound-by-omission creed made
/// concrete: a token stream that is not a clean expression list — a proc-macro
/// DSL, a trailing pattern, a format grammar edge case — yields no arguments
/// and stays completely silent rather than risk a fabricated finding.
fn parse_macro_args(tokens: &TokenStream) -> Option<Punctuated<Expr, Token![,]>> {
    Punctuated::<Expr, Token![,]>::parse_terminated
        .parse2(tokens.clone())
        .ok()
}

/// Collect every identifier token in a stream, descending into delimited groups.
fn collect_idents(tokens: TokenStream, out: &mut Vec<String>) {
    for tree in tokens {
        match tree {
            TokenTree::Ident(ident) => out.push(ident.to_string()),
            TokenTree::Group(group) => collect_idents(group.stream(), out),
            TokenTree::Punct(_) | TokenTree::Literal(_) => {}
        }
    }
}

/// Collect local aliases bound to a clock type (`use std::time::Instant as I`),
/// recursing into non-test inline modules.
fn collect_clock_aliases(items: &[Item], out: &mut BTreeSet<String>) {
    for item in items {
        match item {
            Item::Use(item_use) => collect_clock_aliases_in_tree(&item_use.tree, out),
            Item::Mod(item_mod) if !cfg_pred::is_test_only(&item_mod.attrs) => {
                if let Some((_, inner)) = &item_mod.content {
                    collect_clock_aliases(inner, out);
                }
            }
            _ => {}
        }
    }
}

/// Walk a use tree, recording the alias whenever a clock type is renamed.
fn collect_clock_aliases_in_tree(tree: &UseTree, out: &mut BTreeSet<String>) {
    match tree {
        UseTree::Path(path) => collect_clock_aliases_in_tree(&path.tree, out),
        UseTree::Rename(rename) if effect::is_clock_type(&rename.ident.to_string()) => {
            out.insert(rename.rename.to_string());
        }
        UseTree::Group(group) => {
            for item in &group.items {
                collect_clock_aliases_in_tree(item, out);
            }
        }
        UseTree::Rename(_) | UseTree::Name(_) | UseTree::Glob(_) => {}
    }
}

/// Collect local aliases bound to a hash type (`use std::collections::HashMap as
/// Map`), recursing into non-test inline modules. Mirrors
/// [`collect_clock_aliases`] exactly — only the type predicate differs.
fn collect_hash_aliases(items: &[Item], out: &mut BTreeSet<String>) {
    for item in items {
        match item {
            Item::Use(item_use) => collect_hash_aliases_in_tree(&item_use.tree, out),
            Item::Mod(item_mod) if !cfg_pred::is_test_only(&item_mod.attrs) => {
                if let Some((_, inner)) = &item_mod.content {
                    collect_hash_aliases(inner, out);
                }
            }
            _ => {}
        }
    }
}

/// Walk a use tree, recording the alias whenever a hash type is renamed.
fn collect_hash_aliases_in_tree(tree: &UseTree, out: &mut BTreeSet<String>) {
    match tree {
        UseTree::Path(path) => collect_hash_aliases_in_tree(&path.tree, out),
        UseTree::Rename(rename) if effect::is_hash_type(&rename.ident.to_string()) => {
            out.insert(rename.rename.to_string());
        }
        UseTree::Group(group) => {
            for item in &group.items {
                collect_hash_aliases_in_tree(item, out);
            }
        }
        UseTree::Rename(_) | UseTree::Name(_) | UseTree::Glob(_) => {}
    }
}

/// Flatten a (possibly grouped, renamed, or globbed) use tree into a list of
/// fully-qualified segment paths, so each import can be classified on its own.
fn flatten_use_tree(tree: &UseTree, prefix: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            flatten_use_tree(&path.tree, prefix, out);
            prefix.pop();
        }
        UseTree::Name(name) => {
            let mut full: Vec<String> = prefix.clone();
            full.push(name.ident.to_string());
            out.push(full);
        }
        UseTree::Rename(rename) => {
            // Classify by the original name, not the local alias.
            let mut full: Vec<String> = prefix.clone();
            full.push(rename.ident.to_string());
            out.push(full);
        }
        UseTree::Glob(_) => out.push(prefix.clone()),
        UseTree::Group(group) => {
            for item in &group.items {
                flatten_use_tree(item, prefix, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(src: &str) -> Vec<Finding> {
        scan_with(src, ScanConfig::default())
    }

    fn scan_with(src: &str, config: ScanConfig) -> Vec<Finding> {
        let parsed: syn::File = syn::parse_file(src).expect("valid rust");
        scan_file(&parsed, "lib.rs", src, config)
    }

    fn kinds(src: &str) -> Vec<EffectKind> {
        scan(src).into_iter().map(|f: Finding| f.kind).collect()
    }

    #[test]
    fn async_fn_is_flagged_only_under_strict() {
        let src: &str = "async fn f() -> u8 { 1 }";
        assert_eq!(scan(src), Vec::new(), "default config ignores async");
        let strict: Vec<Finding> = scan_with(
            src,
            ScanConfig {
                flag_async: true,
                flag_hash: false,
            },
        );
        assert_eq!(
            strict
                .into_iter()
                .map(|f: Finding| f.kind)
                .collect::<Vec<_>>(),
            vec![EffectKind::AsyncRuntime]
        );
    }

    #[test]
    fn flags_a_clock_read_in_a_function() {
        assert_eq!(
            kinds("fn f() { let _ = std::time::SystemTime::now(); }"),
            vec![EffectKind::Clock]
        );
    }

    #[test]
    fn flags_an_elapsed_method_call_as_a_clock_read() {
        // `start.elapsed()` reads the clock through a method — the path scan is
        // blind to it, so the method visitor must catch it.
        assert_eq!(
            kinds("fn f(start: std::time::Instant) { let _ = start.elapsed(); }"),
            vec![EffectKind::Clock]
        );
    }

    #[test]
    fn duration_since_is_pure_subtraction_not_a_clock_read() {
        // `end.duration_since(start)` subtracts two held values — no clock read.
        assert_eq!(
            scan("fn f(a: std::time::Instant, b: std::time::Instant) { let _ = a.duration_since(b); }"),
            Vec::new()
        );
    }

    #[test]
    fn flags_an_aliased_clock_call_site() {
        // `use SystemTime as Clk; Clk::now()` (probe P9).
        let src: &str = "use std::time::SystemTime as Clk;\nfn f() { let _ = Clk::now(); }";
        assert_eq!(kinds(src), vec![EffectKind::Clock]);
    }

    #[test]
    fn ignores_a_clock_read_in_a_test_function() {
        assert_eq!(
            scan("#[test]\nfn t() { let _ = std::time::SystemTime::now(); }"),
            Vec::new()
        );
    }

    #[test]
    fn ignores_effects_inside_a_cfg_test_module() {
        let src: &str = "#[cfg(test)]\nmod tests {\n  fn t() { std::fs::read(\"x\").unwrap(); }\n}";
        assert_eq!(scan(src), Vec::new());
    }

    #[test]
    fn audits_a_module_named_tests_without_a_cfg_gate() {
        // No cfg(test) -> production code, even if named `tests` (probe P6).
        let src: &str = "mod tests {\n  fn f() { std::fs::read(\"x\").unwrap(); }\n}";
        assert_eq!(kinds(src), vec![EffectKind::FileSystem]);
    }

    #[test]
    fn audits_a_not_test_module() {
        // `#[cfg(not(test))]` is the real impl — must be audited.
        let src: &str =
            "#[cfg(not(test))]\nmod m {\n  fn f() { std::fs::read(\"x\").unwrap(); }\n}";
        assert_eq!(kinds(src), vec![EffectKind::FileSystem]);
    }

    #[test]
    fn audits_a_feature_gated_module_whose_name_contains_test() {
        let src: &str =
            "#[cfg(feature = \"fastest\")]\nmod m {\n  fn f() { std::fs::read(\"x\").unwrap(); }\n}";
        assert_eq!(kinds(src), vec![EffectKind::FileSystem]);
    }

    #[test]
    fn a_comment_or_string_mentioning_an_effect_is_not_flagged() {
        let src: &str = "fn f() -> &'static str {\n  // calls SystemTime::now somewhere\n  \"std::fs::read\"\n}";
        assert_eq!(scan(src), Vec::new());
    }

    #[test]
    fn flags_an_effectful_import() {
        assert_eq!(
            kinds("use tokio::sync::Mutex;\nfn f() {}"),
            vec![EffectKind::AsyncRuntime]
        );
    }

    #[test]
    fn flags_a_static_mutex_but_not_a_cellophane() {
        // `Mutex` is interior mutability; `Cellophane` only contains "Cell".
        assert!(
            kinds("static C: std::sync::Mutex<u32> = unsafe { todo!() };")
                .contains(&EffectKind::SharedMutableState)
        );
        assert_eq!(scan("static W: Cellophane = make();"), Vec::new());
    }

    #[test]
    fn flags_mutable_thread_local_and_lazy_static_macros() {
        assert!(
            kinds("thread_local! { static C: std::cell::RefCell<u32> = todo!(); }")
                .contains(&EffectKind::SharedMutableState)
        );
        assert!(kinds(
            "lazy_static::lazy_static! { static ref C: std::sync::Mutex<u32> = todo!(); }"
        )
        .contains(&EffectKind::SharedMutableState));
    }

    #[test]
    fn an_immutable_thread_local_is_not_shared_mutable_state() {
        // `thread_local! { static D: u32 = 0; }` hands out `&u32` — unmutable.
        assert_eq!(scan("thread_local! { static D: u32 = 0; }"), Vec::new());
    }

    #[test]
    fn flags_a_console_macro() {
        assert_eq!(
            kinds("fn f() { println!(\"hi\"); }"),
            vec![EffectKind::Console]
        );
    }

    #[test]
    fn an_fc_allow_marker_suppresses_the_finding() {
        let src: &str = "fn f() { let _ = std::time::SystemTime::now(); // fc-allow: shell\n}";
        assert_eq!(scan(src), Vec::new());
    }

    #[test]
    fn a_marker_inside_a_string_literal_does_not_suppress() {
        // The catastrophic false-negative: a string whose *contents* spell the
        // marker must NOT switch off a real effect. Suppression is comment-only.
        let src: &str = "fn f() { let _ = std::fs::read(\"fc-allow: not a real reason\"); }";
        assert_eq!(kinds(src), vec![EffectKind::FileSystem]);
    }

    #[test]
    fn a_marker_in_a_block_comment_suppresses() {
        let src: &str = "fn f() { let _ = std::fs::read(\"x\"); /* fc-allow: shell only */ }";
        assert_eq!(scan(src), Vec::new());
    }

    #[test]
    fn a_pure_function_yields_nothing() {
        assert_eq!(scan("fn add(a: u8, b: u8) -> u8 { a + b }"), Vec::new());
    }

    // ─── The test-gate is one chokepoint, not per-visitor ─────────────────
    // These exercise `#[cfg(test)]` on item kinds other than `fn`/`mod`. The
    // gap in this matrix was exactly the gap in the code: every visitor method
    // that re-implemented the gate could forget it, and four of them did.

    #[test]
    fn ignores_a_cfg_test_use_import() {
        assert_eq!(scan("#[cfg(test)]\nuse std::fs::read;"), Vec::new());
    }

    #[test]
    fn ignores_a_cfg_test_static() {
        assert_eq!(
            scan("#[cfg(test)]\nstatic SEEDED: std::sync::Mutex<u32> = unsafe { todo!() };"),
            Vec::new()
        );
    }

    #[test]
    fn ignores_a_cfg_test_static_mut() {
        assert_eq!(
            scan("#[cfg(test)]\nstatic mut COUNTER: u32 = 0;"),
            Vec::new()
        );
    }

    #[test]
    fn ignores_effects_in_a_cfg_test_impl_block() {
        // The gate is on the `impl`, not the method — only a chokepoint at the
        // item level catches this.
        let src: &str = "struct Order;\n#[cfg(test)]\nimpl Order {\n  fn touch(&self) { std::fs::read(\"x\").unwrap(); }\n}";
        assert_eq!(scan(src), Vec::new());
    }

    #[test]
    fn ignores_a_cfg_test_item_macro() {
        assert_eq!(
            scan("#[cfg(test)]\nthread_local! { static C: std::cell::RefCell<u32> = todo!(); }"),
            Vec::new()
        );
    }

    #[test]
    fn audits_a_not_test_static_mut() {
        // The dual must still fire: `#[cfg(not(test))]` is production state.
        assert!(kinds("#[cfg(not(test))]\nstatic mut COUNTER: u32 = 0;")
            .contains(&EffectKind::SharedMutableState));
    }

    // ─── CHECK A: hash-iteration presence (strict-only) ───────────────────
    // `HashMap`/`HashSet` iteration order is nondeterministic, so under
    // `--strict` their presence in the core's surface is flagged — imports,
    // constructors, and type positions. Structural, so a `struct HashMapper`, a
    // comment, and a string `"HashMap"` stay invisible. Default mode is silent.

    fn strict_hash() -> ScanConfig {
        ScanConfig {
            flag_async: false,
            flag_hash: true,
        }
    }

    fn strict_hash_kinds(src: &str) -> Vec<EffectKind> {
        scan_with(src, strict_hash())
            .into_iter()
            .map(|f: Finding| f.kind)
            .collect()
    }

    #[test]
    fn hash_import_and_turbofish_construct_are_two_findings_only_under_strict() {
        // many: the import and the constructor each land, and only under strict.
        // Turbofish (`HashMap::<u8,u8>::new()`) so there is no type annotation to
        // double-count — the count is exactly two (import + constructor).
        let src: &str =
            "use std::collections::HashMap;\nfn f() { let _ = HashMap::<u8, u8>::new(); }";
        assert_eq!(scan(src), Vec::new(), "default config ignores hash types");
        assert_eq!(
            strict_hash_kinds(src),
            vec![EffectKind::HashIteration, EffectKind::HashIteration]
        );
    }

    #[test]
    fn a_struct_named_hashmapper_a_comment_and_a_string_are_not_flagged() {
        // zero: the whole-identifier match never trips on a struct that merely
        // starts with `HashMap`, a comment, or a string literal.
        let src: &str = "struct HashMapper;\nfn f() -> &'static str {\n  // HashMap in a comment\n  \"HashMap\"\n}";
        assert_eq!(scan_with(src, strict_hash()), Vec::new());
    }

    #[test]
    fn a_hashset_import_is_flagged_under_strict() {
        // one: a bare `HashSet` import is a presence finding.
        assert_eq!(
            strict_hash_kinds("use std::collections::HashSet;\nfn f() {}"),
            vec![EffectKind::HashIteration]
        );
    }

    #[test]
    fn an_aliased_hash_constructor_is_flagged_under_strict() {
        // The alias two-pass records `Map`, so `Map::new()` is caught the same
        // way `use HashMap as Map` is (import finding + aliased-constructor).
        let src: &str = "use std::collections::HashMap as Map;\nfn f() { let _ = Map::new(); }";
        assert_eq!(
            strict_hash_kinds(src),
            vec![EffectKind::HashIteration, EffectKind::HashIteration]
        );
    }

    #[test]
    fn a_held_hashmap_parameter_is_a_presence_finding_under_strict() {
        // one, deliberate: holding a `HashMap` passed in from the shell is still
        // a presence finding — the type is in the core's surface. Conservative.
        assert_eq!(
            strict_hash_kinds("fn f(m: &HashMap<u8, u8>) { let _ = m; }"),
            vec![EffectKind::HashIteration]
        );
    }

    #[test]
    fn a_hashmap_nested_in_a_vec_type_is_still_reached() {
        // The `visit::visit_type_path` recursion must descend into generic args:
        // `Vec<HashMap<..>>` carries no hash leaf at the top, only inside.
        let src: &str = "fn f() -> Vec<std::collections::HashMap<u8, u8>> { Vec::new() }";
        assert_eq!(strict_hash_kinds(src), vec![EffectKind::HashIteration]);
    }

    #[test]
    fn a_cfg_test_hashmap_import_is_ignored_under_strict() {
        // The `visit_item` chokepoint gates hash findings for free, exactly like
        // every other effect kind — a test-only import is not core surface.
        assert_eq!(
            scan_with(
                "#[cfg(test)]\nuse std::collections::HashMap;",
                strict_hash()
            ),
            Vec::new()
        );
    }

    // ─── CHECK B: effects inside evaluated macro arguments (on by default) ─
    // An allowlisted std macro (`format!`, `println!`, `vec!`, `assert_eq!`, …)
    // evaluates its argument expressions call-by-value, so an effect written
    // there is a real runtime call and is scanned with its true span. Quoting
    // macros (`stringify!`), config macros (`cfg!`/`concat!`), and any macro off
    // the allowlist keep their tokens opaque — sound by omission, no fabrication.

    #[test]
    fn a_clock_inside_format_args_is_one_clock_finding() {
        // one: `format!` evaluates its interpolated argument, so the clock read
        // is a real call and lands as a single `clock` finding.
        assert_eq!(
            kinds("fn f() { let _ = format!(\"{}\", SystemTime::now()); }"),
            vec![EffectKind::Clock]
        );
    }

    #[test]
    fn a_clock_inside_a_quoting_or_config_or_unknown_macro_is_invisible() {
        // zero: `stringify!` quotes its input without evaluating it, `cfg!` and
        // `concat!` are compile-time and off the allowlist, and an unknown
        // `my_macro!` is an opaque proc-macro — none are scanned.
        assert_eq!(
            scan("fn f() { let _ = stringify!(SystemTime::now()); }"),
            Vec::new()
        );
        assert_eq!(scan("fn f() { let _ = cfg!(unix); }"), Vec::new());
        assert_eq!(
            scan("fn f() { let _ = concat!(\"a\", \"b\"); }"),
            Vec::new()
        );
        assert_eq!(scan("fn f() { my_macro!(SystemTime::now()); }"), Vec::new());
    }

    #[test]
    fn println_with_a_clock_argument_reports_console_and_clock() {
        // many: the macro name itself is a console effect AND its evaluated
        // argument reads the clock — two findings from one call.
        assert_eq!(
            kinds("fn f() { println!(\"{}\", SystemTime::now()); }"),
            vec![EffectKind::Console, EffectKind::Clock]
        );
    }

    #[test]
    fn a_clock_inside_vec_and_assert_eq_is_scanned() {
        // one each: `vec!` and `assert_eq!` both evaluate their arguments, so a
        // clock read inside either is a real call and is flagged.
        assert_eq!(
            kinds("fn f() { let _ = vec![SystemTime::now()]; }"),
            vec![EffectKind::Clock]
        );
        assert_eq!(
            kinds("fn f(a: std::time::SystemTime) { assert_eq!(a, SystemTime::now()); }"),
            vec![EffectKind::Clock]
        );
    }

    #[test]
    fn a_console_macro_with_a_pure_argument_stays_a_single_console_finding() {
        // Regression guard: scanning evaluated args must not perturb the
        // existing macro behaviour — `println!("hi")` is still exactly one
        // console finding, its string-literal argument carries no effect.
        assert_eq!(
            kinds("fn f() { println!(\"hi\"); }"),
            vec![EffectKind::Console]
        );
    }

    #[test]
    fn an_unparseable_macro_body_stays_silent_even_with_an_effect_inside() {
        // Parse-all-or-nothing, sound by omission: `matches!(now(), ref _x)`
        // holds a real clock read, but `ref _x` is a pattern, not an expression,
        // so the whole `Punctuated<Expr, ,>` parse fails and the macro is
        // skipped wholesale rather than risk fabricating (or half-scanning).
        assert_eq!(
            scan("fn f() { let _ = matches!(SystemTime::now(), ref _x); }"),
            Vec::new()
        );
    }
}
