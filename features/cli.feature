Feature: Command-line contract
  Exit codes distinguish a finding (1) from a tool error (2), so CI can tell a
  leak from a crash. Output, advisory mode, and domain discovery are observed at
  the process boundary against fixture workspaces.

  Scenario: A clean domain crate passes
    Given the "clean" fixture workspace
    When effect-audit runs
    Then it exits with code 0

  Scenario: A leaking domain crate fails with the violation code
    Given the "dirty" fixture workspace
    When effect-audit runs
    Then it exits with code 1
    And stderr contains "clock"

  Scenario: A root-level crate renders a clean src path
    Given the "dirty" fixture workspace
    When effect-audit runs
    Then stderr contains "src/lib.rs"
    And stderr does not contain "Cargo.toml/src"

  Scenario: Advisory mode never fails
    Given the "dirty" fixture workspace
    When effect-audit runs with "--advisory"
    Then it exits with code 0

  Scenario: JSON output is emitted on stdout
    Given the "dirty" fixture workspace
    When effect-audit runs with "--json"
    Then stdout contains "findings"
    And stdout contains "clock"

  Scenario: A test-gated module file is skipped, production is audited
    Given the "gated" fixture workspace
    When effect-audit runs
    Then it exits with code 1
    And stderr contains "src/prod.rs"
    And stderr does not contain "skipme"
    And stderr does not contain "custom_path"

  Scenario: An unknown flag is a tool error, not a violation
    When effect-audit runs with "--no-such-flag"
    Then it exits with code 2

  Scenario: An unparseable domain file is a tool error, never a clean verdict
    Given the "unparseable" fixture workspace
    When effect-audit runs
    Then it exits with code 2
    And stderr contains "cannot parse"
    And stdout does not contain "functional core holds"

  Scenario: skip-unparseable tolerates the file but withholds the clean verdict
    Given the "unparseable" fixture workspace
    When effect-audit runs with "--skip-unparseable"
    Then it exits with code 0
    And stdout does not contain "functional core holds"
    And stderr contains "not vouched for"

  Scenario: A missing domain crate warns but passes by default
    Given the "nodomain" fixture workspace
    When effect-audit runs
    Then it exits with code 0
    And stderr contains "no `role"

  Scenario: require-domain fails when no domain crate is found
    Given the "nodomain" fixture workspace
    When effect-audit runs with "--require-domain"
    Then it exits with code 2
