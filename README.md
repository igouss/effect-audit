# effect-audit

> Prove that your **functional core** is actually pure. `effect-audit` parses
> every `role = "kernel"` and `role = "domain"` crate in a hex-arch workspace
> and flags the side effects — clocks, RNG, I/O, console, shared mutable state,
> runtime/DB deps — that leak into code that's supposed to be values-in,
> values-out.

It enforces Gary Bernhardt's **functional core, imperative shell**: the core (the
domain) computes; the shell (the adapters / Boundaries) touches the world. The
dependency-direction checker can't see effects. This does.

```
$ effect-audit tests/fixtures/dirty

FUNCTIONAL-CORE VIOLATION: effects leaked into the core.

  Bernhardt's rule: the functional core is pure (values in,
  values out); all effects live in the imperative shell.

  src/lib.rs
    src/lib.rs:4  [clock]  std::time::SystemTime::now

  How to fix:
    [clock] take the timestamp as an argument; let an adapter read the clock

  To silence one deliberate effect: add `// fc-allow: <why>`
  on the offending line or the line above it.

  1 effect(s) leaked across 1 file(s); 1 domain crate(s) audited.

$ echo $?
1
```

That `src/lib.rs:4` is clickable. Every finding carries a location, a kind, the
reconstructed call, and a one-line fix — enough to act without re-reading the
rule.

---

## Two core roles, two strictnesses

Both `role = "kernel"` and `role = "domain"` crates are audited, and their
**source** is held to exactly the same effect rules. Their **manifests** are not.

| | may depend on | vouching |
|---|---|---|
| `kernel` | other `kernel` crates in the workspace, and nothing else | none available |
| `domain` | whatever it vouches for via `pure-deps` | `pure-deps` allowlist |

A kernel crate is the floor of the system, and its whole claim is that it has no
dependency graph by construction — so there is nothing for it to vouch for, and
`pure-deps` on a kernel crate is inert (the tool says so rather than ignoring it).
The one exception is another kernel crate, because the kernel layer is closed
under itself. That is the same line
[`hex-lint`](https://github.com/igouss/hex-lint)'s role matrix draws; the two
gates disagreeing about the floor of the system would be a defect in itself.

The verdict line names what it actually audited, so a clean run cannot be read as
covering more than it did:

```
effect-audit: 2 kernel + 8 domain crate(s) clean — functional core holds.
```

`--require-domain` and `--require-kernel` each fail (exit 2) when no crate of
that role is found, so a misconfigured workspace cannot pass green having
audited nothing. They mean exactly what they say: a kernel crate does not
satisfy `--require-domain`.

---

## The problem it solves

A **dependency-direction checker** — like [`hex-lint`](https://github.com/igouss/hex-lint) —
proves the **dependency arrows point inward**: a domain crate has zero internal
path deps. Necessary — but *not sufficient*. It says nothing about **effects**.
A domain entity can import nothing internal and still do all of this:

```rust
let now = std::time::SystemTime::now();   // clock      → nondeterministic
let id  = uuid::Uuid::new_v4();           // randomness → nondeterministic
let cfg = std::fs::read("config")?;       // filesystem → reaches outside
std::thread::spawn(work);                 // concurrency
static CACHE: Mutex<Map> = ...;           // shared mutable state
struct Fake { calls: Mutex<Vec<Call>> }   // ...and so is a field
thread_local! { static C: RefCell<…> }    // hidden global state
```

None of those add a dependency edge, so the arrow checker waves them all
through. Same input, different output — a "pure" core you can't actually
reason about or test deterministically. `effect-audit` closes that gap by
**parsing** each domain crate and flagging effects at the call site.

It's not a competitor to the arrow checker — it's the **other half** of the same
rule. Run both.

| The check… | hex-lint | clippy | manual review | **effect-audit** |
|---|:--:|:--:|:--:|:--:|
| Dependency arrows point inward | ✅ | ❌ | 👁️ | — *(out of scope)* |
| Clock / RNG / I/O *call sites* in the core | ❌ | ❌ | 👁️ | ✅ |
| Effectful *dependencies* (`tokio`, `sqlx`, `reqwest`) | ❌ | ❌ | 👁️ | ✅ |
| Shared mutable state (`static Mutex`, a `Mutex` field, `thread_local!`) | ❌ | ❌ | 👁️ | ✅ |
| Effects inside *evaluated* macro args (`format!("{}", now())`) | ❌ | ❌ | 👁️ | ✅ |
| `HashMap`/`HashSet` iteration-order surface in the core (`--strict`) | ❌ | ❌ | 👁️ | ✅ |
| Allowlist that **compounds** (new effectful crates auto-flagged) | — | ❌ | ❌ | ✅ |
| Structural match, never `str::contains` | — | ✅ | — | ✅ |
| Baseline ratchet for incremental adoption | ❌ | ~ | — | ✅ |

👁️ = "if a human happens to catch it." That's the whole point of a gate: don't
rely on happening to catch it.

---

## Show me it working

Catching a leak in real code — a representative run over a hex-arch workspace
whose domain crates picked up a genuine RNG draw and a runtime dep:

```
$ effect-audit --advisory .

  crates/orders/domain/src/ids.rs
    crates/orders/domain/src/ids.rs:4  [random]  use getrandom::getrandom
    crates/orders/domain/src/ids.rs:23  [random]  getrandom
    crates/orders/domain/src/ids.rs:36  [random]  getrandom
  crates/orders/domain/src/test_support/fakes.rs
    crates/orders/domain/src/test_support/fakes.rs:38  [async-runtime]  use tokio::sync::mpsc::UnboundedReceiver
    crates/orders/domain/src/test_support/fakes.rs:39  [async-runtime]  use tokio::sync::Mutex

  5 effect(s) leaked across 2 file(s); 4 domain crate(s) audited.
  (advisory mode — not failing the build.)
