//! The effect taxonomy and the pure classifiers that map a syntactic
//! construct onto an [`EffectKind`].
//!
//! This module is the functional core of the linter: every function here
//! is total and deterministic — syntax in, classification out, no I/O. It
//! encodes *what counts as a side effect leaking into the domain*, which is
//! the one judgement the dependency-direction checker (`hex-domain-purity.sh`)
//! structurally cannot make.

/// A category of side effect that has no business inside a functional core.
///
/// The variants are ordered roughly by how loudly they violate determinism:
/// a clock or RNG read makes the same input produce different output; real
/// I/O reaches outside the process; shared mutable state smuggles in time
/// and ordering through the back door.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectKind {
    /// Reading wall-clock or monotonic time (`SystemTime::now`, `Utc::now`).
    Clock,
    /// Drawing randomness (`rand`, `thread_rng`, `Uuid::new_v4`).
    Random,
    /// Filesystem access (`std::fs`, `tokio::fs`).
    FileSystem,
    /// Network access (`std::net`, `reqwest`, `hyper`).
    Network,
    /// Spawning or talking to other processes (`std::process`).
    Process,
    /// Reading or mutating the ambient environment (`std::env`).
    Env,
    /// Console / standard-stream I/O (`std::io::stdout`, `println!`, `dbg!`).
    Console,
    /// Pulling in an async runtime (`tokio`, `async_std`).
    AsyncRuntime,
    /// Spawning or sleeping OS threads (`std::thread::spawn`, `::sleep`).
    Concurrency,
    /// Talking to a database (`sqlx`, `rusqlite`, `diesel`, `redis`).
    Database,
    /// Module-level shared mutable state (`static mut`, `static X: Mutex<_>`).
    SharedMutableState,
    /// A normal dependency a domain crate pulled in without vouching for it on
    /// its `pure-deps` allowlist. The effect is unknown — that is the point: the
    /// crate let in something it has not declared pure.
    UnvettedDependency,
}

impl EffectKind {
    /// A short, stable label for reports and baselines.
    pub fn label(self) -> &'static str {
        match self {
            EffectKind::Clock => "clock",
            EffectKind::Random => "random",
            EffectKind::FileSystem => "filesystem",
            EffectKind::Network => "network",
            EffectKind::Process => "process",
            EffectKind::Env => "environment",
            EffectKind::Console => "console-io",
            EffectKind::AsyncRuntime => "async-runtime",
            EffectKind::Concurrency => "concurrency",
            EffectKind::Database => "database",
            EffectKind::SharedMutableState => "shared-mutable-state",
            EffectKind::UnvettedDependency => "unvetted-dependency",
        }
    }

    /// The concrete fix to push toward — every effect resolves to "inject it
    /// as a port and let a Boundary supply it".
    pub fn hint(self) -> &'static str {
        match self {
            EffectKind::Clock => "take the timestamp as an argument; let an adapter read the clock",
            EffectKind::Random => "inject a seed or an id-generator port; resolve it at a Boundary",
            EffectKind::FileSystem => {
                "move file access to a repository/adapter; pass data in as values"
            }
            EffectKind::Network => "define a port for the call; the network adapter implements it",
            EffectKind::Process => "spawning processes is shell work, not domain work",
            EffectKind::Env => "read configuration in the shell; pass it into the core as a value",
            EffectKind::Console => {
                "return a value or an event; let the shell decide how to print it"
            }
            EffectKind::AsyncRuntime => {
                "the domain should be sync and pure; drive it from an async shell"
            }
            EffectKind::Concurrency => {
                "spawning or sleeping threads is shell work; keep the core sync and pure"
            }
            EffectKind::Database => "define a repository port; the DB adapter implements it",
            EffectKind::SharedMutableState => {
                "thread state through arguments and return values instead"
            }
            EffectKind::UnvettedDependency => {
                "add it to [package.metadata.hex-arch] pure-deps if it is a pure-value crate, \
                 otherwise move its use behind a port"
            }
        }
    }
}

