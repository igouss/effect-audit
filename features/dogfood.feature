Feature: The tool obeys its own rule
  effect-audit is hexagonal: its judgement lives in a pure functional core. That
  core should pass its own gate — if it cannot keep effects out of itself, it
  has no business policing anyone else.

  Scenario: effect-audit's own functional core is effect-free
    When the tool audits its own functional core
    Then no effects are reported

  Scenario: effect-audit's own functional core holds under strict too
    When the tool audits its own functional core in strict mode
    Then no effects are reported
