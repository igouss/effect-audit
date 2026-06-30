//! Executable specs. cucumber parses `features/*.feature` and runs the step
//! definitions below, so the Gherkin **is** the behavioural test source.
//!
//! Source-level scenarios drive the pure core in-process via
//! `effect_audit::scan`; CLI scenarios drive the built binary at the process
//! boundary (exit codes, output, baseline). One ordinary `#[tokio::test]`
//! runner (no `harness = false`) keeps `cargo nextest` able to list and run it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code — terse panics on broken invariants beat Result plumbing"
)]
#![allow(
    clippy::needless_pass_by_ref_mut,
    clippy::trivial_regex,
    reason = "cucumber mandates the `&mut World` step signature and treats step \
              patterns as exact prose matchers"
)]

use std::path::PathBuf;
use std::process::{Command, Output};

use cucumber::gherkin::Step;
use cucumber::{given, then, when, World};
use effect_audit::finding::Finding;
use effect_audit::scan::{scan_file, ScanConfig};

/// One scenario's state: either an in-process source audit, or the captured
/// result of running the binary against a fixture.
#[derive(Debug, Default, World)]
struct AuditWorld {
    /// Domain source under test (source-level scenarios).
    source: String,
    /// Findings from the most recent audit (source-level or dogfood).
    findings: Vec<Finding>,
    /// Fixture workspace name for CLI scenarios.
    fixture: Option<String>,
    /// A baseline file frozen during the scenario.
    baseline: Option<PathBuf>,
    /// Last CLI invocation's exit code and streams.
    exit: Option<i32>,
    stdout: String,
    stderr: String,
}

/// The freshly built binary, provided by cargo for integration tests.
fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_effect-audit")
}