/// Classify one normal dependency of a domain crate. This is the gate's core
/// judgement, and it has two modes chosen per crate:
///
/// - **Allowlist** (`pure_deps = Some(list)`): a dep not on `list` is a finding.
///   If the built-in taxonomy recognises it we name the effect (`axum` →
///   network); otherwise it is an [`EffectKind::UnvettedDependency`]. The list
///   is the gate; the taxonomy only enriches the message. This is the shape that
///   *compounds*: a brand-new effectful crate added in 2027 is flagged the day
///   it lands, with zero taxonomy upkeep, because it simply is not on the list.
/// - **Legacy denylist** (`pure_deps = None`): flag only crates the built-in
///   taxonomy knows. A crate opts into the stronger allowlist by adding
///   `pure-deps` to its `[package.metadata.hex-arch]`.
pub fn classify_dependency(name: &str, pure_deps: Option<&[String]>) -> Option<EffectKind> {
    match pure_deps {
        Some(allow) => {
            if allow.iter().any(|p: &String| p == name) {
                None
            } else {
                Some(is_effectful_crate(name).unwrap_or(EffectKind::UnvettedDependency))
            }
        }
        None => is_effectful_crate(name),
    }
}

/// Whether a dependency crate name is a known pure-effect crate.
///
/// This is the **legacy denylist** — the fallback used by a crate that has not
/// adopted a `pure-deps` allowlist (see [`classify_dependency`]), and the
/// enrichment oracle that names the effect of an undeclared dep in allowlist
/// mode. A denylist decays (perpetual whack-a-mole against the ecosystem); the
/// allowlist is the form that compounds, which is why this is no longer the
/// primary gate.
///
/// Deliberately excludes crates with pure value types a domain may legitimately
/// hold: `chrono`/`time` (a `DateTime` value is fine; only `::now()` is not),
/// `uuid` (a `Uuid` value is fine; only `new_v4()` is not), `rand` (a seed is
/// fine). Those are caught precisely at the call site by the AST scan instead.
/// Known families match by prefix, not just exact name, so `aws-sdk-s3`,
/// `aws-sdk-dynamodb`, … all land without listing each.
pub fn is_effectful_crate(name: &str) -> Option<EffectKind> {
    if let Some(kind) = effectful_crate_exact(name) {
        return Some(kind);
    }
    effectful_crate_family(name)
}

/// Exact crate-name matches, grouped by the effect they embody.
fn effectful_crate_exact(name: &str) -> Option<EffectKind> {
    match name {
        "tokio" | "async-std" | "async_std" | "smol" | "async-global-executor" => {
            Some(EffectKind::AsyncRuntime)
        }
        "sqlx" | "rusqlite" | "diesel" | "sea-orm" | "sea_orm" | "redis" | "mongodb"
        | "postgres" | "tokio-postgres" | "mysql" | "mysql_async" | "sled" | "r2d2" => {
            Some(EffectKind::Database)
        }
        "reqwest" | "hyper" | "ureq" | "isahc" | "surf" | "curl" | "tonic" | "axum"
        | "actix-web" | "actix" | "warp" | "rocket" | "tide" | "poem" | "salvo" => {
            Some(EffectKind::Network)
        }
        _ => None,
    }
}

/// Crate-family matches by name prefix, so whole SDKs land without enumeration.
fn effectful_crate_family(name: &str) -> Option<EffectKind> {
    const NETWORK_PREFIXES: &[&str] = &["aws-sdk-", "aws_sdk_", "aws-config", "google-cloud"];
    const DATABASE_PREFIXES: &[&str] = &["deadpool"];
    if NETWORK_PREFIXES.iter().any(|p: &&str| name.starts_with(p)) {
        return Some(EffectKind::Network);
    }
    if DATABASE_PREFIXES.iter().any(|p: &&str| name.starts_with(p)) {
        return Some(EffectKind::Database);
    }
    None
}

/// Console-writing macros. `format!`/`write!`/`assert!` are pure and absent.
pub fn classify_macro(name: &str) -> Option<EffectKind> {
    match name {
        "println" | "print" | "eprintln" | "eprint" | "dbg" => Some(EffectKind::Console),
        _ => None,
    }
}

/// Whether a single type identifier names interior mutability. Matched as a
/// *whole* identifier — not a substring — so `Cellophane` does not trip `Cell`
/// and `LazyThing` does not trip `Lazy`. `Atomic*` is a prefix because the std
/// atomics are a family (`AtomicUsize`, `AtomicBool`, …).
pub fn is_interior_mutability_ident(ident: &str) -> bool {
    const EXACT: &[&str] = &[
        "Mutex",
        "RwLock",
        "RefCell",
        "Cell",
        "OnceCell",
        "OnceLock",
        "UnsafeCell",
        "Lazy",
    ];
    ident.starts_with("Atomic") || EXACT.contains(&ident)
}

