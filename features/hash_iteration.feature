Feature: Flag hash collections in the functional core, under --strict
  A `HashMap`/`HashSet` iterates in a nondeterministic order, so any order that
  escapes the domain smuggles in hash-seed randomness. The finding witnesses the
  type's *presence* in the core's surface — imports, constructors, and type
  positions — not a proven order leak, so it is a conservative over-approximation
  and lives under --strict next to async, with fc-allow/baseline as the pressure
  valves. Matching is structural (whole identifier), so a `struct HashMapper`, a
  comment, and a string literal `"HashMap"` are never mistaken for the type.

  Scenario: An import and a turbofish constructor are two findings, only under strict
    Given domain source:
      """
      use std::collections::HashMap;
      pub fn f() {
          let _ = HashMap::<u8, u8>::new();
      }
      """
    When the source is audited
    Then no effects are reported
    When the source is audited in strict mode
    Then exactly 2 effects are reported
    And a "hash-iteration" effect is reported

  Scenario: A struct named HashMapper, a comment, and a string are not hash types
    Given domain source:
      """
      pub struct HashMapper;
      pub fn note() -> &'static str {
          // HashMap in a comment
          "HashMap"
      }
      """
    When the source is audited in strict mode
    Then no effects are reported

  Scenario: A HashSet import is a presence finding under strict
    Given domain source:
      """
      use std::collections::HashSet;
      pub fn f() {}
      """
    When the source is audited in strict mode
    Then exactly 1 effect is reported
    And a "hash-iteration" effect is reported

  Scenario: An aliased hash constructor is flagged under strict
    Given domain source:
      """
      use std::collections::HashMap as Map;
      pub fn f() {
          let _ = Map::new();
      }
      """
    When the source is audited in strict mode
    Then exactly 2 effects are reported
    And a "hash-iteration" effect is reported

  Scenario: A held HashMap parameter type is a presence finding under strict
    Given domain source:
      """
      use std::collections::HashMap;
      pub fn f(m: &HashMap<u8, u8>) {
          let _ = m;
      }
      """
    When the source is audited in strict mode
    Then exactly 2 effects are reported
    And a "hash-iteration" effect is reported

  Scenario: A cfg(test) HashMap import is not core surface
    Given domain source:
      """
      #[cfg(test)]
      use std::collections::HashMap;
      """
    When the source is audited in strict mode
    Then no effects are reported

  Scenario: A justified fc-allow above a HashMap import suppresses only that import
    Given domain source:
      """
      // fc-allow: bootstrap map, order never escapes the core
      use std::collections::HashMap;
      pub fn f() {
          let _ = HashMap::<u8, u8>::new();
      }
      """
    When the source is audited in strict mode
    Then exactly 1 effect is reported
    And a "hash-iteration" effect is reported

  Scenario: A bare fc-allow without a reason does not suppress the hash finding
    Given domain source:
      """
      // fc-allow
      use std::collections::HashMap;
      pub fn f() {
          let _ = HashMap::<u8, u8>::new();
      }
      """
    When the source is audited in strict mode
    Then exactly 2 effects are reported
