//! Decide whether an item is test-only by evaluating its `#[cfg(...)]`
//! predicate *structurally* — never by string matching.
//!
//! The bug this kills: `tokens.to_string().contains("test")` treats
//! `#[cfg(feature = "fastest")]` as test-only (skipping real code) and, worse,
//! treats `#[cfg(not(test))]` — the canonical "real impl, swapped under test"
//! pattern — as test-only too, so the audit silently drops the one module it
//! most needs to see. Both are the same disease: throwing the parse away.
//!
//! The rule: an item is test-only iff it cannot be compiled when the built-in
//! `test` cfg is false. We evaluate satisfiability of the predicate with
//! `test = false` and every other atom (features, target_os, …) left free.
//! If the predicate can still be true, the item appears in some non-test build
//! and must be audited; if it cannot, it is test-only and is skipped.

use syn::punctuated::Punctuated;
use syn::{Attribute, Meta, MetaList, Token};

/// Whether any attribute marks the item as test-only.
pub fn is_test_only(attrs: &[Attribute]) -> bool {
    attrs.iter().any(is_test_attr)
}

/// One attribute: a `#[test]`/`#[tokio::test]` marker, or a `#[cfg(...)]` whose
/// predicate is unsatisfiable when `test` is false.
fn is_test_attr(attr: &Attribute) -> bool {
    let path: &syn::Path = attr.path();
    if path.is_ident("cfg") {
        return match &attr.meta {
            Meta::List(list) => match syn::parse2::<Meta>(list.tokens.clone()) {
                Ok(pred) => !can_be_true_without_test(&pred),
                Err(_) => false,
            },
            _ => false,
        };
    }
    // `#[test]`, `#[tokio::test]`, `#[rstest]`-style: the trailing segment is
    // `test`. (`#[cfg(...)]` was handled above and never reaches here.)
    path.segments
        .last()
        .is_some_and(|seg: &syn::PathSegment| seg.ident == "test")
}

/// Can this predicate be true with `test = false` and all other atoms free?
fn can_be_true_without_test(meta: &Meta) -> bool {
    match meta {
        // The built-in `test` flag is pinned false; any other flag is free.
        Meta::Path(path) => !path.is_ident("test"),
        // `feature = "x"`, `target_os = "linux"`: free, so satisfiably true.
        Meta::NameValue(_) => true,
        Meta::List(list) => match combinator(list) {
            Combinator::Not => children(list).first().is_none_or(can_be_false_without_test),
            Combinator::All => children(list).iter().all(can_be_true_without_test),
            Combinator::Any => children(list).iter().any(can_be_true_without_test),
            // Unknown form (e.g. `target(...)`): don't claim it's test-only.
            Combinator::Unknown => true,
        },
    }
}

/// Can this predicate be false with `test = false` and all other atoms free?
fn can_be_false_without_test(meta: &Meta) -> bool {
    match meta {
        // `test` is false, so it can be false; any other atom can also be false.
        Meta::Path(_) | Meta::NameValue(_) => true,
        Meta::List(list) => match combinator(list) {
            Combinator::Not => children(list).first().is_none_or(can_be_true_without_test),
            Combinator::All => children(list).iter().any(can_be_false_without_test),
            Combinator::Any => children(list).iter().all(can_be_false_without_test),
            Combinator::Unknown => true,
        },
    }
}

/// The boolean combinator a `cfg` list represents.
enum Combinator {
    Not,
    All,
    Any,
    Unknown,
}

/// Classify a `cfg` list's head identifier.
fn combinator(list: &MetaList) -> Combinator {
    match list.path.get_ident().map(ToString::to_string).as_deref() {
        Some("not") => Combinator::Not,
        Some("all") => Combinator::All,
        Some("any") => Combinator::Any,
        _ => Combinator::Unknown,
    }
}

/// The comma-separated child predicates of a `cfg` list.
fn children(list: &MetaList) -> Vec<Meta> {
    list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
        .map(|punctuated: Punctuated<Meta, Token![,]>| punctuated.into_iter().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    fn attr(src: &str) -> Attribute {
        let item: syn::ItemFn = syn::parse_str(&format!("{src}\nfn f() {{}}")).expect("valid");
        item.attrs.into_iter().next().expect("one attr")
    }

    #[test]
    fn plain_test_attr_is_test_only() {
        let a: Attribute = parse_quote!(#[test]);
        assert!(is_test_only(&[a]));
    }

    #[test]
    fn tokio_test_attr_is_test_only() {
        let a: Attribute = parse_quote!(#[tokio::test]);
        assert!(is_test_only(&[a]));
    }

    #[test]
    fn cfg_test_is_test_only() {
        assert!(is_test_only(&[attr("#[cfg(test)]")]));
    }

    #[test]
    fn cfg_all_test_unix_is_test_only() {
        assert!(is_test_only(&[attr("#[cfg(all(test, unix))]")]));
    }

    #[test]
    fn cfg_not_test_is_production_not_test_only() {
        // The whole point: `not(test)` is the real impl — must be audited.
        assert!(!is_test_only(&[attr("#[cfg(not(test))]")]));
    }

    #[test]
    fn cfg_feature_containing_test_substring_is_not_test_only() {
        assert!(!is_test_only(&[attr("#[cfg(feature = \"fastest\")]")]));
        assert!(!is_test_only(&[attr("#[cfg(feature = \"latest\")]")]));
    }

    #[test]
    fn cfg_not_feature_is_production() {
        assert!(!is_test_only(&[attr("#[cfg(not(feature = \"x\"))]")]));
    }

    #[test]
    fn cfg_any_test_or_feature_is_compiled_in_a_non_test_build() {
        // Satisfiable with test=false when the feature is on, so audit it.
        assert!(!is_test_only(&[attr("#[cfg(any(test, feature = \"x\"))]")]));
    }

    #[test]
    fn no_cfg_attr_is_not_test_only() {
        assert!(!is_test_only(&[attr("#[derive(Debug)]")]));
    }
}