/// Classify a path expression by its segment idents (e.g. `["SystemTime",
/// "now"]`). Returns the effect it performs, or `None` for a pure path.
///
/// Leading `crate`/`self`/`super`/`std`/`core`/`alloc` segments are stripped
/// first so `std::fs::read` and an imported `fs::read` classify identically.
pub fn classify_path(segments: &[String]) -> Option<EffectKind> {
    let segs: Vec<&str> = strip_roots(segments);
    let first: &str = segs.first().copied()?;
    let last: &str = segs.last().copied()?;

    if is_clock(&segs) {
        return Some(EffectKind::Clock);
    }
    if is_random(&segs) {
        return Some(EffectKind::Random);
    }
    // Effectful std modules are matched at the *leaf*, not the whole module:
    // `std::net` holds pure address values (`Ipv4Addr`, `SocketAddr`) next to
    // real sockets, `std::env` holds compile-time `consts` next to `var`, and
    // `std::process` holds `ExitStatus` values next to `Command`. Blanket-
    // matching the module name flagged the pure half — inventing a violation
    // against a value, the one thing this tool must never do. `fs` is the
    // exception: every item under it touches the filesystem, so it stays blanket.
    match first {
        "fs" => return Some(EffectKind::FileSystem),
        "net" if is_net_effect(&segs) => return Some(EffectKind::Network),
        "process" if is_process_effect(&segs) => return Some(EffectKind::Process),
        "env" if is_env_effect(&segs) => return Some(EffectKind::Env),
        "io" if matches!(last, "stdin" | "stdout" | "stderr") => return Some(EffectKind::Console),
        "thread" if matches!(last, "spawn" | "scope" | "sleep") => {
            return Some(EffectKind::Concurrency)
        }
        _ => {}
    }
    if let Some(kind) = is_effectful_crate(first) {
        return Some(kind);
    }
    None
}

/// Whether a `std::net::*` path touches a real socket or resolves DNS, as
/// opposed to naming a pure address *value* a domain may legitimately hold.
/// `Ipv4Addr`, `Ipv6Addr`, `IpAddr`, `SocketAddr*` are data, not I/O — only the
/// connection types and `ToSocketAddrs` (which performs DNS) are effects. The
/// socket type is the operative segment, not the call leaf, so we match any
/// segment: `std::net::TcpStream::connect` and a bare `use std::net::TcpStream`
/// both land.
fn is_net_effect(segs: &[&str]) -> bool {
    segs.iter().any(|s: &&str| {
        matches!(
            *s,
            "TcpStream" | "TcpListener" | "UdpSocket" | "ToSocketAddrs"
        )
    })
}

/// Whether a `std::env::*` path reads or mutates the ambient environment, as
/// opposed to naming a compile-time constant (`env::consts::ARCH`) or a value
/// type (`VarError`). The effect is the call, so we match any segment: the
/// call form `env::var(..)` and `use std::env::set_var` both land, while
/// `env::consts::OS` carries no effectful leaf and passes.
fn is_env_effect(segs: &[&str]) -> bool {
    segs.iter().any(|s: &&str| {
        matches!(
            *s,
            "var"
                | "vars"
                | "var_os"
                | "vars_os"
                | "set_var"
                | "remove_var"
                | "current_dir"
                | "set_current_dir"
                | "current_exe"
                | "temp_dir"
                | "args"
                | "args_os"
        )
    })
}

/// Whether a `std::process::*` path spawns a child or exits the process, as
/// opposed to naming a result value (`Output`, `ExitStatus`, `ExitCode`).
/// `Command` builds and runs a subprocess; `exit`/`abort` terminate — all shell
/// work. Holding an `ExitStatus` someone handed you is pure.
fn is_process_effect(segs: &[&str]) -> bool {
    segs.iter()
        .any(|s: &&str| matches!(*s, "Command" | "exit" | "abort"))
}

