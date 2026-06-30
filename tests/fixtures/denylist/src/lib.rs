//! A pure core; this fixture exercises the legacy denylist on the manifest.

pub fn total(items: &[u32]) -> u32 {
    items.iter().sum()
}
