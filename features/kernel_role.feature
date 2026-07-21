Feature: Audit kernel crates under a stricter rule than domain
  `role = "kernel"` crates were never discovered at all: discovery exact-matched
  `role == "domain"`, so the verdict line counted domain crates and said nothing
  about the kernel. A workspace could hold a kernel crate that had grown a
  dependency and read "clean" — the sentence was true and the reader's inference
  from it was false.

  The kernel is the layer with the strongest claim and the least checking. What
  held it in practice was `#![no_std]` plus a hand-maintained empty dependency
  list: real, but the "doesn't today" form of the guarantee rather than the
  "cannot" form.

  So the kernel rule is stricter than domain's, not the same. A domain crate may
  vouch for a pure-value dependency through `pure-deps`; a kernel crate may not
  vouch for anything, because a crate whose premise is that it has no dependency
  graph by construction has nothing to vouch for. That makes `pure-deps`
  meaningless on a kernel crate, and declaring one is a mistake worth saying out
  loud rather than ignoring.

  The one dependency a kernel may hold is another kernel crate in the same
  workspace. That is not a softening — it is the same line hex-lint's role matrix
  draws ("kernel may depend on nothing but other kernel crates"), and the two
  gates disagreeing about the floor of the system is a defect in its own right.
  Everything else, third-party or otherwise, is a finding.

  Source scanning is unchanged: a kernel crate's code is audited by exactly the
  same effect rules as a domain crate's. Only the manifest rule differs.

  Scenario: A dependency-free kernel crate passes
    Given the "kernel" fixture workspace
    When effect-audit runs
    Then it exits with code 0
    And stdout contains "functional core holds"

  Scenario: The verdict counts kernel crates rather than omitting them
    Given the "kernel" fixture workspace
    When effect-audit runs
    Then stdout contains "kernel"

  Scenario: A kernel crate may depend on another kernel crate
    Given the "kernelpair" fixture workspace
    When effect-audit runs
    Then it exits with code 0
    And stdout contains "2 kernel crate(s) clean"

  Scenario: Any other normal dependency on a kernel crate is a finding
    Given the "kerneldep" fixture workspace
    When effect-audit runs
    Then it exits with code 1
    And stderr contains "serde"

  Scenario: A kernel crate cannot vouch its way out with pure-deps
    Given the "kernelvouch" fixture workspace
    When effect-audit runs
    Then it exits with code 1
    And stderr contains "serde"

  Scenario: Declaring pure-deps on a kernel crate is called out
    Given the "kernelvouch" fixture workspace
    When effect-audit runs
    Then stderr contains "pure-deps"

  Scenario: Effects in kernel source are flagged the same as in domain source
    Given the "kernelio" fixture workspace
    When effect-audit runs
    Then it exits with code 1
    And stderr contains "console-io"

  Scenario: A dev-dependency on a kernel crate is not a finding
    # The stdout assertion is load-bearing: exit 0 alone is also what a
    # workspace with no discovered crates returns, so it would stay green if
    # kernel discovery broke entirely.
    Given the "kerneldev" fixture workspace
    When effect-audit runs
    Then it exits with code 0
    And stdout contains "1 kernel crate(s) clean"

  Scenario: require-domain keeps meaning exactly what it says
    # A kernel crate is not a domain crate. Widening the existing flag to
    # accept one would quietly change what every current caller asserts, so
    # the kernel gets its own flag instead.
    Given the "kernel" fixture workspace
    When effect-audit runs with "--require-domain"
    Then it exits with code 2
    And stderr does not contain "audited nothing"

  Scenario: require-kernel fails when no kernel crate is found
    Given the "nodomain" fixture workspace
    When effect-audit runs with "--require-kernel"
    Then it exits with code 2

  Scenario: require-kernel passes when a kernel crate is present
    Given the "kernel" fixture workspace
    When effect-audit runs with "--require-kernel"
    Then it exits with code 0
