//! Leaks a clock read into the functional core.

pub fn stamp() -> std::time::SystemTime {
    std::time::SystemTime::now()
}
