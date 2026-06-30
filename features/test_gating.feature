Feature: Exempt test code, audit production code
  Only the built-in `test` cfg exempts code. The decision is structural — the
  predicate is evaluated as a boolean, never substring-matched — so `not(test)`
  and a feature whose name merely contains "test" are both audited.

  The gate is applied at one chokepoint over every item kind, not re-derived
  per visitor — so `#[cfg(test)]` on a `use`, a `static`, an `impl` block, or an
  item-position macro is honoured exactly like it is on a `fn` or a `mod`. The
  scenarios below pin each kind so a new item variant cannot silently re-open
  the hole.

  Scenario: A cfg(test) module is skipped
    Given domain source:
      """
      #[cfg(test)]
      mod tests {
          pub fn t() { let _ = std::fs::read("x"); }
      }
      """
    When the source is audited
    Then no effects are reported

  Scenario: A cfg(not(test)) module is the real impl and is audited
    Given domain source:
      """
      #[cfg(not(test))]
      mod real {
          pub fn f() { let _ = std::fs::read("x"); }
      }
      """
    When the source is audited
    Then a "filesystem" effect is reported

  Scenario: A feature named fastest is not test-only
    Given domain source:
      """
      #[cfg(feature = "fastest")]
      mod m {
          pub fn f() { let _ = std::fs::read("x"); }
      }
      """
    When the source is audited
    Then a "filesystem" effect is reported

  Scenario: A module named tests without a cfg gate is production
    Given domain source:
      """
      mod tests {
          pub fn f() { let _ = std::fs::read("x"); }
      }
      """
    When the source is audited
    Then a "filesystem" effect is reported

  Scenario: A cfg(test) use import is skipped
    Given domain source:
      """
      #[cfg(test)]
      use std::fs::read;
      """
    When the source is audited
    Then no effects are reported

  Scenario: A cfg(test) static is skipped
    Given domain source:
      """
      #[cfg(test)]
      static SEEDED: std::sync::Mutex<u32> = unsafe { todo!() };
      """
    When the source is audited
    Then no effects are reported

  Scenario: A cfg(test) static mut is skipped
    Given domain source:
      """
      #[cfg(test)]
      static mut COUNTER: u32 = 0;
      """
    When the source is audited
    Then no effects are reported

  Scenario: Effects inside a cfg(test) impl block are skipped
    Given domain source:
      """
      struct Order;
      #[cfg(test)]
      impl Order {
          fn touch(&self) { let _ = std::fs::read("x"); }
      }
      """
    When the source is audited
    Then no effects are reported

  Scenario: A cfg(test) item-position macro is skipped
    Given domain source:
      """
      #[cfg(test)]
      thread_local! { static C: std::cell::RefCell<u32> = todo!(); }
      """
    When the source is audited
    Then no effects are reported

  Scenario: A cfg(not(test)) static mut is production state and is audited
    Given domain source:
      """
      #[cfg(not(test))]
      static mut COUNTER: u32 = 0;
      """
    When the source is audited
    Then a "shared-mutable-state" effect is reported
