# Evolutions & History for this project

Here you'll find the *backlogs*, *being-done*, *completed* and *bug reports* for this project.

Issues contain a *prefix* letter and a sequence number, possibly followed by a date and a context-full description.



## Prefixes

  - (b) bug fix for broken functionalities
  - (f) new functional requisite
  - (n) new non-functional requisite
  - (r) internal re-engineering / refactor / improvement, to increase speed and/or enable further progress to be done cheaper



# Being-Done



# Backlog

**(f9)** 2024-01-17: Security -- support SSL/TSL + Client & Server fingerprinting for text & binary transmissions (depends on **(n8)**),
where fingerprinting is not an alleged number, but one determined by investigating the TCP/IP layers and the security metadata.
This is to be exposed on the connection event via a non-hashed, stable string containing the following:
1) TLS Client Hello Message: During the SSL/TLS handshake, the client sends a "Client Hello" message that contains several pieces of information
   which can be unique or characteristic to different clients, such as:
     - The list of supported SSL/TLS versions.
     - Supported cipher suites.
     - Extensions supported by the client.
     - Client random value.
     - Session ID.
2) TLS Certificate Details: If client-side TLS certificates are used, details from the client's certificate can be unique identifiers.
3) TCP/IP Layer Information: While limited, certain attributes from the TCP/IP layers can be observed, such as:
   - Source IP address.
   - TCP Timestamps.
   - Window size.

**(n8)** 2024-01-04: Introduce binary messages:
1) Use RKYV for serialization (the fastest & more flexible among current options, after a ChatGPT & bard research)
2) Formats will be textual (with \n separating messages) or binary (with a u16 payload size prefixing each message)
3) Opting between binary or textual should be done easily via ConstConfig -- provided the protocol types implement the appropriate traits
4) Build benchmarks comparing RON vs RKYV

**(f5)** 2023-09-04: Introduce "reconnection" on the client. For this:
  - a new pub method `reconnect()` is to be built: the old connection will be shutdown (if not already) and another one will be created
    -- the connection events callback will be called for the right events
  - a new pub "clone_sender()" must also be introduced:
    - the client keeps track of connections & reconnections, re-writing its internal "sender" channel, cloning & returning it here;
    - its docs must tell the users that the "sender channel" shared via connection events callback is valid only through the connection
      (when a reconnection happens, another "sender channel" will be created and that one is useless and needs to be dropped)
    - it must also be told that using this method has a performance cost and that performant user code must keep track of the sender when
      it is first received in the "connection events callback" -- and that, most probably, Option<UnsafeCell> needs to be used

**(f4)** 2023-08-27: added the extra possibilities for the combinations of fallible & future items yielded by the Streams + those minor things:
1) move the 'end connection' logic inside the `peer.send()` method, so the users don't need to bother to what the `ConstConfig` says about retrying & bailing out
2) review the docs everywhere, specially in the README.md and the PingPong example
3) remove the deprecated retrying logic & consider others
4release this as version 1.0.0

**BENCHMARKS** -- MessageIO, Tower


# Done

**(n11)** 2024-02-07: Simplify our API: REMOVE `ResponsiveMessages`, as it is completely not needed!! The reason is:
  * `is_disconnect_message()` can be done on the processor logic with `stream.take_until(|outgoing_message| outgoing_message == DISCONNECT_MESSAGE)`
  * `is_no_answer_message()` can be done with `stream.filter(...)`\
Also, consider the feasibility of also removing all those responsive/unresponsive methods and macros in the client and server code!
  * Can the same function be overloaded by a different generic parameter? If so, a `where` can be added to the responsive variant requiring the
    item type on the output stream to implement the serialization. Anyway... this is too speculative at this point and, probably, impossible to do right now.
  * Is it possible to add a `.to_responsive_stream()` to `Stream`? If so, that would be 100% cool!!
  * The proposed solution above has the benefit of restoring the possibility of "sending a disconnect message", not allowed after removing `ResponsiveMessages`:
    for this, simply call `.map_to_responsive_stream(|item| (item.to_send, item.to_yield) )` or anything with a better name.

