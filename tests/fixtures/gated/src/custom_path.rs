//! Reached through `#[cfg(test)] #[path = "custom_path.rs"] mod relocated;` —
//! test-only despite the non-conventional location, so it must NOT be audited.

pub fn helper() {
    let _ = std::fs::read("z");
}
