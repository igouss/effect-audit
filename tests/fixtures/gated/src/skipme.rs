//! Reached only through `#[cfg(test)] mod skipme;` — must NOT be audited.

pub fn helper() {
    let _ = std::fs::read("x");
}
