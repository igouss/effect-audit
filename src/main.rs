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
use effect_audit::discovery::{self, DomainCrate};
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
    --strict            Also flag effectful optional deps and `async` in the core.
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
    1   effects leaked into a domain crate (or a stale baseline entry)
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

/// Discover domain crates, audit them, apply the baseline, report, and turn the
/// outcome into an exit code.
fn run() -> Result<ExitCode> {
    let args: Args = parse_args()?;
    let crates: Vec<DomainCrate> = discovery::domain_crates(&args.root, args.strict)?;

    if crates.is_empty() {
        return Ok(handle_no_domain_crates(&args));
    }

    let config: AuditConfig = AuditConfig {
        scan: ScanConfig {
            flag_async: args.strict,
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
fn handle_no_domain_crates(args: &Args) -> ExitCode {
    eprintln!(
        "effect-audit: WARNING — no `role = \"domain\"` crate found; audited nothing.\n  \
         Check the metadata key `[package.metadata.hex-arch] role = \"domain\"` and the run dir."
    );
    if args.require_domain {
        eprintln!("  --require-domain set: failing.");
        return ExitCode::from(EXIT_TOOL_ERROR);
    }
    ExitCode::SUCCESS
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
fn emit(args: &Args, crates: &[DomainCrate], ratchet: &Ratchet, skipped: &[String]) {
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
                "effect-audit: {} domain crate(s) clean — functional core holds.",
                crates.len()
            );
        } else {
            // Honest verdict: we did not read every file, so we will not claim
            // the core holds. The skipped files are named by `warn_skipped`.
            println!(
                "effect-audit: {} domain crate(s) audited, {} file(s) skipped \
                 (unparseable, not vouched for).",
                crates.len(),
                skipped.len()
            );
        }
        warn_skipped(skipped);
        return;
    }
    if !ratchet.fresh.is_empty() {
        eprint!("{}", report::render(&ratchet.fresh));
        eprintln!(
            "\n  {} effect(s) leaked across {} file(s); {} domain crate(s) audited.",
            ratchet.fresh.len(),
            report::distinct_files(&ratchet.fresh),
            crates.len()
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