```

Manifest-level leaks (a domain crate depending on something effectful) point at
the `Cargo.toml`, with no line because there isn't one:

```
$ effect-audit tests/fixtures/denylist

  Cargo.toml
    Cargo.toml  [network]  dependency: reqwest
```

JSON for CI annotations and `jq`:

```
$ effect-audit --json tests/fixtures/dirty
{
  "findings": [
    { "file": "src/lib.rs", "kind": "clock", "line": 4, "snippet": "std::time::SystemTime::now" }
  ],
  "skipped_unparseable": [],
  "stale_baseline": []
}
```

---

## Design philosophy

**1. Structural, never substring.** It parses with `syn` and matches the AST —
never `str::contains` on stringified tokens. A comment that says `// reads the
clock`, a string literal `"std::fs::read"`, a type named `Cellophane` (contains
"Cell"), and a feature named `fastest` (contains "test") all produce **zero**
findings, because the parser sees them for what they are. `#[cfg(...)]`
predicates are evaluated as a boolean AST; interior-mutability types are matched
by whole identifier token. Substring matching is the original sin of text
linters; this tool refuses it everywhere.

**2. A value is fine; *producing* one is the effect.** Holding a `DateTime`, a
`Uuid`, or a seed passed *into* the core is pure. Only the nondeterministic
*call* — `::now()`, `new_v4()`, `thread_rng()` — is flagged. So `chrono`,
`uuid`, and `rand` are not banned dependencies; their draw sites are.

**3. Allowlists compound; denylists decay.** (See below.) A denylist is
perpetual whack-a-mole against the ecosystem. An allowlist flags every new
effectful crate the day it's added, with zero taxonomy upkeep.

