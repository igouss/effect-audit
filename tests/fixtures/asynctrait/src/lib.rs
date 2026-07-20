//! Deliberately effect-free source. The crate's only sin is in its manifest,
//! so this file proves the dependency edge is flagged on its own rather than
//! only as a by-product of scanning a `#[async_trait]` in the source.

pub fn double(n: u8) -> u8 {
    n.saturating_mul(2)
}