/// Drop leading path roots that carry no effect meaning, so callers match on
/// the operative segments regardless of how the path was qualified.
fn strip_roots(segments: &[String]) -> Vec<&str> {
    let mut segs: Vec<&str> = segments.iter().map(String::as_str).collect();
    while let Some(&head) = segs.first() {
        if matches!(head, "crate" | "self" | "super" | "std" | "core" | "alloc") {
            segs.remove(0);
        } else {
            break;
        }
    }
    segs
}

/// A clock read: a `now`-family call whose owning type is a known clock. The
/// `now*` prefix covers `SystemTime::now`, `Instant::now`, and the `time`
/// crate's `OffsetDateTime::now_utc` / `now_local`.
pub fn is_clock(segs: &[&str]) -> bool {
    segs.windows(2)
        .any(|w: &[&str]| is_clock_type(w[0]) && w[1].starts_with("now"))
}

/// Whether an identifier names a clock type (or an alias the caller resolved).
pub fn is_clock_type(ident: &str) -> bool {
    const CLOCKS: &[&str] = &["SystemTime", "Instant", "Utc", "Local", "OffsetDateTime"];
    CLOCKS.contains(&ident)
}

/// A clock read spelled as a *method* on a time value already in hand.
/// `Instant::now()` is a path and caught by [`classify_path`]; `instant.elapsed()`
/// reads the same monotonic clock through a method call (`Instant::now() - self`),
/// which a path scan cannot see. Flagged on the method name alone — `elapsed` is
/// distinctive enough that nobody gives a pure function that name, the same
/// judgement made for `thread_rng`.
///
/// `duration_since` is deliberately **not** here: `a.duration_since(b)` subtracts
/// two values the caller already holds — it reads no clock and is pure. Flagging
/// it would invent a violation against arithmetic, the very thing the leaf
/// matching above exists to avoid.
pub fn is_clock_method(name: &str) -> bool {
    name == "elapsed"
}

