//! A domain file with a real clock read AND a syntax error. `syn` cannot parse
//! it, so the tool cannot see the effect inside — and must NOT print
//! "functional core holds" over code it never read.

pub fn stamp() -> std::time::SystemTime {
    std::time::SystemTime::now()
}

// Stray, unbalanced token: this does not parse, on purpose.
pub fn broken( {
