//! effect-audit — enforce Bernhardt's *functional core, imperative shell* in a
//! hex-arch workspace.
//!
//! `hex-domain-purity.sh` proves the dependency arrows point inward: a
//! `role = "domain"` crate has zero internal path deps. That is necessary but
//! not sufficient — it says nothing about *effects*. A domain entity can import
//! nothing and still call `SystemTime::now()`, draw from `thread_rng()`, read a
//! file, or hide a `static Mutex`. None of those add a dependency edge, so the
//! arrow checker waves them through. This crate closes that gap: it parses every
//! domain crate's source and flags side effects that belong in the shell.
//!
//! It is a library so the binary (`main.rs`) and the executable specs
//! (`tests/cucumber.rs`) share one implementation. The pure core —
//! [`effect`], [`cfg_pred`], [`scan`], [`suppress`], [`finding`], [`report`] —
//! holds no I/O; the shell — [`discovery`], [`modtree`], [`audit`] — performs
//! it. The tool is hexagonal, like the code it polices.

pub mod audit;
pub mod baseline;
pub mod cfg_pred;
pub mod discovery;
pub mod effect;
pub mod finding;
pub mod modtree;
pub mod report;
pub mod scan;
pub mod suppress;
