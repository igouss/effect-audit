Feature: Per-crate dependency allowlist
  A domain crate vouches for its pure-value dependencies in
  [package.metadata.hex-arch] pure-deps. In allowlist mode every normal dep not
  on the list is flagged — a recognised effectful crate by its effect, anything
  else as an "unvetted-dependency". Without the key, the legacy built-in denylist
  applies: only recognised effectful crates fire. This is the polarity flip that
  ends the taxonomy whack-a-mole — a new effectful crate is caught the day it is
  added, because it simply is not on the list.

  Scenario: Allowlist mode flags every undeclared dependency
    Given the "allowlist" fixture workspace
    When effect-audit runs
    Then it exits with code 1
    And stderr contains "reqwest"
    And stderr contains "some-unvetted-lib"
    And stderr contains "unvetted-dependency"
    And stderr does not contain "dependency: serde"

  Scenario: Denylist mode flags only recognised effectful crates
    Given the "denylist" fixture workspace
    When effect-audit runs
    Then it exits with code 1
    And stderr contains "reqwest"
    And stderr does not contain "dependency: serde"
