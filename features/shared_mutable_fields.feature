Feature: Flag interior mutability held in a struct field
  A `static CACHE: Mutex<_>` was already a `shared-mutable-state` finding. A
  `struct Cache { inner: Mutex<_> }` was not, in either mode — the visitor had
  no `visit_field`, so the only shapes it matched were `static`, `static mut`,
  and the `thread_local!` / `lazy_static!` macro bodies.

  That is backwards for the shape the rule exists to catch. A field is how
  interior mutability is actually written: a recording sink threaded through a
  fake, a memo hung off a struct, a counter shared by clones. The module-level
  static is the rare case. The gate matched the rare one and missed the common
  one, which is how a domain crate can carry a dozen `Arc<Mutex<..>>` fields
  while the verdict reads clean.

  A field is one witness, unlike an import-plus-use-site pair: `use
  std::sync::Mutex` is not itself classified as an effect, so the field is the
  only place the fact appears.

  Unconditional, in default mode, deliberately. The predicate is structural —
  the type either names interior mutability or it does not — so there is no
  heuristic here for `--strict` to add pressure to. The judgement call this
  gives up is that a `OnceLock` memoising a pure value reports the same as a
  `Mutex<Vec<Call>>` recording sink; `fc-allow` carries that distinction,
  because only the author knows which one they wrote.

  Scenario: A Mutex in a named field is shared mutable state
    Given domain source:
      """
      pub struct Cache {
          inner: std::sync::Mutex<u32>,
      }
      """
    When the source is audited
    Then exactly 1 effect is reported
    And a "shared-mutable-state" effect is reported

  Scenario: A shared recording sink is shared mutable state
    Given domain source:
      """
      pub struct FakeAgents {
          calls: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
      }
      """
    When the source is audited
    Then exactly 1 effect is reported
    And a "shared-mutable-state" effect is reported

  Scenario: A tuple-struct field is no different
    Given domain source:
      """
      pub struct Counter(std::sync::Mutex<u32>);
      """
    When the source is audited
    Then exactly 1 effect is reported
    And a "shared-mutable-state" effect is reported

  Scenario: An enum variant carrying interior mutability is caught too
    Given domain source:
      """
      pub enum Slot {
          Empty,
          Filled { cell: core::cell::RefCell<u32> },
      }
      """
    When the source is audited
    Then exactly 1 effect is reported
    And a "shared-mutable-state" effect is reported

  Scenario: An atomic field is shared mutable state
    Given domain source:
      """
      pub struct Counter {
          hits: core::sync::atomic::AtomicUsize,
      }
      """
    When the source is audited
    Then exactly 1 effect is reported
    And a "shared-mutable-state" effect is reported

  Scenario: Interior mutability nested inside another type still counts
    Given domain source:
      """
      pub struct Registry {
          slots: Vec<std::sync::Mutex<u32>>,
      }
      """
    When the source is audited
    Then exactly 1 effect is reported
    And a "shared-mutable-state" effect is reported

  Scenario: A memoising OnceLock reports the same as a recording sink
    Given domain source:
      """
      pub struct Memo {
          computed: std::sync::OnceLock<u32>,
      }
      """
    When the source is audited
    Then exactly 1 effect is reported
    And a "shared-mutable-state" effect is reported

  Scenario: Two fields on one struct are two findings
    Given domain source:
      """
      pub struct Fake {
          sent: std::sync::Mutex<Vec<String>>,
          closed: std::sync::Mutex<Vec<String>>,
      }
      """
    When the source is audited
    Then exactly 2 effects are reported

  Scenario: A plain value field is not a finding
    Given domain source:
      """
      pub struct Editor {
          lines: Vec<String>,
          row: usize,
      }
      """
    When the source is audited
    Then no effects are reported

  Scenario: A type that merely contains Cell in its name is not a finding
    Given domain source:
      """
      pub struct Wrapper {
          inner: Cellophane,
      }
      """
    When the source is audited
    Then no effects are reported

  Scenario: A field on a cfg(test) struct is not core surface
    Given domain source:
      """
      #[cfg(test)]
      pub struct Spy {
          calls: std::sync::Mutex<Vec<String>>,
      }
      """
    When the source is audited
    Then no effects are reported

  Scenario: A justified fc-allow above the field suppresses it
    Given domain source:
      """
      pub struct Memo {
          // fc-allow: memoises a pure computation, never observed as state
          computed: std::sync::OnceLock<u32>,
      }
      """
    When the source is audited
    Then no effects are reported

  Scenario: A bare fc-allow without a reason does not suppress it
    Given domain source:
      """
      pub struct Memo {
          // fc-allow
          computed: std::sync::OnceLock<u32>,
      }
      """
    When the source is audited
    Then exactly 1 effect is reported
    And a "shared-mutable-state" effect is reported

  Scenario: The rule needs no strict mode to fire
    Given domain source:
      """
      pub struct Cache {
          inner: std::sync::Mutex<u32>,
      }
      """
    When the source is audited in strict mode
    Then exactly 1 effect is reported
    And a "shared-mutable-state" effect is reported
