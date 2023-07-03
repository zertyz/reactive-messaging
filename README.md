# reactive-messaging

Provides `Stream`-based reactive client/server communications focused on high performance -- through sockets, shared mem IPCs and other goodies -- async-powered by `Tokio`.

The distinct features of this library are:
  - a through protocol modeling, with the help of Rust's powerful `enum`s;
  - ease of logic decoupling by using reactive `Stream`s for the client / server logic.


# Taste it

Take a look at the ping-pong game in `example/`, which shows some nice patterns on how to use this library:
  - How the ping-pong game logic is shared among the client and server;
  - How the protocol is modeled;
  - How to work with server-side sessions;
  - How decoupled and testable those pieces are;
  - How straight-forward it is to create flexible server & client applications once the processors are complete. 


# pre-alpha status
Research & core developments are just done:
  - The main network loop for both client & server is complete & fully optimized;
  - A nice message & network events API eases the modeling of elaborated protocols, with a fully working `example/`;
  - The most performant reactive library & channels has been picked up, as determined by `benches/`;
  - Only textual socket protocols are supported by now -- binary & shared mem IPCs to be added later;
  - The API is not complete -- only "responsive" communications for `Stream`s that generate non-fallible & non-future elements are supported by now.