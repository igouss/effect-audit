//! A pure core; this fixture exists to exercise the manifest allowlist, not the
//! source scanner, so the body is deliberately effect-free.

pub fn total(items: &[u32]) -> u32 {
    items.iter().sum()
}
