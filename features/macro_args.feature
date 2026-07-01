Feature: Flag effects inside evaluated macro arguments
  A fixed allowlist of std macros (`format!`, `println!`, `vec!`, `assert_eq!`, …)
  evaluates its argument expressions call-by-value, so an effect written there is
  a real runtime call and must be seen. This scan is on by default — it fabricates
  nothing. Macros that quote or discard their input (`stringify!`), compile-time
  macros (`cfg!`/`concat!`), and any proc-macro off the list keep their tokens
  opaque: sound by omission, never a fabricated finding.

  Scenario: A clock inside format! is a single clock finding
    Given domain source:
      """
      pub fn f() -> String {
          format!("{}", std::time::SystemTime::now())
      }
      """
    When the source is audited
    Then exactly 1 effect is reported
    And a "clock" effect is reported

  Scenario: A clock inside a quoting macro is invisible
    Given domain source:
      """
      pub fn f() {
          let _ = stringify!(std::time::SystemTime::now());
      }
      """
    When the source is audited
    Then no effects are reported

  Scenario: A config macro is compile-time and off the allowlist
    Given domain source:
      """
      pub fn f() -> bool {
          cfg!(unix)
      }
      """
    When the source is audited
    Then no effects are reported

  Scenario: A concat macro is compile-time and off the allowlist
    Given domain source:
      """
      pub fn f() -> &'static str {
          concat!("a", "b")
      }
      """
    When the source is audited
    Then no effects are reported

  Scenario: An unknown proc-macro keeps its tokens opaque
    Given domain source:
      """
      pub fn f() {
          my_macro!(std::time::SystemTime::now());
      }
      """
    When the source is audited
    Then no effects are reported

  Scenario: A clock inside println! reports console and clock
    Given domain source:
      """
      pub fn f() {
          println!("{}", std::time::SystemTime::now());
      }
      """
    When the source is audited
    Then exactly 2 effects are reported
    And a "console-io" effect is reported
    And a "clock" effect is reported

  Scenario: A clock inside vec! is scanned
    Given domain source:
      """
      pub fn f() {
          let _ = vec![std::time::SystemTime::now()];
      }
      """
    When the source is audited
    Then exactly 1 effect is reported
    And a "clock" effect is reported

  Scenario: A clock inside assert_eq! is scanned
    Given domain source:
      """
      pub fn f(a: std::time::SystemTime) {
          assert_eq!(a, std::time::SystemTime::now());
      }
      """
    When the source is audited
    Then exactly 1 effect is reported
    And a "clock" effect is reported
