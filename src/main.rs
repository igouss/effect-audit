//! effect-audit CLI — the imperative shell over the `effect_audit` library.
//!
//! Parses arguments, drives [`effect_audit::audit`], applies the baseline,
//! renders, and maps the outcome to an exit code. All the judgement lives in
//! the library; this file is argv, I/O, and process exit.
//!
//! Exit codes: 0 = clean (or advisory), 1 = effects leaked, 2 = tool/usage
//! error — so CI can tell "found leaks" from "the audit itself broke".

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};

use effect_audit::audit::{AuditConfig, AuditOutcome};
use effect_audit::baseline::{self, Ratchet};
use effect_audit::discovery::{self, CoreCrate, Role};
use effect_audit::finding::Finding;
use effect_audit::scan::ScanConfig;
use effect_audit::{audit, report};

/// Exit code for "effects leaked" — a policy violation, distinct from a crash.
const EXIT_VIOLATION: u8 = 1;
/// Exit code for a tool or usage error — bad flag, I/O failure, missing domain.
const EXIT_TOOL_ERROR: u8 = 2;

const HELP: &str = "\
effect-audit — enforce functional core / imperative shell in domain crates.

USAGE:
    effect-audit [OPTIONS] [ROOT]

ARGS:
    ROOT    A path inside the workspace to audit (default: current directory).

OPTIONS:
    --advisory          Print findings but always exit 0 (warn-only hook).
    --strict            Also flag effectful optional deps, `async` in the core,
                        and HashMap/HashSet use in the core.
    --require-kernel    Fail (exit 2) if no `role = \"kernel\"` crate is found.
    --require-domain    Fail (exit 2) if no `role = \"domain\"` crate is found,
                        instead of passing green having audited nothing.
    --skip-unparseable  Tolerate a domain file `syn` cannot parse (record it as
                        skipped) instead of failing (exit 2). The clean verdict
                        is still withheld — a skipped file is not vouched for.
    --format <fmt>      Output format: `human` (default) or `json`.
    --json              Shorthand for --format json.
    --baseline <FILE>   Ratchet against accepted findings in FILE; fail only on
                        findings absent from it, and on stale FILE entries.
    --update-baseline   Rewrite the --baseline FILE from current findings.
    -h, --help          Show this help.

EXIT CODES:
    0   clean, or --advisory
    1   effects leaked into a core crate (or a stale baseline entry)
    2   tool/usage error (bad flag, I/O failure, an unparseable domain file
        without --skip-unparseable, or --require-domain with no domain crate)

SUPPRESS:
    Add `// fc-allow: <reason>` on an offending line (or the line above it) to
    silence a single deliberate effect. For bulk adoption, use --baseline.";

/// Output format.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Human,
    Json,
}

/// Parsed command line.
struct Args {
    root: PathBuf,
    advisory: bool,
    strict: bool,
    require_domain: bool,
    require_kernel: bool,
    skip_unparseable: bool,
    format: Format,
    baseline: Option<PathBuf>,
    update_baseline: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("effect-audit: {err:#}");
            ExitCode::from(EXIT_TOOL_ERROR)
        }
    }
}

/// Discover core crates, audit them, apply the baseline, report, and turn the
/// outcome into an exit code.
fn run() -> Result<ExitCode> {
    let args: Args = parse_args()?;
    let crates: Vec<CoreCrate> = discovery::core_crates(&args.root, args.strict)?;

    if crates.is_empty() {
        return Ok(handle_no_core_crates(&args));
    }
    if let Some(missing) = required_role_missing(&args, &crates) {
        return Ok(missing_required_role(missing));
    }
    warn_inert_vouching(&crates);

    let config: AuditConfig = AuditConfig {
        scan: ScanConfig {
            flag_async: args.strict,
            flag_hash: args.strict,
        },
        skip_unparseable: args.skip_unparseable,
    };
    let outcome: AuditOutcome = audit::audit(&crates, config)?;

    if args.update_baseline {
        return update_baseline(&args, &outcome);
    }

    let findings: Vec<Finding> = outcome.findings;
    let skipped: Vec<String> = outcome.skipped;

    let ratchet: Ratchet = match &args.baseline {
        Some(path) => baseline::apply(findings, &baseline::load(path)?),
        None => Ratchet {
            fresh: findings,
            stale: Vec::new(),
        },
    };

    emit(&args, &crates, &ratchet, &skipped);
    Ok(exit_code(&args, &ratchet))
}

