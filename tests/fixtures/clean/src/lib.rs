//! A pure functional core: values in, values out, no effects.

pub fn total(items: &[u32]) -> u32 {
    items.iter().sum()
}

// Test-only scaffolding. None of this is an effect in the production build, and
// the gate must hold at the item level, not just for `fn`/`mod` — so a clean
// run proves the chokepoint, not merely the absence of effects.
#[cfg(test)]
use std::fs::read;

#[cfg(test)]
static SEEDED: std::sync::Mutex<u32> = std::sync::Mutex::new(0);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_sums() {
        let _ = read("Cargo.toml");
        let _ = SEEDED.lock();
        assert_eq!(total(&[1, 2, 3]), 6);
    }
}
