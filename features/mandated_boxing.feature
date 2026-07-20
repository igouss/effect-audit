Feature: Flag async-trait's mandated box in the functional core
  `#[async_trait]` rewrites every `async fn` in a trait into one returning
  `Pin<Box<dyn Future + Send>>`. The allocation is mandated — on every impl and
  every caller, whether or not the trait is ever held erased — and a 0.x
  proc-macro ends up in the layer whose whole value proposition is stability.

  The rule is deliberately narrow, and the narrowness is the point. A domain
  crate that spells the boxed future in its own trait signature is NOT flagged:
  for a port held as `Arc<dyn Port>` the box exists either way, so forbidding it
  would be allocation-purity wearing an effect-purity costume. What is flagged is
  the macro that removes the choice — as an attribute in the source, as an
  import, and as a dependency in the manifest. All three are unconditional: this
  is a structural fact about the crate, not a heuristic needing --strict's
  pressure valve. Import and attribute count separately, the same way an import
  and a constructor do for hash collections — each is its own witness.

  Scenario: The import and the attribute on a trait are two findings
    Given domain source:
      """
      use async_trait::async_trait;

      #[async_trait]
      pub trait Port {
          async fn get(&self) -> u8;
      }
      """
    When the source is audited
    Then exactly 2 effects are reported
    And a "mandated-boxing" effect is reported

  Scenario: The attribute on an impl is a finding
    Given domain source:
      """
      #[async_trait]
      impl Port for Adapter {
          async fn get(&self) -> u8 {
              0
          }
      }
      """
    When the source is audited
    Then exactly 1 effect is reported
    And a "mandated-boxing" effect is reported

  Scenario: The fully-qualified spelling needs no import to be caught
    Given domain source:
      """
      #[async_trait::async_trait]
      pub trait Port {
          async fn get(&self) -> u8;
      }
      """
    When the source is audited
    Then exactly 1 effect is reported
    And a "mandated-boxing" effect is reported

  Scenario: A hand-spelled boxed future is the fix, not a finding
    Given domain source:
      """
      use core::future::Future;
      use core::pin::Pin;

      pub trait Port: Send + Sync {
          fn get(&self) -> Pin<Box<dyn Future<Output = u8> + Send + '_>>;
      }
      """
    When the source is audited
    Then no effects are reported

  Scenario: A similarly-named attribute is not the macro
    Given domain source:
      """
      #[async_trait_ext]
      pub trait Port {
          fn get(&self) -> u8;
      }
      """
    When the source is audited
    Then no effects are reported

  Scenario: A cfg(test) impl carrying the attribute is not core surface
    Given domain source:
      """
      #[cfg(test)]
      #[async_trait]
      impl Port for Stub {
          async fn get(&self) -> u8 {
              0
          }
      }
      """
    When the source is audited
    Then no effects are reported

  Scenario: A justified fc-allow above the attribute suppresses it
    Given domain source:
      """
      // fc-allow: pinned until the port moves to a poll-based shape
      #[async_trait]
      pub trait Port {
          async fn get(&self) -> u8;
      }
      """
    When the source is audited
    Then no effects are reported

  Scenario: A bare fc-allow without a reason does not suppress it
    Given domain source:
      """
      // fc-allow
      #[async_trait]
      pub trait Port {
          async fn get(&self) -> u8;
      }
      """
    When the source is audited
    Then exactly 1 effect is reported
    And a "mandated-boxing" effect is reported

  Scenario: The dependency alone is a finding, with no source using it
    Given the "asynctrait" fixture workspace
    When effect-audit runs
    Then it exits with code 1
    And stderr contains "async-trait"
    And stderr contains "mandated-boxing"