/// Loud-on-empty: never pass silently green having audited nothing.
fn handle_no_core_crates(args: &Args) -> ExitCode {
    eprintln!(
        "effect-audit: WARNING — no `role = \"kernel\"` or `role = \"domain\"` crate found; \
         audited nothing.\n  \
         Check the metadata key `[package.metadata.hex-arch] role` and the run dir."
    );
    if args.require_domain || args.require_kernel {
        eprintln!("  --require-domain/--require-kernel set: failing.");
        return ExitCode::from(EXIT_TOOL_ERROR);
    }
    ExitCode::SUCCESS
}

/// The role a `--require-*` flag demanded but no crate declares, if any. Both
/// flags may be set; the kernel is reported first because it is the inner layer.
fn required_role_missing(args: &Args, crates: &[CoreCrate]) -> Option<Role> {
    let has = |role: Role| crates.iter().any(|c: &CoreCrate| c.role == role);
    if args.require_kernel && !has(Role::Kernel) {
        return Some(Role::Kernel);
    }
    if args.require_domain && !has(Role::Domain) {
        return Some(Role::Domain);
    }
    None
}

/// A role was required but no crate declares it. Distinct from auditing
/// nothing at all: here the tool did work, just not the work that was demanded.
fn missing_required_role(role: Role) -> ExitCode {
    eprintln!(
        "effect-audit: --require-{0} set, but no `role = \"{0}\"` crate was found.",
        role.as_str()
    );
    ExitCode::from(EXIT_TOOL_ERROR)
}

/// How many crates of each role were audited, phrased for the verdict line.
/// Names only the roles actually present, so a domain-only workspace reads
/// exactly as it did before kernels existed.
fn census(crates: &[CoreCrate]) -> String {
    let kernels: usize = crates
        .iter()
        .filter(|c: &&CoreCrate| c.role == Role::Kernel)
        .count();
    let domains: usize = crates.len() - kernels;
    match (kernels, domains) {
        (0, d) => format!("{d} domain crate(s)"),
        (k, 0) => format!("{k} kernel crate(s)"),
        (k, d) => format!("{k} kernel + {d} domain crate(s)"),
    }
}

/// A `pure-deps` list on a kernel crate cannot make a dependency acceptable,
/// because no dependency is. Silently ignoring it would let an author believe
/// they had vouched their way to green.
fn warn_inert_vouching(crates: &[CoreCrate]) {
    for c in crates
        .iter()
        .filter(|c: &&CoreCrate| c.role == Role::Kernel && c.pure_deps.is_some())
    {
        eprintln!(
            "effect-audit: WARNING — {} declares `pure-deps` but is `role = \"kernel\"`; \
             a kernel vouches for nothing, so the list is ignored.",
            c.name
        );
    }
}

/// `--update-baseline`: rewrite the baseline file from the current findings.
fn update_baseline(args: &Args, outcome: &AuditOutcome) -> Result<ExitCode> {
    let path: &PathBuf = args
        .baseline
        .as_ref()
        .context("--update-baseline requires --baseline <FILE>")?;
    baseline::write(path, &outcome.findings)?;
    eprintln!(
        "effect-audit: wrote baseline ({} accepted finding(s)) to {}",
        outcome.findings.len(),
        path.display()
    );
    warn_skipped(&outcome.skipped);
    Ok(ExitCode::SUCCESS)
}

/// Render the outcome in the requested format.
fn emit(args: &Args, crates: &[CoreCrate], ratchet: &Ratchet, skipped: &[String]) {
    if args.format == Format::Json {
        println!(
            "{}",
            report::render_json(&ratchet.fresh, &ratchet.stale, skipped)
        );
        return;
    }
    if ratchet.fresh.is_empty() && ratchet.stale.is_empty() {
        if skipped.is_empty() {
            println!(
                "effect-audit: {} clean — functional core holds.",
                census(crates)
            );
        } else {
            // Honest verdict: we did not read every file, so we will not claim
            // the core holds. The skipped files are named by `warn_skipped`.
            println!(
                "effect-audit: {} audited, {} file(s) skipped \
                 (unparseable, not vouched for).",
                census(crates),
                skipped.len()
            );
        }
        warn_skipped(skipped);
        return;
    }
    if !ratchet.fresh.is_empty() {
        eprint!("{}", report::render(&ratchet.fresh));
        eprintln!(
            "\n  {} effect(s) leaked across {} file(s); {} audited.",
            ratchet.fresh.len(),
            report::distinct_files(&ratchet.fresh),
            census(crates)
        );
    }
    if !ratchet.stale.is_empty() {
        eprintln!(
            "\n  {} stale baseline entr(y/ies) — the leak is gone, remove it with \
             --update-baseline:",
            ratchet.stale.len()
        );
        for signature in &ratchet.stale {
            eprintln!("    {signature}");
        }
    }
    warn_skipped(skipped);
    if args.advisory {
        eprintln!("  (advisory mode — not failing the build.)");
    }
}

