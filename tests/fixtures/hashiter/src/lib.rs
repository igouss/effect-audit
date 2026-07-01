//! Holds a HashMap in the functional core. Under --strict the import and the
//! constructor are each a presence finding — exactly two — and the turbofish
//! (`HashMap::<u8, u8>::new()`) carries no type annotation to double-count.

use std::collections::HashMap;

pub fn size() -> usize {
    HashMap::<u8, u8>::new().len()
}