**4. Sound by omission — it never invents a violation.** When the tool can't see
something (an effect buried in a proc-macro's opaque token stream, or in the
arguments of a macro that quotes rather than evaluates them), it stays silent
rather than guessing. False reds waste your time; this tool would rather miss
than lie. The gaps are documented under [Limitations](#limitations), not hidden.

**5. The escape hatch must say why.** A suppression with no rationale is just a
silent hole. `// fc-allow:` *requires* a non-empty reason or it's ignored.

**6. It is what it polices.** `effect-audit` is itself a functional core with a
thin imperative shell, and its own test suite turns the tool on its own core and
asserts zero findings. If it can't keep effects out of itself, it has no
business policing anyone else.

---

## What it flags

| Kind | Examples |
|------|----------|
| `clock` | `SystemTime::now`, `Instant::now`, `OffsetDateTime::now_utc`, `chrono::Utc::now`, `instant.elapsed()`, aliased `use Instant as I; I::now()` |
| `random` | `rand::*`, `thread_rng`, `getrandom`, `Uuid::new_v4` |
| `filesystem` | `std::fs::*`, `tokio::fs::*` |
| `network` | `std::net::{TcpStream, TcpListener, UdpSocket, ToSocketAddrs}` (sockets + DNS — **not** the pure `Ipv4Addr`/`SocketAddr` value types), `reqwest`, `hyper`, `tonic`, `axum`, `aws-sdk-*` |
| `process` | `std::process::{Command, exit, abort}` (**not** the `Output`/`ExitStatus`/`ExitCode` value types) |
| `environment` | `std::env::{var, vars, set_var, remove_var, current_dir, current_exe, …}` (**not** the compile-time `env::consts::*`) |
| `console-io` | `std::io::stdout`, `println!`, `eprintln!`, `dbg!` |
| `async-runtime` | a `tokio` / `async-std` dependency or path |
| `concurrency` | `std::thread::{spawn, scope, sleep}` |
| `database` | a `sqlx` / `rusqlite` / `redis` / `deadpool-*` dependency or path |
| `shared-mutable-state` | `static mut`, `static X: Mutex<_>` / `Atomic*` / `OnceCell`, a struct or enum-variant **field** of such a type, `thread_local!`, `lazy_static!` |
| `hash-iteration` | `HashMap` / `HashSet` in the core's surface — imports, constructors (`HashMap::new`), and type positions (`&HashMap<..>` params, fields, return types), across `std::collections` and `hashbrown` (**`--strict`**; witnesses presence, not a proven order leak) |
| `unvetted-dependency` | a normal dep not on a crate's `pure-deps` allowlist (allowlist mode only) |
| `mandated-boxing` | an `async-trait` dependency, a `use async_trait::async_trait`, or an `#[async_trait]` / `#[async_trait::async_trait]` attribute. The macro rewrites every `async fn` in a trait to return `Pin<Box<dyn Future + Send>>`, so the allocation is mandated on every impl and every caller. **A boxed future you spell yourself is not a finding** — for a port held as `Arc<dyn Port>` the box exists either way. The rule targets the mandate, not the allocation |

It checks two layers: **manifest** (the dependency policy) and **source** (the
call sites). The crate matcher knows families by prefix (`aws-sdk-…`,
`deadpool…`), not just exact names.

Any of these effects is also caught inside the **evaluated arguments** of an
allowlisted std macro (`format!`, `println!`, `vec!`, `assert_eq!`, …): a clock
read in `format!("{}", SystemTime::now())` is a real runtime call, so it is
flagged as a `clock` with its true line number. Macros that quote rather than
evaluate their input (`stringify!`) and proc-macros stay opaque — see the FAQ.

### The dependency allowlist (`pure-deps`) — the polarity flip

A built-in denylist of effectful crates *decays*: every new async ORM or HTTP
client is a fresh game of whack-a-mole, and a crate the tool has never heard of
sails through. An allowlist *compounds*: declare the pure-value crates a domain
may depend on, and **everything else is flagged the day it's added** — zero
taxonomy upkeep.

Opt a crate in by listing its pure dependencies next to its role marker:

```toml
[package.metadata.hex-arch]
role = "domain"
pure-deps = ["serde", "thiserror", "rust_decimal"]
```

Now any normal dependency *not* on that list is a finding — named by its effect
if the built-in taxonomy recognises it (`reqwest` → `network`), or as an
`unvetted-dependency` otherwise:

```
$ effect-audit tests/fixtures/allowlist

  Cargo.toml
    Cargo.toml  [network]  dependency: reqwest
    Cargo.toml  [unvetted-dependency]  dependency: some-unvetted-lib
```

The list is the gate; the taxonomy is demoted to flavour text. `pure-deps = []`
means "nothing is pure" (flag every dep).

A crate **without** a `pure-deps` key stays in **legacy denylist mode** — only
the recognised effectful crates fire — so adoption is incremental, one crate at
a time. (Dev deps are always excluded; optional deps are excluded unless
`--strict`.)

### What it deliberately does *not* flag

- **`&mut self` on owned data.** Mutating a value you own and return is still
  functional. Only *shared* mutable state (statics, interior mutability,
  `thread_local!`) is nondeterminism.
- **Holding an effectful value type.** A `DateTime`, a `Uuid`, or a seed passed
  *into* the core is fine; only *producing* one via `::now()` / `new_v4()` is an
  effect.
- **Pure value types that live next to effects in a "scary" module.** An
  `Ipv4Addr` / `SocketAddr` (data, not a socket), an `ExitStatus` / `ExitCode`
  (a result you were handed, not a `Command` you ran), and `env::consts::ARCH`
  (a compile-time constant, not a `var()` read) are all data. The effectful std
  modules are matched at the *leaf* — `TcpStream`, `process::exit`, `env::var` —
  never by the module name, so the value half of `net` / `process` / `env` is
  never flagged. `fs` is the one blanket: every item under it is filesystem I/O.
- **Compile-time constants.** `env!("X")`, `include_str!("…")` resolve to pure
  values at build time; flagging them would violate the "a value is fine" rule.
- **Test code.** `#[cfg(test)]` / `#[test]` items, test-only `mod`s,
  dev-dependencies, and (without `--strict`) optional deps are excluded. The
  audit judges the **default-feature production build**.

### A deliberate boundary: feature-gated ≠ test-only

Only the built-in `test` cfg exempts code. A module gated `#[cfg(not(test))]` is
the production impl and **is** audited. A module gated `#[cfg(any(test, feature
= "x"))]` compiles in a non-test build when `x` is on, so it **is** audited too
— even if `x` is conventionally test infrastructure. Exempt such a module with
an explicit `fc-allow` or a baseline entry; the tool will not infer "test" from
a feature or module *name* (that heuristic is both over- and under-inclusive).

### `--strict` adds three opt-in checks

- **Effectful optional deps.** Without `--strict`, a feature-gated (`optional`)
  dep is off in the default build and skipped; with it, an `optional` `reqwest`
  in a domain manifest is still flagged.
- **`async` in the core.** `async fn` and `async { }` blocks are effect-shaped —
  they thread a runtime and suspension points through code that should be pure —
  even with no `tokio` dependency. Low confidence, hence opt-in.
- **`HashMap` / `HashSet` in the core.** The default hasher is seeded from the
  RNG at startup, so any iteration order that escapes the domain smuggles in that
  seed — the same determinism leak as a clock read, by another door. Under
  `--strict` the tool flags the type's *presence* in the core's surface: imports,
  constructors (`HashMap::new`), and type positions (`&HashMap<..>` parameters,
  fields, return types), across both `std::collections` and `hashbrown` (after
  import each is spelled by the same leaf ident). `.iter()` / `.keys()` calls are
  deliberately **not** flagged — a name-only heuristic there would be far too
  noisy.

  **Why opt-in — presence is witnessed, harm is approximated.** A hash-iteration
  finding **witnesses the type's presence**, a fact the parser reads straight off
  the AST — it is never fabricated. The *harm* — a nondeterministic order
  actually escaping the domain — is a conservative **over-approximation**: some
  flagged uses never let order leak. Presence is certain; the leak is inferred,
  so the check lives here beside `async`, and `fc-allow` / the baseline are its
  pressure valves. **Holding a `HashMap` passed in from the shell is still a
  presence finding** — the type is in the core's surface, and that call is
  deliberate and conservative. The fix is `BTreeMap` / `BTreeSet` / `IndexMap` in
  the core, or an `fc-allow` whose reason explains why order can never escape.

---

## Usage

```
effect-audit [OPTIONS] [ROOT]

  ROOT                A path inside the workspace to audit (default: cwd).

  --advisory          print findings but always exit 0 (warn-only hook)
  --strict            also flag effectful optional deps, `async`, and
                      HashMap/HashSet use in the core
  --require-domain    exit 2 if no role="domain" crate is found (anti false-green)
  --skip-unparseable  tolerate a domain file `syn` cannot parse instead of
                      exiting 2; still withholds the clean verdict (anti false-green)
  --format <fmt>      human (default) | json
  --json              shorthand for --format json
  --baseline <FILE>   ratchet against accepted findings; fail only on new ones
  --update-baseline   rewrite the --baseline FILE from current findings
  -h, --help
```

**Exit codes** (so CI can tell a finding from a crash):

| Code | Meaning |
|------|---------|
| `0`  | clean, or `--advisory` |
| `1`  | effects leaked into a domain crate, or a stale baseline entry |
| `2`  | tool/usage error — bad flag, I/O failure, an unparseable domain file (without `--skip-unparseable`), or `--require-domain` with none found |

The exit-2 split matters: a gate that returns the same code for "found a
violation" and "the audit itself crashed" will eventually pass a crash off as
clean. `--require-domain` and the unparseable-file abort are the same instinct —
the tool refuses to exit green having audited *nothing* (a typo'd `role` key, the
wrong run directory) or having *skipped* a domain file it could not read. A file
`syn` cannot parse is a tool error by default; `--skip-unparseable` downgrades it
to a recorded skip (for a nightly syntax the parser lags), but even then the
clean "functional core holds" line is withheld — a skipped file is not vouched
for.

### Running it

Point it at the root of a workspace whose domain crates are marked
`[package.metadata.hex-arch] role = "domain"`:

```sh
cargo install --git https://github.com/igouss/effect-audit
effect-audit .                    # audit the current workspace
effect-audit --require-domain .   # fail (exit 2) if no domain crate is found
```

Or run it straight from a checkout without installing:

```sh
cargo run -- /path/to/workspace
```

Wire it into your repo as a pre-commit hook in `--advisory` mode — it prints,
never blocks:

```sh
#!/usr/bin/env bash
# .git/hooks/pre-commit (or your hook runner)
effect-audit --advisory --require-domain "$(git rev-parse --show-toplevel)" || true
```

Promote it to a blocking gate by dropping `--advisory` once the tree is green
(or baselined).

---

## Architecture

A functional core with a thin imperative shell — the same shape it enforces. The
**pure** modules (left) take syntax/values in and return classifications out,
with no I/O. The **shell** modules (right) do the `cargo metadata`, file reads,
and process exit. `lib.rs` exposes the modules so the binary and the executable
specs share one implementation.

```
                        ┌──────────────────────────────────────────────┐
   cargo metadata ─────▶│ discovery.rs  · shell                        │
                        │   find role="domain" crates;                 │
                        │   extract dep names + pure-deps allowlist    │
                        └───────────────────────┬──────────────────────┘
                                                │  Vec<DomainCrate>
                                                ▼
   ┌─────────────────────────────────────────────────────────────────────────┐
   │ audit.rs  · shell  (one crate at a time)                                  │
   │                                                                           │
   │   Cargo.toml ──▶ classify_dependency ─────────────────┐                   │
   │                  (effect.rs · pure: allow/deny)        │                   │
   │                                                        ├──▶ Vec<Finding>   │
   │   *.rs ──▶ modtree ──▶ scan_file ──▶ suppress ─────────┘                   │
   │           (skip       (scan.rs +    (fc-allow,                            │
   │            test-       effect.rs +   comment-only)                        │
   │            gated mods) cfg_pred · all pure)                              │
   └───────────────────────────────┬───────────────────────────────────────────┘
                                   │  Vec<Finding>
                                   ▼
            baseline.rs ──▶ Ratchet { fresh, stale }      · pure apply + thin I/O
                                   │
                                   ▼
            report.rs ──▶ human / json                    · pure
                                   │
                                   ▼
            main.rs ──▶ exit 0 / 1 / 2                    · shell (argv, exit code)
```

| Module | Role | Responsibility |
|--------|------|----------------|
| `effect.rs` | **core** | The taxonomy + pure classifiers (syntax in → `EffectKind` out), including `classify_dependency` — the allowlist/denylist decision. |
| `cfg_pred.rs` | **core** | Structural `#[cfg(...)]` evaluation (the `test` flag by satisfiability, never substring). |
| `scan.rs` | **core** | A `syn::visit::Visit` walker; `scan_file` is pure. |
| `suppress.rs` | **core** | The `fc-allow` rule, confined to comments via proc-macro2 token spans (no hand-rolled lexer). |
| `baseline.rs` | **core** + I/O | The ratchet. Pure `apply` + thin file read/write. |
| `finding.rs` | **core** | The result value (+ JSON / stable signature). |
| `report.rs` | **core** | Pure rendering (human + json). |
| `discovery.rs` | shell | `cargo metadata` → domain crates, raw dep names, `pure-deps` allowlist (facts only; policy lives in `effect.rs`). |
| `modtree.rs` | shell | Resolve test-gated modules to skip. |
| `audit.rs` | shell | Walk a crate's manifest + source into findings. |
| `main.rs` | shell | The CLI: args, exit code. |

Self-contained (an empty `[workspace]` table in `Cargo.toml`) so the binary
builds with a tiny dep graph and runs fast as a hook — the same pattern as
`tools/fn-hash`. The cucumber/tokio test stack is a `[dev-dependencies]` cost
only.

---

## Rolling it out on an existing codebase

Inline `fc-allow` is for a handful of deliberate exceptions. For a codebase that
already leaks, freeze today's debt and block new debt with a baseline:

```sh
effect-audit --baseline .effect-audit-baseline --update-baseline .   # freeze
effect-audit --baseline .effect-audit-baseline .                     # gate
```

The baseline is a plain TSV with a header:

```
# effect-audit baseline — accepted effects in the functional core.
# Format: <file>\t<kind>\t<snippet>. Ratchet: this file may only shrink.
# Regenerate with: effect-audit --baseline <this-file> --update-baseline
src/lib.rs	clock	std::time::SystemTime::now
```

It can only **shrink**: when a leak is fixed, its entry goes stale and the audit
fails (exit 1) until you re-run `--update-baseline`, so the file never lies
about debt that's already gone. The signature excludes the line number, so a
baseline entry survives unrelated edits to the same file instead of churning on
every commit.

### Suppressing a single deliberate effect

```rust
// fc-allow: W3C trace IDs are definitionally random; observability is cross-cutting
let id = getrandom(&mut buf);
```

The marker silences one finding on its own line or the line directly above it,
and **forces a rationale**: the `:` and a non-empty reason are required. A bare
`// fc-allow` (no colon, or an empty reason) is ignored, so the escape hatch can
never hide an effect without saying why.

It is recognised **only inside a comment** — true to the no-substring creed,
without hand-rolling a lexer. Comments aren't tokens (proc-macro2 discards
them), so a `fc-allow` occurrence is a real directive *iff its byte offset falls
outside every token span*. A marker inside a string is covered by a `Literal`
token; code that merely spells the bytes (`fc-allow::Foo` is `fc - allow::Foo`)
is covered by `Ident` tokens; only a comment leaves a gap. So a string whose
contents spell `fc-allow:` never suppresses anything. The marker has to be a
comment a human wrote, not data the program carries.

