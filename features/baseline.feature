Feature: Baseline ratchet
  A checked-in baseline freezes existing debt so a team can adopt the gate on a
  leaking codebase. It can only shrink: when a leak is fixed its entry goes
  stale and the audit fails until the baseline is regenerated.

  Scenario: Baselined findings are accepted, and a fixed leak goes stale
    Given a baseline frozen from the "dirty" fixture
    When effect-audit runs on "dirty" against that baseline
    Then it exits with code 0
    When effect-audit runs on "clean" against that baseline
    Then it exits with code 1
    And stderr contains "stale"