/// Absolute path to a fixture workspace directory.
fn fixture_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Run the binary, returning (exit code, stdout, stderr).
fn run_cli(args: &[String]) -> (Option<i32>, String, String) {
    let out: Output = Command::new(binary())
        .args(args)
        .output()
        .expect("spawn effect-audit");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Audit a source string in-process and store the findings.
fn audit_source(world: &mut AuditWorld, config: ScanConfig) {
    let parsed: syn::File = syn::parse_file(&world.source).expect("valid rust source");
    world.findings = scan_file(&parsed, "spec.rs", &world.source, config);
}

// ─── Given ───────────────────────────────────────────────────────────

#[given("domain source:")]
fn given_source(world: &mut AuditWorld, step: &Step) {
    world.source = step.docstring.clone().expect("a docstring of source");
}

#[given(regex = r#"^the "([^"]+)" fixture workspace$"#)]
fn given_fixture(world: &mut AuditWorld, name: String) {
    world.fixture = Some(name);
}

#[given(regex = r#"^a baseline frozen from the "([^"]+)" fixture$"#)]
fn given_baseline(world: &mut AuditWorld, name: String) {
    let path: PathBuf = std::env::temp_dir().join(format!(
        "effect-audit-spec-{}-{name}.baseline",
        std::process::id()
    ));
    let dir: PathBuf = fixture_dir(&name);
    let args: Vec<String> = vec![
        "--baseline".to_owned(),
        path.to_string_lossy().into_owned(),
        "--update-baseline".to_owned(),
        dir.to_string_lossy().into_owned(),
    ];
    let (exit, _, stderr) = run_cli(&args);
    assert_eq!(exit, Some(0), "freezing baseline failed: {stderr}");
    world.baseline = Some(path);
}

// ─── When ────────────────────────────────────────────────────────────

#[when("the source is audited")]
fn when_audited(world: &mut AuditWorld) {
    audit_source(world, ScanConfig::default());
}

#[when("the source is audited in strict mode")]
fn when_audited_strict(world: &mut AuditWorld) {
    audit_source(world, ScanConfig { flag_async: true });
}

#[when("effect-audit runs")]
fn when_runs(world: &mut AuditWorld) {
    let dir: PathBuf = fixture_dir(world.fixture.as_deref().expect("a fixture"));
    let (exit, stdout, stderr) = run_cli(&[dir.to_string_lossy().into_owned()]);
    world.exit = exit;
    world.stdout = stdout;
    world.stderr = stderr;
}

#[when(regex = r#"^effect-audit runs with "([^"]*)"$"#)]
fn when_runs_with(world: &mut AuditWorld, flags: String) {
    let mut args: Vec<String> = flags.split_whitespace().map(str::to_owned).collect();
    if let Some(name) = &world.fixture {
        args.push(fixture_dir(name).to_string_lossy().into_owned());
    }
    let (exit, stdout, stderr) = run_cli(&args);
    world.exit = exit;
    world.stdout = stdout;
    world.stderr = stderr;
}

#[when(regex = r#"^effect-audit runs on "([^"]+)" against that baseline$"#)]
fn when_runs_against_baseline(world: &mut AuditWorld, name: String) {
    let base: &PathBuf = world.baseline.as_ref().expect("a frozen baseline");
    let args: Vec<String> = vec![
        "--baseline".to_owned(),
        base.to_string_lossy().into_owned(),
        fixture_dir(&name).to_string_lossy().into_owned(),
    ];
    let (exit, stdout, stderr) = run_cli(&args);
    world.exit = exit;
    world.stdout = stdout;
    world.stderr = stderr;
}

#[when("the tool audits its own functional core")]
fn when_dogfood(world: &mut AuditWorld) {
    // The pure core only — the shell (discovery/modtree/audit/main/baseline)
    // legitimately performs I/O and is excluded by construction.
    const CORE: &[&str] = &[
        "effect.rs",
        "cfg_pred.rs",
        "scan.rs",
        "suppress.rs",
        "finding.rs",
        "report.rs",
    ];
    let src: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    world.findings.clear();
    for file in CORE {
        let path: PathBuf = src.join(file);
        let source: String = std::fs::read_to_string(&path).expect("read core file");
        let parsed: syn::File = syn::parse_file(&source).expect("core file parses");
        world
            .findings
            .extend(scan_file(&parsed, file, &source, ScanConfig::default()));
    }
}

// ─── Then ────────────────────────────────────────────────────────────

#[then(regex = r#"^an? "([^"]+)" effect is reported$"#)]
fn then_effect(world: &mut AuditWorld, kind: String) {
    assert!(
        world
            .findings
            .iter()
            .any(|f: &Finding| f.kind.label() == kind),
        "expected a {kind:?} effect, got {:?}",
        world.findings
    );
}

#[then("no effects are reported")]
fn then_no_effects(world: &mut AuditWorld) {
    assert!(
        world.findings.is_empty(),
        "expected no effects, got {:?}",
        world.findings
    );
}

#[then(regex = r"^exactly (\d+) effect(?:s)? (?:is|are) reported$")]
fn then_exact_count(world: &mut AuditWorld, count: usize) {
    assert_eq!(
        world.findings.len(),
        count,
        "expected {count} effect(s), got {:?}",
        world.findings
    );
}

#[then(regex = r"^it exits with code (\d+)$")]
fn then_exit(world: &mut AuditWorld, code: i32) {
    assert_eq!(world.exit, Some(code), "stderr was: {}", world.stderr);
}

#[then(regex = r#"^stderr contains "([^"]*)"$"#)]
fn then_stderr_contains(world: &mut AuditWorld, text: String) {
    assert!(
        world.stderr.contains(&text),
        "stderr did not contain {text:?}; was:\n{}",
        world.stderr
    );
}

#[then(regex = r#"^stderr does not contain "([^"]*)"$"#)]
fn then_stderr_excludes(world: &mut AuditWorld, text: String) {
    assert!(
        !world.stderr.contains(&text),
        "stderr unexpectedly contained {text:?}; was:\n{}",
        world.stderr
    );
}

#[then(regex = r#"^stdout contains "([^"]*)"$"#)]
fn then_stdout_contains(world: &mut AuditWorld, text: String) {
    assert!(
        world.stdout.contains(&text),
        "stdout did not contain {text:?}; was:\n{}",
        world.stdout
    );
}

#[then(regex = r#"^stdout does not contain "([^"]*)"$"#)]
fn then_stdout_excludes(world: &mut AuditWorld, text: String) {
    assert!(
        !world.stdout.contains(&text),
        "stdout unexpectedly contained {text:?}; was:\n{}",
        world.stdout
    );
}

// ─── Runner ──────────────────────────────────────────────────────────

/// Run the executable specs as one ordinary libtest case so `cargo nextest`
/// can list and drive it natively. An undefined step (spec wording drifted from
/// every `#[given/when/then]`) is treated as a failure.
#[tokio::test]
async fn executable_specs() {
    use cucumber::writer::Stats as _;

    let writer = AuditWorld::cucumber()
        // Hand cucumber an empty CLI so it does not parse libtest/nextest flags.
        .with_default_cli()
        .fail_on_skipped()
        .run(concat!(env!("CARGO_MANIFEST_DIR"), "/features"))
        .await;

    assert!(
        !writer.execution_has_failed(),
        "executable specs failed: {} failed step(s), {} parsing error(s)",
        writer.failed_steps(),
        writer.parsing_errors(),
    );
}
