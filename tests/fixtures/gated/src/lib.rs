//! Crate root wiring two external modules with opposite cfg gates.

#[cfg(test)]
mod skipme; // test-only: its effect must be skipped

#[cfg(test)]
#[path = "custom_path.rs"]
mod relocated; // test-only via #[path]: its effect must also be skipped

#[cfg(not(test))]
mod prod; // production: its effect must be audited

pub fn ok(a: u32, b: u32) -> u32 {
    a + b
}