---

## Troubleshooting

| Symptom | Cause & fix |
|---------|-------------|
| **`no role = "domain" crate found; audited nothing`** | The run dir has no domain crate, or the metadata key is typo'd. Check `[package.metadata.hex-arch] role = "domain"` and that `ROOT` points inside the workspace. With `--require-domain` this is exit 2, not a silent green. |
| **CI exits `2`, not `0`/`1`** | That's a *tool* error, not a violation — a bad flag, an unreadable path, an unparseable domain file, or `--require-domain` with no domain crate. Read stderr; don't treat it as "found leaks." |
| **`cannot parse domain file …` → exit 2** | A `role = "domain"` file `syn` could not parse. The tool will not call a crate clean while a domain file in it is unread. Fix the syntax, or — if it's valid nightly syntax `syn` hasn't caught up to — pass `--skip-unparseable` to record it as skipped (the clean verdict is still withheld and the file is named on stderr). |
| **A clock/RNG call inside a macro — flagged or not?** | Effects inside the *evaluated* arguments of an allowlisted std macro (`format!`, `println!`, `vec!`, the `assert*!` family, `matches!`, …) **are** flagged — `format!("{}", SystemTime::now())` reports a `clock` at its real line. Arguments of a non-allowlisted macro or any proc-macro stay opaque (sound by omission); pull the call out, or `fc-allow` a deliberate one. |
| **My `test_support/` / fixture code got flagged** | It's not `#[cfg(test)]`-gated, so it's part of the production build and *is* audited (see "feature-gated ≠ test-only"). Gate it with `#[cfg(test)]`, move it to `tests/`, or add an `fc-allow` / baseline entry. |
| **One `use std::fs;` import shows up as two findings** | Deliberate: the import line and the call site each keep their own clickable location. Deduping by `(file, kind)` would lose that precision — and would lose glob imports (`use std::fs::*`) entirely, since their calls have no qualified site to flag. |
| **My `pure-deps` allowlist is being ignored** | A malformed `pure-deps` (present but not an array of strings) is treated as *absent* — the crate falls back to legacy denylist mode rather than erroring. Keep it a plain TOML string array; non-string elements are dropped. |
| **`effect-audit --advisory .` shows nothing in the hook** | Advisory mode prints to stderr and always exits 0. If the hook output is swallowed, run `effect-audit --require-domain .` directly to see findings (and a real exit code). |

