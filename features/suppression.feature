Feature: Inline suppression must carry a reason
  The `fc-allow` escape hatch silences one finding — on its own line or the line
  directly above. But it suppresses only when followed by `:` and a non-empty
  rationale, so the hatch can never hide an effect without recording *why*. A
  bare `// fc-allow` is treated as no marker at all.

  Scenario: A justified fc-allow suppresses the finding
    Given domain source:
      """
      pub fn t() {
          let _ = std::time::SystemTime::now(); // fc-allow: shell composition only
      }
      """
    When the source is audited
    Then no effects are reported

  Scenario: A justified fc-allow on the line above suppresses the finding
    Given domain source:
      """
      pub fn t() {
          // fc-allow: bootstrap seed, read once at composition
          let _ = std::time::SystemTime::now();
      }
      """
    When the source is audited
    Then no effects are reported

  Scenario: A bare fc-allow without a reason does not suppress
    Given domain source:
      """
      pub fn t() {
          let _ = std::time::SystemTime::now(); // fc-allow
      }
      """
    When the source is audited
    Then a "clock" effect is reported

  Scenario: An fc-allow with an empty reason does not suppress
    Given domain source:
      """
      pub fn t() {
          let _ = std::time::SystemTime::now(); // fc-allow:
      }
      """
    When the source is audited
    Then a "clock" effect is reported

  Scenario: The marker inside a string literal does not suppress
    Given domain source:
      """
      pub fn load() {
          let _ = std::fs::read("fc-allow: definitely not a real reason");
      }
      """
    When the source is audited
    Then a "filesystem" effect is reported

  Scenario: A double-slash inside a string is not a comment
    Given domain source:
      """
      pub fn load() {
          let _ = std::fs::read("// fc-allow: still a string");
      }
      """
    When the source is audited
    Then a "filesystem" effect is reported

  Scenario: A justified marker in a block comment suppresses
    Given domain source:
      """
      pub fn t() {
          let _ = std::time::SystemTime::now(); /* fc-allow: shell composition */
      }
      """
    When the source is audited
    Then no effects are reported
