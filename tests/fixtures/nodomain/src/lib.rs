//! An adapter, not a domain crate — effects here are fine and unaudited.

pub fn now() -> std::time::SystemTime {
    std::time::SystemTime::now()
}