---

## FAQ

**Why a separate tool from a dependency checker like `hex-lint`?** Because they
prove different things. The arrow checker proves *dependencies* point inward; this proves
*effects* stay out. A crate can pass the first and fail the second — `SystemTime::now()`
adds no dependency edge. Run both; they're two halves of one rule.

**Does it catch effects inside macros?** For a fixed allowlist of std macros that
**evaluate** their arguments — `format!`, `print!` / `println!`, `eprint!` /
`eprintln!`, `write!` / `writeln!`, `format_args!`, `vec!`, the `assert*!` /
`debug_assert*!` family, `panic!` / `todo!` / `unimplemented!`, and `matches!` —
yes. Their argument expressions are real runtime calls, so a clock read in
`format!("{}", SystemTime::now())` is flagged as a `clock`, with its true line
number, and nothing is fabricated. Everything else stays opaque: a proc-macro
(`json!`, `sqlx::query!`) and any macro off the allowlist keep their token
streams sealed, and a quoting macro like `stringify!(SystemTime::now())` never
evaluates its argument — flagging *that* would fabricate a call that never
happens, so we don't. On any parse failure the whole macro is skipped wholesale
rather than half-scanned. `thread_local!` and `lazy_static!` remain special-cased
for shared state.

**Why isn't `chrono` / `uuid` / `rand` a banned dependency?** Because a
`DateTime` value, a `Uuid` value, or a seed *passed into* the core is pure data
— perfectly legitimate in a domain. Only the nondeterministic *call* (`::now()`,
`new_v4()`, `thread_rng()`) is the effect, and that's caught precisely at the
call site by the AST scan.

