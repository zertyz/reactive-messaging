# reactive-messaging

Provides `Stream`-based reactive client/server communications focused on high performance -- through sockets, shared mem IPCs and other goodies -- async-powered by `Tokio`.

The distinct features of this library are:
  - a thorough protocol modeling, with the help of Rust's powerful `enum`s;
  - ease of logic decoupling by using reactive `Stream`s for the client / server logic;
  - the main network loop for both client & server is fully optimized;
  - const-time (yet powerful) configurations, to achieve the most performance possible;
  - zero-cost retrying strategies & dropping the connection vs ignoring on errors through const time configs & the `keen-retry` lib
  - for version 1: only textual socket protocols are supported by now -- binary & shared mem IPCs to be added later;


# Taste it

Take a look at the ping-pong game in `example/`, which shows some nice patterns on how to use this library:
  - How the ping-pong game logic is shared among the client and server;
  - How the protocol is modeled;
  - How to work with server-side sessions;
  - How decoupled and testable those pieces are;
  - How straight-forward it is to create flexible server & client applications once the processors are complete. 


# beta status
The API has been stabilized -- new features yet to be added for v1.0:
  - allow reactive processors to produce all combinations of fallible/non-fallible & futures/non-futures for
    responsive & unresponsive logic processors (currently, only `Stream`s of non-fallible & non-future items are allowed)
  - better docs
  - clean the code for the ping-pong example and the recommended patterns for using this lib
