# Evolutions & History for this project

Here you'll find the *backlogs*, *being-done*, *completed* and *bug reports* for this project.

Issues contain a *prefix* letter and a sequence number, possibly followed by a date and a context-full description.


## Prefixes

  - (b) bug fix for broken functionalities
  - (f) new functional requisite
  - (n) new non-functional requisite
  - (r) internal re-engineering / refactor / improvement, to increase speed and/or enable further progress to be done cheaper




# Being-Done

**(n1)** 2023-07-27: Develop and introduce the freshly planned *Zero-Cost Const Configuration Pattern* pattern.
  1) Build a proof-of-concept in the Rust playground -- https://play.rust-lang.org/?version=stable&mode=debug&edition=2021&gist=f6ce6729eb1cf60021cfa594cace1364 -- adding the rationalle for the pattern, which is:
     > Allow generic types to have comprehensive configuration options with a single constant, since `const`` generics of custom enums/structs is not allowed (yet?)
  2) Tune it to perfection, inspect the assembly and share the ideas on the Rust forum -- asking for feedback;
  3) Refactor the current options in `Server` & `Client` to use that pattern instead -- but, be aware of the followup stories **(n2)** & **(n3)**.

**(n2)** 2023-07-27: Introduce the `keen-retry` crate as a set of generic, zero-cost abstraction options for the `Client` & `Server`:
  1) What to do when the network loop receives a message but cannot enqueue it on the local processor (peer is fast-sending)
  2) What to do when the peer's outgoing buffer is full and cannot enqueue the local outgoing message (peer is slow-reading)
  3) The `Client` connection (with the remote server) drops
  4) For the above, the possible actions are:
     - `Ignore` (without retrying nor dropping the connection)
     - `EndCommunications` (dropping the connection, if not dropped already)"
     - `RetryYieldingForUpTo(n)` for up to n milliseconds, where n is in 0..8 and the milliseconds will be (n^2)ms",
     - `RetrySpinningForUpTo(n)` like the above,
     - `RetrySleepingGeometrically(n)` for `n` attempts (n in 0..8), each one sleeping for (n^2)*10ms",
  5) For a chosen action in (4), introduce a modifyer to **log the occurrence** or not

**(n3)** 2023-07-27: Allow `reactive-mutiny` channels to be configured as an option in the *Const Configuration* pattern.