**Does it flag `&mut self`?** No. Mutating a value you own and return is still
functional, and a plain field (`struct Editor { row: usize }`) is just data.
Only *shared* mutable state — module-level statics, `thread_local!`, and a field
whose **type** names interior mutability (`Mutex`, `RwLock`, `RefCell`, `Cell`,
`OnceLock`, `Atomic*`) — smuggles in nondeterminism, and that's what's flagged.
A field is the shape this usually takes in practice: a recording sink threaded
through a fake, a memo hung off a struct, a counter shared by clones.

**A `OnceLock` memo is not a recording sink — why the same finding?** Because
the tool cannot tell them apart, and guessing would be worse than asking. The
predicate is structural; `fc-allow: <why>` is where the author states which one
they wrote.

**Is it sound or complete?** Sound, not complete. It errs toward missing a
violation (an effect in a *non-allowlisted* macro's arguments, an exotic
effectful leaf of `net`/`env`/`process` not in the enumerated set) over inventing
one — pure value types are never flagged. Two deliberate choices trade a little
precision for recall, each with an escape hatch:

- *Name-based call-site heuristics.* `thread_rng()` and `.elapsed()` are flagged
  on the name alone, so a domain-local `fn elapsed()` or a `thread_rng` that
  genuinely is pure would trip once. Those names are distinctive enough that the
  trade buys real recall.
