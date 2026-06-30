//! Reached through `#[cfg(not(test))] mod prod;` — production, must be audited.

pub fn run() {
    let _ = std::fs::read("y");
}