/// Name every domain file `--skip-unparseable` tolerated, so a skipped file is
/// never silent — the operator sees exactly what was not audited or vouched for.
fn warn_skipped(skipped: &[String]) {
    if skipped.is_empty() {
        return;
    }
    eprintln!(
        "\n  {} unparseable file(s) skipped — NOT audited, not vouched for:",
        skipped.len()
    );
    for file in skipped {
        eprintln!("    {file}");
    }
}

/// 0 if clean or advisory; 1 if any fresh finding or stale baseline entry.
fn exit_code(args: &Args, ratchet: &Ratchet) -> ExitCode {
    let violated: bool = !ratchet.fresh.is_empty() || !ratchet.stale.is_empty();
    if !violated || args.advisory {
        return ExitCode::SUCCESS;
    }
    ExitCode::from(EXIT_VIOLATION)
}

/// Parse the command line. Unknown flags and misuse `bail!` (→ exit 2);
/// `--help` prints and exits 0.
fn parse_args() -> Result<Args> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut root: Option<PathBuf> = None;
    let mut advisory: bool = false;
    let mut strict: bool = false;
    let mut require_domain: bool = false;
    let mut require_kernel: bool = false;
    let mut skip_unparseable: bool = false;
    let mut format: Format = Format::Human;
    let mut baseline: Option<PathBuf> = None;
    let mut update_baseline: bool = false;

    let mut index: usize = 0;
    while index < argv.len() {
        let arg: &str = argv[index].as_str();
        match arg {
            "--advisory" => advisory = true,
            "--strict" => strict = true,
            "--require-domain" => require_domain = true,
            "--require-kernel" => require_kernel = true,
            "--skip-unparseable" => skip_unparseable = true,
            "--json" => format = Format::Json,
            "--format" => {
                index += 1;
                format = parse_format(argv.get(index))?;
            }
            "--baseline" => {
                index += 1;
                let path: &String = argv.get(index).context("--baseline requires a FILE")?;
                baseline = Some(PathBuf::from(path));
            }
            "--update-baseline" => update_baseline = true,
            "-h" | "--help" => {
                println!("{HELP}");
                std::process::exit(0);
            }
            flag if flag.starts_with('-') => bail!("unknown flag: {flag}"),
            path if root.is_some() => bail!("unexpected extra argument: {path}"),
            path => root = Some(PathBuf::from(path)),
        }
        index += 1;
    }

    if update_baseline && baseline.is_none() {
        bail!("--update-baseline requires --baseline <FILE>");
    }

    let requested: PathBuf = root.unwrap_or_else(|| PathBuf::from("."));
    let root: PathBuf = requested
        .canonicalize()
        .with_context(|| format!("cannot resolve root: {}", requested.display()))?;
    Ok(Args {
        root,
        advisory,
        strict,
        require_domain,
        require_kernel,
        skip_unparseable,
        format,
        baseline,
        update_baseline,
    })
}

/// Parse the `--format` value.
fn parse_format(value: Option<&String>) -> Result<Format> {
    match value.map(String::as_str) {
        Some("human") => Ok(Format::Human),
        Some("json") => Ok(Format::Json),
        Some(other) => bail!("unknown format: {other} (expected `human` or `json`)"),
        None => bail!("--format requires a value (`human` or `json`)"),
    }
}

#[cfg(test)]
mod tests {
    use super::HELP;

    /// The `--strict` help entry must name every check `--strict` turns on, so a
    /// silently-added or -renamed check can never drift out of the documented
    /// contract. Guards the third check (`HashMap`/`HashSet`) wired in this bead.
    #[test]
    fn help_strict_names_the_hash_iteration_check() {
        assert!(
            HELP.contains("HashMap"),
            "--strict help must name HashMap: {HELP}"
        );
        assert!(
            HELP.contains("HashSet"),
            "--strict help must name HashSet: {HELP}"
        );
    }

    /// The two long-standing `--strict` checks must stay named too, so widening
    /// the entry for the hash check did not drop the async / optional-dep prose.
    #[test]
    fn help_strict_still_names_the_original_checks() {
        assert!(
            HELP.contains("async"),
            "--strict help must still name async: {HELP}"
        );
        assert!(
            HELP.contains("optional deps"),
            "--strict help must still name optional deps: {HELP}"
        );
    }
}