- *`HashMap` / `HashSet` presence (`--strict`).* A hash-iteration finding
  witnesses the type's **presence** — a fact, never fabricated — but the **harm**
  (an order actually escaping the domain) is a conservative over-approximation,
  so a hash collection whose order never leaks is still flagged.

`fc-allow` / a baseline silences either collision. The known gaps are listed
below.

**Can I run it on a repo that isn't hex-arch?** It only audits crates marked
`[package.metadata.hex-arch] role = "domain"`. No domain crates → it audits
nothing and says so loudly (and fails under `--require-domain`). It's a gate for
codebases that have already drawn the core/shell line.

**Can it auto-fix?** No, and it shouldn't. Every fix is "inject this as a port
and let a Boundary supply it" — an architectural decision, not a mechanical
rewrite. The tool points; you decide where the seam goes.

---

## Limitations

Honest about what it can't see (all **sound by omission** — silence, never a
fabricated violation):

- Effects inside a **non-allowlisted macro or a proc-macro** remain invisible.
  The tool scans the *evaluated* arguments of a fixed allowlist of std macros
  (`format!`, `println!`, `vec!`, the `assert*!` family, `matches!`, …), so
  `format!("{}", SystemTime::now())` **is** now flagged — but a proc-macro
  (`json!`, `sqlx::query!`) keeps its token stream sealed, and a macro that
  quotes rather than evaluates its input (`stringify!`) is left alone precisely so
  the tool never fabricates a call that never happens. `thread_local!` /
  `lazy_static!` stay special-cased for shared state.