/// A randomness draw: a call from an RNG crate (`rand`/`fastrand`/`getrandom`),
/// a `thread_rng` call, or a randomly generated UUID. A bare `random()` is
/// intentionally excluded — too common a name to assume nondeterminism.
fn is_random(segs: &[&str]) -> bool {
    if matches!(
        segs.first().copied(),
        Some("rand" | "fastrand" | "getrandom")
    ) {
        return true;
    }
    // Match the *call position*, not any segment, and only on a *distinctive*
    // name: `thread_rng` is one nobody gives a pure function. A bare `random()`
    // is deliberately NOT a draw — `random` is too ordinary a name (a domain
    // crate may have its own deterministic `fn random()`), and the imported
    // `use rand::random; random()` form is an acceptable recall loss, the same
    // one we already take on aliased method calls. `rand::random` stays caught
    // by the first-segment rule above.
    if matches!(segs.last().copied(), Some("thread_rng")) {
        return true;
    }
    let names_uuid: bool = segs.iter().any(|s: &&str| matches!(*s, "Uuid" | "uuid"));
    let nondeterministic: bool = segs
        .iter()
        .any(|s: &&str| matches!(*s, "new_v4" | "now_v7" | "now_v6" | "now_v1"));
    names_uuid && nondeterministic
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(s: &str) -> Vec<String> {
        s.split("::").map(str::to_owned).collect()
    }

    #[test]
    fn system_time_now_is_a_clock() {
        assert_eq!(
            classify_path(&path("SystemTime::now")),
            Some(EffectKind::Clock)
        );
    }

    #[test]
    fn fully_qualified_clock_matches_the_same() {
        assert_eq!(
            classify_path(&path("std::time::SystemTime::now")),
            Some(EffectKind::Clock)
        );
    }

    #[test]
    fn chrono_utc_now_is_a_clock() {
        assert_eq!(
            classify_path(&path("chrono::Utc::now")),
            Some(EffectKind::Clock)
        );
    }

    #[test]
    fn a_systemtime_value_is_not_an_effect() {
        // Holding the type is fine; only reading the clock is the effect.
        assert_eq!(classify_path(&path("std::time::SystemTime")), None);
    }

    #[test]
    fn random_uuid_is_random_but_a_plain_uuid_is_not() {
        assert_eq!(
            classify_path(&path("Uuid::new_v4")),
            Some(EffectKind::Random)
        );
        assert_eq!(classify_path(&path("Uuid::parse_str")), None);
    }

    #[test]
    fn thread_rng_is_random() {
        assert_eq!(
            classify_path(&path("rand::thread_rng")),
            Some(EffectKind::Random)
        );
    }

    #[test]
    fn a_pure_module_named_random_is_not_a_draw() {
        // `random` in the call's middle is a module name, not a draw site.
        assert_eq!(classify_path(&path("crate::random::from_seed")), None);
        // `rand::random` is a draw — the RNG crate is in call position.
        assert_eq!(
            classify_path(&path("rand::random")),
            Some(EffectKind::Random)
        );
        // But a *bare* `random()` is too common a name to assume nondeterminism:
        // a domain crate may have its own deterministic `pub fn random()`.
        assert_eq!(classify_path(&path("random")), None);
    }

    #[test]
    fn a_pure_fn_named_random_called_bare_is_not_a_draw() {
        // `pub fn pick() -> u32 { random() + random() }` over a domain-local
        // deterministic `random` must yield nothing.
        assert_eq!(classify_path(&path("random")), None);
    }

    #[test]
    fn std_fs_and_aliased_fs_classify_identically() {
        assert_eq!(
            classify_path(&path("std::fs::read")),
            Some(EffectKind::FileSystem)
        );
        assert_eq!(
            classify_path(&path("fs::read")),
            Some(EffectKind::FileSystem)
        );
    }

    #[test]
    fn stdout_is_console_but_other_io_is_not() {
        assert_eq!(
            classify_path(&path("std::io::stdout")),
            Some(EffectKind::Console)
        );
        assert_eq!(classify_path(&path("std::io::Cursor")), None);
    }

    #[test]
    fn a_net_socket_is_an_effect_but_an_address_value_is_not() {
        // A real socket touches the network; the address types are pure data.
        assert_eq!(
            classify_path(&path("std::net::TcpStream::connect")),
            Some(EffectKind::Network)
        );
        assert_eq!(
            classify_path(&path("std::net::TcpListener")),
            Some(EffectKind::Network)
        );
        assert_eq!(
            classify_path(&path("std::net::ToSocketAddrs")),
            Some(EffectKind::Network),
            "name resolution is a real effect"
        );
        // Pure value types a domain may hold — must NOT be flagged.
        assert_eq!(classify_path(&path("std::net::Ipv4Addr::new")), None);
        assert_eq!(classify_path(&path("std::net::SocketAddr")), None);
        assert_eq!(classify_path(&path("net::IpAddr")), None);
    }

    #[test]
    fn env_reads_are_effects_but_compile_time_consts_are_not() {
        assert_eq!(classify_path(&path("std::env::var")), Some(EffectKind::Env));
        assert_eq!(
            classify_path(&path("std::env::set_var")),
            Some(EffectKind::Env)
        );
        assert_eq!(
            classify_path(&path("env::current_dir")),
            Some(EffectKind::Env)
        );
        // `env::consts::ARCH` / `OS` are compile-time constants, not reads.
        assert_eq!(classify_path(&path("std::env::consts::ARCH")), None);
        assert_eq!(classify_path(&path("env::consts::OS")), None);
    }

    #[test]
    fn process_spawn_is_an_effect_but_an_exit_status_value_is_not() {
        assert_eq!(
            classify_path(&path("std::process::Command::new")),
            Some(EffectKind::Process)
        );
        assert_eq!(
            classify_path(&path("std::process::exit")),
            Some(EffectKind::Process)
        );
        // `Output` / `ExitStatus` are result values you were handed — pure.
        assert_eq!(classify_path(&path("std::process::ExitStatus")), None);
        assert_eq!(classify_path(&path("process::Output")), None);
    }

    #[test]
    fn elapsed_is_a_clock_method_but_duration_since_is_pure() {
        // `instant.elapsed()` reads the clock; `a.duration_since(b)` subtracts
        // two held values and reads nothing.
        assert!(is_clock_method("elapsed"));
        assert!(!is_clock_method("duration_since"));
        assert!(!is_clock_method("len"));
    }

    #[test]
    fn tokio_and_sqlx_are_runtime_and_database() {
        assert_eq!(
            classify_path(&path("tokio::spawn")),
            Some(EffectKind::AsyncRuntime)
        );
        assert_eq!(
            classify_path(&path("sqlx::query")),
            Some(EffectKind::Database)
        );
    }

    #[test]
    fn a_pure_domain_path_is_not_flagged() {
        assert_eq!(classify_path(&path("crate::entity::Order::total")), None);
    }

    #[test]
    fn console_macros_are_classified_and_format_is_not() {
        assert_eq!(classify_macro("println"), Some(EffectKind::Console));
        assert_eq!(classify_macro("dbg"), Some(EffectKind::Console));
        assert_eq!(classify_macro("format"), None);
        assert_eq!(classify_macro("assert_eq"), None);
    }

    #[test]
    fn time_crate_now_utc_and_now_local_are_clocks() {
        // The `time` crate spells it `now_utc`, not `now` (probe P4).
        assert_eq!(
            classify_path(&path("time::OffsetDateTime::now_utc")),
            Some(EffectKind::Clock)
        );
        assert_eq!(
            classify_path(&path("OffsetDateTime::now_local")),
            Some(EffectKind::Clock)
        );
    }

    #[test]
    fn thread_spawn_and_sleep_are_concurrency() {
        assert_eq!(
            classify_path(&path("std::thread::spawn")),
            Some(EffectKind::Concurrency)
        );
        assert_eq!(
            classify_path(&path("std::thread::sleep")),
            Some(EffectKind::Concurrency)
        );
    }

    #[test]
    fn interior_mutability_is_matched_by_whole_identifier() {
        assert!(is_interior_mutability_ident("Mutex"));
        assert!(is_interior_mutability_ident("RefCell"));
        assert!(is_interior_mutability_ident("AtomicUsize"));
    }

    #[test]
    fn a_type_that_merely_contains_cell_is_not_interior_mutability() {
        // `Cellophane` contains "Cell" but is not interior mutability (probe P5).
        assert!(!is_interior_mutability_ident("Cellophane"));
        assert!(!is_interior_mutability_ident("LazyThing"));
        assert!(!is_interior_mutability_ident("u32"));
    }

    #[test]
    fn known_effectful_crates_map_to_a_kind() {
        assert_eq!(is_effectful_crate("tokio"), Some(EffectKind::AsyncRuntime));
        assert_eq!(is_effectful_crate("sqlx"), Some(EffectKind::Database));
        assert_eq!(is_effectful_crate("redis"), Some(EffectKind::Database));
        assert_eq!(is_effectful_crate("reqwest"), Some(EffectKind::Network));
        assert_eq!(is_effectful_crate("axum"), Some(EffectKind::Network));
        assert_eq!(is_effectful_crate("serde"), None);
    }

    #[test]
    fn effectful_crate_families_match_by_prefix() {
        assert_eq!(
            is_effectful_crate("aws-sdk-dynamodb"),
            Some(EffectKind::Network)
        );
        assert_eq!(
            is_effectful_crate("deadpool-postgres"),
            Some(EffectKind::Database)
        );
        assert_eq!(is_effectful_crate("serde_json"), None);
    }

    #[test]
    fn denylist_mode_flags_only_known_effectful_crates() {
        // No allowlist declared -> legacy behaviour: serde passes, tokio fails.
        assert_eq!(
            classify_dependency("tokio", None),
            Some(EffectKind::AsyncRuntime)
        );
        assert_eq!(classify_dependency("serde", None), None);
    }

    #[test]
    fn allowlist_mode_permits_a_declared_dep() {
        let allow: Vec<String> = vec!["serde".to_owned(), "thiserror".to_owned()];
        assert_eq!(classify_dependency("serde", Some(&allow)), None);
    }

    #[test]
    fn allowlist_mode_flags_an_undeclared_known_crate_by_its_effect() {
        let allow: Vec<String> = vec!["serde".to_owned()];
        assert_eq!(
            classify_dependency("axum", Some(&allow)),
            Some(EffectKind::Network)
        );
    }

    #[test]
    fn allowlist_mode_flags_an_undeclared_unknown_crate_as_unvetted() {
        let allow: Vec<String> = vec!["serde".to_owned()];
        assert_eq!(
            classify_dependency("some-mystery-lib", Some(&allow)),
            Some(EffectKind::UnvettedDependency)
        );
    }

    #[test]
    fn an_empty_allowlist_vouches_for_nothing() {
        // `pure-deps = []` means "nothing is pure" -> every dep is flagged.
        let allow: Vec<String> = Vec::new();
        assert_eq!(
            classify_dependency("serde", Some(&allow)),
            Some(EffectKind::UnvettedDependency)
        );
    }
}