**(r10)** 2024-01-28: TECH DEBT followup for **(f6)**: Remodel "events", improve event firing tests, refactor duplicated code, simplify the API
1) Remodel `ConnectionEvents`: with the introduction of the Composite Protocols (that allows using several processors), several event handlers
   are also allowed, making events such as "PeerDisconnected" not belonging to any of such handlers (as it is an event of a connection, and not
   a protocol's). To solve: There is a need for a distinction between `ConnectionEvent` and `ProtocolEvent` (for instance, for situations like
   when the processor ends but the connection doesn't -- and vice-versa). Remember this distinction is only needed for the Composite Protocol
   use case; for the single protocol use case, a third model may be created: `SingleProtocolEvent`.
2) There were issues reported regarding the disconnection events not being fired for certain cases. This is alarming, as it causes memory
   leaks on the user application (e.g, a server when handling multiple connections that create a session on connection and drop them on disconnection).
   TO DO: a) write elaborated integration tests (on api.rs or functional_requirements.rs) for the most varying scenarios for the client and server
          -- dropped by the other party, timing out, etc.;
          b) Write stress tests

**(f6)** 2023-11-01: Support Composite Protocol Stacking -- Complex authenticated protocols as well as WebSockets would benefit from this feature, as the whole composite stack could be modeled by different `enums`, avoiding unrepresentable states:
1) The `socket_connection_handler.rs` main logic should be upgraded to:
   a) start processing already opened connections
   b) bailing out without closing, returning the connection when the dialog functions are over
2) The client code, that owns the client instance, would have to receive generic enum with the `CompositeProtocols`:
   a) The processor code, upon commanding the protocol upgrade, would arrange for an instance of `CompositeProtocols` to be returned to the owner of the instance
   containing both the connection and any (custom) session information -- so that the code from the owner could use that connection to spawn a new client.
3) On the counter-part, the server processor would now receive a "ProtocolUpgradeFn" that would simply spawn a new server task with the connection -- like we do today.

**(r7)** 2024-01-04: Refactor the Connection & associated State -- move the state & id from Peer to a new, special `TcpStream` type.\
Taking in consideration "Composite Protocol Stacking" introduced in *(f6)*, a few ties were left behind regarding the connection States:
a) `StateType` had to implement `Default` -- this is used to distinguish if the connection is new (just opened) or reused. The former will
have the state set to `None` and the later have it set to the `Default` -- a safeguard if the processor code doesn't set any state;
b) Due to (a), when a connection is moved to another state (AKA, another protocol processor), the Peer state will be `Default` -- which is
very wrong.
c) Session Keeping: When the connection is passed along to other processors (in a composite protocol stacking service), a new peer id is generated,
which makes impossible for the server logic code to keep the session information (usually based on the ID).
It happens this may be fixed by creating a dedicated "Connection" type, which would contain the `TcpStream`, the `StateType` and the `ConnectionId`.
Suggested Steps:
1) Make `connection_provider.rs` to use the new connection type. New connections (client or server) will have the state set to `None`
2) Remove the `Default` constraint on `StateType` anywhere it is used -- this will reveal the places it should be gathered from the new
   connection type;
3) Remove all occurrences where the tuple `(TcpStream, StateType)` is present, replacing it for the new connection type
4) Update the test socket_client::composite_protocol_stacking_pattern on every `PeerConnected` event to assert that the peer state
   (which is, actually, the connection state) matches the hard coded values
5) Idem for the server version of this test
_6) Address all the TODO 2024-01-03 comments (allowing clients to reuse previous states, which is not currently possible today)_
7) Move `peer.id` to the connection and refactor all related code


**(n1)** 2023-07-27: Develop and introduce the freshly planned *Zero-Cost Const Configuration Pattern*.
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
     - `RetryYieldingForUpTo(n)` for up to n milliseconds, where n is in 0..8 and the milliseconds will be `(n^2)ms`,
     - `RetrySpinningForUpTo(n)` like the above,
     - `RetrySleepingGeometrically(n)` for `n` attempts (n in 0..8), each one sleeping for `(n^2)*10ms`,
5) For a chosen action in (4), introduce a modifier to **log the occurrence** or not

**(n3)** 2023-07-27: Allow `reactive-mutiny` channels to be configured as an option in the *Const Configuration* pattern.