- A **`HashMap` / `HashSet` finding witnesses presence, not a proven leak.** Under
  `--strict` the type's presence in the core's surface is flagged (a fact read off
  the AST), but whether a nondeterministic order actually escapes the domain is a
  conservative over-approximation — a held-and-never-iterated map is still
  reported. Deliberate; `fc-allow` / a baseline is the valve.
- Effects through an aliased *method* call (`x.now()` where `x`'s type is
  unknown) are not resolved; the qualified form `T::now()` and a `use T as A`
  type alias are. The one method caught by name is `.elapsed()` — distinctive
  enough to flag without knowing the receiver type (the same call it makes that
  `Instant::now()` does). `.duration_since(other)` is *not* flagged: it subtracts
  two values you already hold and reads no clock.
- `#[path = "…"]` modules are resolved for test-gating exclusion, but deeply
  relocated submodule trees may not be followed exhaustively.
- One logical leak imported and then called (`use std::fs;` *and* `fs::read(…)`)
  is reported twice — once at the import, once at the call. Deliberate (see
  Troubleshooting).
- A malformed `pure-deps` falls back to legacy denylist mode rather than
  erroring (see Troubleshooting).

---

## Tests

Behaviour is specified as executable Gherkin in `features/*.feature`, run by
`tests/cucumber.rs`. Source-level scenarios drive the pure core in-process; CLI
scenarios drive the built binary against the fixtures in `tests/fixtures/` (exit
codes, JSON, baseline, `--require-domain`). `features/dogfood.feature` turns the
tool on its own functional core and asserts zero findings — if it can't keep
effects out of itself, it has no business policing anyone else.

Fast unit tests (`#[cfg(test)]`) guard the pure classifiers at the
zero/one/many level underneath. Run everything with the project's canonical
runner:

```sh
cargo nextest run --manifest-path tools/effect-audit/Cargo.toml
# or, plain cargo:
cargo test --manifest-path tools/effect-audit/Cargo.toml
```

---

## License

Dual-licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
