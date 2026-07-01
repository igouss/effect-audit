//! Reads the clock inside an evaluated macro argument. `format!` evaluates its
//! interpolated expression call-by-value, so `SystemTime::now()` is a real
//! runtime call — flagged by default, no --strict needed.

pub fn stamp() -> String {
    format!("{}", std::time::SystemTime::now())
}
