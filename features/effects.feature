Feature: Flag side effects in the functional core
  A role="domain" crate must be pure — values in, values out. effect-audit
  flags side effects at the call site, parsing real tokens so comments and
  string literals never trigger a finding.

  Scenario: A wall-clock read is a clock effect
    Given domain source:
      """
      pub fn stamp() -> std::time::SystemTime {
          std::time::SystemTime::now()
      }
      """
    When the source is audited
    Then a "clock" effect is reported

  Scenario: The time crate spells the constructor now_utc
    Given domain source:
      """
      pub fn t() {
          let _ = time::OffsetDateTime::now_utc();
      }
      """
    When the source is audited
    Then a "clock" effect is reported

  Scenario: An aliased clock type is still a clock
    Given domain source:
      """
      use std::time::SystemTime as Clk;
      pub fn t() {
          let _ = Clk::now();
      }
      """
    When the source is audited
    Then a "clock" effect is reported

  Scenario: Random draws are flagged but a module named random is not
    Given domain source:
      """
      pub fn id() {
          let _ = rand::random::<u8>();
      }
      pub fn pure() -> u8 {
          crate::random::from_seed(7)
      }
      """
    When the source is audited
    Then exactly 1 effect is reported
    And a "random" effect is reported

  Scenario: A domain-local fn named random called bare is not a draw
    Given domain source:
      """
      fn random() -> u32 { 4 }
      pub fn pick() -> u32 {
          random() + random()
      }
      """
    When the source is audited
    Then no effects are reported

  Scenario: Filesystem access is an effect
    Given domain source:
      """
      pub fn load() {
          let _ = std::fs::read("config");
      }
      """
    When the source is audited
    Then a "filesystem" effect is reported

  Scenario: A clock read through the elapsed method is a clock effect
    Given domain source:
      """
      pub fn since(start: std::time::Instant) -> std::time::Duration {
          start.elapsed()
      }
      """
    When the source is audited
    Then a "clock" effect is reported

  Scenario: duration_since subtracts two held values and is pure
    Given domain source:
      """
      pub fn between(a: std::time::Instant, b: std::time::Instant) -> std::time::Duration {
          a.duration_since(b)
      }
      """
    When the source is audited
    Then no effects are reported

  Scenario: A network address value is data, not a network effect
    Given domain source:
      """
      pub fn loopback() -> std::net::Ipv4Addr {
          std::net::Ipv4Addr::new(127, 0, 0, 1)
      }
      """
    When the source is audited
    Then no effects are reported

  Scenario: Opening a TCP socket is a network effect
    Given domain source:
      """
      pub fn dial() {
          let _ = std::net::TcpStream::connect("127.0.0.1:80");
      }
      """
    When the source is audited
    Then a "network" effect is reported

  Scenario: A compile-time env const is not an environment read
    Given domain source:
      """
      pub fn arch() -> &'static str {
          std::env::consts::ARCH
      }
      """
    When the source is audited
    Then no effects are reported

  Scenario: Reading an env var is an environment effect
    Given domain source:
      """
      pub fn dir() {
          let _ = std::env::var("HOME");
      }
      """
    When the source is audited
    Then an "environment" effect is reported

  Scenario: A process exit-code value is not a process effect
    Given domain source:
      """
      pub fn ok() -> std::process::ExitCode {
          std::process::ExitCode::SUCCESS
      }
      """
    When the source is audited
    Then no effects are reported

  Scenario: Exiting the process is a process effect
    Given domain source:
      """
      pub fn die() {
          std::process::exit(1);
      }
      """
    When the source is audited
    Then a "process" effect is reported

  Scenario: A console macro is an effect
    Given domain source:
      """
      pub fn shout() {
          println!("hi");
      }
      """
    When the source is audited
    Then a "console-io" effect is reported

  Scenario: Spawning a thread is concurrency
    Given domain source:
      """
      pub fn go() {
          std::thread::spawn(|| {});
      }
      """
    When the source is audited
    Then a "concurrency" effect is reported

  Scenario: A static Mutex is shared mutable state
    Given domain source:
      """
      static CACHE: std::sync::Mutex<u32> = std::sync::Mutex::new(0);
      """
    When the source is audited
    Then a "shared-mutable-state" effect is reported

  Scenario: A type that merely contains Cell is not shared mutable state
    Given domain source:
      """
      static WRAP: Cellophane = make();
      """
    When the source is audited
    Then no effects are reported

  Scenario: An immutable thread_local is not shared mutable state
    Given domain source:
      """
      thread_local! { static DEPTH: u32 = 0; }
      """
    When the source is audited
    Then no effects are reported

  Scenario: A mutable thread_local is shared mutable state
    Given domain source:
      """
      thread_local! { static C: std::cell::RefCell<u32> = todo!(); }
      """
    When the source is audited
    Then a "shared-mutable-state" effect is reported

  Scenario: Receiving an effectful value type is fine
    Given domain source:
      """
      pub fn at(t: std::time::SystemTime) -> std::time::SystemTime {
          t
      }
      """
    When the source is audited
    Then no effects are reported

  Scenario: A comment or string mentioning an effect is not code
    Given domain source:
      """
      pub fn note() -> &'static str {
          // calls SystemTime::now somewhere
          "std::fs::read"
      }
      """
    When the source is audited
    Then no effects are reported

  Scenario: async is only an effect under strict
    Given domain source:
      """
      pub async fn f() {}
      """
    When the source is audited
    Then no effects are reported
    When the source is audited in strict mode
    Then an "async-runtime" effect is reported
