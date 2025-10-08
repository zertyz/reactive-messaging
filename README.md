# reactive-messaging

Provides `Stream`-based reactive client/server communications focused on high performance.

Still in beta, the distinct features of this library are:
  - ease of logic decoupling by using reactive `Stream`s for the client / server logic;
  - protocol modeling through Rust's powerful `enum`s;
  - 3 serialization strategies: textual (new-line separated), fixed size binary, and variable size binary,
    with different compromises between debugging, integration, speed, and flexibility;
  - custom serializers / deserializers allowed, with defaults being `ron` (for textual), `mmap` (for fixed size binary),
    and `rkyv` (for variable size binary);
  - serdes may be selected at runtime, sharing the same logic (with some exceptions in `rkyv`);
  - simple API for creating the client and server instances;
  - yet allowing advanced usages: composite protocols with different models between "states" and state transition logic;
  - the network loops for both client & server are fully optimized;
  - compile-time (generics & const, yet powerful) configurations, to achieve the most performance possible;
  - full support for retrying strategies, using zero-cost abstractions provided by `keen-retry`;


## Taste it

Take a look at the ping-pong game in `example/`, which shows some nice patterns on how to use this library:
  - How the ping-pong game logic is shared among the client and server;
  - How the protocol is modeled (we use the composite-protocol pattern there);
  - How to work with server-side sessions;
  - How decoupled and testable those pieces are;
  - How straight-forward it is to create flexible server & client applications once the processors are complete. 


## beta status

The API has been stabilized -- new features yet to be added for v1.0:
  - allow reactive processors to produce all combinations of fallible/non-fallible & futures/non-futures for;
    responsive & unresponsive logic processors (currently, only `Stream`s of non-fallible & non-future items are allowed);
  - unified tests for all the 3 default `serde`s;
  - better docs;
  - complete the companion book.


## Production-ready?

This crate is currently being used successfully in production for both Textual (`ron`) and Fixed Binary (`mmap`),
supporting 380 msgs/s per instance, with 0.0% CPU overhead -- please check the "Flood Example" to measure base system
resources needed for an arbitrary msgs/s load.

We will formally claim "Production ready" once the book is ready and more tests and examples are made.